//! Yerel saat biçimleme. Yerel UTC farkı çok iş parçacıklı süreçte güvenilir okunamaz
//! (`time` crate'i bunu reddeder), bu yüzden tokio başlamadan önce bir kez okunur.

use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

use time::{OffsetDateTime, UtcOffset};

static OFFSET: OnceLock<UtcOffset> = OnceLock::new();

/// Tek iş parçacıklıyken (runtime kurulmadan önce) çağrılmalı.
// VARSAYIM: okunamazsa (ör. çok iş parçacıklı) UTC kullanılır.
pub fn init_local_offset() {
    let _ = OFFSET.set(UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC));
}

/// Yerel saat: son 24 saat içindeyse `HH:MM:SS`, daha eskiyse `AA-GG HH:MM:SS`.
pub fn local(t: SystemTime) -> String {
    let off = OFFSET.get().copied().unwrap_or(UtcOffset::UTC);
    let dt = OffsetDateTime::from(t).to_offset(off);
    let (h, m, s) = dt.to_hms();
    let recent = SystemTime::now()
        .duration_since(t)
        .is_ok_and(|d| d < Duration::from_secs(86_400));
    if recent {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!(
            "{:02}-{:02} {h:02}:{m:02}:{s:02}",
            dt.month() as u8,
            dt.day()
        )
    }
}

/// "5 sn", "3 dk", "2 sa 10 dk".
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} sn"),
        60..=3599 => format!("{} dk", s / 60),
        _ => format!("{} sa {} dk", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saat_ve_sure_bicimi() {
        assert_eq!(duration(Duration::from_secs(5)), "5 sn");
        assert_eq!(duration(Duration::from_secs(125)), "2 dk");
        assert_eq!(duration(Duration::from_secs(7500)), "2 sa 5 dk");
        assert_eq!(local(SystemTime::now()).len(), 8);
        assert!(local(SystemTime::UNIX_EPOCH).contains('-'));
    }
}
