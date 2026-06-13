import { chromium } from "playwright";

const [, , pageUrl, profileDir, timeoutArg, attemptsArg, modeArg, browserModeArg] = process.argv;
const timeoutMs = Number(timeoutArg || 15000);
const navigationTimeoutMs = Math.min(timeoutMs, 5000);
const attempts = Math.max(1, Number(attemptsArg || 3));
const shouldAbortM3u8 = modeArg !== "allow";
const browserMode = ["headless", "background", "headed"].includes(browserModeArg)
  ? browserModeArg
  : browserModeArg === "hidden"
    ? "background"
    : "headless";
const headless = browserMode === "headless";
const backgroundWindow = browserMode === "background";

if (!pageUrl || !profileDir) {
  console.error("Usage: node scripts/extract-m3u8.mjs <page-url> <profile-dir> [timeout-ms]");
  process.exit(2);
}

const requestCandidates = new Set();
const networkCandidates = new Set();
const responseCandidates = new Set();
const candidateHeaders = new Map();
const candidatePlaylistText = new Map();
const pendingScans = new Set();
const pendingM3u8Responses = new Set();
let firstM3u8RequestAt = 0;
let pageTitle = "";

function log(message) {
  console.error(`[extract] ${message}`);
}

function normalizeUrl(value) {
  return String(value || "")
    .replaceAll("\\/", "/")
    .replaceAll("\\u0026", "&")
    .replaceAll("&amp;", "&");
}

