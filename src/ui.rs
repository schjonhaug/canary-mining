use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    sync::{Arc, LazyLock},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path as AxumPath, Query, State},
    http::{StatusCode, header},
    response::{Html, IntoResponse, Response},
    routing::{delete, get},
};
use bitcoin::pow::Target;
use pool_sv2::{
    miner_stats::{MinerSessionSnapshot, MinerStatsRegistry},
    template_runtime_status::{
        TemplateRuntimeStatus, TemplateStatusKind, TemplateStatusSnapshot, TemplateStatusSource,
    },
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    net::TcpListener,
    sync::RwLock,
    task::JoinHandle,
    time::{Duration, timeout},
};
use tracing::{info, warn};

use crate::{
    app_config::{AppConfig, Network},
    bitcoin_address::validate_payout_address,
    bitcoin_core_rpc::{BitcoinCoreRpc, BitcoinCoreRpcError},
    history::{KnownMiner, MinerHistory, miner_id},
    miner_identity::parse_miner_identity,
    pool_config::resolve_ipc_socket_path,
};

const INDEX_HTML: &str = include_str!("../ui/index.html");
const JQUERY_LITE_JS: &str = include_str!("../ui/jquery-lite.js");
const APP_JS: &str = include_str!("../ui/app.js");
const CANARY_DESIGN_CSS: &str = include_str!("../ui/canary-design.css");
const STYLE_CSS: &str = include_str!("../ui/style.css");
const MEMPOOL_POOLS_JSON: &str = include_str!("../assets/mempool-pools/pools-v2.json");
const GEIST_CYRILLIC_FONT: &[u8] = include_bytes!("../ui/assets/fonts/geist/geist-cyrillic.woff2");
const GEIST_LATIN_EXT_FONT: &[u8] =
    include_bytes!("../ui/assets/fonts/geist/geist-latin-ext.woff2");
const GEIST_LATIN_FONT: &[u8] = include_bytes!("../ui/assets/fonts/geist/geist-latin.woff2");
const GEIST_MONO_CYRILLIC_FONT: &[u8] =
    include_bytes!("../ui/assets/fonts/geist/geist-mono-cyrillic.woff2");
const GEIST_MONO_LATIN_EXT_FONT: &[u8] =
    include_bytes!("../ui/assets/fonts/geist/geist-mono-latin-ext.woff2");
const GEIST_MONO_LATIN_FONT: &[u8] =
    include_bytes!("../ui/assets/fonts/geist/geist-mono-latin.woff2");
const CIRCLE_HELP_ICON: &str = include_str!("../ui/icons/circle-help.svg");
const COPY_ICON: &str = include_str!("../ui/icons/copy.svg");
const CHECK_ICON: &str = include_str!("../ui/icons/check.svg");
const CHAIN_LINK_ICON: &str = include_str!("../ui/icons/chain-link.svg");
const HEART_ICON: &str = include_str!("../ui/icons/heart.svg");
const GITHUB_ICON: &str = include_str!("../ui/icons/github.svg");
const CANARY_LOGO: &str = include_str!("../ui/canary-in-a-coalmine.svg");
const ANTMINER_LOGO: &[u8] = include_bytes!("../ui/miner-logos/antminer.svg");
const BITAXE_LOGO: &[u8] = include_bytes!("../ui/miner-logos/bitaxe.svg");
const BITCRANE_LOGO: &[u8] = include_bytes!("../ui/miner-logos/bitcrane.svg");
const BRAIINS_LOGO: &[u8] = include_bytes!("../ui/miner-logos/braiins.svg");
const GENERIC_MINER_LOGO: &[u8] = include_bytes!("../ui/miner-logos/generic.svg");
const NERDAXE_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdaxe.svg");
const NERDEKO_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdeko.svg");
const NERDOCTAXE_GAMMA_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdoctaxe-gamma.svg");
const NERDOCTAXE_PLUS_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdoctaxe-plus.svg");
const NERDQAXE_PLUS_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdqaxe-plus.svg");
const NERDQAXE_PLUS_PLUS_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nerdqaxe-plus-plus.svg");
const NMMINER_LOGO: &[u8] = include_bytes!("../ui/miner-logos/nmminer.svg");
const OSMU_LOGO: &[u8] = include_bytes!("../ui/miner-logos/osmu.svg");
const PIAXE_LOGO: &[u8] = include_bytes!("../ui/miner-logos/piaxe.svg");
const QAXE_LOGO: &[u8] = include_bytes!("../ui/miner-logos/qaxe.svg");
const EXCHANGE_RATE_REFRESH_SECONDS: i64 = 600;
const COINGECKO_API_BASE_URL: &str = "https://api.coingecko.com";
const ACTIVE_MINER_GRACE_SECONDS: u64 = 120;
const STATUS_RPC_TIMEOUT: Duration = Duration::from_millis(750);
const STATUS_MONITORING_TIMEOUT: Duration = Duration::from_millis(500);
const RECENT_BLOCK_RPC_TIMEOUT: Duration = Duration::from_millis(1_500);
const RECENT_BLOCK_LIMIT: usize = 6;
const SUPPORTED_CURRENCIES: &[&str] = &[
    "USD", "AED", "ARS", "AUD", "BDT", "BHD", "BMD", "BRL", "CAD", "CHF", "CLP", "CNY", "CZK",
    "DKK", "EUR", "GBP", "GEL", "HKD", "HUF", "IDR", "ILS", "INR", "JPY", "KRW", "KWD", "LKR",
    "MMK", "MXN", "MYR", "NGN", "NOK", "NZD", "PHP", "PKR", "PLN", "RUB", "SAR", "SEK", "SGD",
    "THB", "TRY", "TWD", "UAH", "VEF", "VND", "ZAR",
];

#[derive(Clone)]
struct UiState {
    client: Client,
    app_config: AppConfig,
    bitcoin_core_rpc: BitcoinCoreRpc,
    network: Network,
    config: UiConfigSummary,
    miner_setup: MinerSetupSummary,
    metrics_base_url: Option<String>,
    exchange_rates: Arc<RwLock<ExchangeRateCache>>,
    recent_blocks: Arc<RwLock<RecentBlocksCache>>,
    difficulty_period_start: Arc<RwLock<DifficultyPeriodStartCache>>,
    template_runtime: TemplateRuntimeStatus,
    seen_miners: Arc<RwLock<HashMap<String, MinerRow>>>,
    mining_status: MiningStatusCache,
    miner_stats: MinerStatsRegistry,
    history: MinerHistory,
}

#[derive(Clone, Default)]
struct ExchangeRateCache {
    rates: HashMap<String, f64>,
    updated_at: Option<i64>,
}

#[derive(Clone, Default)]
struct RecentBlocksCache {
    chain_height: Option<u64>,
    blocks: Vec<BlockTimelineItem>,
}

#[derive(Clone, Default)]
struct DifficultyPeriodStartCache {
    height: Option<u64>,
    timestamp: Option<u64>,
}

pub type SharedMiningRuntimeStatus = Arc<RwLock<MiningRuntimeStatus>>;

#[derive(Clone, Debug, Default)]
pub struct MiningRuntimeStatus {
    pub pool_started: bool,
    pub pool_error: Option<String>,
}

#[derive(Clone)]
struct MiningStatusCache {
    runtime: SharedMiningRuntimeStatus,
}

#[derive(Clone, Serialize)]
struct UiConfigSummary {
    network: String,
    sv2_listen_address: SocketAddr,
    metrics_listen_address: Option<SocketAddr>,
}

#[derive(Clone, Serialize)]
struct MinerSetupSummary {
    payout_address: String,
    sv2_authority_public_key: String,
}

#[derive(Serialize)]
struct StatusResponse {
    server_time: u64,
    config: UiConfigSummary,
    miner_setup: MinerSetupSummary,
    monitoring: Option<Value>,
    mining: MiningStatusResponse,
    bitcoin_core: BitcoinCoreStatus,
    recent_blocks: Vec<BlockTimelineItem>,
    connected_miners: Vec<MinerRow>,
    miner_workers: Vec<MinerWorker>,
}

