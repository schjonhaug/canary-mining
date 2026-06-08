use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, bail};
use canary_mining::{
    app_config::{AppConfig, Network},
    history::MinerHistory,
    keys::{AuthorityKeys, authority_key_path},
    pool_config::{build_pool_config, resolve_ipc_socket_path},
    ui::{MiningRuntimeStatus, start_ui},
};
use clap::{Parser, Subcommand};
use pool_sv2::{
    PoolSv2, miner_stats::MinerStatsRegistry, template_runtime_status::TemplateRuntimeStatus,
};
use tokio::sync::RwLock;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about = "SV2-only solo mining server for Bitcoin Core IPC"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run {
        #[arg(short, long)]
        config: PathBuf,
    },
    Check {
        #[arg(short, long)]
        config: PathBuf,
    },
    ExampleConfig {
        #[arg(long, default_value = "regtest")]
        network: Network,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    match Cli::parse().command {
        Command::Run { config } => run(config).await,
        Command::Check { config } => check(config),
        Command::ExampleConfig { network } => {
            print!("{}", AppConfig::example(network));
            Ok(())
        }
    }
}

async fn run(config_path: PathBuf) -> anyhow::Result<()> {
    let app_config = load_checked_config(&config_path)?;
    let keys = AuthorityKeys::load_or_create(&app_config.data_dir)?;
    let authority_public_key = keys.public_key.to_string();
    let ipc_socket_path = resolve_ipc_socket_path(&app_config);
    let ipc_available = ipc_socket_path.as_ref().is_some_and(|path| path.exists());
    let pool_config = if ipc_available {
        Some(build_pool_config(&app_config, keys)?)
    } else {
        None
    };

    info!("SV2 listen address: {}", app_config.sv2.listen_address);
    info!("SV2 authority public key: {}", authority_public_key);
    if let Some(path) = ipc_socket_path.as_ref() {
        info!("Bitcoin Core IPC socket: {}", path.display());
        if !path.exists() {
            warn!(
                "Bitcoin Core IPC socket does not exist at {}; SV2 mining will not start",
                path.display()
            );
        }
    } else {
        warn!("Bitcoin Core IPC socket could not be resolved; SV2 mining will not start");
    }

    let history = MinerHistory::open(&app_config)?;
    info!("Miner database: {}", history.path().display());
    let (history_sender, history_receiver) = async_channel::unbounded();
    let _history_writer = history.start_writer(history_receiver);
    let miner_stats = MinerStatsRegistry::with_event_sender(history_sender);
    let mining_runtime = Arc::new(RwLock::new(MiningRuntimeStatus::default()));
    let template_runtime = TemplateRuntimeStatus::default();
    let _ui_handle = start_ui(
        app_config.clone(),
        authority_public_key,
        miner_stats.clone(),
        history,
        mining_runtime.clone(),
        template_runtime.clone(),
    )
    .await?;

    let Some(pool_config) = pool_config else {
        if app_config.ui.enabled {
            info!("UI is running without SV2 mining; press Ctrl+C to stop");
            wait_for_shutdown().await?;
            return Ok(());
        }
        bail!("Bitcoin Core IPC socket unavailable and UI is disabled");
    };

    {
        let mut runtime = mining_runtime.write().await;
        runtime.pool_started = true;
        runtime.pool_error = None;
    }
    let pool = PoolSv2::with_runtime_status(pool_config, miner_stats, template_runtime);
    let pool_for_task = pool.clone();
    let pool_task = tokio::spawn(async move { pool_for_task.start().await });

    tokio::select! {
        result = pool_task => handle_pool_result(result, mining_runtime, app_config.ui.enabled).await,
        result = wait_for_shutdown() => {
            result?;
            info!("Shutdown signal received; stopping SV2 mining");
            if tokio::time::timeout(Duration::from_secs(15), pool.shutdown()).await.is_err() {
                warn!("Timed out waiting for SV2 mining shutdown");
            }
            Ok(())
        }
    }
}

async fn handle_pool_result(
    result: Result<Result<(), pool_sv2::error::PoolErrorKind>, tokio::task::JoinError>,
    mining_runtime: Arc<RwLock<MiningRuntimeStatus>>,
    ui_enabled: bool,
) -> anyhow::Result<()> {
    match result {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => {
            let message = error.to_string();
            {
                let mut runtime = mining_runtime.write().await;
                runtime.pool_started = false;
                runtime.pool_error = Some(message.clone());
            }
            if ui_enabled {
                warn!("SV2 mining stopped: {message}; keeping UI running");
                wait_for_shutdown().await?;
                Ok(())
            } else {
                Err(anyhow::anyhow!(message))
            }
        }
        Err(error) => Err(error).context("SV2 mining task failed"),
    }
}

