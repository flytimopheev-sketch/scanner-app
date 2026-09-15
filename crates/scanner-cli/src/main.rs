//! scanner-cli — консольный режим «Сканера документов».
//!
//! То же ядро, что и у GUI (scanner-core); отдельный воркер не нужен,
//! CLI сам работает в отдельном потоке, для скриптов и автоматизации:
//!
//! ```text
//! scanner-cli --list-devices
//! scanner-cli --profile "Документы" --out ~/scan.pdf
//! scanner-cli --device airscan:e0:Brother --dpi 300 --mode gray \
//!             --source adf --format pdf --ocr --out scan.pdf
//! ```
//!
//! Коды возврата: 0 — успех; 1 — ошибка сканирования/экспорта;
//! 2 — неверные аргументы.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;

use scanner_core::backend::{ScanProgress, ScannerBackend};
use scanner_core::options::{ScanMode, ScanSource};
use scanner_core::{create_backend, naming, ScanOptions, Store};

const USAGE: &str = "\
scanner-cli — консольный режим «Сканера документов»

Использование:
  scanner-cli --list-devices
  scanner-cli [параметры] --out <файл>

Параметры:
  -d, --device <имя>      SANE-имя устройства (см. --list-devices);
                          по умолчанию — первое найденное
  -p, --profile <имя>     сохранённый профиль (устройство/DPI/режим/источник)
      --dpi <75..600>     разрешение (по умолчанию 300)
  -m, --mode <режим>      color | gray | lineart
  -s, --source <источник> flatbed | adf | adf_duplex
  -f, --format <формат>   pdf | png | jpeg | tiff (по умолчанию pdf)
  -o, --out <файл>        куда сохранить (обязателен для сканирования)
      --template <шаблон> шаблон имени: {date} {time} {profile} {device}
      --ocr               поисковый PDF через Tesseract (нужен tesseract)
      --ocr-langs <langs> языки OCR: rus, eng, rus+eng (по умолчанию rus+eng)
      --blank-skip        пропускать пустые страницы
      --preview           предпросмотр планшета в низком DPI (одна страница)
      --list-devices      показать устройства и выйти
  -h, --help              эта справка
  -V, --version           версия

Примеры:
  scanner-cli --list-devices
  scanner-cli -p \"Договоры\" --out /tmp/out.pdf --blank-skip
  SCANNER_BACKEND=mock scanner-cli --out demo.pdf";

/// Аргументы командной строки.
#[derive(Debug, Default)]
struct Args {
    device: Option<String>,
    profile: Option<String>,
    dpi: Option<u32>,
    mode: Option<ScanMode>,
    source: Option<ScanSource>,
    format: Option<Format>,
    out: Option<PathBuf>,
    template: Option<String>,
    ocr: bool,
    ocr_langs: Option<String>,
    blank_skip: bool,
    preview: bool,
    list_devices: bool,
    help: bool,
    version: bool,
}

#[derive(Debug, Clone, Copy)]
enum Format {
    Pdf,
    Png,
    Jpeg,
    Tiff,
}

impl Format {
    fn parse(s: &str) -> Option<Format> {
        match s.to_lowercase().as_str() {
            "pdf" => Some(Format::Pdf),
            "png" => Some(Format::Png),
            "jpg" | "jpeg" => Some(Format::Jpeg),
            "tiff" | "tif" => Some(Format::Tiff),
            _ => None,
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Format::Pdf => "pdf",
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Tiff => "tiff",
        }
    }
}

fn parse_args<I: Iterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut take = |name: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{name}: ожидается значение"))
        };
        match arg.as_str() {
            "-d" | "--device" => a.device = Some(take("--device")?),
            "-p" | "--profile" => a.profile = Some(take("--profile")?),
            "--dpi" => {
                let v = take("--dpi")?;
                a.dpi = Some(v.parse().map_err(|_| format!("--dpi: не число: {v}"))?);
            }
            "-m" | "--mode" => {
                let v = take("--mode")?;
                a.mode = Some(
                    ScanMode::from_key(&v.to_lowercase())
                        .ok_or_else(|| format!("--mode: неизвестный режим: {v}"))?,
                );
            }
            "-s" | "--source" => {
                let v = take("--source")?;
                a.source = Some(
                    ScanSource::from_key(&v.to_lowercase())
                        .ok_or_else(|| format!("--source: неизвестный источник: {v}"))?,
                );
            }
            "-f" | "--format" => {
                let v = take("--format")?;
                a.format = Some(
                    Format::parse(&v)
                        .ok_or_else(|| format!("--format: неизвестный формат: {v}"))?,
                );
            }
            "-o" | "--out" => a.out = Some(PathBuf::from(take("--out")?)),
            "--template" => a.template = Some(take("--template")?),
            "--ocr" => a.ocr = true,
            "--ocr-langs" => a.ocr_langs = Some(take("--ocr-langs")?),
            "--blank-skip" => a.blank_skip = true,
            "--preview" => a.preview = true,
            "--list-devices" => a.list_devices = true,
            "-h" | "--help" => a.help = true,
            "-V" | "--version" => a.version = true,
            other => return Err(format!("неизвестный аргумент: {other} (см. --help)")),
        }
    }
    Ok(a)
}

