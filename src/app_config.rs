use std::{
    fmt, fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub network: Network,
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    #[serde(default)]
    pub bitcoin_core: BitcoinCoreConfig,
    #[serde(default)]
    pub sv2: Sv2Config,
    #[serde(default)]
    pub metrics: MetricsConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default = "default_shares_per_minute")]
    pub shares_per_minute: f32,
    #[serde(default = "default_share_batch_size")]
    pub share_batch_size: usize,
    #[serde(default = "default_cert_validity_sec")]
    pub cert_validity_sec: u64,
    #[serde(default = "default_pool_signature")]
    pub pool_signature: String,
    #[serde(default = "default_server_id")]
    pub server_id: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BitcoinCoreConfig {
    pub data_dir: Option<PathBuf>,
    pub ipc_socket_path: Option<String>,
    pub rpc_url: Option<String>,
    pub rpc_cookie_path: Option<PathBuf>,
    #[serde(default = "default_fee_threshold")]
    pub fee_threshold: u64,
    #[serde(default = "default_min_interval")]
    pub min_interval: u8,
}

impl Default for BitcoinCoreConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            ipc_socket_path: None,
            rpc_url: None,
            rpc_cookie_path: None,
            fee_threshold: default_fee_threshold(),
            min_interval: default_min_interval(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sv2Config {
    #[serde(default = "default_sv2_listen_address")]
    pub listen_address: SocketAddr,
}

impl Default for Sv2Config {
    fn default() -> Self {
        Self {
            listen_address: default_sv2_listen_address(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    pub listen_address: Option<SocketAddr>,
    #[serde(default = "default_monitoring_cache_refresh_secs")]
    pub cache_refresh_secs: u64,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            listen_address: None,
            cache_refresh_secs: default_monitoring_cache_refresh_secs(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UiConfig {
    #[serde(default = "default_ui_enabled")]
    pub enabled: bool,
    #[serde(default = "default_ui_listen_address")]
    pub listen_address: SocketAddr,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: default_ui_enabled(),
            listen_address: default_ui_listen_address(),
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Network {
    Mainnet,
    Testnet4,
    Signet,
    Regtest,
}

impl fmt::Display for Network {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Network::Mainnet => f.write_str("mainnet"),
            Network::Testnet4 => f.write_str("testnet4"),
            Network::Signet => f.write_str("signet"),
            Network::Regtest => f.write_str("regtest"),
        }
    }
}

impl FromStr for Network {
    type Err = ConfigError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "mainnet" => Ok(Network::Mainnet),
            "testnet4" => Ok(Network::Testnet4),
            "signet" => Ok(Network::Signet),
            "regtest" => Ok(Network::Regtest),
            other => Err(ConfigError::InvalidNetwork(other.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("unsupported network: {0}")]
    InvalidNetwork(String),
    #[error("shares_per_minute must be positive")]
    InvalidSharesPerMinute,
    #[error("share_batch_size must be greater than zero")]
    InvalidShareBatchSize,
}

impl AppConfig {
    pub fn read(path: &Path) -> Result<Self, ConfigError> {
        let raw = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        let config = toml::from_str::<Self>(&raw).map_err(|source| ConfigError::Parse {
            path: path.to_owned(),
            source,
        })?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.shares_per_minute <= 0.0 {
            return Err(ConfigError::InvalidSharesPerMinute);
        }
        if self.share_batch_size == 0 {
            return Err(ConfigError::InvalidShareBatchSize);
        }
        Ok(())
    }

    pub fn example(network: Network) -> String {
        match network {
            Network::Regtest => include_str!("../examples/regtest.toml").to_owned(),
            Network::Mainnet => include_str!("../examples/mainnet.toml").to_owned(),
            Network::Testnet4 => include_str!("../examples/testnet4.toml").to_owned(),
            Network::Signet => include_str!("../examples/signet.toml").to_owned(),
        }
    }

    pub fn default_pool_payout_address(&self) -> &'static str {
        match self.network {
            Network::Mainnet => DEFAULT_MAINNET_POOL_PAYOUT_ADDRESS,
            Network::Testnet4 | Network::Signet => DEFAULT_TESTNET_POOL_PAYOUT_ADDRESS,
            Network::Regtest => DEFAULT_REGTEST_POOL_PAYOUT_ADDRESS,
        }
    }
}

const DEFAULT_MAINNET_POOL_PAYOUT_ADDRESS: &str = "bc1qvzvkjn4q3nszqxrv3nraga2r822xjty3ykvkuw";
const DEFAULT_TESTNET_POOL_PAYOUT_ADDRESS: &str =
    "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7";
const DEFAULT_REGTEST_POOL_PAYOUT_ADDRESS: &str = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl";

fn default_data_dir() -> PathBuf {
    PathBuf::from("./data")
}

fn default_sv2_listen_address() -> SocketAddr {
    "0.0.0.0:3333".parse().expect("valid default address")
}

fn default_shares_per_minute() -> f32 {
    6.0
}

fn default_share_batch_size() -> usize {
    10
}

fn default_cert_validity_sec() -> u64 {
    3600
}

fn default_pool_signature() -> String {
    "Canary Mining".to_owned()
}

fn default_server_id() -> u8 {
    1
}

fn default_fee_threshold() -> u64 {
    100_000
}

fn default_min_interval() -> u8 {
    60
}

fn default_monitoring_cache_refresh_secs() -> u64 {
    15
}

fn default_ui_enabled() -> bool {
    false
}

fn default_ui_listen_address() -> SocketAddr {
    "127.0.0.1:8080".parse().expect("valid default address")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_regtest_config() {
        let config: AppConfig = toml::from_str(
            r#"
network = "regtest"
"#,
        )
        .unwrap();

        assert_eq!(config.network, Network::Regtest);
        assert_eq!(config.sv2.listen_address.to_string(), "0.0.0.0:3333");
        assert!(!config.ui.enabled);
        assert_eq!(config.ui.listen_address.to_string(), "127.0.0.1:8080");
        assert_eq!(config.shares_per_minute, 6.0);
        assert_eq!(config.share_batch_size, 10);
        assert_eq!(config.bitcoin_core.fee_threshold, 100_000);
        assert_eq!(config.bitcoin_core.min_interval, 60);
    }

    #[test]
    fn rejects_configured_payout_address() {
        let error = toml::from_str::<AppConfig>(
            r#"
network = "regtest"
payout_address = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl"
"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `payout_address`"));
    }
}
