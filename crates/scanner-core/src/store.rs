//! Хранилище: SQLite (устройства, профили сканирования, настройки).

use rusqlite::{params, Connection};
use std::path::Path;

use crate::errors::{Result, ScannerError};
use crate::options::{PageFormat, ScanMode, ScanOptions, ScanSource};

/// Профиль сканирования.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScanProfile {
    pub id: Option<i64>,
    pub name: String,
    /// Устройство, к которому привязан профиль (None — для любого).
    pub device_name: Option<String>,
    pub options: ScanOptions,
    /// Ручной дуплекс: два прохода податчика с переворотом стопа.
    #[serde(default)]
    pub duplex_manual: bool,
    /// Пропускать пустые страницы при сканировании.
    #[serde(default)]
    pub skip_blank: bool,
    /// Порог пустоты: доля чернил, ниже которой страница пуста (0.01 = 1 %).
    #[serde(default = "default_blank_threshold")]
    pub blank_threshold: f32,
}

fn default_blank_threshold() -> f32 {
    0.01
}

impl ScanProfile {
    /// Профиль «как есть» — только опции сканирования.
    pub fn from_options(name: &str, device: Option<String>, options: ScanOptions) -> Self {
        ScanProfile {
            id: None,
            name: name.into(),
            device_name: device,
            options,
            duplex_manual: false,
            skip_blank: false,
            blank_threshold: default_blank_threshold(),
        }
    }
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&crate::config::db_path())
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(SCHEMA)
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        // Добавочные миграции: поля профилей 0.2 (дуплекс, пустые страницы).
        let cols: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("PRAGMA table_info(profiles)")
                .map_err(|e| ScannerError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| ScannerError::Database(e.to_string()))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        let alters: &[(&str, &str)] = &[
            ("duplex_manual", "ALTER TABLE profiles ADD COLUMN duplex_manual INTEGER NOT NULL DEFAULT 0"),
            ("skip_blank", "ALTER TABLE profiles ADD COLUMN skip_blank INTEGER NOT NULL DEFAULT 0"),
            ("blank_threshold", "ALTER TABLE profiles ADD COLUMN blank_threshold REAL NOT NULL DEFAULT 0.01"),
        ];
        for (col, sql) in alters {
            if !cols.iter().any(|c| c == col) {
                self.conn
                    .execute_batch(sql)
                    .map_err(|e| ScannerError::Database(e.to_string()))?;
            }
        }
        Ok(())
    }

    // ---------- Устройства ----------

    /// Сохранить/обновить устройство (upsert по device_name).
    pub fn save_device(&self, d: &crate::device::ScannerDevice) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO scanners (id, name, backend, device_name, address, protocol, created_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), datetime('now'))
                 ON CONFLICT(device_name) DO UPDATE SET
                    name = excluded.name,
                    address = excluded.address,
                    protocol = excluded.protocol,
                    last_seen_at = datetime('now')",
                params![d.id, d.name, d.backend, d.device_name, d.address, d.protocol],
            )
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        Ok(())
    }

    /// Сохранённые устройства (добавленные вручную по IP и т.п.).
    pub fn list_devices(&self) -> Result<Vec<crate::device::ScannerDevice>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, backend, device_name, address, protocol, kind FROM scanners ORDER BY created_at")
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| ScannerError::Database(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (id, name, backend, device_name, address, protocol, kind) =
                row.map_err(|e| ScannerError::Database(e.to_string()))?;
            let kind = match kind.as_deref() {
                Some("usb") => crate::device::DeviceKind::Usb,
                Some("network") => crate::device::DeviceKind::Network,
                _ => crate::device::DeviceKind::Unknown,
            };
            out.push(crate::device::ScannerDevice {
                id,
                name,
                vendor: None,
                model: None,
                kind,
                backend,
                device_name,
                address,
                protocol,
            });
        }
        Ok(out)
    }

    pub fn delete_device(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM scanners WHERE id = ?1", params![id])
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        Ok(())
    }

    // ---------- Профили ----------

    pub fn save_profile(&self, p: &ScanProfile) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO profiles (name, device_name, resolution, mode, source, format, duplex_manual, skip_blank, blank_threshold)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    p.name,
                    p.device_name,
                    p.options.resolution,
                    p.options.mode.key(),
                    p.options.source.key(),
                    p.options.format.key(),
                    p.duplex_manual as i64,
                    p.skip_blank as i64,
                    p.blank_threshold,
                ],
            )
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_profiles(&self) -> Result<Vec<ScanProfile>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, device_name, resolution, mode, source, format,
                        duplex_manual, skip_blank, blank_threshold
                 FROM profiles ORDER BY id",
            )
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, u32>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, i64>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, f64>(9)?,
                ))
            })
            .map_err(|e| ScannerError::Database(e.to_string()))?;

        let mut out = Vec::new();
        for row in rows {
            let (id, name, device_name, resolution, mode, source, format, duplex, skip, thr) =
                row.map_err(|e| ScannerError::Database(e.to_string()))?;
            out.push(ScanProfile {
                id: Some(id),
                name,
                device_name,
                options: ScanOptions {
                    resolution,
                    mode: ScanMode::from_key(&mode).unwrap_or(ScanMode::Color),
                    source: ScanSource::from_key(&source).unwrap_or(ScanSource::Flatbed),
                    format: PageFormat::from_key(&format).unwrap_or(PageFormat::A4),
                },
                duplex_manual: duplex != 0,
                skip_blank: skip != 0,
                blank_threshold: thr as f32,
            });
        }
        Ok(out)
    }

    pub fn delete_profile(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM profiles WHERE id = ?1", params![id])
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        Ok(())
    }

    // ---------- Настройки ----------

    pub fn get_setting(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |r| r.get(0))
            .ok()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
            .map_err(|e| ScannerError::Database(e.to_string()))?;
        Ok(())
    }
}

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS scanners (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    backend TEXT NOT NULL,
    device_name TEXT NOT NULL UNIQUE,
    address TEXT,
    protocol TEXT,
    kind TEXT,
    created_at TEXT NOT NULL,
    last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS profiles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    device_name TEXT,
    resolution INTEGER NOT NULL DEFAULT 300,
    mode TEXT NOT NULL DEFAULT 'color',
    source TEXT NOT NULL DEFAULT 'flatbed',
    format TEXT NOT NULL DEFAULT 'a4',
    duplex_manual INTEGER NOT NULL DEFAULT 0,
    skip_blank INTEGER NOT NULL DEFAULT 0,
    blank_threshold REAL NOT NULL DEFAULT 0.01,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;
