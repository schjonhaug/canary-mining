let lastStatus = null;
let currentNetwork = "mainnet";
let exchangeRates = {};
const currencyStorageKey = "canary-mining-currency";
let selectedCurrency =
  localStorage.getItem(currencyStorageKey) ||
  localStorage.getItem("canary-solo-currency") ||
  "USD";
let chainTimelineResizeObserver = null;
let chainTimelineObservedElement = null;
const expandedMinerWorkers = new Set();
const searchParams = new URLSearchParams(window.location.search);
const debugMode = searchParams.has("debug");
const debugMinerCount = Math.max(0, Number.parseInt(searchParams.get("debug-miners") || "", 10) || 0);
let payoutAddressValidationRequest = 0;
let serverClockOffsetSeconds = 0;
let payoutAddressLabelTimer = 0;
const statusRefreshIntervalMs = 10000;
const counterRefreshIntervalMs = 1000;
const activeMinerGraceSeconds = 120;

function updateMonitoringVisibility(network) {
  const monitoring = document.querySelector(".monitoring-bottom");
  if (!monitoring) return false;
  const isMainnet = (network || "mainnet") === "mainnet";
  const visible = !isMainnet || debugMode;
  monitoring.hidden = !visible;
  if (!visible) {
    const listener = document.getElementById("metrics-listener");
    const payload = document.getElementById("monitoring");
    if (listener) listener.textContent = "";
    if (payload) payload.textContent = "";
  }
  return visible;
}

function formatTime(epochSeconds) {
  if (!epochSeconds) return "-";
  const timestamp = new Date(epochSeconds * 1000);
  const hours = String(timestamp.getHours()).padStart(2, "0");
  const minutes = String(timestamp.getMinutes()).padStart(2, "0");
  const seconds = String(timestamp.getSeconds()).padStart(2, "0");
  return `${hours}:${minutes}:${seconds}`;
}

function formatHashrate(hashrate) {
  if (hashrate === null || hashrate === undefined) return "unavailable";
  const value = Number(hashrate || 0);
  const units = ["H/s", "KH/s", "MH/s", "GH/s", "TH/s", "PH/s", "EH/s", "ZH/s"];
  let scaled = value;
  let unit = units[0];
  for (let i = 0; i < units.length - 1 && scaled >= 1000; i += 1) {
    scaled /= 1000;
    unit = units[i + 1];
  }
  if (scaled >= 100 || unit === units[0]) return `${scaled.toFixed(0)} ${unit}`;
  if (scaled >= 10) return `${scaled.toFixed(1)} ${unit}`;
  return `${scaled.toFixed(2)} ${unit}`;
}

function formatDifficulty(value) {
  if (value === null || value === undefined) return "-";
  const numeric = Number(value || 0);
  if (numeric > 0 && numeric < 0.01) return numeric.toExponential(2);
  const units = ["", "K", "M", "G", "T", "P", "E"];
  let scaled = numeric;
  let unit = units[0];
  for (let i = 0; i < units.length - 1 && scaled >= 1000; i += 1) {
    scaled /= 1000;
    unit = units[i + 1];
  }
  if (scaled >= 100 || unit === units[0]) return `${scaled.toFixed(0)}${unit}`;
  if (scaled >= 10) return `${scaled.toFixed(1)}${unit}`;
  return `${scaled.toFixed(2)}${unit}`;
}

function formatInteger(value) {
  if (value === null || value === undefined) return "-";
  return Number(value).toLocaleString("en-US");
}

function formatDuration(seconds) {
  if (seconds === null || seconds === undefined) return "-";
  const total = Math.max(0, Number(seconds || 0));
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = Math.floor(total % 60);
  if (days > 0) return `${days.toFixed(0)}d ${hours.toFixed(0)}h`;
  if (hours > 0) return `${hours.toFixed(0)}h ${minutes.toFixed(0)}m`;
  if (minutes > 0) return `${minutes.toFixed(0)}m ${secs.toFixed(0)}s`;
  return `${secs.toFixed(0)}s`;
}

function currentServerSeconds() {
  return Math.floor(Date.now() / 1000 + serverClockOffsetSeconds);
}

function numericOrNull(value) {
  if (value === null || value === undefined || value === "") return null;
  const numeric = Number(value);
  return Number.isFinite(numeric) ? numeric : null;
}

function minerUptimeSeconds(miner) {
  const openedAt = numericOrNull(miner?.opened_at);
  if (openedAt !== null) {
    const closedAt = numericOrNull(miner?.closed_at);
    return Math.max(0, (closedAt ?? currentServerSeconds()) - openedAt);
  }
  return miner?.uptime_seconds ?? null;
}

function minerLastSeenSeconds(miner) {
  const lastSeenAt = numericOrNull(miner?.last_seen_at);
  if (lastSeenAt !== null) return Math.max(0, currentServerSeconds() - lastSeenAt);
  return miner?.last_seen_seconds ?? null;
}

function sessionUptimeSeconds(session) {
  return minerUptimeSeconds(session);
}

function sessionLastSeenSeconds(session) {
  return minerLastSeenSeconds(session);
}

function updateTemplateWeightChart(weight, percent) {
  const fill = document.getElementById("template-weight-chart-fill");
  const description = document.getElementById("template-weight-chart-desc");
  if (!fill || !description) return;

  const hasWeight = weight !== undefined && weight !== null;
  const hasPercent = percent !== undefined && percent !== null;
  const numericPercent = Number(percent);
  const rawPercent = hasPercent && Number.isFinite(numericPercent)
    ? Math.max(0, Math.min(100, numericPercent))
    : 0;
  const visiblePercent = rawPercent > 0 ? Math.max(rawPercent, 0.8) : 0;

  fill.style.strokeDasharray = `${visiblePercent} ${100 - visiblePercent}`;
  fill.classList.toggle("is-empty", !hasPercent || rawPercent === 0);
  description.textContent = hasWeight && hasPercent
    ? `${formatInteger(weight)} WU, ${formatPercent(rawPercent, 2)} of the block weight limit`
    : "No template weight data available";
}

function escapeHtml(value) {
  return String(value ?? "").replace(/[&<>"']/g, (character) => ({
    "&": "&amp;",
    "<": "&lt;",
    ">": "&gt;",
    "\"": "&quot;",
    "'": "&#39;",
  }[character]));
}

function normalizeMinerText(value) {
  return String(value || "")
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .replace(/[^a-z0-9+]+/g, " ");
}

const MINER_LOGO_RULES = [
  { family: "NerdQAxe++", logo: "nerdqaxe-plus-plus.svg", terms: ["nerdqaxe++", "nerdqaxeplus2", "nerdqaxe plus plus"] },
  { family: "NerdQAxe+", logo: "nerdqaxe-plus.svg", terms: ["nerdqaxe+", "nerdqaxeplus", "nerdqaxe plus"] },
  { family: "NerdOCTAXE Gamma", logo: "nerdoctaxe-gamma.svg", terms: ["nerdoctaxe gamma", "nerdoctaxegamma", "octaxe gamma"] },
  { family: "NerdOCTAXE+", logo: "nerdoctaxe-plus.svg", terms: ["nerdoctaxe+", "nerdoctaxeplus", "octaxe+"] },
  { family: "NerdEKO", logo: "nerdeko.svg", terms: ["nerdeko", "nerd eko", "nerdeco", "nerd eco", "nerdico", "nerd ico"] },
  { family: "NerdAxe", logo: "nerdaxe.svg", terms: ["nerdaxe", "nmaxe"] },
  { family: "Qaxe", logo: "qaxe.svg", terms: ["qaxe"] },
  { family: "NerdOCTAXE", logo: "nerdoctaxe-plus.svg", terms: ["nerdoctaxe", "octaxe"] },
  { family: "NMMiner", logo: "nmminer.svg", terms: ["nmminer"] },
  { family: "Bitaxe", logo: "bitaxe.svg", terms: ["bitaxe", "bitaxe gamma", "bitaxe supra", "baxe", "bitax", "gamma", "supra", "601", "602", "bithalo", "gekko"] },
  { family: "PiAxe", logo: "piaxe.svg", terms: ["piaxe", "pi axe"] },
  { family: "Bitcrane", logo: "bitcrane.svg", terms: ["bitcrane", "bit crane"] },
  { family: "Braiins", logo: "braiins.svg", terms: ["braiins", "braiins os", "braiins os+", "brains", "brain"] },
  { family: "Antminer", logo: "antminer.svg", terms: ["antminer", "s19", "s21", "t21"] },
  { family: "OSMU", logo: "osmu.svg", terms: ["nerd miner", "nerdos", "nerdnos", "nerdnos v", "mvi iiax nerd", "mvi", "nerd mille", "nerdz", "nerd"] },
];

