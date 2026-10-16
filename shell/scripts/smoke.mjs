#!/usr/bin/env node
// Smoke test: the built frontend must actually boot in a real browser.
//
// Spawns `vite preview`, loads the app in a headless Chromium with a virtual
// time budget, then asserts:
//   1. the React tree mounted (#root has content — this guards the v0.2.2
//      white-screen class of failures where assets load but nothing renders),
//      and
//   2. no fatal module-chain errors on the console ("require is not defined",
//      unresolved module specifiers, ...).
//
// Console noise that is expected in a plain browser (no Tauri backend: IPC,
// updater, dialog calls; favicon 404) is exempt. Run via `pnpm run smoke`
// (which builds first) — the same command runs in CI and release.

import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const PORT = Number(process.env.SMOKE_PORT) || 4173;
const APP_URL = `http://127.0.0.1:${PORT}/`;
const VITE_BIN = fileURLToPath(
  new URL("../node_modules/vite/bin/vite.js", import.meta.url),
);

// Module-chain breakage: the bundle cannot execute. Any hit fails the smoke.
const FATAL = [
  /require is not defined/i,
  /Failed to resolve module specifier/i,
  /ERR_MODULE_NOT_FOUND/i,
  /Importing a module script failed/i,
  /Uncaught SyntaxError/i,
];

// Expected in a browser without the Tauri backend; never fails the smoke.
const EXEMPT = [
  /tauri|__TAURI/i,
  /\binvoke\b/i,
  /favicon/i,
  /Failed to load resource/i,
];

function findBrowser() {
  const candidates = [
    process.env.CHROME_PATH,
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "/snap/bin/chromium",
  ].filter(Boolean);
  for (const c of candidates) {
    const probe = spawnSync(c, ["--version"], {
      encoding: "utf8",
      timeout: 10_000,
    });
    if (probe.status === 0) return c;
  }
  return null;
}

async function waitForServer(deadlineMs = 20_000, isDead = () => false) {
  const start = Date.now();
  while (Date.now() - start < deadlineMs) {
    if (isDead())
      throw new Error(`vite preview exited early (port ${PORT} in use?)`);
    try {
      const res = await fetch(APP_URL);
      if (res.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error(`vite preview did not become ready on :${PORT}`);
}

function loadInBrowser(browser) {
  // --virtual-time-budget fast-forwards timers until React has mounted, then
  // --dump-dom prints the final DOM to stdout; --enable-logging=stderr routes
  // page console messages to stderr.
  const args = [
    "--headless=new",
    "--disable-gpu",
    "--virtual-time-budget=10000",
    "--dump-dom",
    "--enable-logging=stderr",
    "--v=0",
    APP_URL,
  ];
  return new Promise((resolve, reject) => {
    const child = spawn(browser, args);
    let dom = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("browser run timed out"));
    }, 60_000);
    child.stdout.on("data", (d) => (dom += d));
    child.stderr.on("data", (d) => (stderr += d));
    child.on("error", (e) => {
      clearTimeout(timer);
      reject(e);
    });
    child.on("close", () => {
      clearTimeout(timer);
      resolve({ dom, stderr });
    });
  });
}

function parseConsoleErrors(stderr) {
  // Chromium routes page console messages to stderr as
  // [pid:tid:ts:INFO:CONSOLE:line] "message", source: ... — note the level is
  // always INFO (console.error is not distinguished), so classification is
  // purely content-based. A healthy production bundle is fully silent here
  // (verified: zero messages on a good build), which makes "any non-exempt
  // message fails" the tightest safe policy.
  return stderr
    .split("\n")
    .filter((l) => l.includes(":CONSOLE"))
    .map((l) => {
      const at = l.indexOf("] ");
      return at === -1 ? l : l.slice(at + 2);
    });
}

async function main() {
  const browser = findBrowser();
  if (!browser) {
    console.error(
      "smoke: no chrome/chromium found (set CHROME_PATH to override)",
    );
    process.exit(1);
  }
  console.log(`smoke: browser = ${browser}`);

  // Direct node invocation (no pnpm middle layer) so the kill below is exact.
  const preview = spawn(process.execPath, [
    VITE_BIN,
    "preview",
    "--port",
    String(PORT),
    "--strictPort",
  ]);
  preview.on("error", (e) => {
    console.error(`smoke: failed to start vite preview: ${e.message}`);
    process.exit(1);
  });
  // Fail fast if the preview server dies on startup (e.g. port already in
  // use) instead of waiting out the readiness deadline.
  let previewDead = false;
  preview.on("exit", () => {
    previewDead = true;
  });

  try {
    await waitForServer(20_000, () => previewDead);
    console.log(`smoke: preview ready at ${APP_URL}`);

    const { dom, stderr } = await loadInBrowser(browser);

    // Assertion 1: React mounted into #root. The content must contain a real
    // element tag — comments or stray text don't count as a mounted tree.
    const rootMatch = dom.match(/<div id="root">([\s\S]*?)(<\/div>|$)/);
    const mounted = Boolean(rootMatch && /<\w/.test(rootMatch[1]));
    if (!mounted) {
      console.error("smoke: FAIL — #root is empty, the app did not mount");
      process.exitCode = 1;
      return;
    }

    // Assertion 2: the module entry script made it into the document.
    if (!/<script[^>]*type="module"/.test(dom)) {
      console.error("smoke: FAIL — no module entry <script> in the DOM");
      process.exitCode = 1;
      return;
    }

    // Assertion 3: console errors. Fatal (module-chain) patterns always fail;
    // exempt patterns never fail; anything else fails loudly so new breakage
    // classes cannot slip through as silence.
    const errors = parseConsoleErrors(stderr);
    const fatal = errors.filter((t) => FATAL.some((re) => re.test(t)));
    const other = errors.filter(
      (t) =>
        !FATAL.some((re) => re.test(t)) && !EXEMPT.some((re) => re.test(t)),
    );
    if (fatal.length > 0 || other.length > 0) {
      for (const t of fatal)
        console.error(`smoke: FAIL — fatal module error: ${t}`);
      for (const t of other)
        console.error(`smoke: FAIL — unexpected console error: ${t}`);
      process.exitCode = 1;
      return;
    }

    console.log(
      `smoke: PASS — app mounted, module entry present, console clean ` +
        `(${errors.filter((t) => EXEMPT.some((re) => re.test(t))).length} exempt message(s))`,
    );
  } finally {
    preview.kill();
  }
}

main().catch((e) => {
  console.error(`smoke: FAIL — ${e.message}`);
  process.exit(1);
});
