# Değişiklik Günlüğü

Bu dosyadaki tüm önemli değişiklikler burada tutulur.

Biçim [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/) standardına,
sürümleme [Semantic Versioning](https://semver.org/lang/tr/) kurallarına uyar.

## [Unreleased]

### Değişti

- Config dosyası yokken `nobetci init` / `nobetci --demo` öneren sade bir mesaj gösterilir.
- Hatalar kaynak konumu ve backtrace notu olmadan `Hata: ...` olarak yazılır; tam rapor için `RUST_BACKTRACE=1`.

## [0.1.0] - 2026-11-12

### Eklendi

- CLI: `nobetci` (TUI), `nobetci init [--force]`, `nobetci check [--test-notify]` ve `nobetci daemon` alt komutları; `--config` ve `--demo` genel bayrakları.
- TOML config (`~/.config/nobetci/config.toml`): `interval`, `[[server]]` (`ssh_host` veya `host`/`user`/`port`/`key`, `services`, `allow_actions`), `[notify.*]` ve `[[alert]]` blokları; doğrulama ve örnek `config.example.toml`.
- Agent'sız SSH toplama (saf Rust, `russh`): tek oturumda `/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/net/dev`, `/proc/uptime`, `df` ve `systemctl show` çıktısı; üstel geri çekilmeyle yeniden bağlanma; `~/.ssh/config` desteği; anahtar dosyası ve `ssh-agent` ile kimlik doğrulama. Parser katmanı birim testlerle kapsanır.
- Genel bakış ekranı: sunucu kartları, CPU/RAM/disk göstergeleri, load, uptime, düşen servis ve aktif uyarı sayısı.
- Detay ekranı: çekirdek bazında CPU, RAM, swap, disk, ağ trafiği, sparkline geçmişi ve servis listesi (`s` ile odaklanma).
- Onaylı servis yeniden başlatma (`r`): yalnızca `allow_actions = true` sunucularda; uzak kullanıcı root değilse `sudo -n systemctl restart` (parola istemez), root kontrolü uzakta `id -u` ile yapılır.
- Canlı log görüntüleyici (`l`): `journalctl -f`, sınırlı tampon, takip modu, kaydırma, arama (`/`, `n`/`N`, vurgu) ve yalnızca eşleşenleri gösterme (`f`).
- Uyarı motoru: `cpu`, `mem`, `disk_usage`, `swap`, `load` karşılaştırmaları (`>`, `>=`, `<`, `<=`), `service_down`, `unreachable`, `for` süre koşulu; tek ateşleme ve tek "çözüldü" olayı.
- Discord (webhook) ve Telegram bildirimcileri; yeniden deneme ve hız sınırı.
- `nobetci daemon`: TUI'siz izleme ve bildirim, SIGINT/SIGTERM ile temiz kapanış, loglar stderr'e (journald); `contrib/nobetci.service` systemd kullanıcı birimi.
- TUI Uyarılar ekranı (`a`): aktif uyarılar ve geçmiş. TUI bildirim göndermez, bildirimi yalnızca daemon gönderir.
- `--demo` modu: sahte metrikler, log akışı, restart ve uyarılar (gerçek sunucu gerekmez).
- GitHub Actions: fmt, clippy ve test iş akışı; `v*` etiketinde Linux x86_64 ve aarch64 release paketleri (sha256 ile).
- `assets/demo.tape` ile vhs üzerinden README GIF'i üretimi.

### Güvenlik

- Host anahtarı `~/.ssh/known_hosts` ile doğrulanır; bilinmeyen veya değişmiş anahtar reddedilir. `known_hosts`'a yazma yalnızca `nobetci check` içindeki açık onaydan sonra yapılır.
- SSH parolası desteklenmez.
- Yazma işlemleri (restart) varsayılan olarak kapalıdır ve her seferinde onay ister.
- Webhook URL'si ve bot token'ı `Debug` ve loglarda `***` olarak gizlenir; `nobetci check` sır göstermez.
- Sunucudan gelen log satırlarındaki ANSI/kontrol karakterleri temizlenir.