const MINER_LOGO_CACHE_TOKEN = String(Date.now());

function minerLogoSrc(logo) {
  return `/miner-logos/${encodeURIComponent(String(logo || "generic.svg"))}?v=${MINER_LOGO_CACHE_TOKEN}`;
}

function detectMinerLogo(miner) {
  const text = normalizeMinerText(`${miner.label || ""} ${miner.user_identity || ""}`);
  const match = MINER_LOGO_RULES.find((rule) => rule.terms.some((term) => text.includes(term)));
  return match || { family: "Miner", logo: "generic.svg" };
}

function displayMinerLabel(worker, detected) {
  const label = String(worker.label || worker.worker_id || "-");
  if (!detected?.terms?.length) return label;

  const normalizedLabel = normalizeMinerText(label);
  const normalizedFamily = normalizeMinerText(detected.family || "");
  if (
    normalizedLabel === normalizedFamily ||
    detected.terms.some((term) => normalizedLabel === normalizeMinerText(term))
  ) {
    return "";
  }

  if (!label.includes(".")) return label;

  const parts = label.split(".").filter(Boolean);
  for (let index = 1; index < parts.length; index += 1) {
    const prefix = parts.slice(0, index).join(".");
    const suffix = normalizeMinerText(parts.slice(index).join(" "));
    if (prefix && detected.terms.some((term) => suffix === normalizeMinerText(term))) {
      return prefix === String(worker.payout_address || "") ? "" : prefix;
    }
  }
  return label;
}

function debugActiveMiners(network) {
  if (!debugMode || network !== "regtest") return [];
  const debugMiners = [
    ["nerdqaxe-plus-plus", "bench.nerdqaxe++", "nerdqaxe++", 5_090_000_000_000],
    ["nerdqaxe-plus", "bench.nerdqaxe+", "nerdqaxe+", 4_100_000_000_000],
    ["nerdoctaxe-gamma", "rack.nerdoctaxe.gamma", "nerdoctaxe gamma", 3_700_000_000_000],
    ["nerdoctaxe-plus", "rack.nerdoctaxe+", "nerdoctaxe+", 3_300_000_000_000],
    ["nerdeko", "shelf.nerdeko", "nerdeko", 2_400_000_000_000],
    ["nerdaxe", "desk.nerdaxe", "nerdaxe", 1_300_000_000_000],
    ["qaxe", "desk.qaxe", "qaxe", 4_600_000_000_000],
    ["nmminer", "lab.nmminer", "nmminer", 780_000_000_000],
    ["bitaxe", "garage.bitaxe.gamma", "bitaxe gamma", 1_180_000_000_000],
    ["piaxe", "pi.piaxe", "piaxe", 450_000_000_000],
    ["bitcrane", "rack.bitcrane", "bitcrane", 34_000_000_000_000],
    ["braiins", "garage.braiins", "braiins os+", 92_400_000_000_000],
    ["antminer", "garage.s19", "antminer s19", 92_400_000_000_000],
    ["osmu", "tiny.nerd", "nerd miner", 95_000_000_000],
    ["address-qaxe", "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl.qaxe", "qaxe", 610_000_000_000],
    ["generic", "unknown.worker", "unknown worker", 520_000_000_000],
  ];

  const visibleDebugMiners = debugMinerCount > 0
    ? debugMiners.slice(0, debugMinerCount)
    : debugMiners;

  return visibleDebugMiners.map(([id, label, term, hashrate], index) => ({
    worker_id: `debug-${id}`,
    label,
    user_identity: `bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl.${term}`,
    payout_address: "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl",
    connected: true,
    session_count: 1 + (index % 3),
    total_hashrate: hashrate,
    total_shares: 42 + index * 17,
    best_diff: 12_400 * (index + 1),
    pool_diff: 128 * (1 + (index % 5)),
    total_rejected: index % 4,
    rejection_percent: index % 4 ? 0.25 * (index % 4) : 0,
    uptime_seconds: (18 + index * 7) * 60,
    sessions: [],
  }));
}

function statusMinerWorkers(status) {
  const network = status?.config?.network || currentNetwork || "mainnet";
  return [
    ...(status?.miner_workers || legacyMinerWorkers(status?.connected_miners || [])),
    ...debugActiveMiners(network),
  ];
}

function isWorkerActiveOnTemplate(worker) {
  if (worker?.connected) return true;
  const lastSeenSeconds = minerLastSeenSeconds(worker);
  if (lastSeenSeconds !== null && lastSeenSeconds !== undefined) {
    return lastSeenSeconds <= activeMinerGraceSeconds;
  }
  return Boolean(worker?.active_on_template);
}

function mempoolAddressUrl(address) {
  if (!address) return null;
  const encodedAddress = encodeURIComponent(address);
  switch (currentNetwork) {
    case "mainnet":
      return `https://mempool.space/address/${encodedAddress}`;
    case "testnet4":
      return `https://mempool.space/testnet4/address/${encodedAddress}`;
    case "signet":
      return `https://mempool.space/signet/address/${encodedAddress}`;
    default:
      return `https://mempool.space/address/${encodedAddress}`;
  }
}

function renderPayoutAddress(address) {
  const value = String(address || "");
  const escapedValue = escapeHtml(value);
  const label = escapeHtml(value || "-");
  const url = mempoolAddressUrl(address);
  if (!url) return `<code class="miner-payout-address" title="${escapedValue}" data-full-address="${escapedValue}">${label}</code>`;
  return `<a class="miner-payout-link miner-payout-address" href="${url}" target="_blank" rel="noopener noreferrer" title="${escapedValue}" data-full-address="${escapedValue}">${label}</a>`;
}

function middleTruncate(value, keepStart, keepEnd) {
  if (!value || value.length <= keepStart + keepEnd + 3) return value || "-";
  return `${value.slice(0, keepStart)}...${value.slice(-keepEnd)}`;
}

function updatePayoutAddressLabels() {
  document.querySelectorAll("[data-full-address]").forEach((element) => {
    const fullAddress = element.dataset.fullAddress || "";
    if (!fullAddress) return;
    element.textContent = fullAddress;
    if (element.scrollWidth <= element.clientWidth) return;

    let keepStart = Math.min(16, Math.max(8, Math.floor(fullAddress.length * 0.35)));
    let keepEnd = Math.min(14, Math.max(8, Math.floor(fullAddress.length * 0.25)));
    element.textContent = middleTruncate(fullAddress, keepStart, keepEnd);

    while (element.scrollWidth > element.clientWidth && keepStart > 6 && keepEnd > 6) {
      keepStart -= 1;
      keepEnd -= 1;
      element.textContent = middleTruncate(fullAddress, keepStart, keepEnd);
    }
  });
}

function schedulePayoutAddressLabelUpdate() {
  if (payoutAddressLabelTimer) clearTimeout(payoutAddressLabelTimer);
  payoutAddressLabelTimer = setTimeout(() => {
    updatePayoutAddressLabels();
    payoutAddressLabelTimer = setTimeout(() => {
      payoutAddressLabelTimer = 0;
      updatePayoutAddressLabels();
    }, 80);
  }, 0);
}

