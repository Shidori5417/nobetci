//! Sunucu durumunu temsil eden tipler. Parser bunları üretir, UI ve alert tüketir.
//! Bir alan `None` ise o metrik "bilinmiyor" demektir.

use std::time::{Duration, Instant};

/// `/proc/stat` içindeki tek bir CPU satırının ham sayaçları (jiffies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

/// `/proc/stat` çıktısı: toplam satır + çekirdek başına satırlar.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CpuStat {
    pub total: CpuTimes,
    pub cores: Vec<CpuTimes>,
}

/// İki örnek arası hesaplanmış CPU kullanımı (0.0..=100.0).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CpuUsage {
    pub total: f64,
    pub cores: Vec<f64>,
}

/// `/proc/meminfo` özeti, bayt cinsinden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemInfo {
    pub total: u64,
    pub available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
}

/// `/proc/loadavg`
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// `df -P -B1 -T` satırı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskUsage {
    pub filesystem: String,
    /// Dosya sistemi türü (ext4, btrfs, vfat...). `df -T`'den gelir.
    pub fstype: String,
    pub mount: String,
    pub total: u64,
    pub used: u64,
    pub available: u64,
}

/// `/proc/net/dev` satırı (kümülatif sayaçlar).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetDev {
    pub iface: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// `systemctl show` ile okunan servis durumu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub name: String,
    pub load_state: String,
    pub active_state: String,
    pub sub_state: String,
}

impl ServiceStatus {
    pub fn is_running(&self) -> bool {
        self.active_state == "active"
    }

    /// Unit hiç yüklenmemiş (dosyası yok): `systemctl show` `LoadState=not-found` verir.
    pub fn is_missing(&self) -> bool {
        self.load_state == "not-found"
    }
}

/// Bir sunucunun genel sağlık durumu.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Health {
    Ok,
    Warning,
    Critical,
    #[default]
    Unknown,
}

/// UI'nin tek bir sunucu için gösterdiği anlık görüntü.
#[derive(Debug, Clone, Default)]
pub struct ServerSnapshot {
    pub name: String,
    /// Collector en az bir kez denedi mi? `false` ise sunucu "bilinmiyor" sayılır.
    pub checked: bool,
    pub reachable: bool,
    pub cpu: Option<CpuUsage>,
    pub mem: Option<MemInfo>,
    pub load: Option<LoadAvg>,
    pub disks: Option<Vec<DiskUsage>>,
    pub uptime: Option<Duration>,
    pub services: Vec<ServiceStatus>,
    /// Toplam ağ hızı (bayt/sn); ilk örnekte veya hata durumunda `None`.
    pub net_rx_bps: Option<f64>,
    pub net_tx_bps: Option<f64>,
    /// Son bağlantı/okuma hatası; UI'de seçili sunucu için gösterilir.
    pub error: Option<String>,
    /// Kaynağın her yayında artırdığı örnek numarası; UI tekrar eden örneği geçmişe eklemez.
    pub seq: u64,
    /// Erişilemezken bir sonraki bağlanma denemesinin zamanı.
    pub retry_at: Option<Instant>,
}
