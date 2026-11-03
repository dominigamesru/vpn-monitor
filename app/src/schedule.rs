//! Расписание проверок (0.9.8, заказчик 03.11): всё хорошо — раз в 15 минут, жёлтое или красное — раз в 10. Чтобы все
//! сотрудники не стучались в офис одновременно, у каждого свой сдвиг: минута часа (для 15 — от 1 до 15, для 10 — от 1
//! до 10) и секунда внутри минуты. Сдвиг выбирается случайно один раз и хранится в `user.json` → `slot`.
//! Пример: сдвиг 7 при 15 минутах — проверки в :07, :22, :37, :52.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct Slot {
    /// Минута для 15-минутного интервала, 1–15.
    #[serde(default)]
    pub green_min: u32,
    /// Минута для 10-минутного интервала, 1–10.
    #[serde(default)]
    pub alert_min: u32,
    /// Секунда внутри минуты, 0–59.
    #[serde(default)]
    pub sec: u32,
}

impl Slot {
    /// Случайный сдвиг: из времени запуска, номера процесса и имени компьютера (без внешних библиотек; равномерности
    /// хватает, чтобы разнести сотрудников).
    pub fn random() -> Self {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        h.write(std::env::var("COMPUTERNAME").unwrap_or_default().as_bytes());
        let x = h.finish();
        Self {
            green_min: (x % 15) as u32 + 1,
            alert_min: ((x >> 16) % 10) as u32 + 1,
            sec: ((x >> 32) % 60) as u32,
        }
    }

    pub fn valid(&self) -> bool {
        (1..=15).contains(&self.green_min) && (1..=10).contains(&self.alert_min) && self.sec < 60
    }

    /// Минута сдвига для интервала `interval_min` (15 → `green_min`, 10 → `alert_min`, другой — по модулю).
    pub fn minute_for(&self, interval_min: u64) -> u64 {
        match interval_min {
            15 => self.green_min as u64,
            10 => self.alert_min as u64,
            _ => self.green_min as u64,
        }
    }
}

/// Когда следующая проверка, секунды UNIX: ближайший момент позже `now + min_gap`, когда минута часа ≡ сдвигу по
/// модулю интервала и секунда — `slot.sec`. Интервал не в целых минутах или не делит час — просто `now + interval`.
pub fn next_at(now: u64, interval_secs: u64, slot: &Slot, min_gap: u64) -> u64 {
    if interval_secs == 0
        || !interval_secs.is_multiple_of(60)
        || !3600u64.is_multiple_of(interval_secs)
    {
        return now + interval_secs.max(60);
    }
    let i = interval_secs / 60;
    let off = slot.minute_for(i) % i;
    let start = now + min_gap;
    // минуты вперёд от начала текущей минуты — достаточно двух интервалов
    let base = now / 60 * 60;
    (0..=2 * i + 1)
        .map(|k| base + k * 60 + slot.sec as u64)
        .find(|t| *t > start && (t / 60 % 60) % i == off)
        .unwrap_or(now + interval_secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 1_793_620_800; // 2026-11-03 10:00:00 UTC — ровно час
    fn s(g: u32, a: u32, sec: u32) -> Slot {
        Slot {
            green_min: g,
            alert_min: a,
            sec,
        }
    }

    #[test]
    fn example_from_customer_7_22_37_52() {
        assert_eq!(H % 3600, 0);
        let sl = s(7, 3, 20);
        let mut t = H;
        let mut mins = vec![];
        for _ in 0..4 {
            t = next_at(t, 900, &sl, 60);
            mins.push((t / 60 % 60, t % 60));
        }
        assert_eq!(mins, vec![(7, 20), (22, 20), (37, 20), (52, 20)]);
        // при проблемах — раз в 10 минут со своим сдвигом 3: :03, :13, :23…
        assert_eq!(next_at(H + 5 * 60, 600, &sl, 60) - H, 13 * 60 + 20);
    }

    #[test]
    fn edge_minutes_and_gap() {
        // сдвиг 15 при 15 минутах — :00, :15, :30, :45
        assert_eq!(next_at(H + 1, 900, &s(15, 10, 0), 60) - H, 15 * 60);
        // сдвиг 10 при 10 минутах — :00, :10…
        assert_eq!(next_at(H + 1, 600, &s(1, 10, 0), 60) - H, 10 * 60);
        // только что проверили в :07:20 — следующая не раньше чем через минуту, т.е. в :22:20
        assert_eq!(
            next_at(H + 7 * 60 + 20, 900, &s(7, 3, 20), 60) - H,
            22 * 60 + 20
        );
        // 3 минуты (старые настройки) — тоже по сетке: 7 % 3 = 1 → :01:20, :04:20…
        assert_eq!(next_at(H, 180, &s(7, 3, 20), 60), H + 80);
        // интервал не делит час (из office.json) — просто через него
        assert_eq!(next_at(H, 420, &s(7, 3, 20), 60), H + 420);
    }

    #[test]
    fn random_slot_in_range() {
        for _ in 0..200 {
            assert!(Slot::random().valid());
        }
        assert!(!Slot::default().valid());
    }
}
