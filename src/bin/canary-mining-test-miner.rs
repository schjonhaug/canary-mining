use std::io::Write;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Context;
use canary_mining::{
    app_config::AppConfig,
    test_miner::{ShareMode, TestMinerConfig, run_test_miner},
};
use clap::{Parser, ValueEnum};
use stratum_apps::key_utils::Secp256k1PublicKey;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(author, version, about = "Native SV2 regtest software miner")]
struct Cli {
    #[arg(short, long)]
    config: PathBuf,
    #[arg(long)]
    pool: Option<SocketAddr>,
    #[arg(long)]
    authority_public_key: Option<Secp256k1PublicKey>,
    #[arg(long)]
    user_identity: Option<String>,
    #[arg(long, default_value = "canary-mining-test-miner")]
    device_id: String,
    #[arg(long, default_value_t = 10_000_000.0)]
    nominal_hashrate: f32,
    #[arg(long, default_value_t = 60)]
    timeout_seconds: u64,
    #[arg(long, default_value_t = 1)]
    stop_after_accepted_blocks: u32,
    #[arg(long, default_value_t = 0)]
    linger_after_accepted_seconds: u64,
    #[arg(long, default_value_t = 1)]
    cores: u32,
    #[arg(long, value_enum, default_value_t = CliShareMode::BlockOnly)]
    share_mode: CliShareMode,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum CliShareMode {
    BlockOnly,
    PoolTarget,
}

impl From<CliShareMode> for ShareMode {
    fn from(value: CliShareMode) -> Self {
        match value {
            CliShareMode::BlockOnly => ShareMode::BlockOnly,
            CliShareMode::PoolTarget => ShareMode::PoolTarget,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_logging();

    let cli = Cli::parse();
    let app_config = AppConfig::read(&cli.config)
        .with_context(|| format!("failed to load {}", cli.config.display()))?;
    let mut miner_config =
        TestMinerConfig::from_app_config(&app_config, cli.pool, cli.authority_public_key)?;

    if let Some(user_identity) = cli.user_identity {
        miner_config.user_identity = user_identity;
    }
    miner_config.device_id = cli.device_id;
    miner_config.nominal_hashrate = cli.nominal_hashrate;
    miner_config.timeout = Duration::from_secs(cli.timeout_seconds);
    miner_config.stop_after_accepted_blocks = cli.stop_after_accepted_blocks;
    miner_config.linger_after_accepted = Duration::from_secs(cli.linger_after_accepted_seconds);
    miner_config.cores = cli.cores;
    miner_config.share_mode = cli.share_mode.into();

    let hard_timeout = miner_config
        .timeout
        .checked_add(miner_config.linger_after_accepted)
        .and_then(|duration| duration.checked_add(Duration::from_secs(5)))
        .unwrap_or(miner_config.timeout);
    let accepted_blocks = tokio::time::timeout(hard_timeout, run_test_miner(miner_config))
        .await
        .context("native SV2 test miner hard timeout elapsed")??;
    println!("native_sv2_test_miner_accepted_blocks={accepted_blocks}");
    std::io::stdout().flush().ok();
    std::process::exit(0);
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
