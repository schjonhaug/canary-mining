use std::path::PathBuf;

use pool_sv2::config::{AuthorityConfig, ConnectionConfig, PoolConfig};
use stratum_apps::{
    config_helpers::CoinbaseRewardScript,
    tp_type::{BitcoinNetwork, TemplateProviderType},
};
use thiserror::Error;

use crate::{
    app_config::{AppConfig, Network},
    keys::AuthorityKeys,
};

#[derive(Debug, Error)]
pub enum PoolConfigError {
    #[error("invalid coinbase reward descriptor addr({address}): {message}")]
    InvalidCoinbaseAddress { address: String, message: String },
}

pub fn build_pool_config(
    app_config: &AppConfig,
    keys: AuthorityKeys,
) -> Result<PoolConfig, PoolConfigError> {
    let default_pool_payout_address = app_config.default_pool_payout_address();
    let coinbase_reward_script =
        CoinbaseRewardScript::from_descriptor(&format!("addr({default_pool_payout_address})"))
            .map_err(|source| PoolConfigError::InvalidCoinbaseAddress {
                address: default_pool_payout_address.to_owned(),
                message: source.to_string(),
            })?;

    Ok(PoolConfig::new(
        ConnectionConfig::new(
            app_config.sv2.listen_address,
            app_config.cert_validity_sec,
            app_config.pool_signature.clone(),
        ),
        TemplateProviderType::BitcoinCoreIpc {
            network: app_config.network.into(),
            data_dir: app_config
                .bitcoin_core
                .ipc_socket_path
                .as_deref()
                .map(normalize_ipc_socket_path)
                .or_else(|| app_config.bitcoin_core.data_dir.clone()),
            fee_threshold: app_config.bitcoin_core.fee_threshold,
            min_interval: app_config.bitcoin_core.min_interval,
        },
        AuthorityConfig::new(keys.public_key, keys.secret_key),
        coinbase_reward_script,
        app_config.shares_per_minute,
        app_config.share_batch_size,
        app_config.server_id,
        Vec::new(),
        Vec::new(),
        app_config.metrics.listen_address,
        Some(app_config.metrics.cache_refresh_secs),
        None,
    ))
}

pub fn resolve_ipc_socket_path(config: &AppConfig) -> Option<PathBuf> {
    if let Some(path) = &config.bitcoin_core.ipc_socket_path {
        return Some(normalize_ipc_socket_path(path));
    }

    stratum_apps::tp_type::resolve_ipc_socket_path(
        &config.network.into(),
        config.bitcoin_core.data_dir.clone(),
    )
}

fn normalize_ipc_socket_path(path: &str) -> PathBuf {
    PathBuf::from(path.strip_prefix("unix:").unwrap_or(path))
}

impl From<Network> for BitcoinNetwork {
    fn from(value: Network) -> Self {
        match value {
            Network::Mainnet => BitcoinNetwork::Mainnet,
            Network::Testnet4 => BitcoinNetwork::Testnet4,
            Network::Signet => BitcoinNetwork::Signet,
            Network::Regtest => BitcoinNetwork::Regtest,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn resolves_regtest_ipc_path_from_data_dir() {
        let config: AppConfig = toml::from_str(
            r#"
network = "regtest"

[bitcoin_core]
data_dir = "/bitcoin"
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_ipc_socket_path(&config),
            Some(PathBuf::from("/bitcoin/regtest/node.sock"))
        );
    }

    #[test]
    fn resolves_explicit_ipc_socket_path() {
        let config: AppConfig = toml::from_str(
            r#"
network = "mainnet"

[bitcoin_core]
ipc_socket_path = "unix:/root/.bitcoin/ipc/bitcoin-core.sock"
"#,
        )
        .unwrap();

        assert_eq!(
            resolve_ipc_socket_path(&config),
            Some(PathBuf::from("/root/.bitcoin/ipc/bitcoin-core.sock"))
        );
    }

    #[test]
    fn builds_pool_config_with_default_pool_payout_address() {
        let config: AppConfig = toml::from_str(r#"network = "regtest""#).unwrap();
        let keys = AuthorityKeys::generate();

        build_pool_config(&config, keys).expect("pool config");
    }
}