#[derive(Serialize)]
struct HealthResponse {
    ready: bool,
    server_time: u64,
    sv2_listening: bool,
    template_available: bool,
    ipc_socket_available: bool,
    warning: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct MiningStatusResponse {
    sv2_listening: bool,
    template_provider_available: bool,
    ipc_socket_path: Option<String>,
    warning: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct ValidatePayoutAddressQuery {
    address: String,
}

#[derive(Serialize)]
struct ValidatePayoutAddressResponse {
    valid: bool,
    network: String,
    address: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct DeleteMinerResponse {
    deleted: bool,
}

#[derive(Deserialize)]
struct DeleteMinerQuery {
    miner_id: String,
}

#[derive(Serialize)]
struct BitcoinCoreStatus {
    rpc_available: bool,
    rpc_warning: Option<String>,
    mining_ready: bool,
    network_hashrate: Option<f64>,
    network_difficulty: Option<f64>,
    difficulty_adjustment: Option<DifficultyAdjustmentStatus>,
    chain_height: Option<u64>,
    sync_progress: Option<f64>,
    initial_block_download: Option<bool>,
    template: Option<BlockTemplateStatus>,
}

#[derive(Serialize)]
struct DifficultyAdjustmentStatus {
    next_height: u64,
    blocks_remaining: u64,
    progress_percent: f64,
    estimated_seconds_remaining: Option<u64>,
    projected_percent_change: Option<f64>,
    projected_difficulty: Option<f64>,
}

#[derive(Clone, Serialize)]
struct BlockTemplateStatus {
    height: u64,
    reward_sats: Option<u64>,
    fee_sats: Option<u64>,
    weight: Option<u64>,
    weight_percent: Option<f64>,
    transaction_count: usize,
    updated_at: u64,
    source: &'static str,
    status: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct BlockTimelineItem {
    height: u64,
    hash: String,
    timestamp: u64,
    weight: Option<u64>,
    transaction_count: Option<usize>,
    reward_sats: Option<u64>,
    pool: PoolAttribution,
}

#[derive(Clone, Debug, Serialize)]
struct PoolAttribution {
    name: String,
    slug: String,
    logo: String,
    link: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MempoolPoolEntry {
    name: String,
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    link: Option<String>,
}

#[derive(Serialize)]
struct ExchangeRatesResponse {
    supported_currencies: &'static [&'static str],
    rates: HashMap<String, f64>,
    updated_at: Option<i64>,
    stale: bool,
}

#[derive(serde::Deserialize)]
struct CoinGeckoResponse {
    bitcoin: HashMap<String, f64>,
}

#[derive(Clone, Debug, Serialize)]
struct MinerRow {
    id: String,
    client_id: u64,
    channel_id: u32,
    channel_kind: String,
    connected: bool,
    label: String,
    payout_address: String,
    user_identity: String,
    nominal_hashrate: f64,
    shares_accepted: u64,
    best_diff: f64,
    blocks_found: u64,
    target_hex: Option<String>,
    pool_diff: Option<f64>,
    #[serde(skip)]
    last_seen_at: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct MinerWorker {
    worker_id: String,
    label: String,
    payout_address: String,
    user_identity: String,
    session_count: usize,
    total_hashrate: f64,
    pool_diff: Option<f64>,
    best_diff: f64,
    total_shares: u64,
    total_rejected: u64,
    rejection_percent: f64,
    opened_at: Option<u64>,
    closed_at: Option<u64>,
    last_seen_at: Option<u64>,
    uptime_seconds: Option<u64>,
    last_seen_seconds: Option<u64>,
    connected: bool,
    active_on_template: bool,
    connection_state: &'static str,
    sessions: Vec<MinerSession>,
}

#[derive(Clone, Debug, Serialize)]
struct MinerSession {
    session_id: String,
    client_id: u64,
    channel_id: u32,
    channel_kind: String,
    hashrate: f64,
    pool_diff: Option<f64>,
    best_diff: f64,
    shares_accepted: u64,
    shares_rejected: u64,
    rejection_percent: f64,
    opened_at: Option<u64>,
    closed_at: Option<u64>,
    last_seen_at: Option<u64>,
    uptime_seconds: Option<u64>,
    last_seen_seconds: Option<u64>,
    connected: bool,
}

pub async fn start_ui(
    config: AppConfig,
    authority_public_key: String,
    miner_stats: MinerStatsRegistry,
    history: MinerHistory,
    mining_runtime: SharedMiningRuntimeStatus,
    template_runtime: TemplateRuntimeStatus,
) -> anyhow::Result<Option<JoinHandle<()>>> {
    if !config.ui.enabled {
        return Ok(None);
    }

    let client = Client::new();
    let bitcoin_core_rpc = BitcoinCoreRpc::from_config(&config, client.clone());

    let state = UiState {
        client,
        app_config: config.clone(),
        bitcoin_core_rpc,
        network: config.network,
        config: UiConfigSummary {
            network: config.network.to_string(),
            sv2_listen_address: config.sv2.listen_address,
            metrics_listen_address: config.metrics.listen_address,
        },
        miner_setup: MinerSetupSummary {
            payout_address: config.default_pool_payout_address().to_owned(),
            sv2_authority_public_key: authority_public_key,
        },
        metrics_base_url: config
            .metrics
            .listen_address
            .map(|address| format!("http://{address}")),
        exchange_rates: Arc::new(RwLock::new(ExchangeRateCache::default())),
        recent_blocks: Arc::new(RwLock::new(RecentBlocksCache::default())),
        difficulty_period_start: Arc::new(RwLock::new(DifficultyPeriodStartCache::default())),
        template_runtime,
        seen_miners: Arc::new(RwLock::new(HashMap::new())),
        mining_status: MiningStatusCache {
            runtime: mining_runtime,
        },
        miner_stats,
        history,
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/jquery-lite.js", get(jquery_lite_js))
        .route("/app.js", get(app_js))
        .route("/canary-design.css", get(canary_design_css))
        .route("/style.css", get(style_css))
        .route("/assets/fonts/geist/{*path}", get(geist_font))
        .route("/icons/circle-help.svg", get(circle_help_icon))
        .route("/icons/copy.svg", get(copy_icon))
        .route("/icons/check.svg", get(check_icon))
        .route("/icons/chain-link.svg", get(chain_link_icon))
        .route("/icons/heart.svg", get(heart_icon))
        .route("/icons/github.svg", get(github_icon))
        .route("/canary-in-a-coalmine.svg", get(canary_logo))
        .route("/miner-logos/{*path}", get(miner_logo))
        .route("/pool-logos/{*path}", get(pool_logo))
        .route("/api/status", get(api_status))
        .route("/api/health", get(api_health))
        .route("/api/exchange-rates", get(api_exchange_rates))
        .route(
            "/api/validate-payout-address",
            get(api_validate_payout_address),
        )
        .route("/api/miners", delete(api_delete_miner))
        .route("/api/monitoring/{*path}", get(api_monitoring))
        .with_state(state.clone());

    let listener = TcpListener::bind(config.ui.listen_address)
        .await
        .with_context(|| format!("failed to bind UI server to {}", config.ui.listen_address))?;
    info!("UI server listening on http://{}", config.ui.listen_address);

    Ok(Some(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            warn!("UI server stopped: {error}");
        }
    })))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn app_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        APP_JS,
    )
        .into_response()
}

async fn jquery_lite_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        JQUERY_LITE_JS,
    )
        .into_response()
}

async fn style_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        STYLE_CSS,
    )
        .into_response()
}

async fn canary_design_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        CANARY_DESIGN_CSS,
    )
        .into_response()
}

async fn geist_font(AxumPath(path): AxumPath<String>) -> Response {
    let Some(font) = geist_font_asset(&path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        font,
    )
        .into_response()
}

async fn canary_logo() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        CANARY_LOGO,
    )
        .into_response()
}

async fn circle_help_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        CIRCLE_HELP_ICON,
    )
        .into_response()
}

async fn copy_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        COPY_ICON,
    )
        .into_response()
}

async fn check_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        CHECK_ICON,
    )
        .into_response()
}

async fn chain_link_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        CHAIN_LINK_ICON,
    )
        .into_response()
}

async fn heart_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        HEART_ICON,
    )
        .into_response()
}

async fn github_icon() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        GITHUB_ICON,
    )
        .into_response()
}

async fn miner_logo(AxumPath(path): AxumPath<String>) -> Response {
    let Some((content_type, body)) = miner_logo_asset(path.trim_start_matches('/')) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

fn geist_font_asset(path: &str) -> Option<&'static [u8]> {
    match path.trim_start_matches('/') {
        "geist-cyrillic.woff2" => Some(GEIST_CYRILLIC_FONT),
        "geist-latin-ext.woff2" => Some(GEIST_LATIN_EXT_FONT),
        "geist-latin.woff2" => Some(GEIST_LATIN_FONT),
        "geist-mono-cyrillic.woff2" => Some(GEIST_MONO_CYRILLIC_FONT),
        "geist-mono-latin-ext.woff2" => Some(GEIST_MONO_LATIN_EXT_FONT),
        "geist-mono-latin.woff2" => Some(GEIST_MONO_LATIN_FONT),
        _ => None,
    }
}

