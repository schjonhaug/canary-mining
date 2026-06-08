use serde::Serialize;
use thiserror::Error;

use crate::{app_config::Network, bitcoin_address::validate_payout_address};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MinerIdentity {
    pub label: String,
    pub payout_address: String,
    pub user_identity: String,
}

#[derive(Debug, Error)]
pub enum MinerIdentityError {
    #[error("miner user_identity must not be empty")]
    Empty,
    #[error("miner user_identity must be an address, address.worker, or sri/solo/address/worker")]
    Unsupported,
    #[error("invalid miner payout address: {0}")]
    InvalidAddress(String),
}

pub fn parse_miner_identity(
    user_identity: &str,
    network: Network,
) -> Result<MinerIdentity, MinerIdentityError> {
    let user_identity = user_identity.trim();
    if user_identity.is_empty() {
        return Err(MinerIdentityError::Empty);
    }

    let (address, label) = if let Some(rest) = user_identity.strip_prefix("sri/solo/") {
        let mut parts = rest.split('/');
        let address = parts.next().ok_or(MinerIdentityError::Unsupported)?;
        let label = parts.collect::<Vec<_>>().join("/");
        (address, label)
    } else if user_identity.starts_with("sri/") {
        return Err(MinerIdentityError::Unsupported);
    } else {
        let (address, label) = user_identity
            .split_once('.')
            .map(|(address, label)| (address, label.to_owned()))
            .unwrap_or((user_identity, String::new()));
        (address, label)
    };

    let payout_address = validate_payout_address(address, network)
        .map_err(|source| MinerIdentityError::InvalidAddress(source.to_string()))?;
    let label = if label.trim().is_empty() {
        "default".to_owned()
    } else {
        label.trim().to_owned()
    };

    Ok(MinerIdentity {
        label,
        payout_address,
        user_identity: user_identity.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGTEST_ADDRESS: &str = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl";
    const MAINNET_ADDRESS: &str = "bc1qvzvkjn4q3nszqxrv3nraga2r822xjty3ykvkuw";

    #[test]
    fn parses_address_worker_identity() {
        let identity =
            parse_miner_identity(&format!("{REGTEST_ADDRESS}.garage.s19"), Network::Regtest)
                .unwrap();

        assert_eq!(identity.label, "garage.s19");
        assert_eq!(identity.payout_address, REGTEST_ADDRESS);
    }

    #[test]
    fn parses_sri_solo_identity() {
        let identity = parse_miner_identity(
            &format!("sri/solo/{REGTEST_ADDRESS}/garage/s19"),
            Network::Regtest,
        )
        .unwrap();

        assert_eq!(identity.label, "garage/s19");
        assert_eq!(identity.payout_address, REGTEST_ADDRESS);
    }

    #[test]
    fn rejects_wrong_network_identity() {
        assert!(parse_miner_identity(MAINNET_ADDRESS, Network::Regtest).is_err());
    }

    #[test]
    fn rejects_bare_worker_identity() {
        assert!(parse_miner_identity("garage_s19", Network::Regtest).is_err());
    }
}
