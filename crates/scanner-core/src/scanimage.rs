//! Бэкенд на основе CLI `scanimage` (SANE). Самый быстрый и надёжный путь для MVP:
//! не требует FFI, легко диагностируется, работает со всеми SANE-драйверами.

use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::backend::{DeviceCapabilities, ScanProgress, ScannerBackend, TestResult};
use crate::device::ScannerDevice;
use crate::errors::{Result, ScannerError};
use crate::options::{ScanMode, ScanOptions};

const KNOWN_VENDORS: &[&str] = &[
    "Brother", "Canon", "Epson", "HP", "Kyocera", "Lexmark", "Pantum", "Ricoh",
    "Samsung", "Xerox", "Fujitsu", "Avision", "Plustek", "Mustek", "AGFA",
    "UMAX", "Microtek", "OKI", "HP Inc.",
];

pub struct ScanimageBackend {
    cancel: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl ScanimageBackend {
    pub fn new() -> Self {
        ScanimageBackend { cancel: Arc::new(AtomicBool::new(false)), child: Arc::new(Mutex::new(None)) }
    }

    fn bin() -> String {
        std::env::var("SCANIMAGE_BIN").unwrap_or_else(|_| "scanimage".into())
    }

    fn reset(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// Запуск команды с чтением stdout/stderr в отдельных потоках,
    /// опросом завершения и поддержкой отмены/таймаута.
    fn run(&self, args: &[String], timeout: Duration) -> Result<(String, String, Option<i32>)> {
        let mut cmd = Command::new(Self::bin());
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| ScannerError::Spawn(e.to_string()))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        *self.child.lock().unwrap() = Some(child);

        let so = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = BufReader::new(stdout).read_to_string(&mut s);
            s
        });
        let se = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut s);
            s
        });

        enum Outcome { Done(i32), Cancelled, Timeout }
        let outcome;
        let deadline = Instant::now() + timeout;
        loop {
            let mut guard = self.child.lock().unwrap();
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(st)) => {
                        outcome = Outcome::Done(st.code().unwrap_or(-1));
                        break;
                    }
                    Ok(None) => {
                        if self.cancel.load(Ordering::SeqCst) {
                            let _ = child.kill();
                            let _ = child.wait();
                            outcome = Outcome::Cancelled;
                            break;
                        }
                        if Instant::now() > deadline {
                            let _ = child.kill();
                            let _ = child.wait();
                            outcome = Outcome::Timeout;
                            break;
                        }
                    }
                    Err(e) => return Err(ScannerError::Io(e)),
                }
            } else {
                outcome = Outcome::Cancelled;
                break;
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(60));
        }
        *self.child.lock().unwrap() = None;

        let out = so.join().unwrap_or_default();
        let err = se.join().unwrap_or_default();

        match outcome {
            Outcome::Done(code) => Ok((out, err, Some(code))),
            Outcome::Cancelled => Err(ScannerError::Cancelled),
            Outcome::Timeout => Err(ScannerError::Timeout),
        }
    }
}

impl Default for ScanimageBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl ScannerBackend for ScanimageBackend {
    fn list_devices(&self) -> Result<Vec<ScannerDevice>> {
        self.reset();
        let args = vec!["-L".to_string()];
        let (stdout, _stderr, _code) = self.run(&args, Duration::from_secs(90))?;
        Ok(parse_device_list(&stdout))
    }

    fn capabilities(&self, device: &str) -> Result<DeviceCapabilities> {
        self.reset();
        // Устройство, добавленное вручную по IP — работаем через временный конфиг.
        if let Some((proto, ip, port, alias)) = parse_manual(device) {
            let (cfg_dir, sane_name) = prepare_manual_config(&proto, &ip, &port, &alias)?;
            let args = vec![
                "--help".to_string(),
                "-d".to_string(),
                sane_name,
            ];
            let mut cmd = Command::new(Self::bin());
            cmd.args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .env("SANE_CONFIG_DIR", &cfg_dir);
            let stdout = run_capture(cmd, Duration::from_secs(30))?;
            return Ok(parse_capabilities(&stdout));
        }

        let args = vec!["--help".to_string(), "-d".to_string(), device.to_string()];
        let (stdout, _stderr, _code) = self.run(&args, Duration::from_secs(30))?;
        Ok(parse_capabilities(&stdout))
    }