fn miner_logo_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    match path {
        "antminer.svg" => Some(("image/svg+xml", ANTMINER_LOGO)),
        "bitaxe.svg" => Some(("image/svg+xml", BITAXE_LOGO)),
        "bitcrane.svg" => Some(("image/svg+xml", BITCRANE_LOGO)),
        "braiins.svg" => Some(("image/svg+xml", BRAIINS_LOGO)),
        "generic.svg" => Some(("image/svg+xml", GENERIC_MINER_LOGO)),
        "nerdaxe.svg" => Some(("image/svg+xml", NERDAXE_LOGO)),
        "nerdeko.svg" => Some(("image/svg+xml", NERDEKO_LOGO)),
        "nerdoctaxe-gamma.svg" => Some(("image/svg+xml", NERDOCTAXE_GAMMA_LOGO)),
        "nerdoctaxe-plus.svg" => Some(("image/svg+xml", NERDOCTAXE_PLUS_LOGO)),
        "nerdqaxe-plus.svg" => Some(("image/svg+xml", NERDQAXE_PLUS_LOGO)),
        "nerdqaxe-plus-plus.svg" => Some(("image/svg+xml", NERDQAXE_PLUS_PLUS_LOGO)),
        "nmminer.svg" => Some(("image/svg+xml", NMMINER_LOGO)),
        "osmu.svg" => Some(("image/svg+xml", OSMU_LOGO)),
        "piaxe.svg" => Some(("image/svg+xml", PIAXE_LOGO)),
        "qaxe.svg" => Some(("image/svg+xml", QAXE_LOGO)),
        _ => None,
    }
}

async fn pool_logo(AxumPath(path): AxumPath<String>) -> Response {
    let Some((content_type, body)) = pool_logo_asset(path.trim_start_matches('/')) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
        .into_response()
}

fn pool_logo_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    generated_pool_logo_asset(path)
}

include!(concat!(env!("OUT_DIR"), "/pool_logos.rs"));

async fn api_status(State(state): State<UiState>) -> Result<Json<StatusResponse>, ApiError> {
    let monitoring = fetch_monitoring_json(&state, "api/v1/global").await;
    let mining = mining_status_response(&state).await;
    let bitcoin_core = fetch_bitcoin_core_status(&state).await;
    let recent_blocks = recent_blocks_response(&state, bitcoin_core.chain_height)
        .await
        .unwrap_or_else(|error| {
            warn!("recent block pool attribution unavailable: {error}");
            Vec::new()
        });
    let current_miners = fetch_current_miners(&state).await;
    let connected_miners = fetch_connected_miners(&state, current_miners.clone()).await;
    let miner_workers = fetch_miner_workers(&state, Some(connected_miners.clone())).await;
    Ok(Json(StatusResponse {
        server_time: unix_now() as u64,
        config: state.config,
        miner_setup: state.miner_setup,
        monitoring,
        mining,
        bitcoin_core,
        recent_blocks,
        connected_miners,
        miner_workers,
    }))
}

async fn api_health(State(state): State<UiState>) -> Json<HealthResponse> {
    let runtime = state.mining_status.runtime.read().await.clone();
    let ipc_socket_available = resolve_ipc_socket_path(&state.app_config)
        .as_ref()
        .is_some_and(|path| path.exists());
    let template_available = state.template_runtime.snapshot().is_some();
    let sv2_listening = runtime.pool_started && runtime.pool_error.is_none();
    let warning = if ipc_socket_available {
        None
    } else {
        Some("Bitcoin Core IPC socket is unavailable.".to_owned())
    };
    let ready =
        sv2_listening && ipc_socket_available && template_available && runtime.pool_error.is_none();

    Json(HealthResponse {
        ready,
        server_time: unix_now() as u64,
        sv2_listening,
        template_available,
        ipc_socket_available,
        warning,
        error: runtime.pool_error,
    })
}

async fn mining_status_response(state: &UiState) -> MiningStatusResponse {
    let runtime = state.mining_status.runtime.read().await.clone();
    let ipc_socket_path = resolve_ipc_socket_path(&state.app_config);
    let warning = match ipc_socket_path.as_ref() {
        Some(path) if path.exists() => None,
        Some(path) => Some(format!(
            "Bitcoin Core IPC socket not found at {}. SV2 mining is unavailable, but RPC-backed UI data can still load.",
            path.display()
        )),
        None => Some(
            "Bitcoin Core IPC socket could not be resolved. SV2 mining is unavailable, but RPC-backed UI data can still load."
                .to_owned(),
        ),
    };

    MiningStatusResponse {
        sv2_listening: runtime.pool_started && runtime.pool_error.is_none() && warning.is_none(),
        template_provider_available: warning.is_none(),
        ipc_socket_path: ipc_socket_path.map(|path| path.display().to_string()),
        warning,
        error: runtime.pool_error,
    }
}

async fn api_validate_payout_address(
    State(state): State<UiState>,
    Query(query): Query<ValidatePayoutAddressQuery>,
) -> Json<ValidatePayoutAddressResponse> {
    let address = query.address.trim();
    match validate_payout_address(address, state.network) {
        Ok(validated) => Json(ValidatePayoutAddressResponse {
            valid: true,
            network: state.network.to_string(),
            address: Some(validated),
            error: None,
        }),
        Err(error) => Json(ValidatePayoutAddressResponse {
            valid: false,
            network: state.network.to_string(),
            address: None,
            error: Some(error.to_string()),
        }),
    }
}

async fn api_exchange_rates(State(state): State<UiState>) -> Response {
    match exchange_rates_response(&state).await {
        Ok(response) => Json(response).into_response(),
        Err(error) => {
            warn!("failed to fetch exchange rates: {error}");
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": "exchange rates unavailable",
                    "supported_currencies": SUPPORTED_CURRENCIES,
                })),
            )
                .into_response()
        }
    }
}

async fn api_delete_miner(
    State(state): State<UiState>,
    Query(query): Query<DeleteMinerQuery>,
) -> Response {
    let miner_id = query.miner_id.trim().to_owned();
    if miner_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "miner_id_required" })),
        )
            .into_response();
    }

    match state.history.delete_miner(miner_id).await {
        Ok(deleted) => Json(DeleteMinerResponse { deleted }).into_response(),
        Err(error) => {
            warn!("failed to delete miner: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "delete_miner_failed" })),
            )
                .into_response()
        }
    }
}

async fn api_monitoring(
    State(state): State<UiState>,
    AxumPath(path): AxumPath<String>,
) -> Result<Json<Value>, ApiError> {
    let value = fetch_monitoring_json(&state, &path)
        .await
        .ok_or(ApiError::MonitoringUnavailable)?;
    Ok(Json(value))
}

async fn fetch_monitoring_json(state: &UiState, path: &str) -> Option<Value> {
    let base = state.metrics_base_url.as_ref()?;
    let path = path.trim_start_matches('/');
    let url = format!("{base}/{path}");
    let response = timeout(STATUS_MONITORING_TIMEOUT, state.client.get(url).send())
        .await
        .ok()?
        .ok()?;
    let mut value: Value = timeout(STATUS_MONITORING_TIMEOUT, response.json())
        .await
        .ok()?
        .ok()?;
    remove_monitoring_uptime(&mut value);
    Some(value)
}

fn remove_monitoring_uptime(value: &mut Value) {
    if let Value::Object(object) = value {
        object.remove("uptime_secs");
    }
}

async fn exchange_rates_response(state: &UiState) -> anyhow::Result<ExchangeRatesResponse> {
    let now = unix_now();
    {
        let cache = state.exchange_rates.read().await;
        if let Some(updated_at) = cache.updated_at {
            if now - updated_at < EXCHANGE_RATE_REFRESH_SECONDS {
                return Ok(ExchangeRatesResponse {
                    supported_currencies: SUPPORTED_CURRENCIES,
                    rates: cache.rates.clone(),
                    updated_at: cache.updated_at,
                    stale: false,
                });
            }
        }
    }

    match fetch_exchange_rates(&state.client).await {
        Ok(rates) => {
            let mut cache = state.exchange_rates.write().await;
            cache.rates = rates.clone();
            cache.updated_at = Some(now);
            Ok(ExchangeRatesResponse {
                supported_currencies: SUPPORTED_CURRENCIES,
                rates,
                updated_at: Some(now),
                stale: false,
            })
        }
        Err(error) => {
            let cache = state.exchange_rates.read().await;
            if cache.rates.is_empty() {
                return Err(error);
            }
            Ok(ExchangeRatesResponse {
                supported_currencies: SUPPORTED_CURRENCIES,
                rates: cache.rates.clone(),
                updated_at: cache.updated_at,
                stale: true,
            })
        }
    }
}

