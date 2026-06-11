import { chromium } from "@playwright/test"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, "../..")

const inputPath =
  process.env.CANARY_MINING_UMBREL_SCREENSHOT ||
  path.join(repoRoot, "screenshots/dashboard.png")
const outputDir =
  process.env.CANARY_MINING_UMBREL_GALLERY_DIR ||
  path.join(repoRoot, "screenshots/umbrel")
const iconPath =
  process.env.CANARY_MINING_UMBREL_ICON ||
  path.join(repoRoot, "ui/canary-in-a-coalmine.svg")

const width = Number(process.env.CANARY_MINING_UMBREL_GALLERY_WIDTH || "1440")
const height = Number(process.env.CANARY_MINING_UMBREL_GALLERY_HEIGHT || "900")

const slides = [
  {
    file: "1.png",
    eyebrow: "CANARY MINING",
    title: "Native Stratum V2 solo mining for Umbrel",
    caption: "Connect compatible miners directly to your own Bitcoin node.",
    cropY: "0%",
  },
  {
    file: "2.png",
    eyebrow: "MINING TEMPLATE",
    title: "See the block your miners are working on",
    caption: "Track template value, fees, weight, transactions, and connected miners.",
    cropY: "39%",
  },
  {
    file: "3.png",
    eyebrow: "RECENT BLOCKS",
    title: "Keep mining context in view",
    caption: "Monitor recent blocks, pool attribution, network difficulty, and hashrate.",
    cropY: "72%",
  },
]

function assertFile(filePath, label) {
  if (!fs.existsSync(filePath)) {
    throw new Error(`${label} not found: ${filePath}`)
  }
}

function cssString(value) {
  return String(value).replace(/\\/g, "\\\\").replace(/"/g, '\\"')
}

function dataUrl(filePath) {
  const ext = path.extname(filePath).toLowerCase()
  const mimeTypes = new Map([
    [".png", "image/png"],
    [".jpg", "image/jpeg"],
    [".jpeg", "image/jpeg"],
    [".svg", "image/svg+xml"],
    [".webp", "image/webp"],
  ])
  const mimeType = mimeTypes.get(ext) || "application/octet-stream"
  return `data:${mimeType};base64,${fs.readFileSync(filePath).toString("base64")}`
}

async function renderSlide(page, slide, assets) {
  const html = `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <style>
      * {
        box-sizing: border-box;
      }

      body {
        margin: 0;
        width: ${width}px;
        height: ${height}px;
        overflow: hidden;
        background:
          radial-gradient(circle at 14% 18%, rgba(246, 190, 0, 0.24), transparent 24%),
          radial-gradient(circle at 84% 72%, rgba(22, 132, 67, 0.14), transparent 28%),
          linear-gradient(135deg, #f8faf7 0%, #eef3f8 58%, #f6f7ef 100%);
        color: #172033;
        font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      }

      .stage {
        position: relative;
        width: 100%;
        height: 100%;
        padding: 62px 76px;
      }

      .brand-row {
        display: flex;
        align-items: center;
        gap: 18px;
      }

      .icon {
        width: 72px;
        height: 72px;
        border-radius: 18px;
        box-shadow: 0 18px 48px rgba(23, 32, 51, 0.18);
      }

      .eyebrow {
        color: #315998;
        font-size: 22px;
        font-weight: 800;
        letter-spacing: 0;
        text-transform: uppercase;
      }

      h1 {
        max-width: 760px;
        margin: 26px 0 0;
        font-size: 54px;
        line-height: 1.02;
        letter-spacing: 0;
      }

      p {
        max-width: 680px;
        margin: 18px 0 0;
        color: #52627c;
        font-size: 25px;
        line-height: 1.28;
      }

      .screenshot-frame {
        position: absolute;
        left: 76px;
        right: 76px;
        bottom: 58px;
        height: 430px;
        overflow: hidden;
        border: 1px solid rgba(98, 117, 144, 0.2);
        border-radius: 28px;
        background: #fff;
        box-shadow: 0 32px 92px rgba(23, 32, 51, 0.24);
      }

      .screenshot {
        width: 100%;
        height: 100%;
        object-fit: cover;
        object-position: center ${cssString(slide.cropY)};
        display: block;
      }

      .accent {
        position: absolute;
        top: 82px;
        right: 82px;
        width: 164px;
        height: 164px;
        border: 18px solid rgba(22, 132, 67, 0.12);
        border-radius: 50%;
      }
    </style>
  </head>
  <body>
    <main class="stage">
      <div class="accent" aria-hidden="true"></div>
      <div class="brand-row">
        <img class="icon" src="${assets.icon}" alt="">
        <div class="eyebrow">${slide.eyebrow}</div>
      </div>
      <h1>${slide.title}</h1>
      <p>${slide.caption}</p>
      <div class="screenshot-frame">
        <img class="screenshot" src="${assets.screenshot}" alt="">
      </div>
    </main>
  </body>
</html>`

  await page.setContent(html, { waitUntil: "load" })
  await page.waitForFunction(() =>
    Array.from(document.images).every((image) => image.complete && image.naturalWidth > 0)
  )
  await page.screenshot({
    path: path.join(outputDir, slide.file),
    fullPage: false,
    animations: "disabled",
  })
}

async function main() {
  assertFile(inputPath, "Dashboard screenshot")
  assertFile(iconPath, "Canary Mining icon")
  fs.mkdirSync(outputDir, { recursive: true })
  const assets = {
    icon: dataUrl(iconPath),
    screenshot: dataUrl(inputPath),
  }

  const browser = await chromium.launch()
  const page = await browser.newPage({
    viewport: { width, height },
    deviceScaleFactor: 1,
  })

  try {
    for (const slide of slides) {
      await renderSlide(page, slide, assets)
    }
  } finally {
    await browser.close()
  }

  console.log(`Wrote Umbrel gallery images to ${path.relative(repoRoot, outputDir)}`)
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
