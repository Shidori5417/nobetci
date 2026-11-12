//! Parser katmanının genel API'si üzerinden uçtan uca testler.

use nobetci::parser::{
    ParseError, build_command, cpu, disk, load, mem, net, parse_batch, service, split_sections,
    uptime,
};
use std::time::Duration;

const STAT1: &str = include_str!("fixtures/proc_stat.txt");
const STAT2: &str = include_str!("fixtures/proc_stat_2.txt");
const BATCH: &str = include_str!("fixtures/batch_output.txt");
const BROKEN: &str = include_str!("fixtures/batch_broken.txt");
const DESKTOP_DF: &str = include_str!("fixtures/df_desktop.txt");

#[test]
fn cpu_usage_between_two_samples() {
    let a = cpu::parse_stat(STAT1).unwrap();
    let b = cpu::parse_stat(STAT2).unwrap();
    let u = cpu::cpu_usage(&a, &b);
    assert!((u.total - 20.0).abs() < 1e-9);
    assert_eq!(u.cores.len(), 4);
    assert!((u.cores[0] - 50.0).abs() < 1e-9);
}

#[test]
fn individual_parsers_on_fixtures() {
    let m = mem::parse_meminfo(include_str!("fixtures/meminfo.txt")).unwrap();
    assert_eq!(m.total, 16315288 * 1024);
    let old = mem::parse_meminfo(include_str!("fixtures/meminfo_old_kernel.txt")).unwrap();
    assert_eq!(old.available, (102400 + 51200 + 307200) * 1024);
    assert_eq!(
        load::parse_loadavg(include_str!("fixtures/loadavg.txt"))
            .unwrap()
            .five,
        0.37
    );
    assert_eq!(
        uptime::parse_uptime(include_str!("fixtures/uptime.txt"))
            .unwrap()
            .as_secs(),
        1234567
    );
    assert_eq!(
        disk::parse_df(include_str!("fixtures/df.txt"))
            .unwrap()
            .len(),
        3
    );
    let nd = net::parse_net_dev(include_str!("fixtures/net_dev.txt")).unwrap();
    assert!(nd.iter().all(|d| d.iface != "lo"));
    let sv = service::parse_services(include_str!("fixtures/systemctl_show.txt"));
    assert_eq!(sv[0].name, "nginx");
    assert!(sv[2].is_missing());
    assert!(!sv[0].is_missing());
}

/// Gerçek masaüstü çıktısı: AppImage FUSE, btrfs alt birimleri ve sanal mountlar
/// elenmeli; yalnızca kök (btrfs) ve /boot (vfat) kalmalı.
#[test]
fn desktop_df_keeps_only_real_disks() {
    let d = disk::parse_df(DESKTOP_DF).unwrap();
    let mounts: Vec<_> = d.iter().map(|x| x.mount.as_str()).collect();
    assert_eq!(mounts, ["/", "/boot"]);
    assert_eq!(d[0].fstype, "btrfs");
    assert_eq!(d[1].fstype, "vfat");
}

#[test]
fn net_rate_roundtrip() {
    let mut a = net::parse_net_dev(include_str!("fixtures/net_dev.txt")).unwrap();
    let b = a.clone();
    a[0].rx_bytes -= 10_000;
    a[0].tx_bytes -= 5_000;
    let r = net::net_rates(&a, &b, Duration::from_secs(5));
    assert_eq!(r[0].rx_bps, 2000.0);
    assert_eq!(r[0].tx_bps, 1000.0);
    assert_eq!(r[1].rx_bps, 0.0);
}

#[test]
fn malformed_inputs_never_panic() {
    for (name, bad) in [
        ("stat", include_str!("fixtures/malformed_stat.txt")),
        ("meminfo", include_str!("fixtures/malformed_meminfo.txt")),
        ("df", include_str!("fixtures/malformed_df.txt")),
        ("net", include_str!("fixtures/malformed_net_dev.txt")),
    ] {
        match name {
            "stat" => assert!(cpu::parse_stat(bad).is_err()),
            "meminfo" => assert!(mem::parse_meminfo(bad).is_err()),
            "df" => assert!(disk::parse_df(bad).is_err()),
            _ => assert!(net::parse_net_dev(bad).is_err()),
        }
    }
}

#[test]
fn every_parser_survives_hostile_input() {
    let nasty = [
        "",
        "\0\0\0",
        "\u{feff}çğşöü💥\n\n",
        "::::\n====\n@@@@\n",
        &"x".repeat(100_000),
        &"9".repeat(5_000),
        "cpu\ncpu0\ncpu99999999999999999999999 1 2 3 4\n",
        "MemTotal:\nMemTotal:: kB\n:\n",
    ];
    for s in nasty {
        let _ = cpu::parse_stat(s);
        let _ = mem::parse_meminfo(s);
        let _ = load::parse_loadavg(s);
        let _ = uptime::parse_uptime(s);
        let _ = disk::parse_df(s);
        let _ = net::parse_net_dev(s);
        let _ = service::parse_services(s);
        let _ = split_sections(s);
        let _ = parse_batch(s);
    }
}

#[test]
fn batch_end_to_end() {
    let p = parse_batch(BATCH);
    assert!(p.stat.is_ok() && p.mem.is_ok() && p.load.is_ok() && p.uptime.is_ok());
    assert_eq!(p.disks.unwrap()[2].mount, "/mnt/yedek diski");
    let services = p.services.unwrap();
    assert_eq!(services[1].active_state, "failed");
    assert!(services[2].is_missing());
    assert!(!services[1].is_missing());
}

#[test]
fn batch_partial_failure_is_isolated() {
    let p = parse_batch(BROKEN);
    assert!(p.stat.is_ok());
    assert!(p.mem.is_err());
    assert_eq!(p.uptime, Err(ParseError::MissingSection("uptime")));
    assert!(p.net.is_ok());
}

#[test]
fn command_output_shape_matches_splitter() {
    let cmd = build_command(&["nginx".to_string(), "bad name".to_string()]);
    assert!(cmd.contains("systemctl show nginx --property"));
    // Komutun ürettiği ayraç adları split_sections ile uyumlu olmalı
    let sections = split_sections(BATCH);
    for name in [
        "stat", "meminfo", "loadavg", "uptime", "df", "netdev", "services",
    ] {
        assert!(cmd.contains(&format!("@@NOBETCI:{name}@@")));
        assert!(sections.contains_key(name), "{name}");
    }
}