function formatPercent(value, digits = 1) {
  if (value === null || value === undefined) return "-";
  return `${Number(value).toFixed(digits)}%`;
}

function formatSignedPercent(value, digits = 1) {
  if (value === null || value === undefined) return "-";
  const numeric = Number(value);
  if (!Number.isFinite(numeric)) return "-";
  const sign = numeric > 0 ? "+" : "";
  return `${sign}${numeric.toFixed(digits)}%`;
}

function formatSyncProgress(value) {
  if (value === null || value === undefined) return "-";
  return formatPercent(Number(value) * 100, 2);
}

function formatSats(value) {
  if (value === null || value === undefined) return "-";
  return `${formatInteger(value)} sats`;
}

function formatBtcFromSats(sats) {
  if (sats === null || sats === undefined) return "-";
  return `${(Number(sats) / 100000000).toFixed(8)} BTC`;
}

function formatCompactBtcFromSats(sats) {
  if (sats === null || sats === undefined) return "-";
  return `${(Number(sats) / 100000000).toFixed(8).replace(/\.?0+$/, "")} BTC`;
}

function formatFiatFromSats(sats) {
  if (sats === null || sats === undefined) return "-";
  const rate = exchangeRates[selectedCurrency];
  if (!Number.isFinite(rate)) return `${selectedCurrency} unavailable`;
  const fiat = (Number(sats) / 100000000) * rate;
  return `${formatInteger(Math.round(fiat))} ${selectedCurrency}`;
}

function updateBlockValue(template) {
  const rewardSats = template.reward_sats;
  const feeSats = template.fee_sats;
  const subsidySats =
    rewardSats !== null && rewardSats !== undefined && feeSats !== null && feeSats !== undefined
      ? Math.max(0, Number(rewardSats) - Number(feeSats))
      : null;
  $("#template-reward-btc").text(formatBtcFromSats(rewardSats));
  $("#template-reward-breakdown").text(
    subsidySats !== null
      ? `Subsidy ${formatCompactBtcFromSats(subsidySats)} · Fees ${formatCompactBtcFromSats(feeSats)}`
      : "-"
  );
  $("#template-reward-fiat").text(formatFiatFromSats(rewardSats));
}

function templateWeightText(template) {
  return template?.weight !== undefined && template?.weight !== null
    ? `${formatInteger(template.weight)} WU`
    : "unavailable";
}

function templateWeightPercentText(template) {
  return template?.weight_percent !== undefined && template?.weight_percent !== null
    ? `${formatPercent(template.weight_percent, 2)} of limit`
    : "-";
}

function templateRewardBreakdownText(template) {
  const rewardSats = template?.reward_sats;
  const feeSats = template?.fee_sats;
  const subsidySats =
    rewardSats !== null && rewardSats !== undefined && feeSats !== null && feeSats !== undefined
      ? Math.max(0, Number(rewardSats) - Number(feeSats))
      : null;
  return subsidySats !== null
    ? `Subsidy ${formatCompactBtcFromSats(subsidySats)} · Fees ${formatCompactBtcFromSats(feeSats)}`
    : "-";
}

function updateTemplateMetrics(template = {}) {
  $("#template-weight").text(templateWeightText(template));
  $("#template-weight-percent").text(templateWeightPercentText(template));
  updateTemplateWeightChart(template.weight, template.weight_percent);
  $("#template-txs").text(formatInteger(template.transaction_count));
  $("#template-reward-btc").text(formatBtcFromSats(template.reward_sats));
  $("#template-reward-breakdown").text(templateRewardBreakdownText(template));
  $("#template-reward-fiat").text(formatFiatFromSats(template.reward_sats));
}

function updateDifficultyCard(bitcoinCore) {
  const adjustment = bitcoinCore.difficulty_adjustment || {};
  const difficulty = bitcoinCore.network_difficulty;
  const fill = document.getElementById("difficulty-progress-fill");
  const projection = document.getElementById("difficulty-projection");
  $("#network-difficulty").text(formatDifficulty(difficulty));
  $("#difficulty-next").text(
    adjustment.blocks_remaining !== undefined && adjustment.next_height !== undefined
      ? `${formatInteger(adjustment.blocks_remaining)} blocks to ${formatInteger(adjustment.next_height)}`
      : "Adjustment unavailable"
  );
  $("#difficulty-eta").text(
    adjustment.estimated_seconds_remaining !== undefined && adjustment.estimated_seconds_remaining !== null
      ? `~${formatDuration(adjustment.estimated_seconds_remaining)} remaining`
      : "ETA unavailable"
  );

  const percent = Number(adjustment.progress_percent || 0);
  if (fill) fill.style.width = `${Math.max(0, Math.min(100, percent))}%`;

  const projectedChange = adjustment.projected_percent_change;
  const projectedDifficulty = adjustment.projected_difficulty;
  const hasProjection = projectedChange !== undefined && projectedChange !== null;
  if (projection) {
    projection.classList.toggle("is-up", hasProjection && Number(projectedChange) > 0);
    projection.classList.toggle("is-down", hasProjection && Number(projectedChange) < 0);
  }
  $("#difficulty-projection").text(
    hasProjection
      ? `${formatSignedPercent(projectedChange)} projected (${formatDifficulty(projectedDifficulty)})`
      : "Projection unavailable"
  );
}

function setHealth(state, text) {
  const colors = {
    ok: {
      background: "#edf7ef",
      borderColor: "#b8dfc0",
      color: "#176529",
    },
    warn: {
      background: "#fff4e5",
      borderColor: "#f0c36d",
      color: "#8a5400",
    },
    error: {
      background: "#fff0f0",
      borderColor: "#efb3b3",
      color: "#a33522",
    },
  };
  const color = colors[state] || colors.warn;
  const health = document.getElementById("health");
  if (!health) return;
  health.hidden = false;
  health.textContent = text;
  Object.assign(health.style, color);
}

function clearHealth() {
  const health = document.getElementById("health");
  if (!health) return;
  health.textContent = "";
  health.hidden = true;
}

function updateHealth(status, bitcoinCore) {
  const mining = status.mining || {};
  if (mining.setup_required) {
    setHealth("warn", "Mining setup required");
    return;
  }
  if (!bitcoinCore.rpc_available) {
    if (bitcoinCore.template?.height) {
      setHealth("warn", "Bitcoin Core RPC warning");
    } else {
      setHealth("error", "Bitcoin Core RPC unreachable");
    }
    return;
  }
  if (bitcoinCore.rpc_warning) {
    setHealth("warn", "Bitcoin Core RPC warning");
    return;
  }
  if (mining.error || mining.warning || !mining.sv2_listening) {
    setHealth("warn", "SV2 mining unavailable");
    return;
  }
  if (!status.monitoring) {
    setHealth("error", "SRI monitoring unreachable");
    return;
  }
  clearHealth();
}

function cardStatus(kind, text) {
  return { kind, text };
}

function miningUnavailableStatus(mining) {
  if (!mining) return null;
  if (mining.setup_required) {
    return cardStatus("warn", mining.warning || "Bitcoin Core IPC mining is not enabled.");
  }
  const message = mining.error || mining.warning;
  if (message) return cardStatus("warn", message);
  if (!mining.sv2_listening) return cardStatus("warn", "SV2 mining server is not listening.");
  return null;
}

