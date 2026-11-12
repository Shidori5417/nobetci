# Nöbetçi — Geliştirme Prompt'u

> Bu dosyayı Claude Code'a (veya başka bir kodlama ajanına) ver. Proje sıfırdan buna göre kurulacak.

---

Rust ile **Nöbetçi** adında bir çoklu sunucu izleme aracı geliştirmeni istiyorum. Araç terminal arayüzü (TUI) olarak çalışacak, GitHub'da açık kaynak yayınlanacak. Bu yüzden kod kalitesi, README ve dokümantasyon önemli.

## Amaç

Birden fazla Linux sunucusuna (VDS/VPS) **SSH üzerinden, agent kurmadan** bağlanmak. CPU, RAM, disk, ağ, systemd servisleri ve logları tek ekranda göstermek. Sorun olduğunda Discord ve/veya Telegram'a bildirim göndermek.

## Teknik gereksinimler

- **Dil:** Tamamen Rust (stable). Python, shell script bağımlılığı veya Rust dışında yardımcı binary olmayacak.
- **TUI:** `ratatui` + `crossterm`
- **Async:** `tokio`
- **SSH:** `russh` + `russh-keys`, yani saf Rust. Sistemdeki `ssh` binary'si çağrılmayacak.
  - `~/.ssh/config` okunabilmeli (`ssh_host = "vds1"` gibi).
  - Kimlik doğrulama: anahtar dosyası ve `ssh-agent`. Şifre desteği olmayacak.
  - Host anahtarı `~/.ssh/known_hosts` ile doğrulanacak. Bilinmeyen host varsayılan olarak reddedilecek, kabul etmek için açık bir onay gerekecek.
- **Config:** TOML (`serde` + `toml`), konum `~/.config/nobetci/config.toml` (`directories` crate)
- **CLI:** `clap` (derive). Alt komutlar: `nobetci` (TUI), `nobetci daemon`, `nobetci init`, `nobetci check` (config ve bağlantı testi)
- **HTTP (bildirimler):** `reqwest` (rustls, native-tls yok)
- **Hata yönetimi:** `thiserror` (kütüphane katmanı) + `color-eyre` (binary)
- **Log:** `tracing` + `tracing-subscriber`. TUI açıkken loglar dosyaya yazılacak, ekrana basılmayacak.

## Mimari

Cargo workspace ya da en azından net modül ayrımı olacak:

- `collector`: Her sunucu için ayrı bir tokio task'ı. Tek bir SSH oturumunu açık tutup periyodik olarak komut çalıştıracak.
  - CPU: `/proc/stat` (iki örnek arasındaki fark), RAM ve swap: `/proc/meminfo`, load: `/proc/loadavg`
  - Disk: `df -P -B1`, ağ: `/proc/net/dev` (fark), uptime: `/proc/uptime`
  - Servisler: `systemctl show <svc> --property=ActiveState,SubState,...`
  - Mümkünse metrikleri **tek bir SSH exec çağrısında** topla (komutları birleştir, çıktıyı ayraçlarla böl). Her tick'te yeni bağlantı açılmayacak.
  - Bağlantı koptuğunda exponential backoff ile yeniden bağlanacak.
- `parser`: Ham çıktıyı tiplenmiş struct'lara çeviren saf fonksiyonlar. **Bu katman unit test ile iyi kapsanmalı**, örnek `/proc` çıktıları `tests/fixtures/` içinde tutulacak.
- `state`: Sunucu başına son durum + ring buffer geçmişi (sparkline için, ör. son 120 örnek)
- `alert`: Kural motoru. `disk_usage > 90`, `cpu > 85`, `mem > 90`, `service_down`, `unreachable` gibi kurallar ve `for = "5m"` süre koşulu olacak. Aynı uyarı tekrar tekrar gönderilmeyecek, sorun düzelince "çözüldü" bildirimi gidecek.
- `notify`: `Notifier` trait'i, Discord (webhook embed) ve Telegram implementasyonları. Rate limit ve retry olacak.
- `ui`: ratatui ekranları
- `daemon`: TUI'siz çalışma modu. `collector`, `alert` ve `notify` katmanlarını aynen kullanacak. `contrib/nobetci.service` systemd unit dosyası da eklenecek.

Collector'lar ile UI ve alert arasındaki iletişim kanallarla (`tokio::sync::watch` / `mpsc`) yapılacak. UI collector'ı asla bloklamayacak.

## TUI ekranları

1. **Genel bakış:** Her sunucu bir kart/satır. Ad, durum (🟢/🟡/🔴), CPU, RAM ve disk gauge'ları, load, uptime, düşen servis sayısı.
2. **Sunucu detayı:** CPU (çekirdek bazında), RAM ve ağ sparkline'ları, disk tablosu, servis listesi.
3. **Loglar:** Seçili sunucu/servis için canlı `journalctl -f -u <svc>`, `/` ile arama ve filtre, scroll.
4. **Uyarılar:** Aktif ve geçmiş uyarılar.
5. **Yardım (`?`):** Kısayol listesi.

Kısayollar: `j/k` ve oklar, `Enter`, `Esc`, `l`, `s`, `r` (onaylı restart), `/`, `?`, `q`. Terminal yeniden boyutlandırılınca düzgün çizilecek. Panic olursa terminal eski haline döndürülecek.

## Güvenlik

- Yazma işlemleri (servis restart vb.) varsayılan olarak **kapalı**, `allow_actions = true` ile açılacak, her işlemde onay istenecek.
- Config'teki token/webhook değerleri loglara yazılmayacak.
- Sunucudan gelen çıktı güvenilmez kabul edilecek. Parse hataları uygulamayı çökertmeyecek, sadece o metriği "bilinmiyor" gösterecek.

## Kalite ve GitHub

- `cargo fmt`, `cargo clippy -- -D warnings` temiz olacak
- GitHub Actions: fmt, clippy, test; tag atınca Linux x86_64 ve aarch64 için release binary'leri
- `LICENSE` (MIT), `README.md` (taslağı repoda var), `CHANGELOG.md`, `config.example.toml`
- README görselleri için `assets/demo.tape` dosyası ([vhs](https://github.com/charmbracelet/vhs) ile GIF üretmek için)
- Gerçek sunucu olmadan geliştirme yapabilmek için `--demo` bayrağı olacak. Bu bayrak sahte veriyle TUI'yi çalıştıracak (ekran görüntüsü almak için de işe yarar).

## Çalışma şekli

Aşamalar halinde ilerle, her aşamanın sonunda derlenen ve çalışan bir durum olsun:

1. İskelet: workspace, CLI, config okuma, `--demo` ile sahte veriyle genel bakış ekranı
2. SSH collector + parser'lar + testler, tek sunucuda gerçek veri
3. Çoklu sunucu, yeniden bağlanma, detay ekranı ve sparkline'lar
4. Servis takibi + log görüntüleyici
5. Alert motoru + Discord/Telegram + daemon modu
6. CI, release, README görselleri, son temizlik

Her aşamadan önce kısa bir plan yaz, sonra kodla. Belirsiz bir nokta olursa varsayım yapıp `// VARSAYIM:` yorumuyla işaretle.