function collectFromText(text, baseUrl) {
  const normalized = normalizeUrl(text);
  const lower = normalized.toLowerCase();
  let offset = 0;

  while (true) {
    const found = lower.indexOf(".m3u8", offset);
    if (found === -1) break;

    let start = found;
    while (start > 0 && !/[\s"'`<>,;]/.test(normalized[start - 1])) {
      start -= 1;
    }

    let end = found + ".m3u8".length;
    while (end < normalized.length && !/[\s"'`<>,;]/.test(normalized[end])) {
      end += 1;
    }

    const raw = normalized.slice(start, end).replace(/^[([{]+|[)\]}]+$/g, "");
    try {
      const url = new URL(raw, baseUrl).toString();
      requestCandidates.add(url);
      try {
        const source = new URL(baseUrl);
        rememberHeaders(url, { referer: `${source.origin}/` });
      } catch {
        // Ignore missing source URL.
      }
    } catch {
      // Ignore malformed candidate.
    }
    offset = end;
  }
}

function rememberHeaders(url, headers) {
  const existing = candidateHeaders.get(url) || {};
  candidateHeaders.set(url, {
    referer: existing.referer || headers.referer || headers.referrer || "",
    origin: existing.origin || headers.origin || "",
    cookie: existing.cookie || headers.cookie || "",
    userAgent: existing.userAgent || headers["user-agent"] || "",
    accept: existing.accept || headers.accept || "",
    acceptLanguage: existing.acceptLanguage || headers["accept-language"] || "",
  });
}

function trackScan(promise) {
  pendingScans.add(promise);
  promise.finally(() => pendingScans.delete(promise));
}

function trackM3u8Response(promise) {
  pendingM3u8Responses.add(promise);
  promise.finally(() => pendingM3u8Responses.delete(promise));
}

async function rememberPageTitle(page) {
  const title = await page.title().catch(() => "");
  if (title && !pageTitle) {
    pageTitle = title.trim();
    log(`page title ${pageTitle}`);
  }
}

async function flushPendingScans() {
  if (pendingScans.size === 0) return;
  await Promise.allSettled([...pendingScans]);
}

async function flushPendingM3u8Responses() {
  if (pendingM3u8Responses.size === 0) return;
  await Promise.allSettled([...pendingM3u8Responses]);
}

async function scanPage(page) {
  log("scan rendered DOM");
  collectFromText(await page.content().catch(() => ""), pageUrl);
  const domValues = await page
    .evaluate(() => {
      const values = [];
      document.querySelectorAll("video, source, iframe, script, a, div").forEach((node) => {
        for (const attr of ["src", "data-src", "href", "data-url", "data-play", "data-player"]) {
          const value = node.getAttribute(attr);
          if (value) values.push(value);
        }
        if (node.textContent) values.push(node.textContent);
      });
      performance.getEntriesByType("resource").forEach((entry) => values.push(entry.name));
      return values;
    })
    .catch(() => []);

  for (const value of domValues) {
    collectFromText(value, pageUrl);
  }

  for (const frame of page.frames()) {
    collectFromText(await frame.content().catch(() => ""), frame.url() || pageUrl);
  }
}

async function waitForM3u8OrTimeout(page, timeoutMs) {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    if (candidatePlaylistText.size > 0) {
      log("m3u8 response captured, stop waiting");
      return;
    }

    if (networkCandidates.size > 0 && shouldAbortM3u8) {
      log("m3u8 request captured");
      return;
    }

    if (networkCandidates.size > 0 && Date.now() - firstM3u8RequestAt > 5000) {
      log("m3u8 request captured but no readable response");
      return;
    }

    await page.waitForTimeout(250).catch(() => {
      throw new Error("page closed while waiting for m3u8");
    });
  }
}

log(`launch chromium profile=${profileDir} mode=${shouldAbortM3u8 ? "abort" : "allow"} browser=${browserMode}`);

const context = await chromium.launchPersistentContext(profileDir, {
  headless,
  viewport: { width: 1280, height: 900 },
  args: backgroundWindow
    ? [
        "--window-position=-32000,-32000",
        "--window-size=1280,900",
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
        "--disable-background-timer-throttling",
      ]
    : [],
});

try {
  const page = context.pages()[0] || await context.newPage();
  if (backgroundWindow) {
    const session = await context.newCDPSession(page).catch(() => null);
    if (session) {
      const targetWindow = await session.send("Browser.getWindowForTarget").catch(() => null);
      if (targetWindow?.windowId) {
        await session
          .send("Browser.setWindowBounds", {
            windowId: targetWindow.windowId,
            bounds: {
              left: -32000,
              top: -32000,
              width: 1280,
              height: 900,
              windowState: "normal",
            },
          })
          .catch(() => {});
      }
    }
  }

  const record = async (request) => {
    const url = request.url();
    if (String(url).toLowerCase().includes(".m3u8")) {
      const headers = await request.allHeaders().catch(() => request.headers());
      rememberHeaders(url, headers);
      log(
        `candidate ${url} referer=${headers.referer || ""} origin=${headers.origin || ""} cookie=${headers.cookie ? "<present>" : "<none>"}`
      );
      if (!firstM3u8RequestAt) firstM3u8RequestAt = Date.now();
      networkCandidates.add(url);
      requestCandidates.add(url);
    }
  };

  if (shouldAbortM3u8) {
    await context.route("**/*", async (route, request) => {
      if (String(request.url()).toLowerCase().includes(".m3u8")) {
        await record(request);
        log(`abort browser m3u8 request so downloader can use token first ${request.url()}`);
        await route.abort("aborted").catch(() => {});
        return;
      }
      await route.continue().catch(() => {});
    });
  }

  page.on("request", (request) => record(request));
  page.on("response", (response) => {
    const responseUrl = response.url();
    if (String(responseUrl).toLowerCase().includes(".m3u8")) {
      trackM3u8Response(
        (async () => {
          log(`m3u8 response status=${response.status()} ${responseUrl}`);
          if (response.ok()) {
            const text = await response.text().catch(() => "");
            if (text.includes("#EXTM3U")) {
              candidatePlaylistText.set(responseUrl, text);
              responseCandidates.add(responseUrl);
            }
          }
          requestCandidates.add(responseUrl);
        })()
      );
    }
    const contentType = String(response.headers()["content-type"] || "").toLowerCase();
    if (
      contentType.includes("html") ||
      contentType.includes("javascript") ||
      contentType.includes("json") ||
      response.url().toLowerCase().includes(".js")
    ) {
      trackScan(
        response
          .text()
          .then((text) => collectFromText(text, response.url()))
          .catch(() => {})
      );
    }
  });

  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    log(`goto attempt=${attempt}/${attempts} ${pageUrl}`);
    await page.goto(pageUrl, { waitUntil: "domcontentloaded", timeout: navigationTimeoutMs }).catch((error) => {
      console.error(`page.goto failed: ${error.message}`);
    });
    await rememberPageTitle(page);

    log(`wait up to ${timeoutMs}ms for m3u8 requests`);
    await waitForM3u8OrTimeout(page, timeoutMs).catch((error) => {
      console.error(`wait for m3u8 failed: ${error.message}`);
    });
    await flushPendingM3u8Responses();
    if (networkCandidates.size === 0) {
      await flushPendingScans();
      await scanPage(page).catch((error) => {
        console.error(`scan page failed: ${error.message}`);
      });
    }

    const totalCandidates = new Set([...responseCandidates, ...requestCandidates]).size;
    log(`attempt=${attempt} candidates=${totalCandidates} okResponses=${responseCandidates.size}`);
    if (totalCandidates > 0) {
      break;
    }
  }

  const candidates = [...responseCandidates, ...requestCandidates].filter(
    (candidate, index, values) => values.indexOf(candidate) === index
  ).map((url) => ({
    url,
    ...(pageTitle ? { pageTitle } : {}),
    ...(candidateHeaders.get(url) || {}),
    ...(candidatePlaylistText.has(url) ? { playlistText: candidatePlaylistText.get(url) } : {}),
  }));
  log(`done candidates=${candidates.length} okResponses=${responseCandidates.size}`);
  console.log(JSON.stringify({ candidates }));
} finally {
  log("close chromium");
  await context.close();
}
