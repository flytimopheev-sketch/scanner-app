//! Прямая работа с libsane через FFI (production-путь, версия 2).
//!
//! Включается cargo-фичей `sane-ffi` (требует `sane-backends-devel`/`libsane-dev`
//! на машине сборки и `SCANNER_BACKEND=sane` в рантайме).
//!
//! Реализован минимальный, но полный цикл SANE API:
//! sane_init -> sane_get_devices -> sane_open -> sane_get_option_descriptor /
//! sane_control_option -> sane_start -> sane_get_parameters -> sane_read -> sane_cancel.

#![allow(non_camel_case_types, non_upper_case_globals, dead_code)]

use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::backend::{DeviceCapabilities, ScanProgress, ScannerBackend, TestResult};
use crate::device::ScannerDevice;
use crate::errors::{Result, ScannerError};
use crate::options::{ScanMode, ScanOptions};

// ---------- SANE types ----------

type SANE_Int = c_int;
type SANE_Bool = c_int;
type SANE_Status = c_int;
type SANE_String = *mut c_char;
type SANE_String_Const = *const c_char;
type SANE_Handle = *mut c_void;
type SANE_Byte = u8;
type SANE_Fixed = c_int;

const SANE_FIXED_SCALE: f64 = 65536.0;

const SANE_STATUS_GOOD: SANE_Status = 0;
const SANE_STATUS_UNSUPPORTED: SANE_Status = 1;
const SANE_STATUS_CANCELLED: SANE_Status = 2;
const SANE_STATUS_DEVICE_BUSY: SANE_Status = 3;
const SANE_STATUS_INVAL: SANE_Status = 4;
const SANE_STATUS_EOF: SANE_Status = 5;
const SANE_STATUS_JAMMED: SANE_Status = 6;
const SANE_STATUS_NO_MEM: SANE_Status = 7;
const SANE_STATUS_ACCESS_DENIED: SANE_Status = 8;
const SANE_STATUS_IO_ERROR: SANE_Status = 9;
const SANE_STATUS_NO_DOCS: SANE_Status = 10;
const SANE_STATUS_COVER_OPEN: SANE_Status = 11;

const SANE_FRAME_GRAY: SANE_Int = 0;
const SANE_FRAME_RGB: SANE_Int = 1;
const SANE_FRAME_RED: SANE_Int = 2;
const SANE_FRAME_GREEN: SANE_Int = 3;
const SANE_FRAME_BLUE: SANE_Int = 4;

const SANE_TYPE_BOOL: SANE_Int = 0;
const SANE_TYPE_INT: SANE_Int = 1;
const SANE_TYPE_FIXED: SANE_Int = 2;
const SANE_TYPE_STRING: SANE_Int = 3;
const SANE_TYPE_BUTTON: SANE_Int = 4;
const SANE_TYPE_GROUP: SANE_Int = 5;

const SANE_ACTION_GET_VALUE: SANE_Int = 0;
const SANE_ACTION_SET_VALUE: SANE_Int = 1;
const SANE_ACTION_SET_AUTO: SANE_Int = 2;

const SANE_CAP_INACTIVE: SANE_Int = 32;

const SANE_CONSTRAINT_NONE: SANE_Int = 0;
const SANE_CONSTRAINT_RANGE: SANE_Int = 1;
const SANE_CONSTRAINT_WORD_LIST: SANE_Int = 2;
const SANE_CONSTRAINT_STRING_LIST: SANE_Int = 3;

#[repr(C)]
#[derive(Default)]
struct SANE_Version {
    _major: SANE_Int,
    _minor: SANE_Int,
}