async fn fetch_exchange_rates(client: &Client) -> anyhow::Result<HashMap<String, f64>> {
    let currencies = SUPPORTED_CURRENCIES.join(",").to_lowercase();
    let url = format!(
        "{}/api/v3/simple/price?ids=bitcoin&vs_currencies={}",
        COINGECKO_API_BASE_URL, currencies
    );
    let response = client
        .get(url)
        .header(
            "User-Agent",
            format!("Canary Mining/{}", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .context("failed to fetch exchange rates")?;
    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read exchange rate response")?;
    if !status.is_success() {
        anyhow::bail!(
            "exchange rate API returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(500).collect::<String>()
        );
    }
    let response: CoinGeckoResponse =
        serde_json::from_str(&body).context("failed to parse exchange rate response")?;
    Ok(response
        .bitcoin
        .into_iter()
        .map(|(currency, rate)| (currency.to_uppercase(), rate))
        .collect())
}

async fn fetch_connected_miners(
    state: &UiState,
    current_miners: Option<Vec<MinerRow>>,
) -> Vec<MinerRow> {
    let mut seen_miners = state.seen_miners.write().await;
    merge_seen_miners(&mut seen_miners, current_miners, unix_now() as u64);

    sorted_miner_rows(seen_miners.values().cloned().collect())
}

fn merge_seen_miners(
    seen_miners: &mut HashMap<String, MinerRow>,
    current_miners: Option<Vec<MinerRow>>,
    now: u64,
) {
    if let Some(current_miners) = current_miners {
        for miner in seen_miners.values_mut() {
            miner.connected = false;
            miner.nominal_hashrate = 0.0;
        }
        for mut miner in current_miners {
            miner.last_seen_at = Some(now);
            seen_miners.insert(miner.id.clone(), miner);
        }
    }
}

fn sorted_miner_rows(mut miners: Vec<MinerRow>) -> Vec<MinerRow> {
    miners.sort_by(|left, right| {
        right
            .connected
            .cmp(&left.connected)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.payout_address.cmp(&right.payout_address))
    });
    miners
}

async fn fetch_current_miners(state: &UiState) -> Option<Vec<MinerRow>> {
    let Some(clients) = fetch_monitoring_json(state, "api/v1/clients?limit=100").await else {
        return None;
    };
    let Some(items) = clients.get("items").and_then(Value::as_array) else {
        return None;
    };

    let mut miners = Vec::new();
    for item in items {
        let Some(client_id) = item.get("client_id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(channels) = fetch_monitoring_json(
            state,
            &format!("api/v1/clients/{client_id}/channels?limit=100"),
        )
        .await
        else {
            return None;
        };

        append_channel_rows(
            state.network,
            client_id,
            "standard",
            channels.get("standard_channels").and_then(Value::as_array),
            &mut miners,
        );
        append_channel_rows(
            state.network,
            client_id,
            "extended",
            channels.get("extended_channels").and_then(Value::as_array),
            &mut miners,
        );
    }

    Some(miners)
}

fn append_channel_rows(
    network: Network,
    client_id: u64,
    channel_kind: &str,
    channels: Option<&Vec<Value>>,
    miners: &mut Vec<MinerRow>,
) {
    let Some(channels) = channels else {
        return;
    };

    for channel in channels {
        let user_identity = channel
            .get("user_identity")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let identity = parse_miner_identity(&user_identity, network);
        let (label, payout_address) = identity
            .map(|identity| (identity.label, identity.payout_address))
            .unwrap_or_else(|_| ("invalid identity".to_owned(), "-".to_owned()));
        let id = miner_row_id(network, &user_identity, &label, &payout_address);

        miners.push(MinerRow {
            id,
            client_id,
            channel_id: channel
                .get("channel_id")
                .and_then(Value::as_u64)
                .unwrap_or_default() as u32,
            channel_kind: channel_kind.to_owned(),
            connected: true,
            label,
            payout_address,
            user_identity,
            nominal_hashrate: channel
                .get("nominal_hashrate")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            shares_accepted: channel
                .get("shares_accepted")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            best_diff: channel
                .get("best_diff")
                .and_then(Value::as_f64)
                .unwrap_or_default(),
            blocks_found: channel
                .get("blocks_found")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
            target_hex: channel
                .get("target_hex")
                .and_then(Value::as_str)
                .map(str::to_owned),
            pool_diff: channel
                .get("target_hex")
                .and_then(Value::as_str)
                .and_then(target_hex_to_difficulty),
            last_seen_at: None,
        });
    }
}

fn miner_row_id(
    network: Network,
    user_identity: &str,
    label: &str,
    payout_address: &str,
) -> String {
    if payout_address != "-" {
        return miner_id(network, payout_address, label);
    }
    if !user_identity.trim().is_empty() {
        return user_identity.to_owned();
    }
    format!("{payout_address}:{label}")
}

async fn fetch_miner_workers(
    state: &UiState,
    current_miners: Option<Vec<MinerRow>>,
) -> Vec<MinerWorker> {
    let stats = state.miner_stats.snapshots();
    let now = unix_now() as u64;
    let known_miners = state.history.miners().await.unwrap_or_else(|error| {
        warn!("failed to fetch known miners: {error}");
        Vec::new()
    });
    build_miner_workers(state.network, current_miners, stats, known_miners, now)
}

fn build_miner_workers(
    network: Network,
    current_miners: Option<Vec<MinerRow>>,
    stats: Vec<MinerSessionSnapshot>,
    known_miners: Vec<KnownMiner>,
    now: u64,
) -> Vec<MinerWorker> {
    let mut stats_by_channel = stats
        .into_iter()
        .map(|stat| {
            (
                (
                    stat.downstream_id as u64,
                    stat.channel_id,
                    stat.channel_kind.to_owned(),
                ),
                stat,
            )
        })
        .collect::<HashMap<_, _>>();

    let mut workers = HashMap::<String, MinerWorker>::new();
    if let Some(current_miners) = current_miners {
        for miner in current_miners {
            let key = (
                miner.client_id,
                miner.channel_id,
                miner.channel_kind.clone(),
            );
            let stat = stats_by_channel.remove(&key);
            let session = miner_session_from_row(&miner, stat.as_ref(), now);
            push_worker_session(&mut workers, network, &miner.user_identity, session);
        }
    }

    for stat in stats_by_channel.into_values() {
        let session = miner_session_from_stat(&stat, now);
        push_worker_session(&mut workers, network, &stat.user_identity, session);
    }

    for miner in known_miners {
        if workers.contains_key(&miner.miner_id) {
            continue;
        }
        let miner_session = miner_session_from_known_miner(&miner, now);
        push_worker_session(&mut workers, network, &miner.user_identity, miner_session);
    }

    let mut workers = workers.into_values().collect::<Vec<_>>();
    for worker in &mut workers {
        finalize_worker(worker);
    }
    workers.sort_by(|left, right| {
        right
            .active_on_template
            .cmp(&left.active_on_template)
            .then_with(|| right.connected.cmp(&left.connected))
            .then_with(|| {
                left.last_seen_seconds
                    .unwrap_or(u64::MAX)
                    .cmp(&right.last_seen_seconds.unwrap_or(u64::MAX))
            })
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.payout_address.cmp(&right.payout_address))
    });
    workers
}

fn push_worker_session(
    workers: &mut HashMap<String, MinerWorker>,
    network: Network,
    user_identity: &str,
    session: MinerSession,
) {
    let identity = parse_miner_identity(user_identity, network);
    let (label, payout_address) = identity
        .map(|identity| (identity.label, identity.payout_address))
        .unwrap_or_else(|_| ("invalid identity".to_owned(), "-".to_owned()));
    let worker_id = miner_row_id(network, user_identity, &label, &payout_address);
    let worker = workers
        .entry(worker_id.clone())
        .or_insert_with(|| MinerWorker {
            worker_id,
            label,
            payout_address,
            user_identity: user_identity.to_owned(),
            session_count: 0,
            total_hashrate: 0.0,
            pool_diff: None,
            best_diff: 0.0,
            total_shares: 0,
            total_rejected: 0,
            rejection_percent: 0.0,
            opened_at: None,
            closed_at: None,
            last_seen_at: None,
            uptime_seconds: None,
            last_seen_seconds: None,
            connected: false,
            active_on_template: false,
            connection_state: "offline",
            sessions: Vec::new(),
        });
    worker.sessions.push(session);
}

