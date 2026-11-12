# Değişiklik Günlüğü

Bu dosyadaki tüm önemli değişiklikler burada tutulur.

Biçim [Keep a Changelog](https://keepachangelog.com/tr/1.1.0/) standardına,
sürümleme [Semantic Versioning](https://semver.org/lang/tr/) kurallarına uyar.

## [Unreleased]

### Eklendi

- Proje iskeleti (Cargo projesi, modül yapısı, temel bağımlılıklar).
- CLI: `nobetci` (TUI), `nobetci init`, `nobetci check` ve `nobetci daemon` alt komutları.
- Config okuma: `~/.config/nobetci/config.toml` dosyasının okunması ve doğrulanması.
- `--demo` modu: gerçek sunucu olmadan sahte veriyle TUI'yi çalıştırma.
- Genel bakış ekranı: sunucu kartları, CPU/RAM/disk gauge'ları, load, uptime ve düşen servis sayısı.
- `/proc` parser'ları (`/proc/stat`, `/proc/meminfo`, `/proc/loadavg`, `/proc/net/dev`, `/proc/uptime`) ve bunların birim testleri.