#[repr(C)]
struct SANE_Device {
    name: SANE_String_Const,
    vendor: SANE_String_Const,
    model: SANE_String_Const,
    typ: SANE_String_Const,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct SANE_Parameters {
    format: SANE_Int,
    last_frame: SANE_Bool,
    bytes_per_line: SANE_Int,
    pixels_per_line: SANE_Int,
    lines: SANE_Int,
    depth: SANE_Int,
}

#[repr(C)]
struct SANE_Range {
    min: SANE_Int,
    max: SANE_Int,
    quant: SANE_Int,
}

#[repr(C)]
struct SANE_Option_Descriptor {
    name: SANE_String_Const,
    title: SANE_String_Const,
    desc: SANE_String_Const,
    type_: SANE_Int,
    unit: SANE_Int,
    size: SANE_Int,
    cap: SANE_Int,
    constraint_type: SANE_Int,
    constraint: *mut c_void,
}

extern "C" {
    fn sane_init(version_code: *mut SANE_Int, auth: Option<extern "C" fn()>) -> SANE_Status;
    fn sane_exit();
    fn sane_get_devices(device_list: *mut *const *const SANE_Device, local_only: SANE_Bool) -> SANE_Status;
    fn sane_open(name: SANE_String_Const, handle: *mut SANE_Handle) -> SANE_Status;
    fn sane_close(handle: SANE_Handle);
    fn sane_get_option_descriptor(
        handle: SANE_Handle,
        option: SANE_Int,
        desc: *mut *const SANE_Option_Descriptor,
    ) -> SANE_Status;
    fn sane_control_option(
        handle: SANE_Handle,
        option: SANE_Int,
        action: SANE_Int,
        value: *mut c_void,
        info: *mut SANE_Int,
    ) -> SANE_Status;
    fn sane_start(handle: SANE_Handle) -> SANE_Status;
    fn sane_get_parameters(handle: SANE_Handle, params: *mut SANE_Parameters) -> SANE_Status;
    fn sane_read(handle: SANE_Handle, buf: *mut SANE_Byte, maxlen: SANE_Int, len: *mut SANE_Int) -> SANE_Status;
    fn sane_cancel(handle: SANE_Handle);
}

fn status_to_err(status: SANE_Status) -> ScannerError {
    let name = match status {
        SANE_STATUS_GOOD => "good",
        SANE_STATUS_UNSUPPORTED => "unsupported",
        SANE_STATUS_CANCELLED => "cancelled",
        SANE_STATUS_DEVICE_BUSY => "device busy",
        SANE_STATUS_INVAL => "invalid value",
        SANE_STATUS_EOF => "end of data",
        SANE_STATUS_JAMMED => "document jammed",
        SANE_STATUS_NO_MEM => "out of memory",
        SANE_STATUS_ACCESS_DENIED => "access denied",
        SANE_STATUS_IO_ERROR => "io error",
        SANE_STATUS_NO_DOCS => "no documents",
        SANE_STATUS_COVER_OPEN => "cover open",
        _ => "unknown error",
    };
    match status {
        SANE_STATUS_CANCELLED => ScannerError::Cancelled,
        _ => ScannerError::Sane(format!("SANE: {name} (#{status})")),
    }
}

// SANE не потокобезопасен глобально: все вызовы под мьютексом.
static SANE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

fn lock() -> std::sync::MutexGuard<'static, ()> {
    SANE_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn ensure_init() {
    if INITIALIZED.load(Ordering::SeqCst) {
        return;
    }
    let _g = lock();
    if INITIALIZED.load(Ordering::SeqCst) {
        return;
    }
    unsafe {
        let mut version: SANE_Int = 0;
        let st = sane_init(&mut version, None);
        if st != SANE_STATUS_GOOD {
            panic!("sane_init failed: {st}");
        }
    }
    INITIALIZED.store(true, Ordering::SeqCst);
}

fn sane_str(p: SANE_String_Const) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    }
}

pub struct SaneFfiBackend {
    cancel: AtomicBool,
}

impl SaneFfiBackend {
    pub fn new() -> Self {
        Self { cancel: AtomicBool::new(false) }
    }

    /// Найти индекс опции по имени.
    fn find_option(&self, handle: SANE_Handle, name: &str) -> Result<Option<SANE_Int>> {
        unsafe {
            let mut desc: *const SANE_Option_Descriptor = std::ptr::null();
            let st = sane_get_option_descriptor(handle, 0, &mut desc);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
            let n = (*desc).size; // опция 0 возвращает число опций в size
            for i in 1..n {
                let mut d: *const SANE_Option_Descriptor = std::ptr::null();
                let st = sane_get_option_descriptor(handle, i, &mut d);
                if st != SANE_STATUS_GOOD {
                    continue;
                }
                if (*d).cap & SANE_CAP_INACTIVE != 0 {
                    continue;
                }
                if let Some(nm) = sane_str((*d).name) {
                    if nm == name {
                        return Ok(Some(i));
                    }
                }
            }
        }
        Ok(None)
    }

