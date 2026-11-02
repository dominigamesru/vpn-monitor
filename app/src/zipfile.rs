//! ZIP с одним файлом без сжатия (STORE) — вложение `metrics_7d.zip` в запрос в Jira (v0.9). Без внешних библиотек:
//! формат простой, а Jira и Windows открывают такой архив сами.

/// CRC-32 (IEEE), как в ZIP.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                (c >> 1) ^ 0xEDB8_8320
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Архив из одного файла `name` с содержимым `data`.
pub fn one(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let (n, len) = (name.as_bytes(), data.len() as u32);
    let mut out = Vec::with_capacity(data.len() + 128);
    // локальный заголовок
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]); // версия, флаги, STORE, время 00:00, дата 1980-01-01
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(n.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(n);
    out.extend_from_slice(data);
    let cd = out.len() as u32;
    // центральный каталог
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(n.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0; 12]); // доп. поле, комментарий, диск, атрибуты
    out.extend_from_slice(&0u32.to_le_bytes()); // смещение локального заголовка
    out.extend_from_slice(n);
    let cd_len = out.len() as u32 - cd;
    // конец каталога
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 1, 0, 1, 0]);
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_layout() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let z = one("metrics_7d.json", b"{\"records\":[]}");
        assert_eq!(&z[..4], b"PK\x03\x04");
        assert_eq!(&z[z.len() - 22..z.len() - 18], b"PK\x05\x06");
        assert!(z.windows(15).any(|w| w == b"metrics_7d.json"));
        let _ = std::fs::write(std::env::temp_dir().join("vpnmon-zip-test.zip"), &z);
    }
}
