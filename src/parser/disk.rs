//! `df -P -B1 -T` ayrıştırma.

use super::{ParseError, malformed};
use crate::model::DiskUsage;

/// Baştaki boşlukları atıp ilk boşluksuz parçayı ve geri kalanını döner.
fn next_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// Gerçek yerel disk olmayan (sanal/geçici) dosya sistemi türü mü?
fn is_pseudo(fstype: &str) -> bool {
    // VARSAYIM: fuse.* (AppImage, sshfs, fuse-overlayfs) izlenecek yerel disk değildir;
    // fuse.mergerfs birleştirilmiş yerel diskler olduğu için istisnadır.
    const TYPES: &[&str] = &[
        "tmpfs", "devtmpfs", "squashfs", "overlay", "efivarfs", "ramfs", "proc", "sysfs", "devpts",
    ];
    TYPES.contains(&fstype)
        || fstype.starts_with("cgroup")
        || (fstype.starts_with("fuse.") && fstype != "fuse.mergerfs")
}

/// Tek satırı çözer: `Some(None)` = tanınan ama süzülen satır, `None` = tanınmadı.
fn parse_line(line: &str) -> Option<Option<DiskUsage>> {
    // VARSAYIM: aygıt adı ve dosya sistemi türü boşluk içermez; bağlama noktası içerebilir.
    let (filesystem, rest) = next_token(line)?;
    let (fstype, rest) = next_token(rest)?;
    let (total, rest) = next_token(rest)?;
    let (used, rest) = next_token(rest)?;
    let (avail, rest) = next_token(rest)?;
    let (cap, rest) = next_token(rest)?;
    let (total, used, available) = (
        total.parse::<u64>().ok()?,
        used.parse::<u64>().ok()?,
        avail.parse::<u64>().ok()?,
    );
    cap.strip_suffix('%')
        .filter(|p| p == &"-" || p.parse::<u32>().is_ok())
        .or_else(|| (cap == "-").then_some(cap))?;
    // Satırın kalanı bağlama noktasıdır; iç boşlukları korunur.
    let mount = rest.trim().to_string();
    if mount.is_empty() {
        return None;
    }
    if total == 0 || is_pseudo(fstype) {
        return Some(None);
    }
    Some(Some(DiskUsage {
        filesystem: filesystem.to_string(),
        fstype: fstype.to_string(),
        mount,
        total,
        used,
        available,
    }))
}