function chainCardStatus(bitcoinCore, mining) {
  if (!bitcoinCore.rpc_available) {
    return bitcoinCore.template?.height
      ? cardStatus("warn", "Bitcoin Core RPC unavailable; showing cached chain data and live IPC template.")
      : cardStatus("error", "Bitcoin Core RPC unavailable.");
  }
  if (bitcoinCore.initial_block_download) {
    return cardStatus("warn", `Node syncing (${formatSyncProgress(bitcoinCore.sync_progress)} verified). Mining is paused.`);
  }
  const miningStatus = miningUnavailableStatus(mining);
  if (miningStatus) return miningStatus;
  if (!bitcoinCore.mining_ready) return cardStatus("warn", "Local chain tip is available, but mining is paused.");
  if (bitcoinCore.rpc_warning) return cardStatus("warn", "Bitcoin Core reported an RPC warning.");
  return cardStatus("ok", "Local chain tip ready.");
}

function templateCardStatus(bitcoinCore, template, mining) {
  const miningStatus = miningUnavailableStatus(mining);
  if (miningStatus) return miningStatus;
  if (!bitcoinCore.rpc_available) {
    return template?.height
      ? cardStatus("warn", `Candidate block ${formatInteger(template.height)} available from IPC; Bitcoin Core RPC status is unavailable.`)
      : cardStatus("error", "No candidate template: Bitcoin Core RPC is unavailable.");
  }
  if (bitcoinCore.initial_block_download) return cardStatus("warn", "Template paused while the node is in initial block download.");
  if (!template?.height) return cardStatus("warn", "No IPC candidate template available; template refresh is idle or still bootstrapping.");
  if (!bitcoinCore.mining_ready) return cardStatus("warn", "Template unavailable; mining is paused.");
  return cardStatus("ok", `Candidate block ${formatInteger(template.height)} ready.`);
}

function updateCardStatus(id, status) {
  const element = document.getElementById(id);
  if (!element) return;
  element.textContent = status.text;
  element.className = `card-status is-${status.kind}`;
  element.hidden = status.kind === "ok";
}

function updateTemplateAvailability(bitcoinCore, template) {
  const summary = document.getElementById("template-summary");
  const status = document.getElementById("template-status");
  const hasTemplate = Boolean(template?.height && bitcoinCore.mining_ready);
  if (summary) summary.hidden = !hasTemplate;
  if (status) status.classList.toggle("is-prominent", !hasTemplate);
}

function blockMetric(value, fallback = "-") {
  return value !== undefined && value !== null ? value : fallback;
}

function shortHash(hash) {
  if (!hash) return "-";
  return `${hash.slice(0, 8)}...${hash.slice(-6)}`;
}

function renderChainMetric(label, value, detail = "") {
  return `
    <div class="chain-row-metric">
      <span>${label}</span>
      <strong>${value}</strong>
      ${detail ? `<em>${detail}</em>` : ""}
    </div>
  `;
}

function renderTemplateMetric(label, value, detail, iconLabel) {
  return `
    <div class="template-metric">
      <span class="template-metric-icon" aria-hidden="true">${iconLabel}</span>
      <div class="template-metric-values">
        <span>${label}</span>
        <strong>${value}</strong>
        ${detail || ""}
      </div>
    </div>
  `;
}

function bitcoinIconSvg() {
  return `
    <svg class="template-metric-svg" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M11.767 19.089c4.924.868 6.14-6.025 1.216-6.894m-1.216 6.894L5.86 18.047m5.908 1.042-.347 1.97m1.563-8.864c4.924.869 6.14-6.025 1.215-6.893m-1.215 6.893-3.94-.694m5.155-6.2L8.29 4.26m5.908 1.042.348-1.97M7.48 20.364l3.126-17.727" fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" stroke-width="2"/>
    </svg>
  `;
}

function renderTemplateMinerRows(workers) {
  const activeWorkers = (Array.isArray(workers) ? workers : []).filter(isWorkerActiveOnTemplate);
  if (!activeWorkers.length) {
    return `
      <div class="template-miners-empty">
        <span>No active miners connected</span>
        <button class="panel-action template-add-miner" type="button" data-open-add-miner>+ Add Miner</button>
      </div>
    `;
  }

  return `
    <div class="template-miners-list">
      <div class="template-miner-row template-miner-header" aria-hidden="true">
        <span title="Detected miner model and meaningful worker label.">Miner</span>
        <span title="Payout address from the miner username.">Payout</span>
        <span title="Estimated current hashrate for this active worker.">Hashrate</span>
        <span title="How long this worker has been connected in the current session.">Uptime</span>
        <span title="Most recent monitoring or share sample for this worker.">Last Seen</span>
        <span title="Current share difficulty assigned by the pool.">Pool diff</span>
        <span title="Accepted shares submitted by this worker.">Accepted</span>
        <span title="Rejected shares submitted by this worker.">Rejected</span>
        <span title="Best share difficulty submitted by this worker.">Best diff</span>
      </div>
      ${activeWorkers.map((worker) => {
        const detected = detectMinerLogo(worker);
        const label = worker.label || worker.worker_id || "-";
        const displayLabel = displayMinerLabel(worker, detected);
        const rejected = Number(worker.total_rejected || 0);
        const sharesLabel = `${formatInteger(worker.total_shares)} ${Number(worker.total_shares) === 1 ? "share" : "shares"}`;
        const isRecentlySeen = !worker.connected && isWorkerActiveOnTemplate(worker);
        return `
        <div class="template-miner-row ${isRecentlySeen ? "is-recently-seen" : ""}" data-template-worker-id="${escapeHtml(String(worker.worker_id || worker.label || "-"))}">
          <span class="template-miner-identity" title="${escapeHtml(label)}">
            <span class="template-miner-mark" title="${escapeHtml(detected.family)}" aria-label="${escapeHtml(detected.family)}">
              <img class="miner-logo template-miner-logo" src="${escapeHtml(minerLogoSrc(detected.logo))}" alt="${escapeHtml(detected.family)} logo" title="${escapeHtml(detected.family)}" loading="lazy">
              ${displayLabel ? `<strong>${escapeHtml(displayLabel)}</strong>` : ""}
            </span>
          </span>
          <span data-label="Payout" class="template-miner-payout">${renderPayoutAddress(worker.payout_address)}</span>
          <span data-label="Hashrate">${formatHashrate(worker.total_hashrate)}</span>
          <span data-label="Uptime" data-template-worker-cell="uptime_seconds">${formatDuration(minerUptimeSeconds(worker))}</span>
          <span data-label="Last Seen" data-template-worker-cell="last_seen_seconds">${formatLastSeen(minerLastSeenSeconds(worker))}</span>
          <span data-label="Pool diff">${formatDifficulty(worker.pool_diff)}</span>
          <span data-label="Accepted">${sharesLabel}</span>
          <span data-label="Rejected">${formatInteger(rejected)}</span>
          <span data-label="Best diff">${formatDifficulty(worker.best_diff)}</span>
        </div>
      `; }).join("")}
    </div>
  `;
}