    fn set_string_option(&self, handle: SANE_Handle, name: &str, value: &str) -> Result<()> {
        let Some(idx) = self.find_option(handle, name)? else { return Ok(()) };
        unsafe {
            let mut buf: Vec<u8> = value.as_bytes().to_vec();
            buf.push(0);
            let mut info: SANE_Int = 0;
            let st = sane_control_option(
                handle,
                idx,
                SANE_ACTION_SET_VALUE,
                buf.as_mut_ptr() as *mut c_void,
                &mut info,
            );
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
        }
        Ok(())
    }

    fn set_int_option(&self, handle: SANE_Handle, name: &str, value: u32) -> Result<()> {
        let Some(idx) = self.find_option(handle, name)? else { return Ok(()) };
        unsafe {
            let mut desc: *const SANE_Option_Descriptor = std::ptr::null();
            sane_get_option_descriptor(handle, idx, &mut desc);
            let is_fixed = !desc.is_null() && (*desc).type_ == SANE_TYPE_FIXED;
            let v: SANE_Int = if is_fixed {
                (value as f64 * SANE_FIXED_SCALE) as SANE_Fixed
            } else {
                value as SANE_Int
            };
            let mut info: SANE_Int = 0;
            let st = sane_control_option(handle, idx, SANE_ACTION_SET_VALUE, &v as *const _ as *mut c_void, &mut info);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
        }
        Ok(())
    }

    /// Чтение одного кадра (страницы).
    fn read_frame(&self, handle: SANE_Handle, cancel: &AtomicBool) -> Result<Option<image::DynamicImage>> {
        unsafe {
            let mut params = SANE_Parameters {
                format: 0,
                last_frame: 0,
                bytes_per_line: 0,
                pixels_per_line: 0,
                lines: 0,
                depth: 8,
            };
            let st = sane_get_parameters(handle, &mut params);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
            if params.lines == 0 {
                return Ok(None); // пустая страница
            }

            let bpl = params.bytes_per_line as usize;
            let w = params.pixels_per_line as usize;
            let h = params.lines as usize;
            let mut buf: Vec<u8> = Vec::with_capacity(bpl * h);
            let mut chunk = vec![0u8; 32 * 1024];

            loop {
                if cancel.load(Ordering::SeqCst) {
                    sane_cancel(handle);
                    return Err(ScannerError::Cancelled);
                }
                let mut len: SANE_Int = 0;
                let st = sane_read(handle, chunk.as_mut_ptr(), chunk.len() as SANE_Int, &mut len);
                match st {
                    SANE_STATUS_GOOD => {
                        buf.extend_from_slice(&chunk[..len as usize]);
                    }
                    SANE_STATUS_EOF => break,
                    other => return Err(status_to_err(other)),
                }
            }

            let img = match (params.format, params.depth) {
                (SANE_FRAME_RGB, 8) => {
                    let mut rgb = image::RgbImage::new(w as u32, h as u32);
                    for y in 0..h {
                        let row = &buf[y * bpl..(y + 1) * bpl];
                        for x in 0..w {
                            let o = x * 3;
                            if o + 2 < bpl {
                                rgb.put_pixel(x as u32, y as u32, image::Rgb([row[o], row[o + 1], row[o + 2]]));
                            }
                        }
                    }
                    image::DynamicImage::ImageRgb8(rgb)
                }
                (SANE_FRAME_GRAY, d) => {
                    let mut gray = image::GrayImage::new(w as u32, h as u32);
                    for y in 0..h {
                        let row = &buf[y * bpl..(y + 1) * bpl];
                        for x in 0..w {
                            let v = match d {
                                1 => ((row[x / 8] >> (7 - x % 8)) & 1) as u8 * 255,
                                8 => row[x],
                                16 => row[x * 2],
                                _ => row[x],
                            };
                            gray.put_pixel(x as u32, y as u32, image::Luma([v]));
                        }
                    }
                    image::DynamicImage::ImageLuma8(gray)
                }
                (f, _) => {
                    // RED/GREEN/BLUE (трёхпроходные сканеры) не поддерживаем для MVP
                    return Err(ScannerError::Unsupported(format!("frame format {f}")));
                }
            };
            Ok(Some(img))
        }
    }
}

