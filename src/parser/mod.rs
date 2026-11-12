//! Ham komut çıktısını `crate::model` tiplerine çeviren saf fonksiyonlar.
//!
//! Bu katmanda IO yoktur; girdi sunucudan gelen güvenilmez metindir, bu yüzden
//! hiçbir fonksiyon panik atmaz. Bozuk girdi `ParseError` döner, böylece sadece
//! ilgili metrik "bilinmiyor" olur.

pub mod batch;
pub mod cpu;
pub mod disk;
pub mod load;
pub mod mem;
pub mod net;
pub mod service;
pub mod uptime;

pub use batch::{ParsedBatch, build_command, parse_batch, split_sections};

/// Parser hataları.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// Girdi boş (ya da sadece boşluk).
    #[error("girdi boş")]
    Empty,
    /// Girdi beklenen biçimde değil. `detail` en fazla 80 karakterdir.
    #[error("{what} ayrıştırılamadı: {detail}")]
    Malformed { what: &'static str, detail: String },
    /// Zorunlu bir alan bulunamadı.
    #[error("eksik alan: {0}")]
    MissingField(&'static str),
    /// Toplu çıktıda beklenen bölüm yok.
    #[error("eksik bölüm: {0}")]
    MissingSection(&'static str),
}

/// `Malformed` üretir; sunucu çıktısını loglara/UI'ya taşırken şişmesin diye kısaltır.
pub(crate) fn malformed(what: &'static str, detail: &str) -> ParseError {
    // VARSAYIM: 80 karakter hata ayıklamak için yeterli.
    let detail: String = detail.chars().take(80).collect();
    ParseError::Malformed { what, detail }
}
