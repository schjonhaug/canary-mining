import { chromium, expect } from "@playwright/test"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, "../..")

const dashboardUrl = process.env.CANARY_MINING_DASHBOARD_URL || "http://127.0.0.1:8080"
const screenshotMode = process.env.CANARY_MINING_SCREENSHOT_MODE || "screenshot"
const screenshotMinerCount = Number(process.env.CANARY_MINING_SCREENSHOT_MINERS || "2")
const outputPath =
  process.env.CANARY_MINING_README_SCREENSHOT ||
  path.join(repoRoot, "screenshots/dashboard.png")
const width = Number(process.env.CANARY_MINING_SCREENSHOT_WIDTH || "1440")
const height = Number(process.env.CANARY_MINING_SCREENSHOT_HEIGHT || "900")
const fullPage = process.env.CANARY_MINING_SCREENSHOT_FULL_PAGE !== "0"
const waitMs = Number(process.env.CANARY_MINING_SCREENSHOT_WAIT_MS || "1500")
const blockSubsidySats = 312_500_000
const fallbackPriceUsd = 62_600
const fallbackDifficulty = 138_955_357_012_247.3
const fallbackHeight = 953_204
const fallbackBlockTime = 1781161468
const fallbackHash =
  "00000000000000000000316ac66c683fa0d034616a2d0d5b81c8a66b18f33d8c"
const payoutAddress = "bc1qjjj03ayzv7u3q26k8g4zy8x6q53wc74xk5r3jg"

function numeric(value, fallback) {
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : fallback
}

async function fetchJson(url, fallback) {
  try {
    const response = await fetch(url, {
      headers: { accept: "application/json" },
    })
    if (!response.ok) throw new Error(`${url} returned ${response.status}`)
    return await response.json()
  } catch (error) {
    console.warn(`Using fallback screenshot data: ${error.message}`)
    return fallback
  }
}

function fallbackBlocks() {
  return Array.from({ length: 6 }, (_, index) => ({
    id: `${fallbackHash.slice(0, -String(index).length)}${index}`,
    height: fallbackHeight - index,
    timestamp: fallbackBlockTime - index * 620,
    weight: 3_993_000 - index * 321,
    tx_count: 3_900 - index * 97,
    difficulty: fallbackDifficulty,
  }))
}

function poolSlug(name) {
  return String(name || "")
    .replace(/[^a-zA-Z0-9]/g, "")
    .toLowerCase()
}

function poolAttribution(pool) {
  if (!pool?.name) {
    return {
      name: "Unknown",
      slug: "unknown",
      logo: "unknown.svg",
      link: null,
    }
  }

  const slug = poolSlug(pool.name)
  return {
    name: pool.name,
    slug,
    logo: `${slug}.svg`,
    link: null,
  }
}

async function enrichBlocks(blocks) {
  return Promise.all(blocks.slice(0, 6).map(async (block) => {
    const detail = await fetchJson(`https://mempool.space/api/v1/block/${block.id}`, null)
    const detailedBlock = detail && typeof detail === "object" ? detail : block
    return {
      ...block,
      ...detailedBlock,
      tx_count: detailedBlock.tx_count ?? block.tx_count,
      weight: detailedBlock.weight ?? block.weight,
      timestamp: detailedBlock.timestamp ?? block.timestamp,
      pool: poolAttribution(detailedBlock.extras?.pool),
      reward_sats: detailedBlock.extras?.reward ?? blockSubsidySats,
    }
  }))
}

function networkHashrateFromDifficulty(difficulty) {
  return difficulty * 2 ** 32 / 600
}

function demoMinerWorkers(now, count) {
  const miners = [
    {
      worker_id: "demo-nerdqaxe-plus-plus",
      label: "bench.nerdqaxe++",
      user_identity: `${payoutAddress}.nerdqaxe++`,
      total_hashrate: 5_090_000_000_000,
      pool_diff: 128,
      best_diff: 12_400,
      total_shares: 42,
      total_rejected: 0,
      uptime_seconds: 18 * 60,
    },
    {
      worker_id: "demo-antminer-s19",
      label: "garage.s19",
      user_identity: `${payoutAddress}.antminer s19`,
      total_hashrate: 92_400_000_000_000,
      pool_diff: 512,
      best_diff: 48_900,
      total_shares: 173,
      total_rejected: 1,
      uptime_seconds: 2 * 3600 + 14 * 60,
    },
    {
      worker_id: "demo-bitaxe-gamma",
      label: "garage.bitaxe.gamma",
      user_identity: `${payoutAddress}.bitaxe gamma`,
      total_hashrate: 1_180_000_000_000,
      pool_diff: 64,
      best_diff: 9_800,
      total_shares: 27,
      total_rejected: 0,
      uptime_seconds: 43 * 60,
    },
  ]

  return miners.slice(0, count).map((miner, index) => ({
    ...miner,
    payout_address: payoutAddress,
    session_count: 1,
    rejection_percent: miner.total_shares > 0
      ? miner.total_rejected / miner.total_shares * 100
      : 0,
    opened_at: now - miner.uptime_seconds,
    closed_at: null,
    last_seen_at: now - 8 - index * 11,
    last_seen_seconds: 8 + index * 11,
    connected: true,
    active_on_template: true,
    connection_state: "online",
    sessions: [],
  }))
}