    fn scan(
        &self,
        device: &str,
        options: &ScanOptions,
        out_dir: &Path,
        progress: &dyn Fn(ScanProgress),
    ) -> Result<Vec<PathBuf>> {
        options.validate()?;
        self.reset();
        std::fs::create_dir_all(out_dir)?;

        // Устройство по IP: временный конфиг + airscan:<alias>.
        let mut manual_env: Option<(PathBuf, String)> = None;
        let sane_device = if let Some((proto, ip, port, alias)) = parse_manual(device) {
            let (cfg, sane_name) = prepare_manual_config(&proto, &ip, &port, &alias)?;
            manual_env = Some((cfg, sane_name.clone()));
            sane_name
        } else {
            device.to_string()
        };

        let mut args: Vec<String> = vec![
            "-d".into(), sane_device,
            "--format".into(), "tiff".into(),
            "--resolution".into(), options.resolution.to_string(),
            "--mode".into(), options.mode.scanimage_name().into(),
        ];
        if let Some(src) = options.source.scanimage_name() {
            args.push("--source".into());
            args.push(src.into());
        }

        let use_batch = options.source != crate::options::ScanSource::Flatbed;
        if use_batch {
            // ADF: пакетный режим, имена страниц печатает --batch-print.
            args.push("--batch=page-%04d.tiff".into());
            args.push("--batch-print".into());
        }

        let mut cmd = Command::new(Self::bin());
        cmd.args(&args)
            .current_dir(out_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some((cfg_dir, _)) = &manual_env {
            cmd.env("SANE_CONFIG_DIR", cfg_dir);
        }

        // Планшет: единственная страница уходит в stdout — перенаправим в файл.
        let out_file = if !use_batch {
            Some(out_dir.join("page-0001.tiff"))
        } else {
            None
        };

        let mut child = cmd.spawn().map_err(|e| ScannerError::Spawn(e.to_string()))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        *self.child.lock().unwrap() = Some(child);

        let cancel = self.cancel.clone();

        // stdout: ADF — счётчик строк --batch-print; планшет — писатель в файл.
        let (batch_reader, single_writer) = if use_batch {
            let h = std::thread::spawn(move || {
                let mut n = 0u32;
                for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
                    if !line.trim().is_empty() {
                        n += 1;
                    }
                }
                n
            });
            (Some(h), None)
        } else {
            let path = out_file.expect("flatbed output path");
            let h = std::thread::spawn(move || {
                use std::io::{Read, Write};
                let mut total = 0usize;
                if let Ok(mut file) = std::fs::File::create(&path) {
                    let mut buf = [0u8; 64 * 1024];
                    let mut reader = BufReader::new(stdout);
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) | Err(_) => break,
                            Ok(k) => {
                                if file.write_all(&buf[..k]).is_err() {
                                    break;
                                }
                                total += k;
                            }
                        }
                    }
                }
                (path, total)
            });
            (None, Some(h))
        };

        let se = std::thread::spawn(move || {
            let mut collected = String::new();
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        });

        // Отслеживаем появление страниц (ADF) для прогресса.
        let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        enum Outcome {
            Done(bool),
            Cancelled,
        }
        let outcome;
        loop {
            {
                let mut guard = self.child.lock().unwrap();
                let Some(c) = guard.as_mut() else {
                    outcome = Outcome::Cancelled;
                    break;
                };
                match c.try_wait() {
                    Ok(Some(st)) => {
                        let _ = c.wait();
                        outcome = Outcome::Done(st.success());
                        break;
                    }
                    Ok(None) => {
                        if cancel.load(Ordering::SeqCst) {
                            let _ = c.kill();
                            let _ = c.wait();
                            outcome = Outcome::Cancelled;
                            break;
                        }
                    }
                    Err(e) => return Err(ScannerError::Io(e)),
                }
            }
            if use_batch {
                for f in collect_batch_files(out_dir) {
                    if seen.insert(f.clone()) {
                        progress(ScanProgress::PageDone(seen.len() as u32, f));
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(300));
        }

        if let Some(h) = batch_reader {
            let _ = h.join();
        }
        let single = single_writer.and_then(|h| h.join().ok());
        let stderr_text = se.join().unwrap_or_default();
        *self.child.lock().unwrap() = None;

        match outcome {
            Outcome::Cancelled => Err(ScannerError::Cancelled),
            Outcome::Done(true) => {
                let mut files: Vec<PathBuf> = if use_batch {
                    collect_batch_files(out_dir)
                } else {
                    Vec::new()
                };
                if let Some((path, total)) = single {
                    if total > 0 {
                        files.push(path);
                    }
                }
                if files.is_empty() {
                    return Err(ScannerError::Sane(if stderr_text.trim().is_empty() {
                        "сканер не вернул изображений".into()
                    } else {
                        stderr_text.trim().lines().last().unwrap_or("").to_string()
                    }));
                }
                Ok(files)
            }
            Outcome::Done(false) => {
                if self.cancel.load(Ordering::SeqCst) {
                    return Err(ScannerError::Cancelled);
                }
                let msg = stderr_text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .last()
                    .unwrap_or("scanimage завершился с ошибкой");
                Err(ScannerError::Sane(msg.to_string()))
            }
        }
    }

    fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                let _ = child.kill();
            }
        }
    }

    /// Предпросмотр: одна страница с планшета в низком DPI, формат PNG.
    /// Не использует --batch — единственный кадр уходит в stdout.
    fn preview(&self, device: &str, dpi: u32, out_dir: &Path) -> Result<PathBuf> {
        self.reset();
        std::fs::create_dir_all(out_dir)?;
        let dpi = dpi.clamp(50, 300).max(50);

        let mut env_dir: Option<PathBuf> = None;
        let sane_device = if let Some((proto, ip, port, alias)) = parse_manual(device) {
            let (cfg, sane_name) = prepare_manual_config(&proto, &ip, &port, &alias)?;
            env_dir = Some(cfg);
            sane_name
        } else {
            device.to_string()
        };

        let args = vec![
            "-d".to_string(),
            sane_device,
            "--format".to_string(),
            "png".to_string(),
            "--resolution".to_string(),
            dpi.to_string(),
        ];
        let mut cmd = Command::new(Self::bin());
        cmd.args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cfg) = &env_dir {
            cmd.env("SANE_CONFIG_DIR", cfg);
        }
        let mut child = cmd.spawn().map_err(|e| ScannerError::Spawn(e.to_string()))?;
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        *self.child.lock().unwrap() = Some(child);

        let out_path = out_dir.join("preview.png");
        let writer = std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut total = 0usize;
            if let Ok(mut file) = std::fs::File::create(&out_path) {
                let mut buf = [0u8; 64 * 1024];
                let mut reader = BufReader::new(stdout);
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(k) => {
                            if file.write_all(&buf[..k]).is_err() {
                                break;
                            }
                            total += k;
                        }
                    }
                }
            }
            (out_path, total)
        });
        let se = std::thread::spawn(move || {
            let mut collected = String::new();
            for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                collected.push_str(&line);
                collected.push('\n');
            }
            collected
        });

        // Ожидание завершения с учётом отмены.
        loop {
            let mut guard = self.child.lock().unwrap();
            let Some(c) = guard.as_mut() else {
                let _ = writer.join();
                return Err(ScannerError::Cancelled);
            };
            match c.try_wait() {
                Ok(Some(_)) => {
                    let _ = c.wait();
                    break;
                }
                Ok(None) => {
                    if self.cancel.load(Ordering::SeqCst) {
                        let _ = c.kill();
                        let _ = c.wait();
                        let _ = writer.join();
                        return Err(ScannerError::Cancelled);
                    }
                }
                Err(e) => return Err(ScannerError::Io(e)),
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(150));
        }

        let (path, total) = writer.join().unwrap_or((out_dir.join("preview.png"), 0));
        *self.child.lock().unwrap() = None;
        if total == 0 {
            let err = se.join().unwrap_or_default();
            return Err(ScannerError::Sane(if err.trim().is_empty() {
                "сканер не вернул изображение предпросмотра".into()
            } else {
                err.trim().lines().last().unwrap_or("").to_string()
            }));
        }
        Ok(path)
    }

    fn test_ip(&self, address: &str, protocol: &str) -> Result<TestResult> {
        let ip: IpAddr = address
            .trim()
            .parse()
            .map_err(|_| ScannerError::Other(format!("некорректный IP-адрес: {address}")))?;

        // 1. Попытка найти устройство через airscan-discover (если установлен).
        if let Ok(out) = Command::new("airscan-discover").output() {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            for line in text.lines() {
                if let Some(dev) = line.strip_prefix("device `") {
                    if let Some(rest) = dev.split('`').next() {
                        if line.contains(address) {
                            return Ok(TestResult {
                                ok: true,
                                device_name: Some(rest.to_string()),
                                message: format!("найдено через airscan-discover: {rest}"),
                            });
                        }
                    }
                }
            }
        }

        // 2. TCP-проверка типовых портов сканеров.
        let ports: &[u16] = match protocol {
            "wsd" => &[5357, 80, 8080],
            "escl" => &[8080, 80, 9090, 443],
            _ => &[8080, 80, 9090, 631, 5357],
        };
        for port in ports {
            let addr = std::net::SocketAddr::new(ip, *port);
            if TcpStream::connect_timeout(&addr, Duration::from_millis(1500)).is_ok() {
                return Ok(TestResult {
                    ok: true,
                    device_name: None,
                    message: format!("порт {port} отвечает"),
                });
            }
        }
        Ok(TestResult {
            ok: false,
            device_name: None,
            message: "ни один порт сканера не отвечает".into(),
        })
    }
}