impl Default for SaneFfiBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ScannerBackend for SaneFfiBackend {
    fn list_devices(&self) -> Result<Vec<ScannerDevice>> {
        ensure_init();
        let _g = lock();
        self.cancel.store(false, Ordering::SeqCst);
        unsafe {
            let mut list: *const *const SANE_Device = std::ptr::null();
            let st = sane_get_devices(&mut list, 0);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
            let mut out = Vec::new();
            let mut i = 0;
            loop {
                let dev = *list.add(i);
                if dev.is_null() {
                    break;
                }
                let name = sane_str((*dev).name).unwrap_or_default();
                let vendor = sane_str((*dev).vendor);
                let model = sane_str((*dev).model);
                out.push(ScannerDevice::from_sane_string(&name, vendor, model));
                i += 1;
            }
            Ok(out)
        }
    }

    fn capabilities(&self, device: &str) -> Result<DeviceCapabilities> {
        ensure_init();
        let _g = lock();
        let cname = std::ffi::CString::new(device).map_err(|_| ScannerError::Other("bad device name".into()))?;
        unsafe {
            let mut handle: SANE_Handle = std::ptr::null_mut();
            let st = sane_open(cname.as_ptr(), &mut handle);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }
            let mut caps = DeviceCapabilities::default();
            let mut desc: *const SANE_Option_Descriptor = std::ptr::null();
            if sane_get_option_descriptor(handle, 0, &mut desc) == SANE_STATUS_GOOD {
                let n = (*desc).size;
                for i in 1..n {
                    let mut d: *const SANE_Option_Descriptor = std::ptr::null();
                    if sane_get_option_descriptor(handle, i, &mut d) != SANE_STATUS_GOOD || d.is_null() {
                        continue;
                    }
                    let name = sane_str((*d).name).unwrap_or_default();
                    if (*d).cap & SANE_CAP_INACTIVE != 0 {
                        continue;
                    }
                    match name.as_str() {
                        "resolution" => {
                            if (*d).constraint_type == SANE_CONSTRAINT_RANGE {
                                let r = (*d).constraint as *const SANE_Range;
                                if !r.is_null() {
                                    let (lo, hi) = ((*r).min.max(75), (*r).max.min(600));
                                    caps.resolutions = vec![lo as u32, 150, 200, 300, hi as u32];
                                    caps.resolutions.sort_unstable();
                                    caps.resolutions.dedup();
                                    caps.resolutions.retain(|v| (75..=600).contains(v));
                                }
                            }
                        }
                        "mode" | "scan-mode" => {
                            if (*d).constraint_type == SANE_CONSTRAINT_STRING_LIST {
                                let mut k = 0;
                                loop {
                                    let p = *((*d).constraint as *const SANE_String_Const).add(k);
                                    match sane_str(p) {
                                        Some(s) => {
                                            if let Some(m) = map_mode(&s) {
                                                caps.modes.push(m);
                                            }
                                            k += 1;
                                        }
                                        None => break,
                                    }
                                }
                            }
                        }
                        "source" | "scan-source" => {
                            if (*d).constraint_type == SANE_CONSTRAINT_STRING_LIST {
                                let mut k = 0;
                                loop {
                                    let p = *((*d).constraint as *const SANE_String_Const).add(k);
                                    match sane_str(p) {
                                        Some(s) => {
                                            if let Some(src) = crate::options::ScanSource::from_scanimage_name(&s) {
                                                caps.sources.push(src);
                                            }
                                            k += 1;
                                        }
                                        None => break,
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            sane_close(handle);
            if caps.modes.is_empty() {
                caps.modes = ScanMode::all().to_vec();
            }
            if caps.sources.is_empty() {
                caps.sources = vec![crate::options::ScanSource::Flatbed];
            }
            if caps.resolutions.is_empty() {
                caps.resolutions = vec![75, 150, 200, 300, 600];
            }
            Ok(caps)
        }
    }

    fn scan(
        &self,
        device: &str,
        options: &ScanOptions,
        out_dir: &Path,
        progress: &dyn Fn(ScanProgress),
    ) -> Result<Vec<PathBuf>> {
        options.validate()?;
        ensure_init();
        std::fs::create_dir_all(out_dir)?;
        let _g = lock();
        self.cancel.store(false, Ordering::SeqCst);

        let cname = std::ffi::CString::new(device).map_err(|_| ScannerError::Other("bad device name".into()))?;
        let mut files = Vec::new();

        unsafe {
            let mut handle: SANE_Handle = std::ptr::null_mut();
            let st = sane_open(cname.as_ptr(), &mut handle);
            if st != SANE_STATUS_GOOD {
                return Err(status_to_err(st));
            }

            let set_res = self.set_int_option(handle, "resolution", options.resolution);
            if set_res.is_err() {
                log::warn!("resolution not set: {set_res:?}");
            }
            if let Err(e) = self.set_string_option(handle, "mode", options.mode.scanimage_name()) {
                log::warn!("mode not set: {e}");
            }
            if let Some(src) = options.source.scanimage_name() {
                if let Err(e) = self.set_string_option(handle, "source", src) {
                    log::warn!("source not set: {e}");
                }
            }

            let cancel = &self.cancel as *const AtomicBool;
            let mut page = 0u32;
            loop {
                if self.cancel.load(Ordering::SeqCst) {
                    sane_cancel(handle);
                    sane_close(handle);
                    return Err(ScannerError::Cancelled);
                }
                page += 1;
                progress(ScanProgress::PageStarted(page));
                let st = sane_start(handle);
                match st {
                    SANE_STATUS_GOOD => {}
                    SANE_STATUS_NO_DOCS => break,
                    other => {
                        sane_close(handle);
                        return Err(status_to_err(other));
                    }
                }

                // Кадры страницы (обычно один RGB/Gray кадр).
                let mut img = None;
                loop {
                    match self.read_frame(handle, &*cancel)? {
                        Some(frame) => img = Some(match img {
                            None => frame,
                            Some(prev) => crate::imageproc::merge_frames(prev, frame),
                        }),
                        None => break,
                    }
                    if {
                        // sane_get_parameters вызывается во внешнем unsafe-блоке сканирования.
                        let mut params: SANE_Parameters = std::mem::zeroed();
                        let st = sane_get_parameters(handle, &mut params);
                        st == SANE_STATUS_GOOD && params.last_frame != 0
                    } {
                        break;
                    }
                }

                if let Some(image) = img {
                    let path = out_dir.join(format!("page-{page:04}.png"));
                    image
                        .save_with_format(&path, image::ImageFormat::Png)
                        .map_err(|e| ScannerError::Image(e.to_string()))?;
                    progress(ScanProgress::PageDone(page, path.clone()));
                    files.push(path);
                }
                sane_cancel(handle); // сброс состояния перед следующей страницей
            }

            sane_close(handle);
        }

        if files.is_empty() {
            return Err(ScannerError::Sane("сканер не вернул изображений".into()));
        }
        Ok(files)
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        // sane_cancel безопасно вызывать из другого потока (спецификация SANE).
        // ВАЖНО: не берём глобальный SANE_LOCK — если операция «зависла»,
        // удерживая замок, Cancel на нём бы заблокировался навсегда.
        // read_frame дополнительно проверяет флаг в цикле чтения.
    }

    fn test_ip(&self, address: &str, protocol: &str) -> Result<TestResult> {
        // Общая логика совпадает со scanimage-бэкендом.
        crate::scanimage::ScanimageBackend::new().test_ip(address, protocol)
    }
}

fn map_mode(s: &str) -> Option<ScanMode> {
    let l = s.to_lowercase();
    if l.contains("color") || l.contains("colour") {
        Some(ScanMode::Color)
    } else if l.contains("gray") || l.contains("grey") {
        Some(ScanMode::Gray)
    } else if l.contains("line") || l.contains("binary") || l.contains("mono") || l.contains("black") {
        Some(ScanMode::Lineart)
    } else {
        None
    }
}