function demoStatus({ blocks, difficultyAdjustment, now }) {
  const latest = blocks[0] || fallbackBlocks()[0]
  const difficulty = numeric(latest.difficulty, fallbackDifficulty)
  const nextRetargetHeight = numeric(
    difficultyAdjustment.nextRetargetHeight,
    Math.ceil((latest.height + 1) / 2016) * 2016
  )
  const remainingBlocks = numeric(
    difficultyAdjustment.remainingBlocks,
    Math.max(0, nextRetargetHeight - latest.height)
  )
  const remainingTimeMs = numeric(difficultyAdjustment.remainingTime, remainingBlocks * 600_000)
  const progressPercent = numeric(
    difficultyAdjustment.progressPercent,
    ((latest.height % 2016) / 2016) * 100
  )
  const difficultyChange = numeric(difficultyAdjustment.difficultyChange, 0)
  const templateFees = 9_420_000
  const templateWeight = 3_993_000
  const templateTxCount = 2_481
  const miners = demoMinerWorkers(now, Math.max(0, screenshotMinerCount))

  return {
    server_time: now,
    config: {
      network: "mainnet",
      sv2_listen_address: "0.0.0.0:3335",
      metrics_listen_address: "127.0.0.1:9090",
    },
    miner_setup: {
      payout_address: payoutAddress,
      sv2_authority_public_key: "9bbN477wFeyxgAwN8wZron6i4jTAVtJdGJ5XCCdp3sKZotCfdJJ",
    },
    monitoring: {},
    mining: {
      sv2_listening: true,
      template_provider_available: true,
      ipc_socket_path: "/root/.bitcoin/node.sock",
      warning: null,
      error: null,
    },
    bitcoin_core: {
      rpc_available: true,
      rpc_warning: null,
      mining_ready: true,
      network_hashrate: networkHashrateFromDifficulty(difficulty),
      network_difficulty: difficulty,
      difficulty_adjustment: {
        next_height: nextRetargetHeight,
        blocks_remaining: remainingBlocks,
        progress_percent: progressPercent,
        estimated_seconds_remaining: Math.round(remainingTimeMs / 1000),
        projected_percent_change: difficultyChange,
        projected_difficulty: difficulty * (1 + difficultyChange / 100),
      },
      chain_height: latest.height,
      sync_progress: 1,
      initial_block_download: false,
      template: {
        height: latest.height + 1,
        reward_sats: blockSubsidySats + templateFees,
        fee_sats: templateFees,
        weight: templateWeight,
        weight_percent: templateWeight / 4_000_000 * 100,
        transaction_count: templateTxCount,
        updated_at: now,
        source: "ipc_mempool",
        status: "available",
      },
    },
    recent_blocks: blocks.slice(0, 6).map((block) => ({
      height: block.height,
      hash: block.id,
      timestamp: block.timestamp,
      weight: block.weight,
      transaction_count: block.tx_count,
      reward_sats: block.reward_sats ?? blockSubsidySats,
      pool: block.pool || poolAttribution(null),
    })),
    connected_miners: [],
    miner_workers: miners,
  }
}

async function installDemoRoutes(page) {
  if (screenshotMode !== "screenshot") return

  const [rawBlocks, difficultyAdjustment, prices] = await Promise.all([
    fetchJson("https://mempool.space/api/blocks", fallbackBlocks()),
    fetchJson("https://mempool.space/api/v1/difficulty-adjustment", {}),
    fetchJson("https://mempool.space/api/v1/prices", { USD: fallbackPriceUsd }),
  ])
  const blocks = await enrichBlocks(Array.isArray(rawBlocks) && rawBlocks.length ? rawBlocks : fallbackBlocks())
  const now = Math.floor(Date.now() / 1000)
  const status = demoStatus({
    blocks,
    difficultyAdjustment: difficultyAdjustment || {},
    now,
  })
  const usd = numeric(prices?.USD, fallbackPriceUsd)

  await page.route("**/api/status", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify(status),
    })
  })

  await page.route("**/api/exchange-rates", async (route) => {
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        supported_currencies: ["USD", "EUR", "GBP", "CAD", "CHF", "AUD", "JPY"],
        rates: {
          USD: usd,
          EUR: numeric(prices?.EUR, usd),
          GBP: numeric(prices?.GBP, usd),
          CAD: numeric(prices?.CAD, usd),
          CHF: numeric(prices?.CHF, usd),
          AUD: numeric(prices?.AUD, usd),
          JPY: numeric(prices?.JPY, usd),
        },
        updated_at: now,
        stale: false,
      }),
    })
  })
}

async function waitForDashboard(page) {
  const url = new URL(dashboardUrl)
  if (screenshotMode !== "screenshot" && !url.searchParams.has("debug")) {
    url.searchParams.set("debug", "1")
  }
  if (screenshotMode !== "screenshot" && !url.searchParams.has("debug-miners") && Number.isFinite(screenshotMinerCount)) {
    url.searchParams.set("debug-miners", String(Math.max(0, screenshotMinerCount)))
  }

  const response = await page.goto(url.toString(), {
    waitUntil: "domcontentloaded",
    timeout: 30_000,
  })

  if (!response?.ok()) {
    throw new Error(`Dashboard returned HTTP ${response?.status() ?? "no response"}`)
  }

  await expect(page.locator("body")).toContainText("Canary Mining", {
    timeout: 15_000,
  })

  await page.waitForLoadState("networkidle", { timeout: 15_000 }).catch(() => {})
  await page.waitForTimeout(waitMs)
}

async function main() {
  fs.mkdirSync(path.dirname(outputPath), { recursive: true })

  const browser = await chromium.launch()
  const page = await browser.newPage({
    viewport: { width, height },
    deviceScaleFactor: 1,
  })

  try {
    await installDemoRoutes(page)
    await waitForDashboard(page)
    await page.screenshot({
      path: outputPath,
      fullPage,
      animations: "disabled",
    })
  } finally {
    await browser.close()
  }

  console.log(`Wrote ${path.relative(repoRoot, outputPath)} from ${dashboardUrl}`)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