async fn wait_for_shutdown() -> anyhow::Result<()> {
    tokio::select! {
        result = wait_for_process_signal() => result,
        result = wait_for_stdin_interrupt() => result,
    }
}

async fn wait_for_process_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut interrupt =
            signal(SignalKind::interrupt()).context("failed to listen for SIGINT")?;
        let mut terminate =
            signal(SignalKind::terminate()).context("failed to listen for SIGTERM")?;

        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }

        return Ok(());
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for Ctrl+C")?;
        Ok(())
    }
}

async fn wait_for_stdin_interrupt() -> anyhow::Result<()> {
    use tokio::io::AsyncReadExt;

    let _terminal_mode = TerminalInterruptMode::enable();
    let mut stdin = tokio::io::stdin();
    let mut byte = [0_u8; 1];
    let mut saw_caret = false;

    loop {
        let read = stdin
            .read(&mut byte)
            .await
            .context("failed to listen for stdin Ctrl+C")?;

        if read == 0 {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            continue;
        }

        if byte[0] == 0x03 || (saw_caret && byte[0] == b'C') {
            return Ok(());
        }
        saw_caret = byte[0] == b'^';
    }
}

struct TerminalInterruptMode {
    #[cfg(unix)]
    original: Option<libc::termios>,
}

impl TerminalInterruptMode {
    fn enable() -> Self {
        #[cfg(unix)]
        {
            match enable_terminal_interrupt_mode() {
                Ok(original) => Self {
                    original: Some(original),
                },
                Err(error) => {
                    warn!("Could not adjust terminal Ctrl+C handling: {error:#}");
                    Self { original: None }
                }
            }
        }

        #[cfg(not(unix))]
        {
            Self {}
        }
    }
}

impl Drop for TerminalInterruptMode {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(original) = self.original.as_ref() {
            let result = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, original) };
            if result != 0 {
                warn!(
                    "Could not restore terminal Ctrl+C handling: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }
}

#[cfg(unix)]
fn enable_terminal_interrupt_mode() -> anyhow::Result<libc::termios> {
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    let result = unsafe { libc::tcgetattr(libc::STDIN_FILENO, original.as_mut_ptr()) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("tcgetattr failed");
    }

    let original = unsafe { original.assume_init() };
    let mut current = original;
    current.c_lflag &= !(libc::ICANON | libc::ISIG | libc::ECHOCTL);
    current.c_cc[libc::VMIN] = 1;
    current.c_cc[libc::VTIME] = 0;

    let result = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &current) };
    if result != 0 {
        return Err(std::io::Error::last_os_error()).context("tcsetattr failed");
    }

    Ok(original)
}

fn check(config_path: PathBuf) -> anyhow::Result<()> {
    let app_config = load_checked_config(&config_path)?;
    let keys = AuthorityKeys::load_or_create(&app_config.data_dir)?;
    let _pool_config = build_pool_config(&app_config, keys)?;

    println!("config: ok");
    println!("network: {}", app_config.network);
    println!("sv2_listen_address: {}", app_config.sv2.listen_address);
    println!("ui_enabled: {}", app_config.ui.enabled);
    if app_config.ui.enabled {
        println!("ui_listen_address: {}", app_config.ui.listen_address);
    }
    println!(
        "default_pool_payout_address: {}",
        app_config.default_pool_payout_address()
    );
    println!("authority_public_key: {}", keys.public_key);
    println!(
        "authority_keys_path: {}",
        authority_key_path(&app_config.data_dir).display()
    );

    match resolve_ipc_socket_path(&app_config) {
        Some(path) => {
            println!("bitcoin_core_ipc_socket: {}", path.display());
            println!("bitcoin_core_ipc_socket_exists: {}", path.exists());
        }
        None => println!("bitcoin_core_ipc_socket: unresolved"),
    }

    Ok(())
}

fn load_checked_config(config_path: &PathBuf) -> anyhow::Result<AppConfig> {
    AppConfig::read(config_path)
        .with_context(|| format!("failed to load {}", config_path.display()))
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
