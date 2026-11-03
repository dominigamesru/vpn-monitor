//! ZIP без сжатия (STORE) — вложения запроса в Jira: `metrics.zip` (сутки и неделя), v0.9 / 0.9.9. Без внешних библиотек:
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
    many(&[(name, data)])
}

/// Архив из нескольких файлов (0.9.9: `metrics.zip` — история за сутки и за неделю).
pub fn many(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32(data);
        let (n, len) = (name.as_bytes(), data.len() as u32);
        let offset = out.len() as u32;
        // локальный заголовок: версия, флаги, STORE, время 00:00, дата 1980-01-01
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(n);
        out.extend_from_slice(data);
        // запись центрального каталога
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&(n.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0; 12]); // доп. поле, комментарий, диск, атрибуты
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(n);
    }
    let cd = out.len() as u32;
    let cd_len = central.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&central);
    // конец каталога
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
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

    #[test]
    fn two_files_in_one_archive() {
        let z = many(&[
            ("metrics_1d.json", b"{}"),
            ("metrics_7d.json", b"{\"r\":1}"),
        ]);
        // конец каталога: 2 записи
        let e = &z[z.len() - 22..];
        assert_eq!(&e[..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([e[8], e[9]]), 2);
        assert_eq!(z.windows(4).filter(|w| *w == b"PK\x01\x02").count(), 2);
        let _ = std::fs::write(std::env::temp_dir().join("vpnmon-zip2-test.zip"), &z);
    }
}
