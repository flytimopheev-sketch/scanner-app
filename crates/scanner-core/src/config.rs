//! Пути к данным приложения (XDG Base Directory).

use std::path::PathBuf;

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp"))
}

fn xdg_dir(var: &str, default: &str) -> PathBuf {
    std::env::var(var).map(PathBuf::from).unwrap_or_else(|_| home().join(default))
}

/// Каталог данных: `$XDG_DATA_HOME/scanner-app` (`~/.local/share/scanner-app`).
pub fn data_dir() -> PathBuf {
    let dir = xdg_dir("XDG_DATA_HOME", ".local/share").join("scanner-app");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Каталог конфигурации: `$XDG_CONFIG_HOME/scanner-app`.
pub fn config_dir() -> PathBuf {
    let dir = xdg_dir("XDG_CONFIG_HOME", ".config").join("scanner-app");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Путь к базе SQLite (устройства, профили, настройки).
pub fn db_path() -> PathBuf {
    data_dir().join("scanner-app.db")
}

/// Временный каталог для отсканированных страниц.
pub fn tmp_scan_dir() -> PathBuf {
    let dir = data_dir().join("tmp").join("scans");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Каталог журналов.
pub fn logs_dir() -> PathBuf {
    let dir = data_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Путь к журналу ошибок GUI.
pub fn gui_log_path() -> PathBuf {
    logs_dir().join("gui.log")
}

/// Простое добавление строки в журнал ошибок с меткой времени.
pub fn append_log(msg: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(gui_log_path()) {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}