function renderTemplateStackItem(template, workers) {
  const activeWorkers = (Array.isArray(workers) ? workers : []).filter(isWorkerActiveOnTemplate);
  const isActivelyMining = activeWorkers.length > 0;
  const weightText = templateWeightText(template);
  const weightPercentText = templateWeightPercentText(template);
  const transactionText = formatInteger(template.transaction_count);
  const rewardText = formatBtcFromSats(template.reward_sats);
  const rewardBreakdownText = templateRewardBreakdownText(template);
  const rewardFiatText = formatFiatFromSats(template.reward_sats);
  return `
    <div class="chain-stack-item is-template ${isActivelyMining ? "is-active-mining" : "is-ready-template"}">
      <div class="chain-stack-marker" aria-hidden="true"></div>
      <div class="chain-stack-card template-stack-card" id="template-summary">
        <div class="chain-row-heading">
          <div>
            <span class="chain-row-kicker">Mining Template</span>
            <strong>${formatInteger(template.height)}</strong>
          </div>
          <div class="template-actions">
            <span class="chain-row-badge">${isActivelyMining ? "Mining" : "Ready to mine"}</span>
          </div>
        </div>
        <div class="chain-row-metrics template-metrics">
          <div class="template-metric">
            <svg class="donut-chart" viewBox="0 0 44 44" role="img" aria-labelledby="template-weight-chart-title template-weight-chart-desc">
              <title id="template-weight-chart-title">Template block fullness</title>
              <desc id="template-weight-chart-desc">No template weight data available</desc>
              <circle class="donut-track" cx="22" cy="22" r="17"></circle>
              <circle class="donut-fill" id="template-weight-chart-fill" cx="22" cy="22" r="17" pathLength="100"></circle>
            </svg>
            <div class="template-metric-values">
              <span>Weight</span>
              <strong id="template-weight">${weightText}</strong>
              <em id="template-weight-percent">${weightPercentText}</em>
            </div>
          </div>
          ${renderTemplateMetric("Transactions", `<span id="template-txs">${transactionText}</span>`, "", "TX")}
          ${renderTemplateMetric(
            "Value",
            `<span id="template-reward-btc">${rewardText}</span>`,
            `<em id="template-reward-breakdown">${rewardBreakdownText}</em><em id="template-reward-fiat">${rewardFiatText}</em>`,
            bitcoinIconSvg()
          )}
        </div>
        <div class="template-miners">
          ${renderTemplateMinerRows(activeWorkers)}
        </div>
      </div>
    </div>
  `;
}

function updateAddMinerPlacement(workers) {
  const topButton = document.getElementById("top-add-miner");
  if (!topButton) return;
  const activeWorkers = (Array.isArray(workers) ? workers : []).filter(isWorkerActiveOnTemplate);
  topButton.hidden = activeWorkers.length === 0;
}

function renderBlockStackItem(block, chainHeight) {
  const isTip = Number(block.height) === Number(chainHeight);
  const pool = block.pool || {};
  const title = `${isTip ? "Tip" : "Block"} ${formatInteger(block.height)} · ${pool.name || "Unknown"}`;
  const weight = blockMetric(block.weight !== undefined && block.weight !== null ? `${formatInteger(block.weight)} WU` : null);
  const txs = blockMetric(block.transaction_count !== undefined && block.transaction_count !== null ? `${formatInteger(block.transaction_count)} tx` : null);
  const value = formatBtcFromSats(block.reward_sats);
  return `
    <div class="chain-stack-item ${isTip ? "is-tip" : "is-block"}">
      <div class="chain-stack-marker" aria-hidden="true"></div>
      <div class="chain-stack-card chain-compact-card" title="${escapeHtml(title)}">
        <div class="chain-compact-main">
          <strong>${formatInteger(block.height)}</strong>
          <span class="chain-row-pool">
            <img src="/pool-logos/${escapeHtml(pool.logo || "unknown.svg")}" alt="" loading="eager" decoding="async">
            <span>${escapeHtml(pool.name || "Unknown")}</span>
          </span>
        </div>
        <div class="chain-compact-metrics">
          <span>${weight}</span>
          <span>${txs}</span>
          <span>${value}</span>
          <span>${formatTime(block.timestamp)}</span>
        </div>
        <div class="chain-compact-hash">
          ${escapeHtml(shortHash(block.hash))}
        </div>
      </div>
    </div>
  `;
}

function updateChainTimeline(bitcoinCore, template, recentBlocks = [], workers = []) {
  const chainHeight = Number(bitcoinCore.chain_height);
  const templateHeight = Number(template.height);
  const hasChainHeight = Number.isFinite(chainHeight);
  const hasTemplateHeight = Number.isFinite(templateHeight);
  const timeline = document.getElementById("chain-timeline");
  if (!timeline) return;

  if (!hasChainHeight && !hasTemplateHeight) {
    timeline.innerHTML = '<div class="empty-state">No chain data available</div>';
    return;
  }

  const items = [];
  if (hasTemplateHeight && (!hasChainHeight || templateHeight > chainHeight)) {
    items.push({ type: "template", height: templateHeight });
  }

  const knownBlocks = Array.isArray(recentBlocks) ? recentBlocks : [];
  if (knownBlocks.length) {
    knownBlocks.slice(-5).reverse().forEach((block) => {
      const height = Number(block.height);
      if (!Number.isFinite(height)) return;
      items.push({
        height,
        type: height === chainHeight ? "tip" : "confirmed",
        hash: block.hash,
        timestamp: block.timestamp,
        weight: block.weight,
        transaction_count: block.transaction_count,
        reward_sats: block.reward_sats,
        pool: block.pool,
      });
    });
  } else if (hasChainHeight) {
    const startHeight = Math.max(0, chainHeight - 4);
    for (let height = chainHeight; height >= startHeight; height -= 1) {
      items.push({ height, type: height === chainHeight ? "tip" : "confirmed" });
    }
  }

  const timelineSignature = JSON.stringify(items.map((item) => ({
    height: item.height,
    type: item.type,
    hash: item.hash || "",
    weight: item.weight || "",
    transaction_count: item.transaction_count || "",
    reward_sats: item.reward_sats || "",
    pool: item.pool ? `${item.pool.name || ""}:${item.pool.logo || ""}` : "",
  })).concat([
    selectedCurrency,
    exchangeRates[selectedCurrency] || "",
    ...workers.map((worker) => `${worker.worker_id || worker.label || ""}:${Boolean(worker.connected)}:${Boolean(isWorkerActiveOnTemplate(worker))}:${worker.connection_state || ""}:${worker.total_hashrate || 0}:${worker.uptime_seconds || 0}:${worker.last_seen_seconds || 0}:${worker.pool_diff || 0}:${worker.total_shares || 0}:${worker.total_rejected || 0}:${worker.best_diff || 0}`),
  ]));
  if (timeline.dataset.chainSignature === timelineSignature && timeline.querySelector(".chain-stack-item")) {
    return;
  }
  timeline.dataset.chainSignature = timelineSignature;
  timeline.innerHTML = items.map((item) => (
    item.type === "template"
      ? renderTemplateStackItem(template, workers)
      : renderBlockStackItem(item, chainHeight)
  )).join("");
  updateTemplateMetrics(template);

  if (!chainTimelineResizeObserver) {
    chainTimelineResizeObserver = new ResizeObserver(() => {
      const latestBitcoinCore = lastStatus?.bitcoin_core;
      if (latestBitcoinCore) updateChainTimeline(
        latestBitcoinCore,
        latestBitcoinCore.template || {},
        lastStatus?.recent_blocks || [],
        statusMinerWorkers(lastStatus),
      );
    });
  }
  if (chainTimelineObservedElement !== timeline) {
    if (chainTimelineObservedElement) chainTimelineResizeObserver.unobserve(chainTimelineObservedElement);
    chainTimelineResizeObserver.observe(timeline);
    chainTimelineObservedElement = timeline;
  }
}

function updateTitle(network) {
  const suffix = network && network !== "mainnet" ? ` on ${network}` : "";
  const title = `Canary Mining${suffix}`;
  $("#app-title").text(title);
  document.title = title;
}

function parseListenAddress(address) {
  const value = String(address || "");
  const bracketMatch = value.match(/^\[([^\]]+)\]:(\d+)$/);
  if (bracketMatch) {
    return { host: bracketMatch[1], port: bracketMatch[2] };
  }

  const splitAt = value.lastIndexOf(":");
  if (splitAt === -1) return { host: value, port: "" };
  return {
    host: value.slice(0, splitAt),
    port: value.slice(splitAt + 1),
  };
}

function isWildcardHost(host) {
  return ["", "0.0.0.0", "::", "[::]"].includes(String(host || "").trim());
}

function minerSetupHost(listenAddress) {
  const parsed = parseListenAddress(listenAddress);
  if (!isWildcardHost(parsed.host)) return parsed.host;
  return window.location.hostname || parsed.host || "localhost";
}

function minerSetupUsername(setup) {
  const payoutAddress = setup?.payout_address || "<payout-address>";
  return `${payoutAddress}.qaxe`;
}

