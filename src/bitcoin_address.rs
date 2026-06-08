use std::str::FromStr;

use bitcoin::{Address, Network as BitcoinNetwork};
use thiserror::Error;

use crate::app_config::Network;

#[derive(Debug, Error)]
pub enum PayoutAddressError {
    #[error("payout address must not be empty")]
    Empty,
    #[error("invalid payout address: {0}")]
    Invalid(String),
}

pub fn validate_payout_address(
    address: &str,
    network: Network,
) -> Result<String, PayoutAddressError> {
    let trimmed = address.trim();
    if trimmed.is_empty() {
        return Err(PayoutAddressError::Empty);
    }

    Address::from_str(trimmed)
        .map_err(|source| PayoutAddressError::Invalid(source.to_string()))?
        .require_network(network.into())
        .map(|address| address.to_string())
        .map_err(|source| PayoutAddressError::Invalid(source.to_string()))
}

impl From<Network> for BitcoinNetwork {
    fn from(value: Network) -> Self {
        match value {
            Network::Mainnet => BitcoinNetwork::Bitcoin,
            Network::Testnet4 => BitcoinNetwork::Testnet4,
            Network::Signet => BitcoinNetwork::Signet,
            Network::Regtest => BitcoinNetwork::Regtest,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAINNET_ADDRESS: &str = "bc1qvzvkjn4q3nszqxrv3nraga2r822xjty3ykvkuw";
    const TESTNET_ADDRESS: &str = "tb1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3q0sl5k7";
    const REGTEST_ADDRESS: &str = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl";

    #[test]
    fn accepts_address_for_configured_network() {
        assert_eq!(
            validate_payout_address(REGTEST_ADDRESS, Network::Regtest).unwrap(),
            REGTEST_ADDRESS
        );
        assert_eq!(
            validate_payout_address(TESTNET_ADDRESS, Network::Testnet4).unwrap(),
            TESTNET_ADDRESS
        );
        assert_eq!(
            validate_payout_address(TESTNET_ADDRESS, Network::Signet).unwrap(),
            TESTNET_ADDRESS
        );
        assert_eq!(
            validate_payout_address(MAINNET_ADDRESS, Network::Mainnet).unwrap(),
            MAINNET_ADDRESS
        );
    }

    #[test]
    fn rejects_address_for_wrong_network() {
        assert!(validate_payout_address(MAINNET_ADDRESS, Network::Regtest).is_err());
        assert!(validate_payout_address(REGTEST_ADDRESS, Network::Mainnet).is_err());
    }

    #[test]
    fn rejects_invalid_address() {
        assert!(validate_payout_address("not an address", Network::Regtest).is_err());
    }
}