fn finalize_worker(worker: &mut MinerWorker) {
    worker.sessions.sort_by(|left, right| {
        right
            .connected
            .cmp(&left.connected)
            .then_with(|| left.client_id.cmp(&right.client_id))
            .then_with(|| left.channel_id.cmp(&right.channel_id))
    });
    worker.session_count = worker.sessions.len();
    worker.connected = worker.sessions.iter().any(|session| session.connected);
    worker.total_hashrate = worker
        .sessions
        .iter()
        .filter(|session| session.connected)
        .map(|session| session.hashrate)
        .sum();
    worker.best_diff = worker
        .sessions
        .iter()
        .map(|session| session.best_diff)
        .fold(0.0, f64::max);
    worker.total_shares = worker
        .sessions
        .iter()
        .map(|session| session.shares_accepted)
        .sum();
    worker.total_rejected = worker
        .sessions
        .iter()
        .map(|session| session.shares_rejected)
        .sum();
    worker.rejection_percent = rejection_percent(worker.total_shares, worker.total_rejected);
    let display_sessions = worker
        .sessions
        .iter()
        .filter(|session| session.connected)
        .collect::<Vec<_>>();
    let display_sessions = if display_sessions.is_empty() {
        worker.sessions.iter().collect::<Vec<_>>()
    } else {
        display_sessions
    };
    let uptime_session = display_sessions
        .iter()
        .filter_map(|session| session.uptime_seconds.map(|uptime| (*session, uptime)))
        .max_by_key(|(_, uptime)| *uptime)
        .map(|(session, _)| session);
    worker.opened_at = uptime_session.and_then(|session| session.opened_at);
    worker.closed_at = uptime_session.and_then(|session| session.closed_at);
    worker.uptime_seconds = uptime_session.and_then(|session| session.uptime_seconds);
    let last_seen_session = display_sessions
        .iter()
        .filter_map(|session| {
            session
                .last_seen_seconds
                .map(|last_seen| (*session, last_seen))
        })
        .min_by_key(|(_, last_seen)| *last_seen)
        .map(|(session, _)| session);
    worker.last_seen_at = last_seen_session.and_then(|session| session.last_seen_at);
    worker.last_seen_seconds = last_seen_session.and_then(|session| session.last_seen_seconds);
    worker.pool_diff = common_pool_diff(&worker.sessions);
    worker.active_on_template = worker.connected
        || worker
            .last_seen_seconds
            .is_some_and(|seconds| seconds <= ACTIVE_MINER_GRACE_SECONDS);
    worker.connection_state = if worker.connected {
        "online"
    } else if worker.active_on_template {
        "recently_seen"
    } else {
        "offline"
    };
}

fn miner_session_from_row(
    miner: &MinerRow,
    stat: Option<&MinerSessionSnapshot>,
    now: u64,
) -> MinerSession {
    let shares_rejected = stat.map(|stat| stat.shares_rejected).unwrap_or_default();
    let last_seen_at = stat.map(|stat| stat.last_seen_at).or(miner.last_seen_at);
    let opened_at = stat.map(|stat| stat.opened_at);
    let closed_at = stat.and_then(|stat| stat.closed_at);
    MinerSession {
        session_id: format!(
            "{}:{}:{}",
            miner.client_id, miner.channel_kind, miner.channel_id
        ),
        client_id: miner.client_id,
        channel_id: miner.channel_id,
        channel_kind: miner.channel_kind.clone(),
        hashrate: if miner.connected {
            miner.nominal_hashrate
        } else {
            0.0
        },
        pool_diff: miner.pool_diff,
        best_diff: miner.best_diff,
        shares_accepted: miner.shares_accepted,
        shares_rejected,
        rejection_percent: rejection_percent(miner.shares_accepted, shares_rejected),
        opened_at,
        closed_at,
        last_seen_at,
        uptime_seconds: stat.map(|stat| session_uptime_seconds(stat, now)),
        last_seen_seconds: last_seen_at.map(|last_seen_at| now.saturating_sub(last_seen_at)),
        connected: miner.connected,
    }
}

fn miner_session_from_stat(stat: &MinerSessionSnapshot, now: u64) -> MinerSession {
    MinerSession {
        session_id: format!(
            "{}:{}:{}",
            stat.downstream_id, stat.channel_kind, stat.channel_id
        ),
        client_id: stat.downstream_id as u64,
        channel_id: stat.channel_id,
        channel_kind: stat.channel_kind.to_owned(),
        hashrate: if stat.closed_at.is_none() {
            stat.nominal_hashrate
        } else {
            0.0
        },
        pool_diff: None,
        best_diff: stat.best_diff,
        shares_accepted: stat.shares_accepted,
        shares_rejected: stat.shares_rejected,
        rejection_percent: rejection_percent(stat.shares_accepted, stat.shares_rejected),
        opened_at: Some(stat.opened_at),
        closed_at: stat.closed_at,
        last_seen_at: Some(stat.last_seen_at),
        uptime_seconds: Some(session_uptime_seconds(stat, now)),
        last_seen_seconds: Some(now.saturating_sub(stat.last_seen_at)),
        connected: stat.closed_at.is_none(),
    }
}

fn miner_session_from_known_miner(miner: &KnownMiner, now: u64) -> MinerSession {
    MinerSession {
        session_id: miner.miner_id.clone(),
        client_id: 0,
        channel_id: 0,
        channel_kind: "last".to_owned(),
        hashrate: if miner.connected {
            miner.nominal_hashrate
        } else {
            0.0
        },
        pool_diff: None,
        best_diff: miner.best_diff,
        shares_accepted: miner.shares_accepted,
        shares_rejected: miner.shares_rejected,
        rejection_percent: rejection_percent(miner.shares_accepted, miner.shares_rejected),
        opened_at: miner.opened_at,
        closed_at: miner.closed_at,
        last_seen_at: Some(miner.last_seen_at),
        uptime_seconds: if miner.connected {
            miner
                .opened_at
                .map(|opened_at| now.saturating_sub(opened_at))
        } else {
            miner.uptime_seconds
        },
        last_seen_seconds: Some(now.saturating_sub(miner.last_seen_at)),
        connected: miner.connected,
    }
}

fn session_uptime_seconds(stat: &MinerSessionSnapshot, now: u64) -> u64 {
    stat.closed_at.unwrap_or(now).saturating_sub(stat.opened_at)
}

fn rejection_percent(shares_accepted: u64, shares_rejected: u64) -> f64 {
    let total = shares_accepted + shares_rejected;
    if total == 0 {
        0.0
    } else {
        (shares_rejected as f64 / total as f64) * 100.0
    }
}

fn common_pool_diff(sessions: &[MinerSession]) -> Option<f64> {
    let mut active_diffs = sessions
        .iter()
        .filter(|session| session.connected)
        .filter_map(|session| session.pool_diff);
    let first = active_diffs.next()?;
    if active_diffs.all(|value| (value - first).abs() <= first.abs().max(1.0) * 0.000001) {
        Some(first)
    } else {
        None
    }
}

fn target_hex_to_difficulty(value: &str) -> Option<f64> {
    let mut bytes = [0_u8; 32];
    let value = value.strip_prefix("0x").unwrap_or(value);
    if value.len() != 64 {
        return None;
    }
    for index in 0..32 {
        bytes[index] = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    let target = Target::from_be_bytes(bytes);
    let difficulty = target.difficulty_float();
    difficulty.is_finite().then_some(difficulty)
}

const DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS: u64 = 2016;
const TARGET_BLOCK_SPACING_SECONDS: f64 = 600.0;

async fn fetch_bitcoin_core_status(state: &UiState) -> BitcoinCoreStatus {
    let (cached_mining_ready, cached_template) = cached_block_template_status(state).await;
    match rpc_with_timeout(
        "getblockchaininfo",
        state.bitcoin_core_rpc.blockchain_info(),
    )
    .await
    {
        Ok(chain) => {
            let (network_hashrate, network_difficulty) = tokio::join!(
                optional_rpc_with_timeout(
                    "getnetworkhashps",
                    state.bitcoin_core_rpc.network_hashrate()
                ),
                optional_rpc_with_timeout(
                    "getdifficulty",
                    state.bitcoin_core_rpc.network_difficulty()
                )
            );
            let difficulty_adjustment =
                fetch_difficulty_adjustment(state, chain.blocks, network_difficulty).await;
            let rpc_warning = rpc_warning_text(&chain.warnings);
            let (mining_ready, template) = if chain.initialblockdownload {
                (false, None)
            } else {
                (cached_mining_ready, cached_template)
            };

            BitcoinCoreStatus {
                rpc_available: true,
                rpc_warning,
                mining_ready,
                network_hashrate,
                network_difficulty,
                difficulty_adjustment,
                chain_height: Some(chain.blocks),
                sync_progress: Some(chain.verificationprogress),
                initial_block_download: Some(chain.initialblockdownload),
                template,
            }
        }
        Err(error) => {
            warn!("Bitcoin Core RPC status unavailable: {error}");
            BitcoinCoreStatus {
                rpc_available: false,
                rpc_warning: None,
                mining_ready: cached_mining_ready,
                network_hashrate: None,
                network_difficulty: None,
                difficulty_adjustment: None,
                chain_height: None,
                sync_progress: None,
                initial_block_download: None,
                template: cached_template,
            }
        }
    }
}

async fn rpc_with_timeout<T, F>(label: &str, future: F) -> Result<T, BitcoinCoreRpcError>
where
    F: Future<Output = Result<T, BitcoinCoreRpcError>>,
{
    timeout(STATUS_RPC_TIMEOUT, future).await.map_err(|_| {
        BitcoinCoreRpcError::UnexpectedResult {
            method: format!(
                "{label} timed out after {}ms",
                STATUS_RPC_TIMEOUT.as_millis()
            ),
        }
    })?
}

async fn optional_rpc_with_timeout<T, F>(label: &str, future: F) -> Option<T>
where
    F: Future<Output = Result<T, BitcoinCoreRpcError>>,
{
    match rpc_with_timeout(label, future).await {
        Ok(value) => Some(value),
        Err(error) => {
            warn!("optional Bitcoin Core RPC {label} unavailable: {error}");
            None
        }
    }
}

async fn cached_block_template_status(state: &UiState) -> (bool, Option<BlockTemplateStatus>) {
    let template = state
        .template_runtime
        .snapshot()
        .map(block_template_status_from_ipc_snapshot);
    (template.is_some(), template)
}

fn block_template_status_from_ipc_snapshot(
    snapshot: TemplateStatusSnapshot,
) -> BlockTemplateStatus {
    BlockTemplateStatus {
        height: snapshot.height,
        reward_sats: snapshot.reward_sats,
        fee_sats: snapshot.fee_sats,
        weight: snapshot.weight,
        weight_percent: snapshot.weight_percent,
        transaction_count: snapshot.transaction_count,
        updated_at: snapshot.updated_at,
        source: template_status_source(snapshot.source),
        status: template_status_kind(snapshot.status),
    }
}

fn template_status_source(source: TemplateStatusSource) -> &'static str {
    match source {
        TemplateStatusSource::IpcBootstrap => "ipc_bootstrap",
        TemplateStatusSource::IpcChainTip => "ipc_chain_tip",
        TemplateStatusSource::IpcMempool => "ipc_mempool",
    }
}