fn print_devices(backend: &dyn ScannerBackend) -> anyhow::Result<()> {
    let devices = backend.list_devices()?;
    if devices.is_empty() {
        println!("Сканеры не найдены.");
        return Ok(());
    }
    println!("Доступные сканеры:");
    for d in &devices {
        println!("  {} — {}", d.device_name, d.name);
    }
    Ok(())
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    let mut args = match parse_args(std::env::args().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            return ExitCode::from(2);
        }
    };
    if args.help {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.version {
        println!("scanner-cli {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let backend = create_backend();

    if args.list_devices {
        return match print_devices(backend.as_ref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Ошибка поиска устройств: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // --- Подготовка параметров ---
    let store = Store::open_default().ok();
    let mut options = ScanOptions::default();
    let mut device_name = args.device.clone();
    let mut profile_name: Option<String> = None;
    let mut blank_threshold = 0.01f32;

    if let Some(name) = &args.profile {
        let profiles = store
            .as_ref()
            .map(|s| s.list_profiles().unwrap_or_default())
            .unwrap_or_default();
        match profiles.iter().find(|p| &p.name == name) {
            Some(p) => {
                options = p.options;
                if p.device_name.is_some() && device_name.is_none() {
                    device_name = p.device_name.clone();
                }
                profile_name = Some(p.name.clone());
                if p.skip_blank {
                    args.blank_skip = true; // флаги складываются
                }
                blank_threshold = p.blank_threshold;
            }
            None => {
                eprintln!("Ошибка: профиль не найден: {name}");
                eprintln!(
                    "Доступные: {}",
                    profiles
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                return ExitCode::from(2);
            }
        }
    }
    if let Some(dpi) = args.dpi {
        options.resolution = dpi;
    }
    if let Some(mode) = args.mode {
        options.mode = mode;
    }
    if let Some(source) = args.source {
        options.source = source;
    }

    if !args.preview && args.out.is_none() {
        eprintln!("Ошибка: для сканирования укажите --out <файл> (или --preview / --list-devices).");
        return ExitCode::from(2);
    }

    // --- Предпросмотр ---
    if args.preview {
        let device = match resolve_device(backend.as_ref(), device_name.as_deref()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Ошибка: {e}");
                return ExitCode::FAILURE;
            }
        };
        let out = args.out.clone().unwrap_or_else(|| PathBuf::from("preview"));
        let dir = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        return match backend.preview(&device, 75, &dir) {
            Ok(path) => {
                let target = PathBuf::from(format!("{}.png", out.display()));
                if path != target {
                    if std::fs::rename(&path, &target).is_err() {
                        let _ = std::fs::copy(&path, &target);
                    }
                }
                println!("Предпросмотр: {}", target.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("Ошибка предпросмотра: {e}");
                ExitCode::FAILURE
            }
        };
    }

    // --- Сканирование ---
    let out = args.out.expect("out проверен выше");
    let format = args.format.unwrap_or(Format::Pdf);
    let device = match resolve_device(backend.as_ref(), device_name.as_deref()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Ошибка: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = options.validate() {
        eprintln!("Ошибка: {e}");
        return ExitCode::from(2);
    }

    let out_dir = scanner_core::config::tmp_scan_dir()
        .join(format!("cli-{}", unix_stamp()));
    let (tx, rx) = mpsc::channel::<ScanProgress>();

    let backend_scan = backend.clone();
    let device_scan = device.clone();
    let scan_thread = std::thread::spawn(move || {
        backend_scan.scan(&device_scan, &options, &out_dir, &|p| {
            let _ = tx.send(p);
        })
    });

    // Прогресс в stderr; пропуск пустых страниц — по мере поступления.
    let mut pages: Vec<PathBuf> = Vec::new();
    let mut skipped_blank = 0usize;
    for ev in rx {
        match ev {
            ScanProgress::PageStarted(n) => eprintln!("страница {n}…"),
            ScanProgress::PageProgress(n, pct) => eprintln!("страница {n}: {pct:.0}%"),
            ScanProgress::PageDone(n, path) => {
                if args.blank_skip {
                    match scanner_core::imageproc::is_blank_file(&path, blank_threshold) {
                        Ok(true) => {
                            eprintln!("страница {n}: пустая — пропущена");
                            skipped_blank += 1;
                            continue;
                        }
                        _ => {}
                    }
                }
                pages.push(path);
            }
        }
    }

    let _ = scan_thread.join();
    if pages.is_empty() {
        eprintln!("Ошибка: не получено ни одной страницы (пропущено пустых: {skipped_blank})");
        return ExitCode::FAILURE;
    }

    // --- Экспорт ---
    let ctx = naming::NameContext {
        profile: profile_name.unwrap_or_else(|| "Скан".into()),
        device,
    };
    let out_stem = out
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let base_name = match &args.template {
        // Явный шаблон имеет приоритет.
        Some(t) => naming::expand_template(t, &ctx),
        // Иначе используем имя файла из --out, если оно задано.
        None => out_stem
            .unwrap_or_else(|| naming::expand_template(naming::DEFAULT_TEMPLATE, &ctx)),
    };
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    // Расширение берём из --out, если задано, иначе — от формата.
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| format.ext().to_string());
    let target = dir.join(format!("{base_name}.{ext}"));

    let result: anyhow::Result<()> = match format {
        Format::Pdf => {
            let want_ocr = args.ocr;
            if want_ocr && !scanner_core::ocr::available() {
                eprintln!("Предупреждение: tesseract не найден — сохраняю обычный PDF.");
            }
            if want_ocr && scanner_core::ocr::available() {
                let langs = args.ocr_langs.clone().unwrap_or_else(|| "rus+eng".into());
                ocr_pdf(&pages, &target, &langs)
            } else {
                scanner_core::export::export_pdf(&pages, &target, options.resolution, 88)
                    .map_err(anyhow::Error::from)
            }
        }
        Format::Png => scanner_core::export::export_png(&pages, &target.with_extension(""))
            .map(|_| ())
            .map_err(anyhow::Error::from),
        Format::Jpeg => {
            scanner_core::export::export_jpeg(&pages, &target.with_extension(""), 90)
                .map(|_| ())
                .map_err(anyhow::Error::from)
        }
        Format::Tiff => scanner_core::export::export_tiff(&pages, &target.with_extension(""))
            .map(|_| ())
            .map_err(anyhow::Error::from),
    };

    match result {
        Ok(()) => {
            println!("Сохранено: {}", target.display());
            if skipped_blank > 0 {
                println!("Пропущено пустых страниц: {skipped_blank}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Ошибка экспорта: {e}");
            ExitCode::FAILURE
        }
    }
}

fn unix_stamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Выбрать устройство: указанное или первое найденное.
fn resolve_device(backend: &dyn ScannerBackend, device: Option<&str>) -> Result<String, String> {
    if let Some(d) = device {
        return Ok(d.to_string());
    }
    let list = backend
        .list_devices()
        .map_err(|e| format!("не удалось получить список устройств: {e}"))?;
    list.first()
        .map(|d| d.device_name.clone())
        .ok_or_else(|| "сканеры не найдены; укажите --device".into())
}

/// OCR: страницы -> постраничные поисковые PDF -> слияние (+ .txt рядом).
fn ocr_pdf(pages: &[PathBuf], target: &std::path::Path, langs: &str) -> anyhow::Result<()> {
    let tmp = target
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join(format!(".ocr-{}", unix_stamp()));
    std::fs::create_dir_all(&tmp)?;
    let mut page_pdfs = Vec::new();
    let mut texts = String::new();
    for (i, page) in pages.iter().enumerate() {
        let base = tmp.join(format!("page-{:04}", i + 1));
        let (pdf, txt) = scanner_core::ocr::ocr_page(page, &base, langs, true, true)?;
        if let Some(pdf) = pdf {
            page_pdfs.push(pdf);
        }
        if let Some(txt) = txt {
            if let Ok(s) = std::fs::read_to_string(&txt) {
                texts.push_str(&s);
                texts.push('\n');
            }
        }
    }
    if page_pdfs.is_empty() {
        anyhow::bail!("tesseract не вернул поисковых страниц");
    }
    scanner_core::export::merge_pdfs(&page_pdfs, target)?;
    if let Some(dir) = target.parent() {
        let stem = target
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scan");
        let _ = std::fs::write(dir.join(format!("{stem}.txt")), texts);
    }
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}
