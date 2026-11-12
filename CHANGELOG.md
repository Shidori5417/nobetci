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
- Detay ekranı: çekirdek çubukları alana göre çok sütunlu ızgarada gösterilir; sığmayanlar için "+N çekirdek" göstergesi.
- Servis takibi: detay ekranında `s` ile servis listesine odaklanma ve `j`/`k` ile gezinme.
- Onaylı servis yeniden başlatma (`r`): yalnızca `allow_actions = true` olan sunucularda; root değilse `sudo -n systemctl restart`, parola istemez; sonuç durum satırında gösterilir.
- Canlı log görüntüleyici (`l`): `journalctl -f` akışı, sınırlı tampon (5000 satır), takip modu, kaydırma, büyük/küçük harf duyarsız süzme; sunucu çıktısındaki ANSI/kontrol karakterleri temizlenir; kapatınca uzak `journalctl` durdurulur.
- Log araması: `/` arama (eşleşmeler vurgulanır, Türkçe/çok baytlı metinde güvenli), `n`/`N` ile eşleşmeler arasında gezinme (uçlarda sarar, takipten çıkar), `f` ile yalnızca eşleşenleri gösterme.
- Restart: root kontrolü SSH kullanıcı adına göre değil, uzakta çalışma anında (`id -u`) yapılır.
- `--demo`: sahte log akışı ve sahte restart (yalnızca `vds-1` için işlemler açık).