/// Разбор имени устройства, добавленного вручную:
/// `manual|<proto>|<ip>|<port>|<alias>`.
pub fn parse_manual(device: &str) -> Option<(String, String, String, String)> {
    let parts: Vec<&str> = device.split('|').collect();
    if parts.len() == 5 && parts[0] == "manual" {
        Some((parts[1].into(), parts[2].into(), parts[3].into(), parts[4].into()))
    } else {
        None
    }
}

/// Временный SANE_CONFIG_DIR для устройства по IP — пользователю не нужно
/// править /etc/sane.d/airscan.conf. Возвращает (каталог, имя для scanimage).
pub fn prepare_manual_config(
    proto: &str,
    ip: &str,
    port: &str,
    alias: &str,
) -> Result<(PathBuf, String)> {
    let safe_alias: String = alias.replace('"', "'").replace('|', " ");
    let dir = crate::config::tmp_scan_dir().join(format!("sane-cfg-{}", crate::device::short_id(&safe_alias)));
    std::fs::create_dir_all(&dir)?;
    let uri_scheme = if proto == "wsd" { "wsd" } else { "escl" };
    std::fs::write(dir.join("dll.conf"), "airscan\n")?;
    let conf = format!(
        "device \"{safe_alias}\" {{\n    uri = \"{uri_scheme}:http://{ip}:{port}\"\n}}\n"
    );
    std::fs::write(dir.join("airscan.conf"), conf)?;
    Ok((dir, format!("airscan:{safe_alias}")))
}