function selectedMinerLabel() {
  const input = document.getElementById("setup-miner-label");
  const normalized = String(input?.value || "")
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._+-]+/g, "-")
    .replace(/^[._-]+|[._-]+$/g, "")
    .slice(0, 48);
  return normalized;
}

function setGeneratedUsername(value, enabled) {
  const copyable = Boolean(enabled && value && value !== "-");
  setSetupValue("setup-generated-username", value || "-");
  const target = document.getElementById("setup-generated-username");
  if (target) target.dataset.copyEnabled = copyable ? "true" : "false";
  const copyButton = document.querySelector('[data-copy-target="setup-generated-username"]');
  if (copyButton) copyButton.disabled = !copyable;
}

function setAddressValidation(state, message) {
  const element = document.getElementById("setup-address-validation");
  if (!element) return;
  element.classList.remove("is-ok", "is-error", "is-pending");
  if (state) element.classList.add(`is-${state}`);
  element.textContent = message;
}

function updateGeneratedUsername(address) {
  if (!address) {
    setGeneratedUsername("-", false);
    return;
  }
  const label = selectedMinerLabel();
  setGeneratedUsername(label ? `${address}.${label}` : address, true);
}

async function validateSetupPayoutAddress() {
  const input = document.getElementById("setup-payout-address");
  const address = input?.value.trim() || "";
  const requestId = ++payoutAddressValidationRequest;
  if (!address) {
    setAddressValidation(null, "");
    updateGeneratedUsername(null);
    return;
  }
  updateGeneratedUsername(null);
  try {
    const response = await fetch(`/api/validate-payout-address?address=${encodeURIComponent(address)}`, {
      headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new Error(`validation returned ${response.status}`);
    const result = await response.json();
    if (requestId !== payoutAddressValidationRequest) return;
    if (result.valid && result.address) {
      setAddressValidation(null, "");
      updateGeneratedUsername(result.address);
    } else {
      setAddressValidation("error", result.error || `Invalid ${currentNetwork} payout address.`);
      updateGeneratedUsername(null);
    }
  } catch (error) {
    if (requestId !== payoutAddressValidationRequest) return;
    setAddressValidation("error", "Address validation failed.");
    updateGeneratedUsername(null);
    console.error(error);
  }
}

function setSetupValue(id, value, copyValue = value) {
  const element = document.getElementById(id);
  if (!element) return;
  element.textContent = value;
  element.dataset.copyValue = copyValue;
}

function updateMinerSetup(status) {
  const setup = status.miner_setup || {};
  const listenAddress = status.config?.sv2_listen_address;
  const parsedAddress = parseListenAddress(listenAddress);
  setSetupValue("setup-host", minerSetupHost(listenAddress));
  setSetupValue("setup-port", parsedAddress.port || "-");
  const username = minerSetupUsername(setup);
  const dialogOpen = Boolean(document.getElementById("add-miner-dialog")?.open);
  if (!dialogOpen) {
    setSetupValue("setup-generated-username", username, username);
    setGeneratedUsername("-", false);
    setAddressValidation(null, "");
  }
  setSetupValue("setup-authority-key", setup.sv2_authority_public_key || "-");
}

async function copySetupValue(button) {
  const targetId = button.getAttribute("data-copy-target");
  const target = targetId ? document.getElementById(targetId) : null;
  if (!target || !navigator.clipboard?.writeText) return;

  const originalTitle = button.getAttribute("title") || "Copy";
  const originalLabel = button.getAttribute("aria-label") || "Copy";
  const icon = button.querySelector("img");
  const originalIcon = icon?.getAttribute("src") || "/icons/copy.svg";
  const value = target.dataset.copyValue ?? target.textContent ?? "";
  await navigator.clipboard.writeText(value);
  button.classList.add("is-copied");
  button.setAttribute("title", "Copied");
  button.setAttribute("aria-label", "Copied");
  if (icon) icon.setAttribute("src", "/icons/check.svg");
  button.disabled = true;
  setTimeout(() => {
    button.classList.remove("is-copied");
    button.setAttribute("title", originalTitle);
    button.setAttribute("aria-label", originalLabel);
    if (icon) icon.setAttribute("src", originalIcon);
    button.disabled = target.dataset.copyEnabled !== "true";
  }, 1200);
}

async function refreshStatus() {
  const status = await $.getJSON("/api/status");
  if (Number.isFinite(Number(status.server_time))) {
    serverClockOffsetSeconds = Number(status.server_time) - Date.now() / 1000;
  }
  lastStatus = status;
  currentNetwork = status.config.network || "mainnet";
  const showMonitoring = updateMonitoringVisibility(currentNetwork);
  updateTitle(currentNetwork);
  updateMinerSetup(status);
  const bitcoinCore = status.bitcoin_core || {};
  const mining = status.mining || {};
  const template = bitcoinCore.template || {};
  const minerWorkers = statusMinerWorkers(status);
  updateAddMinerPlacement(minerWorkers);
  $("#network-hashrate").text(formatHashrate(bitcoinCore.network_hashrate));
  $("#network-name").text(currentNetwork);
  updateDifficultyCard(bitcoinCore);
  updateCardStatus("chain-status", chainCardStatus(bitcoinCore, mining));
  updateCardStatus("template-status", templateCardStatus(bitcoinCore, template, mining));
  updateTemplateAvailability(bitcoinCore, template);
  updateChainTimeline(bitcoinCore, template, status.recent_blocks || [], minerWorkers);
  updateTemplateMetrics(template);
  if (showMonitoring) {
    $("#metrics-listener").text(status.config.metrics_listen_address || "disabled");
    $("#monitoring").text(JSON.stringify(status.monitoring || {}, null, 2));
  }
  updateConnectedMiners(minerWorkers.filter((worker) => !isWorkerActiveOnTemplate(worker)));
  updateHealth(status, bitcoinCore);
  schedulePayoutAddressLabelUpdate();
}

function legacyMinerWorkers(miners) {
  return miners.map((miner) => ({
    worker_id: miner.id,
    label: miner.label,
    payout_address: miner.payout_address,
    user_identity: miner.user_identity,
    session_count: 1,
    pool_diff: miner.pool_diff,
    best_diff: miner.best_diff,
    total_shares: miner.shares_accepted,
    total_rejected: 0,
    rejection_percent: 0,
    opened_at: null,
    closed_at: null,
    last_seen_at: null,
    uptime_seconds: null,
    last_seen_seconds: null,
    connected: miner.connected,
    active_on_template: miner.connected,
    connection_state: miner.connected ? "online" : "offline",
    sessions: [{
      session_id: `${miner.channel_kind}:${miner.channel_id}`,
      client_id: miner.client_id || 0,
      channel_id: miner.channel_id,
      channel_kind: miner.channel_kind,
      pool_diff: miner.pool_diff,
      best_diff: miner.best_diff,
      shares_accepted: miner.shares_accepted,
      shares_rejected: 0,
      rejection_percent: 0,
      opened_at: null,
      closed_at: null,
      last_seen_at: null,
      uptime_seconds: null,
      last_seen_seconds: null,
      connected: miner.connected,
    }],
  }));
}

function updateConnectedMiners(workers) {
  const minersElement = document.getElementById("miners");
  if (!minersElement) return;
  const minersPanel = document.getElementById("miner-history-panel");

  if (!workers.length) {
    if (minersPanel) minersPanel.hidden = true;
    if (minersElement.dataset.minersSignature !== "empty") {
      minersElement.innerHTML = "";
      minersElement.dataset.minersSignature = "empty";
    }
    return;
  }

  if (minersPanel) minersPanel.hidden = false;
  const signature = minerWorkersStructureSignature(workers);
  if (minersElement.dataset.minersSignature === signature && minersElement.querySelector(".miners-table")) {
    updateMinerWorkerCells(minersElement, workers);
    return;
  }

  minersElement.dataset.minersSignature = signature;
  minersElement.innerHTML =
    `<div class="miners-table-wrap">
      <table class="miners-table">
        <thead>
          <tr>
            <th>Worker</th>
            <th>Session ID</th>
            <th>Pool Diff</th>
            <th>Best Difficulty</th>
            <th>Total Shares</th>
            <th>Total Rejected</th>
            <th>Rej %</th>
            <th>Uptime</th>
            <th>Last Seen</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          ${workers.map(renderMinerWorkerRows).join("")}
        </tbody>
      </table>
    </div>`;
}

function renderMinerWorkerRows(worker) {
  const detected = detectMinerLogo(worker);
  const label = worker.label || "-";
  const displayLabel = displayMinerLabel(worker, detected);
  const workerId = String(worker.worker_id || label);
  const expanded = expandedMinerWorkers.has(workerId);
  const sessions = worker.sessions || [];
  const sessionLabel = `${formatInteger(worker.session_count)} ${worker.session_count === 1 ? "Session" : "Sessions"}`;
  const workerRow = `
    <tr class="miner-worker-row ${worker.connected ? "is-online" : "is-offline"}" data-worker-id="${escapeHtml(workerId)}">
      <td data-label="Worker">
        <div class="miner-table-identity" title="${escapeHtml(label)}">
          <img class="miner-logo" src="${escapeHtml(minerLogoSrc(detected.logo))}" alt="${escapeHtml(detected.family)} logo" loading="lazy">
          <div>
            ${displayLabel ? `<strong>${escapeHtml(displayLabel)}</strong>` : ""}
            <span>${escapeHtml(detected.family)}</span>
          </div>
        </div>
      </td>
      <td data-label="Sessions">
        <button class="session-toggle" type="button" data-worker-toggle="${escapeHtml(workerId)}" aria-expanded="${expanded ? "true" : "false"}">
          <span aria-hidden="true">${expanded ? "-" : "+"}</span>
          ${sessionLabel}
        </button>
      </td>
      <td data-label="Pool Diff" data-worker-cell="pool_diff">${formatDifficulty(worker.pool_diff)}</td>
      <td data-label="Best Difficulty" data-worker-cell="best_diff">${formatDifficulty(worker.best_diff)}</td>
      <td data-label="Total Shares" data-worker-cell="total_shares">${formatInteger(worker.total_shares)}</td>
      <td data-label="Total Rejected" data-worker-cell="total_rejected">${formatInteger(worker.total_rejected)}</td>
      <td data-label="Rej %" data-worker-cell="rejection_percent">${formatPercent(worker.rejection_percent, 2)}</td>
      <td data-label="Uptime" data-worker-cell="uptime_seconds">${formatDuration(minerUptimeSeconds(worker))}</td>
      <td data-label="Last Seen" data-worker-cell="last_seen_seconds">${formatLastSeen(minerLastSeenSeconds(worker))}</td>
      <td data-label="Actions">
        <button class="miner-delete-button" type="button" data-delete-miner="${escapeHtml(worker.worker_id || "")}" data-delete-miner-label="${escapeHtml(label)}">Delete</button>
      </td>
    </tr>
  `;
  return workerRow + sessions.map((session) => renderMinerSessionRow(session, expanded)).join("");
}

function renderMinerSessionRow(session, expanded) {
  return `
    <tr class="miner-session-row ${session.connected ? "is-online" : "is-offline"} ${expanded ? "" : "is-collapsed"}" data-session-id="${escapeHtml(session.session_id || "-")}">
      <td data-label="Channel"><span class="session-channel">${escapeHtml(session.channel_kind || "channel")}</span></td>
      <td data-label="Session ID"><code>${escapeHtml(session.session_id || "-")}</code></td>
      <td data-label="Pool Diff" data-session-cell="pool_diff">${formatDifficulty(session.pool_diff)}</td>
      <td data-label="Best Difficulty" data-session-cell="best_diff">${formatDifficulty(session.best_diff)}</td>
      <td data-label="Shares Accepted" data-session-cell="shares_accepted">${formatInteger(session.shares_accepted)}</td>
      <td data-label="Shares Rejected" data-session-cell="shares_rejected">${formatInteger(session.shares_rejected)}</td>
      <td data-label="Rej %" data-session-cell="rejection_percent">${formatPercent(session.rejection_percent, 2)}</td>
      <td data-label="Uptime" data-session-cell="uptime_seconds">${formatDuration(sessionUptimeSeconds(session))}</td>
      <td data-label="Last Seen" data-session-cell="last_seen_seconds">${formatLastSeen(sessionLastSeenSeconds(session))}</td>
      <td data-label="Actions"></td>
    </tr>
  `;
}

function minerWorkersStructureSignature(workers) {
  return JSON.stringify(workers.map((worker) => {
    const detected = detectMinerLogo(worker);
    const workerId = String(worker.worker_id || worker.label || "-");
    return {
      worker_id: workerId,
      label: worker.label || "-",
      logo: detected.logo,
      family: detected.family,
      session_count: worker.session_count,
      connected: Boolean(worker.connected),
      user_identity: worker.user_identity || "",
      expanded: expandedMinerWorkers.has(workerId),
      sessions: (worker.sessions || []).map((session) => ({
        session_id: session.session_id || "-",
        channel_kind: session.channel_kind || "channel",
        connected: Boolean(session.connected),
      })),
    };
  }));
}

function updateMinerWorkerCells(container, workers) {
  workers.forEach((worker) => {
    const workerId = String(worker.worker_id || worker.label || "-");
    const row = Array.from(container.querySelectorAll(".miner-worker-row"))
      .find((candidate) => candidate.dataset.workerId === workerId);
    if (!row) return;
    setCellText(row, "worker", "pool_diff", formatDifficulty(worker.pool_diff));
    setCellText(row, "worker", "best_diff", formatDifficulty(worker.best_diff));
    setCellText(row, "worker", "total_shares", formatInteger(worker.total_shares));
    setCellText(row, "worker", "total_rejected", formatInteger(worker.total_rejected));
    setCellText(row, "worker", "rejection_percent", formatPercent(worker.rejection_percent, 2));
    setCellText(row, "worker", "uptime_seconds", formatDuration(minerUptimeSeconds(worker)));
    setCellText(row, "worker", "last_seen_seconds", formatLastSeen(minerLastSeenSeconds(worker)));
  });

  const sessions = workers.flatMap((worker) => worker.sessions || []);
  sessions.forEach((session) => {
    const sessionId = session.session_id || "-";
    const row = Array.from(container.querySelectorAll(".miner-session-row"))
      .find((candidate) => candidate.dataset.sessionId === sessionId);
    if (!row) return;
    setCellText(row, "session", "pool_diff", formatDifficulty(session.pool_diff));
    setCellText(row, "session", "best_diff", formatDifficulty(session.best_diff));
    setCellText(row, "session", "shares_accepted", formatInteger(session.shares_accepted));
    setCellText(row, "session", "shares_rejected", formatInteger(session.shares_rejected));
    setCellText(row, "session", "rejection_percent", formatPercent(session.rejection_percent, 2));
    setCellText(row, "session", "uptime_seconds", formatDuration(sessionUptimeSeconds(session)));
    setCellText(row, "session", "last_seen_seconds", formatLastSeen(sessionLastSeenSeconds(session)));
  });
}

function setCellText(row, scope, key, value) {
  const cell = row.querySelector(`[data-${scope}-cell="${key}"]`);
  if (cell && cell.textContent !== value) cell.textContent = value;
}

function updateTemplateMinerCells(workers) {
  const template = document.getElementById("template-summary");
  if (!template) return;
  workers.forEach((worker) => {
    const workerId = String(worker.worker_id || worker.label || "-");
    const row = Array.from(template.querySelectorAll(".template-miner-row"))
      .find((candidate) => candidate.dataset.templateWorkerId === workerId);
    if (!row) return;
    const uptime = row.querySelector('[data-template-worker-cell="uptime_seconds"]');
    const lastSeen = row.querySelector('[data-template-worker-cell="last_seen_seconds"]');
    const uptimeText = formatDuration(minerUptimeSeconds(worker));
    const lastSeenText = formatLastSeen(minerLastSeenSeconds(worker));
    if (uptime && uptime.textContent !== uptimeText) uptime.textContent = uptimeText;
    if (lastSeen && lastSeen.textContent !== lastSeenText) lastSeen.textContent = lastSeenText;
  });
}

function updateLiveCounters() {
  if (!lastStatus) return;
  const workers = statusMinerWorkers(lastStatus);
  updateConnectedMiners(workers.filter((worker) => !isWorkerActiveOnTemplate(worker)));
  updateTemplateMinerCells(workers);
  if (lastStatus.bitcoin_core) {
    updateChainTimeline(
      lastStatus.bitcoin_core,
      lastStatus.bitcoin_core.template || {},
      lastStatus.recent_blocks || [],
      workers,
    );
  }
}

function formatLastSeen(seconds) {
  if (seconds === null || seconds === undefined) return "-";
  if (Number(seconds) <= 0) return "now";
  return `${formatDuration(seconds)} ago`;
}

function updateConnectedMinersCards(miners) {
  $("#miners").html(
    miners.map((miner) => {
      const detected = detectMinerLogo(miner);
      const label = miner.label || "-";
      const payoutAddress = miner.payout_address || "";
      return `
      <article class="miner-card">
        <div class="miner-card-header">
          <img class="miner-logo miner-logo-large" src="${escapeHtml(minerLogoSrc(detected.logo))}" alt="${escapeHtml(detected.family)} logo" loading="lazy">
          <div class="miner-identity">
            <span class="miner-family">${escapeHtml(detected.family)}</span>
            <strong class="miner-name">${escapeHtml(label)}</strong>
            <span class="miner-status ${miner.connected ? "is-online" : "is-offline"}">${miner.connected ? "Online" : "Offline"}</span>
          </div>
        </div>
        <div class="miner-card-metrics">
          <div>
            <span>Best diff</span>
            <strong>${formatDifficulty(miner.best_diff)}</strong>
          </div>
          <div>
            <span>Shares</span>
            <strong>${formatInteger(miner.shares_accepted)}</strong>
          </div>
          <div>
            <span>Blocks</span>
            <strong>${formatInteger(miner.blocks_found)}</strong>
          </div>
        </div>
        <div class="miner-payout">
          <span>Payout</span>
          ${renderPayoutAddress(payoutAddress)}
        </div>
      </article>
    `;
    }).join("") || `<p class="empty-state">No miners connected.</p>`
  );
}

function updateCurrencySelect(currencies) {
  const select = document.getElementById("currency-select");
  if (!select) return;
  select.innerHTML = currencies
    .map((currency) => `<option value="${escapeHtml(currency)}">${escapeHtml(currency)}</option>`)
    .join("");
  if (!currencies.includes(selectedCurrency)) selectedCurrency = "USD";
  select.value = selectedCurrency;
}

async function refreshExchangeRates() {
  const response = await $.getJSON("/api/exchange-rates");
  exchangeRates = response.rates || {};
  updateCurrencySelect(response.supported_currencies || ["USD"]);
  if (lastStatus?.bitcoin_core?.template) updateTemplateMetrics(lastStatus.bitcoin_core.template);
  if (lastStatus?.bitcoin_core) {
    updateChainTimeline(
      lastStatus.bitcoin_core,
      lastStatus.bitcoin_core.template || {},
      lastStatus.recent_blocks || [],
      statusMinerWorkers(lastStatus),
    );
  }
}

async function deleteMinerHistory(button) {
  const minerId = button.getAttribute("data-delete-miner") || "";
  const label = button.getAttribute("data-delete-miner-label") || "this miner";
  if (!minerId) return;
  if (!window.confirm(`Delete ${label} from miners?`)) return;

  button.disabled = true;
  const originalText = button.textContent;
  button.textContent = "Deleting...";
  try {
    const response = await fetch(`/api/miners?miner_id=${encodeURIComponent(minerId)}`, {
      method: "DELETE",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) throw new Error(`delete miner returned ${response.status}`);
    expandedMinerWorkers.delete(minerId);
    await refreshStatus();
  } finally {
    button.disabled = false;
    button.textContent = originalText;
  }
}

async function refreshAll() {
  try {
    await refreshStatus();
  } catch (error) {
    setHealth("error", "refresh failed");
    console.error(error);
  }
}

$(function () {
  const select = document.getElementById("currency-select");
  const addMinerDialog = document.getElementById("add-miner-dialog");
  if (select) {
    select.value = selectedCurrency;
    select.addEventListener("change", () => {
      selectedCurrency = select.value || "USD";
      localStorage.setItem(currencyStorageKey, selectedCurrency);
      if (lastStatus?.bitcoin_core?.template) updateTemplateMetrics(lastStatus.bitcoin_core.template);
      if (lastStatus?.bitcoin_core) {
        updateChainTimeline(
          lastStatus.bitcoin_core,
          lastStatus.bitcoin_core.template || {},
          lastStatus.recent_blocks || [],
          statusMinerWorkers(lastStatus),
        );
      }
    });
  }
  if (addMinerDialog) {
    addMinerDialog.addEventListener("click", (event) => {
      if (event.target === addMinerDialog) addMinerDialog.close();
    });
  }
  const payoutInput = document.getElementById("setup-payout-address");
  const minerLabelInput = document.getElementById("setup-miner-label");
  if (payoutInput) {
    payoutInput.addEventListener("input", () => {
      validateSetupPayoutAddress().catch((error) => console.error(error));
    });
  }
  if (minerLabelInput) {
    minerLabelInput.addEventListener("input", () => {
      validateSetupPayoutAddress().catch((error) => console.error(error));
    });
  }
  refreshExchangeRates().catch((error) => console.error(error));
  document.addEventListener("click", (event) => {
    const addMinerTrigger = event.target.closest("[data-open-add-miner]");
    if (addMinerTrigger && addMinerDialog) {
      if (lastStatus) updateMinerSetup(lastStatus);
      selectedMinerLabel();
      validateSetupPayoutAddress().catch((error) => console.error(error));
      addMinerDialog.showModal();
      return;
    }

    const deleteButton = event.target.closest("[data-delete-miner]");
    if (deleteButton) {
      deleteMinerHistory(deleteButton).catch((error) => console.error(error));
      return;
    }

    const button = event.target.closest("[data-copy-target]");
    if (!button) return;
    copySetupValue(button).catch((error) => console.error(error));
  });
  document.addEventListener("click", (event) => {
    const button = event.target.closest("[data-worker-toggle]");
    if (!button) return;
    const workerId = button.getAttribute("data-worker-toggle");
    if (!workerId) return;
    if (expandedMinerWorkers.has(workerId)) {
      expandedMinerWorkers.delete(workerId);
    } else {
      expandedMinerWorkers.add(workerId);
    }
    updateConnectedMiners(statusMinerWorkers(lastStatus).filter((worker) => !isWorkerActiveOnTemplate(worker)));
    schedulePayoutAddressLabelUpdate();
  });
  refreshAll();
  window.addEventListener("resize", schedulePayoutAddressLabelUpdate);
  window.addEventListener("load", schedulePayoutAddressLabelUpdate);
  if (document.fonts?.ready) {
    document.fonts.ready.then(schedulePayoutAddressLabelUpdate).catch(() => {});
  }
  setInterval(refreshAll, statusRefreshIntervalMs);
  setInterval(updateLiveCounters, counterRefreshIntervalMs);
  setInterval(() => refreshExchangeRates().catch((error) => console.error(error)), 600000);
});
