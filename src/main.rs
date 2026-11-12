use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Result, bail, eyre};
use nobetci::actions::Actions;
use nobetci::config::{self, Config};
use nobetci::demo;
use nobetci::monitor::{self, Monitor};
use nobetci::{daemon, notify, timefmt, ui};

/// Nöbetçi: SSH üzerinden agent'sız çoklu sunucu izleme.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Sahte veriyle çalıştır (config gerekmez)
    #[arg(long, global = true)]
    demo: bool,
    /// Config dosyası yolu
    #[arg(long, global = true, value_name = "YOL")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// TUI'siz arka plan izleme ve bildirim gönderme
    Daemon,
    /// Örnek config dosyası oluştur
    Init {
        /// Var olan dosyanın üzerine yaz
        #[arg(long)]
        force: bool,
    },
    /// Config ve bağlantı testi
    Check {
        /// Yapılandırılmış her bildiriciye test mesajı gönder
        #[arg(long)]
        test_notify: bool,
    },
}

fn config_path(cli: &Cli) -> Result<PathBuf> {
    cli.config
        .clone()
        .or_else(config::default_config_path)
        .ok_or_else(|| eyre!("config yolu belirlenemedi, --config kullanın"))
}

/// TUI çalışırken loglar dosyaya yazılır, ekrana asla basılmaz.
fn init_logging() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let dirs = directories::ProjectDirs::from("", "", "nobetci")?;
    // VARSAYIM: state dizini yoksa (Linux dışı) yerel veri dizini kullanılır.
    let dir = dirs
        .state_dir()
        .unwrap_or_else(|| dirs.data_local_dir())
        .to_path_buf();
    std::fs::create_dir_all(&dir).ok()?;
    let (writer, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(dir, "nobetci.log"));
    tracing_subscriber::fmt()
        .with_writer(writer)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nobetci=info".into()),
        )
        .init();
    Some(guard)
}

/// Daemon logları stderr'e gider (journald yakalar); TTY değilse ANSI renk yok.
fn init_daemon_logging() {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nobetci=info".into()),
        )
        .init();
}

fn init_config(path: PathBuf, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} zaten var, üzerine yazmak için --force kullanın",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, config::EXAMPLE_CONFIG)?;
    println!("Örnek config yazıldı: {}", path.display());
    Ok(())
}

fn load_config(cli: &Cli) -> Result<Config> {
    Config::load(&config_path(cli)?)
        .map_err(|e| eyre!("{e}\nİpucu: `nobetci init` ile config oluşturun"))
}

fn main() -> Result<()> {
    // Panic hook'u ratatui::init'ten önce kurulmalı; ratatui bunu zincirler.
    color_eyre::install()?;
    // Yerel saat farkı, iş parçacığı açılmadan önce okunmalı.
    timefmt::init_local_offset();
    tokio::runtime::Runtime::new()?.block_on(run())
}

async fn run() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Cmd::Init { force }) => return init_config(config_path(&cli)?, force),
        Some(Cmd::Daemon) => {
            init_daemon_logging();
            let (monitor, notifiers) = if cli.demo {
                // Demo: bildirici yok, uyarılar yalnızca loglanır.
                (monitor::demo(), vec![])
            } else {
                let cfg = load_config(&cli)?;
                (monitor::from_config(&cfg), notify::from_config(&cfg.notify))
            };
            return daemon::run(monitor, notifiers).await;
        }
        Some(Cmd::Check { test_notify }) => {
            let cfg = load_config(&cli)?;
            if !nobetci::check::run(&cfg, test_notify).await {
                std::process::exit(1);
            }
            return Ok(());
        }
        None => {}
    }

    let _guard = init_logging();

    // VARSAYIM: TUI bildirim göndermez (daemon bildiricidir; ikisi birlikte çalışınca
    // çift bildirim olmasın). TUI yalnızca uyarıları motorla hesaplayıp gösterir.
    let (
        Monitor {
            snapshots, alerts, ..
        },
        actions,
    ) = if cli.demo {
        (monitor::demo(), Actions::new(demo::server_configs(), true))
    } else {
        let cfg = load_config(&cli)?;
        (monitor::from_config(&cfg), Actions::new(cfg.servers, false))
    };
    tracing::info!("TUI başlıyor (demo={})", cli.demo);
    ui::run(snapshots, alerts, actions.0, actions.1).await
}