/// Одноразовый запуск с чтением stdout (для --help с кастомным окружением).
fn run_capture(mut cmd: Command, timeout: Duration) -> Result<String> {
    use std::io::Read;
    let mut child = cmd.spawn().map_err(|e| ScannerError::Spawn(e.to_string()))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    return Err(ScannerError::Timeout);
                }
            }
            Err(e) => return Err(ScannerError::Io(e)),
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    Ok(out)
}

fn collect_batch_files(out_dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(out_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with("page-") && n.ends_with(".tiff"))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Разбор вывода `scanimage -L`.
pub fn parse_device_list(stdout: &str) -> Vec<ScannerDevice> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let line = line.trim_start();
        let Some(rest) = line.strip_prefix("device `") else { continue };
        let Some((name, desc)) = rest.split_once("' is a ") else { continue };
        let desc = desc.trim().trim_end_matches('.').to_string();
        let human = desc
            .trim_end_matches(" scanner")
            .trim_end_matches(" multi-function peripheral")
            .trim_end_matches(" virtual device")
            .trim()
            .to_string();
        let human = if human.is_empty() { name.to_string() } else { human };

        let mut words = human.split_whitespace();
        let first = words.next().unwrap_or("").to_string();
        let (vendor, display) = match KNOWN_VENDORS.iter().find(|v| v.eq_ignore_ascii_case(&first)) {
            Some(canonical) => {
                // "CANON Canon PIXMA MP610" -> vendor Canon, имя без дубля производителя.
                let second = human.split_whitespace().nth(1).unwrap_or("");
                let display = if second.eq_ignore_ascii_case(&first) {
                    human[first.len()..].trim().to_string()
                } else {
                    human.clone()
                };
                (Some((*canonical).to_string()), display)
            }
            None => (None, human.clone()),
        };
        let display = if display.is_empty() { human.clone() } else { display };
        out.push(ScannerDevice::from_sane_string(name, vendor, Some(display)));
    }
    out
}

