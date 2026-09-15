//! Модель устройства сканирования.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Usb,
    Network,
    Unknown,
}

/// Устройство сканирования.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannerDevice {
    /// Стабильный идентификатор в приложении (хэш полного SANE-имени).
    pub id: String,
    /// Отображаемое имя (модель).
    pub name: String,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub kind: DeviceKind,
    /// Имя SANE-backend'а ("airscan", "pixma", "epsonds", ...).
    pub backend: String,
    /// Полное имя устройства для SANE / scanimage.
    pub device_name: String,
    /// Адрес (IP/URI) для сетевых устройств.
    pub address: Option<String>,
    /// Протокол ("eSCL", "WSD", "usb", ...).
    pub protocol: Option<String>,
}

impl ScannerDevice {
    /// Сборка устройства из строки, которую вернул `scanimage -L` / SANE API.
    pub fn from_sane_string(raw: &str, vendor: Option<String>, model: Option<String>) -> Self {
        let backend = raw.split(':').next().unwrap_or("unknown").to_string();

        let (kind, protocol, address) = classify(&backend, raw);

        let name = model
            .clone()
            .or_else(|| vendor.clone())
            .unwrap_or_else(|| raw.to_string());

        ScannerDevice {
            id: short_id(raw),
            name,
            vendor,
            model,
            kind,
            backend,
            device_name: raw.to_string(),
            address,
            protocol,
        }
    }
}

/// Псевдостабильный короткий id: последние 8 hex-символов FNV-1a.
pub fn short_id(s: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", (hash & 0xffff_ffff) as u32)
}

fn classify(backend: &str, raw: &str) -> (DeviceKind, Option<String>, Option<String>) {
    let lower = raw.to_lowercase();

    // airscan:w1:HP LaserJet MFP — WSD; airscan:e0:Brother — eSCL
    if backend == "airscan" {
        let protocol = if lower.contains(":w") { Some("WSD".into()) } else { Some("eSCL".into()) };
        let address = extract_ip(raw);
        return (DeviceKind::Network, protocol, address);
    }
    if backend == "escl" {
        let address = extract_ip(raw);
        return (DeviceKind::Network, Some("eSCL".into()), address);
    }
    if lower.contains("http://") || lower.contains("https://") || lower.contains(":net:") {
        return (DeviceKind::Network, None, extract_ip(raw));
    }
    if backend == "ippusb" {
        return (DeviceKind::Usb, Some("eSCL (ipp-usb)".into()), None);
    }
    if lower.contains(":usb:") || backend != "test" && raw.contains(":usb:") {
        return (DeviceKind::Usb, Some("usb".into()), None);
    }
    (DeviceKind::Unknown, None, extract_ip(raw))
}

/// Поиск первого корректного IPv4-адреса в произвольной строке.
fn extract_ip(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_digit() && (i == 0 || !bytes[i - 1].is_ascii_digit()) {
            let mut j = i;
            let mut octets = [0u32; 4];
            let mut ok = true;
            for k in 0..4 {
                let (mut val, mut len) = (0u32, 0usize);
                while j < n && bytes[j].is_ascii_digit() && len < 3 {
                    val = val * 10 + (bytes[j] - b'0') as u32;
                    j += 1;
                    len += 1;
                }
                if len == 0 || val > 255 {
                    ok = false;
                    break;
                }
                octets[k] = val;
                if k < 3 {
                    if j < n && bytes[j] == b'.' {
                        j += 1;
                    } else {
                        ok = false;
                        break;
                    }
                }
            }
            // После адреса не должно идти продолжение числа.
            let boundary = j >= n || !bytes[j].is_ascii_digit();
            if ok && boundary {
                return Some(s[i..j].to_string());
            }
            i += 1;
        } else {
            i += 1;
        }
    }
    None
}