fn template_status_kind(status: TemplateStatusKind) -> &'static str {
    match status {
        TemplateStatusKind::Available => "available",
    }
}

async fn fetch_difficulty_adjustment(
    state: &UiState,
    chain_height: u64,
    current_difficulty: Option<f64>,
) -> Option<DifficultyAdjustmentStatus> {
    let period_start_height = (chain_height / DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS)
        * DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS;
    if let Some(period_start_time) =
        cached_difficulty_period_start_time(state, period_start_height).await
    {
        return Some(calculate_difficulty_adjustment(
            chain_height,
            period_start_time,
            unix_now().max(0) as u64,
            current_difficulty,
        ));
    }

    let period_start_block = timeout(STATUS_RPC_TIMEOUT, async {
        let period_start_hash = state
            .bitcoin_core_rpc
            .block_hash(period_start_height)
            .await?;
        state
            .bitcoin_core_rpc
            .verbose_block(&period_start_hash)
            .await
    })
    .await
    .ok()?
    .ok()?;
    {
        let mut cache = state.difficulty_period_start.write().await;
        cache.height = Some(period_start_height);
        cache.timestamp = Some(period_start_block.time);
    }
    Some(calculate_difficulty_adjustment(
        chain_height,
        period_start_block.time,
        unix_now().max(0) as u64,
        current_difficulty,
    ))
}

async fn cached_difficulty_period_start_time(
    state: &UiState,
    period_start_height: u64,
) -> Option<u64> {
    let cache = state.difficulty_period_start.read().await;
    (cache.height == Some(period_start_height))
        .then_some(cache.timestamp)
        .flatten()
}

fn calculate_difficulty_adjustment(
    chain_height: u64,
    period_start_time: u64,
    now: u64,
    current_difficulty: Option<f64>,
) -> DifficultyAdjustmentStatus {
    let next_height = next_difficulty_adjustment_height(chain_height);
    let blocks_remaining = next_height.saturating_sub(chain_height);
    let blocks_mined_in_period =
        (chain_height % DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS).saturating_add(1);
    let progress_percent =
        (blocks_mined_in_period as f64 / DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS as f64) * 100.0;

    let elapsed_blocks = chain_height.saturating_sub(
        (chain_height / DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS)
            * DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS,
    );
    let actual_elapsed_seconds = now.saturating_sub(period_start_time);
    let average_block_seconds = if elapsed_blocks > 0 && actual_elapsed_seconds > 0 {
        Some(actual_elapsed_seconds as f64 / elapsed_blocks as f64)
    } else {
        None
    };
    let estimated_seconds_remaining = average_block_seconds
        .map(|seconds| (seconds * blocks_remaining as f64).round() as u64)
        .or_else(|| Some((blocks_remaining as f64 * TARGET_BLOCK_SPACING_SECONDS).round() as u64));

    let projected_change_multiplier = if elapsed_blocks > 0 && actual_elapsed_seconds > 0 {
        let expected_elapsed_seconds = elapsed_blocks as f64 * TARGET_BLOCK_SPACING_SECONDS;
        Some((expected_elapsed_seconds / actual_elapsed_seconds as f64).clamp(0.25, 4.0))
    } else {
        None
    };
    let projected_percent_change =
        projected_change_multiplier.map(|multiplier| (multiplier - 1.0) * 100.0);
    let projected_difficulty = current_difficulty
        .zip(projected_change_multiplier)
        .map(|(difficulty, multiplier)| difficulty * multiplier)
        .filter(|difficulty| difficulty.is_finite());

    DifficultyAdjustmentStatus {
        next_height,
        blocks_remaining,
        progress_percent,
        estimated_seconds_remaining,
        projected_percent_change,
        projected_difficulty,
    }
}

fn next_difficulty_adjustment_height(chain_height: u64) -> u64 {
    let next_block_height = chain_height.saturating_add(1);
    next_block_height.saturating_add(DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS - 1)
        / DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS
        * DIFFICULTY_ADJUSTMENT_INTERVAL_BLOCKS
}