/// Разбор вывода `scanimage --help -d DEVICE`.
pub fn parse_capabilities(text: &str) -> DeviceCapabilities {
    let mut caps = DeviceCapabilities::default();

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("--mode") {
            let values = parse_pipe_values(rest);
            if !values.is_empty() {
                caps.modes = values
                    .iter()
                    .filter_map(|v| map_mode(v))
                    .collect::<Vec<_>>();
                if caps.modes.is_empty() {
                    caps.modes = ScanMode::all().to_vec();
                }
            }
        } else if let Some(rest) = line.strip_prefix("--resolution") {
            caps.resolutions = parse_resolution(rest);
        } else if let Some(rest) = line.strip_prefix("--source") {
            let values = parse_pipe_values(rest);
            let mapped: Vec<crate::options::ScanSource> = values
                .iter()
                .filter_map(|v| crate::options::ScanSource::from_scanimage_name(v))
                .collect();
            caps.sources = dedup(mapped);
        }
    }

    if caps.modes.is_empty() {
        caps.modes = ScanMode::all().to_vec();
    }
    if caps.sources.is_empty() {
        caps.sources = vec![crate::options::ScanSource::Flatbed];
    }
    caps
}

/// Значения вида "Color|Gray|Lineart [Color]" -> ["Color", "Gray", "Lineart"].
fn parse_pipe_values(after_flag: &str) -> Vec<String> {
    let s = after_flag.trim();
    let s = s.split("[").next().unwrap_or(s).trim();
    if !s.contains('|') {
        return Vec::new();
    }
    s.split('|').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// Разбор "75..600dpi (in steps of 1) [300]" или "75|150|300|600dpi".
fn parse_resolution(after_flag: &str) -> Vec<u32> {
    let s = after_flag.trim().split('[').next().unwrap_or("").trim();
    let first = s
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches("dpi")
        .trim()
        .to_string();
    let standard = [75u32, 150, 200, 300, 600];

    if let Some((lo, hi)) = first.split_once("..") {
        if let (Ok(lo), Ok(hi)) = (lo.trim().parse::<u32>(), hi.trim().parse::<u32>()) {
            let mut v: Vec<u32> = standard.into_iter().filter(|r| *r >= lo && *r <= hi).collect();
            if v.is_empty() {
                v.push(lo);
            }
            if *v.last().unwrap() != hi && hi <= 1200 {
                v.push(hi);
            }
            return v;
        }
        return Vec::new();
    }
    if first.contains('|') {
        return first
            .split('|')
            .filter_map(|p| p.trim().trim_end_matches("dpi").trim().parse::<u32>().ok())
            .collect();
    }
    Vec::new()
}

fn map_mode(s: &str) -> Option<ScanMode> {
    let l = s.to_lowercase();
    if l.contains("color") || l.contains("colour") {
        Some(ScanMode::Color)
    } else if l.contains("gray") || l.contains("grey") {
        Some(ScanMode::Gray)
    } else if l.contains("line") || l.contains("binary") || l.contains("black") || l.contains("mono") {
        Some(ScanMode::Lineart)
    } else {
        None
    }
}

fn dedup(v: Vec<crate::options::ScanSource>) -> Vec<crate::options::ScanSource> {
    let mut out: Vec<crate::options::ScanSource> = Vec::new();
    for s in v {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}
