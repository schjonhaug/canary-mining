use std::{
    env, fs,
    path::{Path, PathBuf},
};

use reqwest::Client;
use serde::{
    Deserialize, Serialize,
    de::{IgnoredAny, MapAccess, Visitor},
};
use serde_json::{Value, json, value::RawValue};
use thiserror::Error;

use crate::app_config::{AppConfig, Network};

#[derive(Clone)]
pub struct BitcoinCoreRpc {
    client: Client,
    url: String,
    cookie_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum BitcoinCoreRpcError {
    #[error("failed to read Bitcoin Core RPC cookie {path}: {source}")]
    ReadCookie {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid Bitcoin Core RPC cookie {0}")]
    InvalidCookie(PathBuf),
    #[error("Bitcoin Core RPC request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("Bitcoin Core RPC method {method} returned error: {error}")]
    Rpc { method: String, error: Value },
    #[error("Bitcoin Core RPC method {method} returned non-numeric result")]
    NonNumericResult { method: String },
    #[error("Bitcoin Core RPC method {method} returned unexpected result")]
    UnexpectedResult { method: String },
    #[error("Bitcoin Core RPC method {method} returned no result")]
    MissingResult { method: String },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockchainInfo {
    pub blocks: u64,
    pub initialblockdownload: bool,
    pub verificationprogress: f64,
    #[serde(default)]
    pub warnings: Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockTemplateSummary {
    pub height: u64,
    #[serde(default)]
    pub coinbasevalue: Option<u64>,
    #[serde(default)]
    pub weight: Option<u64>,
    #[serde(default)]
    pub weightlimit: Option<u64>,
    #[serde(default)]
    pub transactions: Vec<BlockTemplateSummaryTransaction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockTemplateSummaryTransaction {
    #[serde(default)]
    pub fee: Option<i64>,
    #[serde(default)]
    pub weight: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerboseBlock {
    pub hash: String,
    pub height: u64,
    pub time: u64,
    #[serde(default)]
    pub weight: Option<u64>,
    #[serde(default, rename = "nTx")]
    pub n_tx: Option<usize>,
    #[serde(default)]
    pub tx: Vec<VerboseTransaction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerboseTransaction {
    #[serde(default)]
    pub vin: Vec<VerboseTransactionInput>,
    #[serde(default)]
    pub vout: Vec<VerboseTransactionOutput>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerboseTransactionInput {
    #[serde(default)]
    pub coinbase: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VerboseTransactionOutput {
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: VerboseScriptPubKey,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VerboseScriptPubKey {
    #[serde(default)]
    pub address: Option<String>,
}

#[derive(Serialize)]
struct RpcRequest<'a> {
    jsonrpc: &'static str,
    id: &'static str,
    method: &'a str,
    params: Value,
}

struct RpcResponse<'a> {
    result: Option<&'a RawValue>,
    result_present: bool,
    error: Option<Value>,
}

impl<'de> Deserialize<'de> for RpcResponse<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct RpcResponseVisitor;

        impl<'de> Visitor<'de> for RpcResponseVisitor {
            type Value = RpcResponse<'de>;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON-RPC response object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut result = None;
                let mut result_present = false;
                let mut error = None;

                while let Some(key) = map.next_key::<&str>()? {
                    match key {
                        "result" => {
                            result_present = true;
                            result = Some(map.next_value::<&RawValue>()?);
                        }
                        "error" => {
                            error = map.next_value()?;
                        }
                        _ => {
                            map.next_value::<IgnoredAny>()?;
                        }
                    }
                }

                Ok(RpcResponse {
                    result,
                    result_present,
                    error,
                })
            }
        }

        deserializer.deserialize_map(RpcResponseVisitor)
    }
}

impl BitcoinCoreRpc {
    pub fn from_config(config: &AppConfig, client: Client) -> Self {
        Self {
            client,
            url: resolve_rpc_url(config),
            cookie_path: resolve_rpc_cookie_path(config),
        }
    }

    pub async fn network_hashrate(&self) -> Result<f64, BitcoinCoreRpcError> {
        self.call_number("getnetworkhashps").await
    }

    pub async fn network_difficulty(&self) -> Result<f64, BitcoinCoreRpcError> {
        self.call_number("getdifficulty").await
    }

    pub async fn blockchain_info(&self) -> Result<BlockchainInfo, BitcoinCoreRpcError> {
        self.call("getblockchaininfo", Vec::new()).await
    }

    pub async fn block_template_summary(
        &self,
    ) -> Result<BlockTemplateSummary, BitcoinCoreRpcError> {
        self.call(
            "getblocktemplate",
            vec![json!({
                "rules": ["segwit"],
            })],
        )
        .await
    }

    pub async fn block_hash(&self, height: u64) -> Result<String, BitcoinCoreRpcError> {
        self.call("getblockhash", vec![json!(height)]).await
    }

    pub async fn verbose_block(&self, hash: &str) -> Result<VerboseBlock, BitcoinCoreRpcError> {
        self.call("getblock", vec![json!(hash), json!(2)]).await
    }

    async fn call<T>(&self, method: &str, params: Vec<Value>) -> Result<T, BitcoinCoreRpcError>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.call_at_url(&self.url, method, Value::Array(params))
            .await
    }

    async fn call_at_url<T>(
        &self,
        url: &str,
        method: &str,
        params: Value,
    ) -> Result<T, BitcoinCoreRpcError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let (username, password) = read_rpc_cookie(&self.cookie_path)?;
        let body = self
            .client
            .post(url)
            .basic_auth(username, Some(password))
            .json(&RpcRequest {
                jsonrpc: "1.0",
                id: "canary-mining",
                method,
                params,
            })
            .send()
            .await?
            .bytes()
            .await?;

        decode_rpc_response(method, &body)
    }

    async fn call_number(&self, method: &str) -> Result<f64, BitcoinCoreRpcError> {
        let result: Value = self.call(method, Vec::new()).await?;
        result
            .as_f64()
            .ok_or_else(|| BitcoinCoreRpcError::NonNumericResult {
                method: method.to_owned(),
            })
    }
}

fn decode_rpc_response<T>(method: &str, body: &[u8]) -> Result<T, BitcoinCoreRpcError>
where
    T: for<'de> Deserialize<'de>,
{
    let response: RpcResponse =
        serde_json::from_slice(body).map_err(|_| BitcoinCoreRpcError::UnexpectedResult {
            method: method.to_owned(),
        })?;

    if let Some(error) = response.error {
        return Err(BitcoinCoreRpcError::Rpc {
            method: method.to_owned(),
            error,
        });
    }

    if !response.result_present {
        return Err(BitcoinCoreRpcError::MissingResult {
            method: method.to_owned(),
        });
    };
    let result = response.result.expect("present result must have raw JSON");

    serde_json::from_str(result.get()).map_err(|_| BitcoinCoreRpcError::UnexpectedResult {
        method: method.to_owned(),
    })
}

pub fn resolve_rpc_url(config: &AppConfig) -> String {
    config
        .bitcoin_core
        .rpc_url
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", rpc_port(config.network)))
}

pub fn resolve_rpc_cookie_path(config: &AppConfig) -> PathBuf {
    config
        .bitcoin_core
        .rpc_cookie_path
        .clone()
        .unwrap_or_else(|| network_data_dir(config).join(".cookie"))
}

pub fn network_data_dir(config: &AppConfig) -> PathBuf {
    let base = config
        .bitcoin_core
        .data_dir
        .clone()
        .unwrap_or_else(default_bitcoin_data_dir);
    match config.network {
        Network::Mainnet => base,
        Network::Testnet4 => base.join("testnet4"),
        Network::Signet => base.join("signet"),
        Network::Regtest => base.join("regtest"),
    }
}

fn rpc_port(network: Network) -> u16 {
    match network {
        Network::Mainnet => 8332,
        Network::Testnet4 => 48332,
        Network::Signet => 38332,
        Network::Regtest => 18443,
    }
}

fn default_bitcoin_data_dir() -> PathBuf {
    let home = env::var_os("HOME").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    {
        home.map(|path| path.join("Library/Application Support/Bitcoin"))
            .unwrap_or_else(|| PathBuf::from(".bitcoin"))
    }
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("Bitcoin"))
            .or_else(|| home.map(|path| path.join("AppData/Roaming/Bitcoin")))
            .unwrap_or_else(|| PathBuf::from("Bitcoin"))
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        home.map(|path| path.join(".bitcoin"))
            .unwrap_or_else(|| PathBuf::from(".bitcoin"))
    }
}

fn read_rpc_cookie(path: &Path) -> Result<(String, String), BitcoinCoreRpcError> {
    let raw = fs::read_to_string(path).map_err(|source| BitcoinCoreRpcError::ReadCookie {
        path: path.to_owned(),
        source,
    })?;
    let (username, password) = raw
        .trim()
        .split_once(':')
        .ok_or_else(|| BitcoinCoreRpcError::InvalidCookie(path.to_owned()))?;
    Ok((username.to_owned(), password.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_rpc_success_without_materializing_ignored_transaction_data() {
        let large_data = "ab".repeat(64 * 1024);
        let body = format!(
            r#"{{
                "result": {{
                    "height": 950001,
                    "coinbasevalue": 312500000,
                    "weight": 12345,
                    "weightlimit": 4000000,
                    "transactions": [
                        {{"data": "{large_data}", "fee": 1000, "weight": 400}}
                    ]
                }},
                "error": null,
                "id": "canary-mining"
            }}"#
        );

        let summary: BlockTemplateSummary =
            decode_rpc_response("getblocktemplate", body.as_bytes()).unwrap();

        assert_eq!(summary.height, 950001);
        assert_eq!(summary.transactions.len(), 1);
        assert_eq!(summary.transactions[0].fee, Some(1000));
        assert_eq!(summary.transactions[0].weight, Some(400));
    }

    #[test]
    fn decodes_rpc_error_with_null_result() {
        let error = decode_rpc_response::<Option<String>>(
            "getblocktemplate",
            br#"{"result": null, "error": {"code": -10, "message": "Loading block index"}, "id": "canary-mining"}"#,
        )
        .unwrap_err();

        match error {
            BitcoinCoreRpcError::Rpc { method, error } => {
                assert_eq!(method, "getblocktemplate");
                assert_eq!(error["code"], -10);
            }
            other => panic!("expected RPC error, got {other:?}"),
        }
    }

    #[test]
    fn resolves_regtest_rpc_defaults_from_bitcoin_data_dir() {
        let config: AppConfig = toml::from_str(
            r#"
network = "regtest"

[bitcoin_core]
data_dir = "/bitcoin"
"#,
        )
        .unwrap();

        assert_eq!(resolve_rpc_url(&config), "http://127.0.0.1:18443");
        assert_eq!(
            resolve_rpc_cookie_path(&config),
            PathBuf::from("/bitcoin/regtest/.cookie")
        );
    }

    #[test]
    fn explicit_rpc_settings_override_network_defaults() {
        let config: AppConfig = toml::from_str(
            r#"
network = "regtest"

[bitcoin_core]
data_dir = "/bitcoin"
rpc_url = "http://127.0.0.1:28443"
rpc_cookie_path = "/tmp/custom.cookie"
"#,
        )
        .unwrap();

        assert_eq!(resolve_rpc_url(&config), "http://127.0.0.1:28443");
        assert_eq!(
            resolve_rpc_cookie_path(&config),
            PathBuf::from("/tmp/custom.cookie")
        );
    }
}
