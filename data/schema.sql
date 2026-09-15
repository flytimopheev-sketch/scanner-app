-- Схема базы данных «Сканера документов» (SQLite).
-- Файл: ~/.local/share/scanner-app/scanner-app.db
-- Эта схема также встроена в scanner-core/src/store.rs и создаётся автоматически.

CREATE TABLE IF NOT EXISTS scanners (
    id TEXT PRIMARY KEY,              -- короткий хэш device_name
    name TEXT NOT NULL,               -- отображаемое имя
    backend TEXT NOT NULL,            -- sane backend: airscan, pixma, epsonds...
    device_name TEXT NOT NULL UNIQUE, -- полное имя устройства для scanimage
    address TEXT,                     -- IP:порт для сетевых устройств
    protocol TEXT,                    -- escl / wsd / usb
    kind TEXT,                        -- usb / network / unknown
    created_at TEXT NOT NULL,
    last_seen_at TEXT
);

CREATE TABLE IF NOT EXISTS profiles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    device_name TEXT,                 -- привязка к устройству (NULL — универсальный)
    resolution INTEGER NOT NULL DEFAULT 300,
    mode TEXT NOT NULL DEFAULT 'color',     -- color / gray / lineart
    source TEXT NOT NULL DEFAULT 'flatbed', -- flatbed / adf / adf_duplex
    format TEXT NOT NULL DEFAULT 'a4',      -- a4 / a5 / letter / max
    -- 0.2: режимы обработки (старые базы обновляются миграцией в store.rs)
    duplex_manual INTEGER NOT NULL DEFAULT 0,      -- ручной дуплекс (2 прохода)
    skip_blank INTEGER NOT NULL DEFAULT 0,         -- пропускать пустые страницы
    blank_threshold REAL NOT NULL DEFAULT 0.01,    -- порог пустоты (доля чернил)
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,             -- lang / theme / device / last_options / name_template / sign_cmd / watched_* / ocr_lang
    value TEXT NOT NULL
);