/// `df -P -B1 -T` çıktısını okur. İlk satır (yerel dilde olabilen) başlıktır ve
/// metni kullanılmadan atlanır. Tanınmayan satırlar atlanır; sanal dosya sistemleri
/// süzülür; aynı aygıtın ikinci ve sonraki bağlama noktaları (btrfs alt birimleri)
/// atılır. Hiç satır tanınmazsa hata döner.
pub fn parse_df(input: &str) -> Result<Vec<DiskUsage>, ParseError> {
    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let mut recognized = 0usize;
    let mut out: Vec<DiskUsage> = Vec::new();
    for line in input.lines().skip(1) {
        if let Some(parsed) = parse_line(line) {
            recognized += 1;
            // VARSAYIM: aynı aygıt birden fazla yerde bağlıysa ilk bağlama noktası kalır.
            if let Some(d) = parsed {
                if !out.iter().any(|o| o.filesystem == d.filesystem) {
                    out.push(d);
                }
            }
        }
    }
    if recognized == 0 {
        return Err(malformed("df", input.lines().nth(1).unwrap_or("")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DF: &str = include_str!("../../tests/fixtures/df.txt");
    const BAD: &str = include_str!("../../tests/fixtures/malformed_df.txt");
    const HDR: &str = "Filesystem Type 1024-blok Used Available Capacity Mounted on\n";

    fn df(body: &str) -> String {
        format!("{HDR}{body}")
    }

    #[test]
    fn parses_fixture_and_filters() {
        let d = parse_df(DF).unwrap();
        let mounts: Vec<_> = d.iter().map(|x| x.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/boot/efi", "/mnt/yedek diski"]);
        assert_eq!(d[0].filesystem, "/dev/sda1");
        assert_eq!(d[0].fstype, "ext4");
        assert_eq!(d[0].total, 52576092160);
        assert_eq!(d[0].used, 21030436864);
        assert_eq!(d[0].available, 28829163520);
    }

    #[test]
    fn mount_with_spaces_preserved() {
        let d = parse_df(&df("/dev/sdc1 ext4 100 50 50 50% /mnt/a  b c\n")).unwrap();
        assert_eq!(d[0].mount, "/mnt/a  b c");
    }

    #[test]
    fn header_text_is_ignored() {
        // Yerel dilde başlık: içerik değil konum önemlidir.
        let d = parse_df("Dosya_sistemi Türü 1024-blok Kullanılan Boş Kapasite Bağlanma_noktası\n/dev/sda1 ext4 100 50 50 50% /\n").unwrap();
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn empty_and_garbage() {
        assert_eq!(parse_df(""), Err(ParseError::Empty));
        assert!(parse_df(&df("hello world\n")).is_err());
        assert!(parse_df(BAD).is_err());
        // sadece başlık
        assert!(parse_df(HDR).is_err());
    }

    #[test]
    fn only_pseudo_gives_empty_vec() {
        let d = parse_df(&df("tmpfs tmpfs 100 0 100 0% /run\n")).unwrap();
        assert!(d.is_empty());
    }

    #[test]
    fn pseudo_types_are_excluded() {
        let body = "\
udev devtmpfs 100 0 100 0% /dev
a tmpfs 100 0 100 0% /run
b squashfs 100 100 0 100% /snap/x
c overlay 100 50 50 50% /var/lib/docker
d efivarfs 100 10 90 10% /sys/firmware/efi/efivars
e ramfs 100 0 100 0% /ram
f proc 100 0 100 0% /proc
g sysfs 100 0 100 0% /sys
h devpts 100 0 100 0% /dev/pts
i cgroup2 100 0 100 0% /sys/fs/cgroup
j fuse.sshfs 100 50 50 50% /mnt/uzak
k fuse.T3-Code-0.1.9-x86_64.AppImage 100 100 0 100% /tmp/.mount_T3
";
        assert!(parse_df(&df(body)).unwrap().is_empty());
    }

    #[test]
    fn mergerfs_and_fuseblk_are_kept() {
        let body = "\
mergerfs fuse.mergerfs 100 50 50 50% /mnt/havuz
/dev/sdb1 fuseblk 100 50 50 50% /mnt/ntfs
";
        let d = parse_df(&df(body)).unwrap();
        assert_eq!(d.len(), 2);
    }

    #[test]
    fn same_device_keeps_first_mount_only() {
        let body = "\
/dev/mapper/root btrfs 100 50 50 50% /
/dev/mapper/root btrfs 100 50 50 50% /home
/dev/mapper/root btrfs 100 50 50 50% /srv
";
        let d = parse_df(&df(body)).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mount, "/");
    }

    #[test]
    fn dedup_ignores_filtered_entries() {
        // Toplamı 0 olan ilk satır süzülür; aynı aygıtın gerçek bağlanması kalır.
        let d = parse_df(&df(
            "/dev/x ext4 0 0 0 0% /a\n/dev/x ext4 100 50 50 50% /b\n",
        ))
        .unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].mount, "/b");
    }

    #[test]
    fn truncated_line_skipped() {
        let d = parse_df(&df("/dev/sda1 ext4 100 50 50 50% /\n/dev/sdb1 ext4 100 5")).unwrap();
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn huge_numbers() {
        let d = parse_df(&df(&format!(
            "/dev/sda1 ext4 {m} {m} 0 100% /\n",
            m = u64::MAX
        )))
        .unwrap();
        assert_eq!(d[0].total, u64::MAX);
        // u64 dışı satır atlanır, panik yok
        assert!(parse_df(&df("/dev/sda1 ext4 99999999999999999999 1 1 1% /\n")).is_err());
    }

    #[test]
    fn dash_capacity_accepted() {
        let d = parse_df(&df("/dev/sda1 ext4 100 0 0 - /data\n")).unwrap();
        assert_eq!(d[0].mount, "/data");
    }
}