fn rpc_warning_text(value: &Value) -> Option<String> {
    match value {
        Value::String(warning) if !warning.trim().is_empty() => Some(warning.clone()),
        Value::Array(warnings) => {
            let text = warnings
                .iter()
                .filter_map(Value::as_str)
                .filter(|warning| !warning.trim().is_empty())
                .collect::<Vec<_>>()
                .join("; ");
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

fn unknown_pool() -> PoolAttribution {
    PoolAttribution {
        name: "Unknown".to_owned(),
        slug: "unknown".to_owned(),
        logo: "unknown.svg".to_owned(),
        link: Some("https://learnmeabitcoin.com/technical/coinbase-transaction".to_owned()),
    }
}

fn canary_pool() -> PoolAttribution {
    PoolAttribution {
        name: "Canary Solo".to_owned(),
        slug: "canarysolo".to_owned(),
        logo: "default.svg".to_owned(),
        link: None,
    }
}

static MEMPOOL_POOL_REGISTRY: LazyLock<Vec<MempoolPoolEntry>> = LazyLock::new(|| {
    serde_json::from_str(MEMPOOL_POOLS_JSON).expect("vendored mempool pools-v2.json is valid")
});

async fn recent_blocks_response(
    state: &UiState,
    chain_height: Option<u64>,
) -> anyhow::Result<Vec<BlockTimelineItem>> {
    let previous = {
        let cache = state.recent_blocks.read().await;
        if cache.chain_height == chain_height {
            return Ok(cache.blocks.clone());
        }
        cache.clone()
    };

    if chain_height.is_none() {
        return Ok(previous.blocks);
    }

    let blocks = fetch_recent_blocks(state, chain_height).await?;
    if !recent_blocks_complete(chain_height, &blocks) {
        if blocks.is_empty() {
            return Ok(previous.blocks);
        }
        let mut cache = state.recent_blocks.write().await;
        cache.chain_height = None;
        cache.blocks = blocks.clone();
        return Ok(blocks);
    }

    let mut cache = state.recent_blocks.write().await;
    cache.chain_height = chain_height;
    cache.blocks = blocks.clone();
    Ok(blocks)
}

fn recent_blocks_complete(chain_height: Option<u64>, blocks: &[BlockTimelineItem]) -> bool {
    match chain_height {
        Some(height) => blocks.iter().any(|block| block.height == height),
        None => blocks.is_empty(),
    }
}

async fn fetch_recent_blocks(
    state: &UiState,
    chain_height: Option<u64>,
) -> anyhow::Result<Vec<BlockTimelineItem>> {
    let Some(chain_height) = chain_height else {
        return Ok(Vec::new());
    };

    let first_height = first_recent_block_height(chain_height);
    let mut blocks = {
        let cache = state.recent_blocks.read().await;
        cache
            .blocks
            .iter()
            .filter(|block| block.height >= first_height && block.height <= chain_height)
            .cloned()
            .collect::<Vec<_>>()
    };

    blocks.sort_by_key(|block| block.height);
    let mut next_height = blocks
        .last()
        .map(|block| block.height.saturating_add(1))
        .unwrap_or(first_height);
    while next_height <= chain_height {
        match fetch_recent_block(state, next_height).await {
            Ok(block) => blocks.push(block),
            Err(error) => {
                warn!("recent block {next_height} unavailable: {error}");
                break;
            }
        }
        next_height = next_height.saturating_add(1);
    }
    blocks.retain(|block| block.height >= first_height && block.height <= chain_height);
    blocks.sort_by_key(|block| block.height);
    Ok(blocks)
}

fn first_recent_block_height(chain_height: u64) -> u64 {
    let count = (RECENT_BLOCK_LIMIT as u64).min(chain_height.saturating_add(1));
    chain_height.saturating_add(1).saturating_sub(count)
}

async fn fetch_recent_block(state: &UiState, height: u64) -> anyhow::Result<BlockTimelineItem> {
    let block = timeout(RECENT_BLOCK_RPC_TIMEOUT, async {
        let hash = state.bitcoin_core_rpc.block_hash(height).await?;
        state.bitcoin_core_rpc.verbose_block(&hash).await
    })
    .await
    .context("recent block RPC timed out")??;
    let coinbase = block.tx.first();
    Ok(BlockTimelineItem {
        height: block.height,
        hash: block.hash,
        timestamp: block.time,
        weight: block.weight,
        transaction_count: block.n_tx.or(Some(block.tx.len())),
        reward_sats: coinbase.and_then(coinbase_reward_sats),
        pool: detect_block_pool(state, coinbase),
    })
}

fn coinbase_reward_sats(coinbase: &crate::bitcoin_core_rpc::VerboseTransaction) -> Option<u64> {
    let total_btc = coinbase
        .vout
        .iter()
        .filter_map(|output| output.value)
        .sum::<f64>();
    total_btc
        .is_finite()
        .then_some((total_btc * 100_000_000.0).round() as u64)
}

fn detect_block_pool(
    state: &UiState,
    coinbase: Option<&crate::bitcoin_core_rpc::VerboseTransaction>,
) -> PoolAttribution {
    let Some(coinbase) = coinbase else {
        return unknown_pool();
    };
    let coinbase_address = coinbase
        .vout
        .first()
        .and_then(|output| output.script_pub_key.address.as_deref());
    if coinbase_address == Some(state.app_config.default_pool_payout_address()) {
        return canary_pool();
    }

    for pool in MEMPOOL_POOL_REGISTRY.iter() {
        if let Some(address) = coinbase_address {
            if pool.addresses.iter().any(|candidate| candidate == address) {
                return mempool_pool_attribution(pool);
            }
        }
    }

    let script_sig = coinbase
        .vin
        .first()
        .and_then(|input| input.coinbase.as_deref())
        .and_then(hex_to_lossy_ascii)
        .unwrap_or_default();
    for pool in MEMPOOL_POOL_REGISTRY.iter() {
        if mempool_pool_matches_script_sig(pool, &script_sig) {
            return mempool_pool_attribution(pool);
        }
    }

    unknown_pool()
}

fn mempool_pool_matches_script_sig(pool: &MempoolPoolEntry, script_sig: &str) -> bool {
    pool.tags
        .iter()
        .filter(|tag| !tag.trim().is_empty())
        .any(|tag| contains_case_insensitive(script_sig, tag))
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

fn mempool_pool_attribution(pool: &MempoolPoolEntry) -> PoolAttribution {
    let slug = pool_slug(&pool.name);
    PoolAttribution {
        name: pool.name.clone(),
        logo: pool_logo_for_slug(&slug),
        slug,
        link: pool.link.as_ref().filter(|link| !link.is_empty()).cloned(),
    }
}

fn pool_slug(name: &str) -> String {
    name.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn pool_logo_for_slug(slug: &str) -> String {
    let logo = format!("{slug}.svg");
    if pool_logo_asset(&logo).is_some() {
        logo
    } else {
        "default.svg".to_owned()
    }
}

fn hex_to_lossy_ascii(value: &str) -> Option<String> {
    let hex = value.strip_prefix("0x").unwrap_or(value);
    if hex.len() % 2 != 0 {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for index in (0..hex.len()).step_by(2) {
        bytes.push(u8::from_str_radix(&hex[index..index + 2], 16).ok()?);
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn unix_now() -> i64 {
    unix_timestamp(SystemTime::now())
}

fn unix_timestamp(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug)]
enum ApiError {
    MonitoringUnavailable,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::MonitoringUnavailable => (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "monitoring_unavailable" })),
            )
                .into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGTEST_ADDRESS: &str = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl";

    #[test]
    fn wildcard_routes_use_axum_0_8_syntax() {
        let _ = Router::<()>::new()
            .route("/assets/fonts/geist/{*path}", get(|| async {}))
            .route("/miner-logos/{*path}", get(|| async {}))
            .route("/pool-logos/{*path}", get(|| async {}))
            .route("/api/monitoring/{*path}", get(|| async {}));
    }

    fn test_miner_row(user_identity: &str) -> MinerRow {
        MinerRow {
            id: miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19"),
            client_id: 1,
            channel_id: 2,
            channel_kind: "extended".to_owned(),
            connected: true,
            label: "garage.s19".to_owned(),
            payout_address: REGTEST_ADDRESS.to_owned(),
            user_identity: user_identity.to_owned(),
            nominal_hashrate: 5_000_000_000_000.0,
            shares_accepted: 10,
            best_diff: 1_000.0,
            blocks_found: 0,
            target_hex: None,
            pool_diff: Some(128.0),
            last_seen_at: None,
        }
    }

    fn test_session_snapshot(
        user_identity: &str,
        closed_at: Option<u64>,
        last_seen_at: u64,
    ) -> MinerSessionSnapshot {
        MinerSessionSnapshot {
            downstream_id: 1,
            channel_id: 2,
            channel_kind: "extended",
            user_identity: user_identity.to_owned(),
            opened_at: 900,
            closed_at,
            last_seen_at,
            nominal_hashrate: 5_000_000_000_000.0,
            shares_accepted: 10,
            best_diff: 1_000.0,
            blocks_found: 0,
            shares_rejected: 1,
            shares_rejected_by_reason: HashMap::new(),
        }
    }

    fn test_known_miner(user_identity: &str, connected: bool, last_seen_at: u64) -> KnownMiner {
        KnownMiner {
            miner_id: miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19"),
            user_identity: user_identity.to_owned(),
            payout_address: REGTEST_ADDRESS.to_owned(),
            label: "garage.s19".to_owned(),
            connected,
            first_seen_at: 100,
            last_seen_at,
            opened_at: Some(900),
            closed_at: (!connected).then_some(last_seen_at),
            uptime_seconds: (!connected).then_some(last_seen_at.saturating_sub(900)),
            nominal_hashrate: if connected { 5_000_000_000_000.0 } else { 0.0 },
            shares_accepted: 10,
            best_diff: 1_000.0,
            blocks_found: 0,
            shares_rejected: 1,
        }
    }

    #[test]
    fn channel_rows_include_label_and_payout_address() {
        let channels = vec![json!({
            "channel_id": 7,
            "user_identity": format!("{REGTEST_ADDRESS}.garage.s19"),
            "nominal_hashrate": 123.0,
            "shares_accepted": 4,
            "best_diff": 5.5,
            "blocks_found": 1
        })];
        let mut miners = Vec::new();

        append_channel_rows(
            Network::Regtest,
            3,
            "standard",
            Some(&channels),
            &mut miners,
        );

        assert_eq!(miners.len(), 1);
        assert_eq!(
            miners[0].id,
            miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19")
        );
        assert_eq!(miners[0].client_id, 3);
        assert!(miners[0].connected);
        assert_eq!(miners[0].channel_id, 7);
        assert_eq!(miners[0].label, "garage.s19");
        assert_eq!(miners[0].payout_address, REGTEST_ADDRESS);
    }

    #[test]
    fn vendored_mempool_registry_detects_spiderpool_block_950085_markers() {
        let script_sig =
            hex_to_lossy_ascii("03457f0e04e2620c6a537069646572506f6f6c2f3536392f").unwrap();
        assert!(script_sig.contains("SpiderPool/569/"));

        let spiderpool = MEMPOOL_POOL_REGISTRY
            .iter()
            .find(|pool| pool.name == "SpiderPool")
            .expect("SpiderPool rule");
        assert!(
            spiderpool
                .addresses
                .iter()
                .any(|address| address == "1BM1sAcrfV6d4zPKytzziu4McLQDsFC2Qc")
        );
        assert!(mempool_pool_matches_script_sig(spiderpool, &script_sig));
    }

    #[test]
    fn difficulty_adjustment_counts_down_to_next_retarget_block() {
        assert_eq!(next_difficulty_adjustment_height(0), 2016);
        assert_eq!(next_difficulty_adjustment_height(2015), 2016);
        assert_eq!(next_difficulty_adjustment_height(2016), 4032);

        let status = calculate_difficulty_adjustment(2015, 0, 2015 * 600, Some(10.0));
        assert_eq!(status.next_height, 2016);
        assert_eq!(status.blocks_remaining, 1);
        assert_eq!(status.estimated_seconds_remaining, Some(600));
        assert_eq!(status.projected_percent_change, Some(0.0));
        assert_eq!(status.projected_difficulty, Some(10.0));
    }

    #[test]
    fn difficulty_adjustment_projects_direction_from_observed_block_pace() {
        let faster = calculate_difficulty_adjustment(1008, 0, 1008 * 300, Some(20.0));
        assert_eq!(faster.projected_percent_change, Some(100.0));
        assert_eq!(faster.projected_difficulty, Some(40.0));

        let slower = calculate_difficulty_adjustment(1008, 0, 1008 * 1200, Some(20.0));
        assert_eq!(slower.projected_percent_change, Some(-50.0));
        assert_eq!(slower.projected_difficulty, Some(10.0));
    }

    #[test]
    fn recent_block_cache_requires_current_tip_before_marking_complete() {
        let blocks = vec![BlockTimelineItem {
            height: 99,
            hash: "00".to_owned(),
            timestamp: 1,
            weight: None,
            transaction_count: None,
            reward_sats: None,
            pool: unknown_pool(),
        }];

        assert!(!recent_blocks_complete(Some(100), &blocks));
        assert!(recent_blocks_complete(Some(99), &blocks));
        assert!(recent_blocks_complete(None, &[]));
    }

    #[test]
    fn ipc_template_snapshot_is_surfaced_for_dashboard() {
        let status = block_template_status_from_ipc_snapshot(TemplateStatusSnapshot {
            height: 950_001,
            reward_sats: Some(312_500_123),
            fee_sats: Some(123),
            weight: Some(1_500_000),
            weight_percent: Some(37.5),
            transaction_count: 42,
            updated_at: 1_700_000_000,
            source: TemplateStatusSource::IpcMempool,
            status: TemplateStatusKind::Available,
        });

        assert_eq!(status.height, 950_001);
        assert_eq!(status.reward_sats, Some(312_500_123));
        assert_eq!(status.fee_sats, Some(123));
        assert_eq!(status.weight, Some(1_500_000));
        assert_eq!(status.transaction_count, 42);
        assert_eq!(status.source, "ipc_mempool");
        assert_eq!(status.status, "available");
    }

    #[test]
    fn missing_ipc_template_has_no_dashboard_template() {
        let runtime = TemplateRuntimeStatus::default();

        assert!(runtime.snapshot().is_none());
    }

    #[test]
    fn connected_worker_uses_active_session_uptime() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let workers = build_miner_workers(
            Network::Regtest,
            Some(vec![test_miner_row(&user_identity)]),
            vec![test_session_snapshot(&user_identity, None, 995)],
            vec![test_known_miner(&user_identity, false, 80_000)],
            1_000,
        );

        assert_eq!(workers.len(), 1);
        assert!(workers[0].connected);
        assert!(workers[0].active_on_template);
        assert_eq!(workers[0].connection_state, "online");
        assert_eq!(workers[0].session_count, 1);
        assert_eq!(workers[0].opened_at, Some(900));
        assert_eq!(workers[0].closed_at, None);
        assert_eq!(workers[0].last_seen_at, Some(995));
        assert_eq!(workers[0].uptime_seconds, Some(100));
        assert_eq!(workers[0].last_seen_seconds, Some(5));
        assert_eq!(workers[0].sessions[0].opened_at, Some(900));
        assert_eq!(workers[0].sessions[0].closed_at, None);
        assert_eq!(workers[0].sessions[0].last_seen_at, Some(995));
    }

    #[test]
    fn recently_seen_worker_stays_active_during_grace_window() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let workers = build_miner_workers(
            Network::Regtest,
            Some(Vec::new()),
            vec![test_session_snapshot(&user_identity, Some(980), 980)],
            Vec::new(),
            1_000,
        );

        assert_eq!(workers.len(), 1);
        assert!(!workers[0].connected);
        assert!(workers[0].active_on_template);
        assert_eq!(workers[0].connection_state, "recently_seen");
        assert_eq!(workers[0].opened_at, Some(900));
        assert_eq!(workers[0].closed_at, Some(980));
        assert_eq!(workers[0].last_seen_at, Some(980));
        assert_eq!(workers[0].last_seen_seconds, Some(20));
    }

    #[test]
    fn worker_moves_off_template_after_grace_window() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let workers = build_miner_workers(
            Network::Regtest,
            Some(Vec::new()),
            vec![test_session_snapshot(&user_identity, Some(879), 879)],
            Vec::new(),
            1_000,
        );

        assert_eq!(workers.len(), 1);
        assert!(!workers[0].connected);
        assert!(!workers[0].active_on_template);
        assert_eq!(workers[0].connection_state, "offline");
        assert_eq!(workers[0].last_seen_at, Some(879));
        assert_eq!(workers[0].last_seen_seconds, Some(121));
    }

    #[test]
    fn previously_seen_miner_uses_stored_last_seen_without_monitoring_row() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let mut miner = test_miner_row(&user_identity);
        miner.connected = false;
        miner.nominal_hashrate = 0.0;
        miner.last_seen_at = Some(940);

        let workers = build_miner_workers(
            Network::Regtest,
            Some(vec![miner]),
            Vec::new(),
            Vec::new(),
            1_000,
        );

        assert_eq!(workers.len(), 1);
        assert!(!workers[0].connected);
        assert!(workers[0].active_on_template);
        assert_eq!(workers[0].connection_state, "recently_seen");
        assert_eq!(workers[0].total_hashrate, 0.0);
        assert_eq!(workers[0].opened_at, None);
        assert_eq!(workers[0].closed_at, None);
        assert_eq!(workers[0].last_seen_at, Some(940));
        assert_eq!(workers[0].last_seen_seconds, Some(60));
    }

    #[test]
    fn disconnected_seen_miner_remains_available_for_history() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let mut miner = test_miner_row(&user_identity);
        miner.connected = false;
        miner.nominal_hashrate = 42_000.0;

        let workers = build_miner_workers(
            Network::Regtest,
            Some(vec![miner]),
            Vec::new(),
            Vec::new(),
            1_000,
        );

        assert_eq!(workers.len(), 1);
        assert!(!workers[0].connected);
        assert!(!workers[0].active_on_template);
        assert_eq!(workers[0].connection_state, "offline");
        assert_eq!(workers[0].total_hashrate, 0.0);
        assert_eq!(workers[0].label, "garage.s19");
    }

    #[test]
    fn unavailable_monitoring_does_not_clear_seen_miners() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let key = miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19");
        let mut seen = HashMap::from([(key.clone(), test_miner_row(&user_identity))]);

        merge_seen_miners(&mut seen, None, 1_000);

        assert!(seen[&key].connected);
        assert_eq!(seen[&key].nominal_hashrate, 5_000_000_000_000.0);
    }

    #[test]
    fn authoritative_empty_monitoring_marks_seen_miners_disconnected() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let key = miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19");
        let mut seen = HashMap::from([(key.clone(), test_miner_row(&user_identity))]);

        merge_seen_miners(&mut seen, Some(Vec::new()), 1_000);

        assert!(!seen[&key].connected);
        assert_eq!(seen[&key].nominal_hashrate, 0.0);
        assert_eq!(seen[&key].last_seen_at, None);
    }

    #[test]
    fn monitoring_sample_records_seen_miner_last_seen_at() {
        let user_identity = format!("{REGTEST_ADDRESS}.garage.s19");
        let key = miner_id(Network::Regtest, REGTEST_ADDRESS, "garage.s19");
        let mut seen = HashMap::new();

        merge_seen_miners(&mut seen, Some(vec![test_miner_row(&user_identity)]), 1_000);
        merge_seen_miners(&mut seen, Some(Vec::new()), 1_030);

        assert!(!seen[&key].connected);
        assert_eq!(seen[&key].last_seen_at, Some(1_000));
    }

    #[test]
    fn embedded_miner_logo_lookup_serves_known_assets_only() {
        assert_eq!(miner_logo_asset("bitaxe.svg").unwrap().0, "image/svg+xml");
        assert_eq!(miner_logo_asset("qaxe.svg").unwrap().0, "image/svg+xml");
        assert_eq!(miner_logo_asset("osmu.svg").unwrap().0, "image/svg+xml");
        assert!(miner_logo_asset("apollo.svg").is_none());
        assert!(miner_logo_asset("../ui/style.css").is_none());
        assert!(geist_font_asset("geist-latin.woff2").is_some());
        assert!(geist_font_asset("geist-mono-latin.woff2").is_some());
        assert!(geist_font_asset("../style.css").is_none());
        assert_eq!(
            pool_logo_asset("spiderpool.svg").unwrap().0,
            "image/svg+xml"
        );
    }
}
