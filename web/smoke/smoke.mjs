#!/usr/bin/env node
// Browser smoke test for a Ponko web build (see README.md).
//
//   node web/smoke/smoke.mjs --dir target/web/web_smoke --backend webgl2
//   node web/smoke/smoke.mjs --fixture web_smoke_3d --build --browser firefox
//
// Serves the built folder over HTTP, opens it in a headless browser (Chromium,
// Firefox or WebKit) and checks that the game starts, fetches its assets, keeps simulating, reacts to a
// click and draws a nonblank canvas. Exit codes:
//
//   0  passed (for a --case self-test: the expected failure was detected)
//   1  the game failed (a panic, a failed asset fetch, a stall, ...)
//   2  unsupported: the browser could not start, or lacks the backend asked for
//   3  bad arguments or a missing build
//
// Every run writes report.json (browser, backend, adapter, timings, console),
// screenshots, a Playwright trace (trace.zip) and the browser's own output
// (browser.log) to --out.
//
// No run can hang: once the page is open every browser call has a deadline
// (CALL_TIMEOUT_MS), a crashed page fails at once, and the whole run has one
// (--run-timeout). Both fail as "browser-hang" (or "browser-crash") with the
// step that hung and the tail of browser.log.

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { dirname, extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
// Before playwright-core: it keeps the browser's own output for the report.
import { browserLog } from "./browser-log.mjs";
import { chromium, firefox, webkit } from "playwright-core";
import {
  BrowserHang,
  CALL_TIMEOUT_MS,
  CASES,
  EXIT,
  MIN_PROGRESS_TICKS,
  USAGE,
  UsageError,
  backendAvailable,
  checkEngineGpu,
  checkMinimums,
  checkMusicBeforeClick,
  launchConfig,
  launchFailureKind,
  logTail,
  musicAfterClick,
  parseArgs,
  parseFixtures,
  startWatchdog,
  unsupportedReason,
  withDeadline,
} from "./lib.mjs";

// The canvas web/index.html gives the engine (its CANVAS_ID in src/web.rs).
export const GAME_CANVAS = "canvas#ponko";

const HERE = dirname(fileURLToPath(import.meta.url));
export const LAUNCHERS = { chromium, firefox, webkit };

function usage(message) {
  console.error(`smoke: ${message}`);
  console.error(USAGE);
  process.exit(EXIT.usage);
}

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".png": "image/png",
  ".glb": "model/gltf-binary",
  ".wgsl": "text/plain",
};

/** An empty page on the same (secure) origin: WebGPU isn't exposed on about:blank. */
const PROBE_PATH = "/__smoke_probe.html";

/** A static file server for `root` on a free localhost port. */
export async function serve(root) {
  const server = createServer(async (req, res) => {
    const path = decodeURIComponent(new URL(req.url, "http://x").pathname);
    if (path === PROBE_PATH) {
      res.writeHead(200, { "content-type": MIME[".html"] }).end("<!doctype html><title>probe</title>");
      return;
    }
    const file = normalize(join(root, path.endsWith("/") ? path + "index.html" : path));
    if (file !== root && !file.startsWith(root + sep)) {
      res.writeHead(403).end();
      return;
    }
    try {
      const body = await readFile(file);
      res.writeHead(200, { "content-type": MIME[extname(file)] ?? "application/octet-stream" }).end(body);
    } catch {
      res.writeHead(404).end("not found");
    }
  });
  await new Promise((ok) => server.listen(0, "127.0.0.1", ok));
  return server;
}

const sleep = (ms) => new Promise((ok) => setTimeout(ok, ms));

/**
 * What the browser offers, probed on a blank page before the game loads.
 * WebGL2 is asked for first, and a WebGL2 run skips the WebGPU canvas test:
 * on a GPU-less runner that test (a software WebGPU device presenting a
 * canvas) can leave the browser without WebGL2 for the pages that follow.
 */
export async function probe(context, url, backend) {
  const page = await context.newPage();
  try {
    await page.goto(new URL(PROBE_PATH, url).href);
    return await page.evaluate(async (webgpuCanvasTest) => {
      const result = { webgpu: null, webgl2: null, hasNavigatorGpu: Boolean(navigator.gpu) };
      const gl = document.createElement("canvas").getContext("webgl2");
      if (gl) {
        const debug = gl.getExtension("WEBGL_debug_renderer_info");
        result.webgl2 = {
          vendor: gl.getParameter(debug ? debug.UNMASKED_VENDOR_WEBGL : gl.VENDOR),
          renderer: gl.getParameter(debug ? debug.UNMASKED_RENDERER_WEBGL : gl.RENDERER),
        };
      }
      if (navigator.gpu) {
        try {
          const adapter = await navigator.gpu.requestAdapter();
          if (adapter) {
            const info = adapter.info ?? {};
            result.webgpu = {
              vendor: info.vendor ?? "",
              architecture: info.architecture ?? "",
              description: info.description ?? "",
              fallback: Boolean(adapter.isFallbackAdapter ?? info.isFallbackAdapter),
            };
            if (!webgpuCanvasTest) return result;
            // Can this browser show a WebGPU canvas at all? Clear one to
            // red without the engine and copy it back. (Some headless
            // setups have an adapter but no canvas presentation.)
            const device = await adapter.requestDevice();
            const errors = [];
            device.addEventListener("uncapturederror", (e) => errors.push(e.error.message));
            const canvas = document.createElement("canvas");
            canvas.width = canvas.height = 16;
            document.body.append(canvas);
            const context = canvas.getContext("webgpu");
            context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: "opaque" });
            const encoder = device.createCommandEncoder();
            encoder
              .beginRenderPass({
                colorAttachments: [{ view: context.getCurrentTexture().createView(), loadOp: "clear", storeOp: "store", clearValue: [1, 0, 0, 1] }],
              })
              .end();
            device.queue.submit([encoder.finish()]);
            // Read it back in the same task as the submit: a WebGPU canvas's
            // texture expires when its frame is presented, and a copy taken
            // after that reads transparent black even on a working GPU.
            const copy = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
            copy.canvas.width = copy.canvas.height = 16;
            copy.drawImage(canvas, 0, 0);
            const [r, g, b] = copy.getImageData(8, 8, 1, 1).data;
            result.webgpu.canvas = r > 200 && g < 50 && b < 50 && errors.length === 0;
            if (!result.webgpu.canvas) result.webgpu.canvasProblem = errors[0] ?? `cleared red, read back rgb(${r}, ${g}, ${b})`;
            device.destroy();
          }
        } catch (e) {
          result.webgpuError = String(e);
          if (result.webgpu) {
            result.webgpu.canvas = false;
            result.webgpu.canvasProblem = `probing a WebGPU canvas threw: ${e}`;
          }
        }
      }
      return result;
    }, backend !== "webgl2");
  } finally {
    await page.close();
  }
}

/**
 * Counts colors and pixels unlike the most common one, inside the browser:
 * of a PNG screenshot, or (without one) of the game canvas copied with
 * drawImage, which also sees WebGPU canvases that headless screenshots miss.
 */
export async function canvasStats(page, png) {
  return page.evaluate(async ([base64, selector]) => {
    let image;
    if (base64) {
      image = new Image();
      image.src = `data:image/png;base64,${base64}`;
      await image.decode();
    } else {
      image = document.querySelector(selector);
    }
    const canvas = document.createElement("canvas");
    canvas.width = image.width;
    canvas.height = image.height;
    const ctx = canvas.getContext("2d");
    if (base64) {
      ctx.drawImage(image, 0, 0);
    } else {
      // Copy inside an animation frame, in the same task as the game's own
      // frame callback: a WebGPU canvas is only readable until its frame is
      // presented.
      await new Promise((ok) =>
        requestAnimationFrame(() => {
          ctx.drawImage(image, 0, 0);
          ok();
        }),
      );
    }
    const { data } = ctx.getImageData(0, 0, image.width, image.height);
    const counts = new Map();
    for (let i = 0; i < data.length; i += 4) {
      const key = (data[i] << 16) | (data[i + 1] << 8) | data[i + 2];
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    const pixels = data.length / 4;
    const top = Math.max(...counts.values());
    return { width: image.width, height: image.height, colors: counts.size, foreground: (pixels - top) / pixels };
  }, [png ? png.toString("base64") : null, GAME_CANVAS]);
}

/** The fixture's `<html data-smoke>` state, or null before its first frame. */
async function smokeState(page) {
  return page.evaluate(() => {
    const raw = document.documentElement.getAttribute("data-smoke");
    const status = document.getElementById("status");
    const gpu = document.documentElement.getAttribute("data-smoke-gpu");
    return {
      state: raw ? JSON.parse(raw) : null,
      gpu: gpu ? JSON.parse(gpu) : null,
      status: status && status.className === "error" ? status.textContent : null,
    };
  });
}

/**
 * Records the page's AudioContexts and the buffers it plays: how many it
 * starts, how many hold sound (the engine's audio output schedules one
 * buffer source per block), and how many end, so a run can tell that music
 * was heard, or else where the output stopped. Installed before the page's
 * own scripts run.
 */
function watchAudio() {
  const seen = { contexts: [], audible: 0, started: 0, ended: 0 };
  Object.defineProperty(window, "__smokeAudio", { value: seen });
  const Original = window.AudioContext;
  if (!Original) return;
  window.AudioContext = class extends Original {
    constructor(...options) {
      super(...options);
      seen.contexts.push(this);
    }
  };
  const start = AudioBufferSourceNode.prototype.start;
  AudioBufferSourceNode.prototype.start = function (...when) {
    seen.started++;
    this.addEventListener("ended", () => seen.ended++);
    const buffer = this.buffer;
    if (buffer && buffer.getChannelData(0).some((sample) => Math.abs(sample) > 1e-4)) seen.audible++;
    return start.apply(this, when);
  };
}

/**
 * What `watchAudio` saw: each AudioContext's state, clock (seconds), sample
 * rate and output channels, and the buffers started, ended and with sound
 * in them.
 */
async function audioSeen(page) {
  return page.evaluate(() => {
    const seen = window.__smokeAudio ?? { contexts: [], audible: 0, started: 0, ended: 0 };
    return {
      contexts: seen.contexts.map((c) => c.state),
      clocks: seen.contexts.map((c) => Math.round(c.currentTime * 1000) / 1000),
      sampleRates: seen.contexts.map((c) => c.sampleRate),
      maxChannels: seen.contexts.map((c) => c.destination.maxChannelCount),
      audible: seen.audible,
      started: seen.started,
      ended: seen.ended,
    };
  });
}

/** Which context the game's canvas got: a second getContext of another kind returns null. */
export async function canvasBackend(page) {
  return page.evaluate((selector) => {
    const canvas = document.querySelector(selector);
    if (!canvas) return "unknown";
    if (canvas.getContext("webgl2")) return "webgl2";
    if (navigator.gpu && canvas.getContext("webgpu")) return "webgpu";
    return "unknown";
  }, GAME_CANVAS);
}

class Failure extends Error {
  constructor(kind, message) {
    super(message);
    this.kind = kind;
  }
}

/** `--build`: builds the fixture's example the way the fixture says (its Cargo features). */
function build(args) {
  const buildArgs = [join(HERE, "..", "build.sh")];
  if (args.features.length) buildArgs.push("--features", args.features.join(","));
  buildArgs.push(args.example, args.dir);
  console.log(`smoke: web/build.sh ${buildArgs.slice(1).join(" ")}`);
  const built = spawnSync("bash", buildArgs, { stdio: "inherit" });
  if (built.status !== 0) usage(`building ${args.example} failed`);
}

async function main() {
  let args;
  try {
    args = parseArgs(process.argv.slice(2), parseFixtures(await readFile(join(HERE, "fixtures.json"), "utf8")));
  } catch (e) {
    if (e instanceof UsageError) usage(e.message);
    throw e;
  }
  if (args.build) build(args);
  const root = resolve(args.dir);
  try {
    await stat(join(root, "index.html"));
  } catch {
    usage(`${root}/index.html not found: build it first, e.g. web/build.sh web_smoke (or pass --fixture <name> --build)`);
  }
  await mkdir(args.out, { recursive: true });
  // Don't leave an earlier run's artifacts beside this one's.
  for (const name of ["report.json", "console.log", "browser.log", "canvas.png", "page.png", "trace.zip", "chrome-trace.json"]) {
    await rm(join(args.out, name), { force: true });
  }

  const started = Date.now();
  const elapsed = () => (Date.now() - started) / 1000;
  const report = {
    fixture: args.fixture ?? null,
    case: args.case,
    requestedBrowser: args.browser,
    requestedBackend: args.backend,
    platform: `${process.platform}-${process.arch}`,
    startupTimeoutSeconds: args.timeout,
    runTimeoutSeconds: args.runTimeout,
    traces: { playwright: args.trace ? "trace.zip" : null, chromium: args.chromeTrace && args.browser === "chromium" ? "chrome-trace.json" : null },
    console: [],
    pageErrors: [],
    requests: [],
  };
  const log = (line) => console.log(`[${elapsed().toFixed(1)}s] ${line}`);
  let outcome;
  // What the run is doing, for a hang's message.
  let step = "starting the browser";

  const finish = async (status, kind, message) => {
    if (report.outcome) return;
    report.outcome = { status, kind, message, step };
    report.elapsedSeconds = elapsed();
    await writeFile(join(args.out, "report.json"), JSON.stringify(report, null, 2) + "\n");
    await writeFile(
      join(args.out, "console.log"),
      report.console.map((m) => `[${m.type}] ${m.text}`).join("\n") + "\n",
    );
    await writeFile(join(args.out, "browser.log"), browserLog.join("\n") + "\n");
    outcome = status;
  };
  const verdict = () => {
    const { kind } = report.outcome;
    const message = report.outcome.message.replace(/\s*\n\s*/g, " ").trim();
    const label = kind === "unsupported" ? "UNSUPPORTED" : outcome === EXIT.pass ? "PASS" : "FAIL";
    console.log(`${label} (${kind}): ${message} [${report.elapsedSeconds.toFixed(1)}s, report in ${args.out}]`);
    if (kind === "browser-hang" || kind === "browser-crash") {
      console.log(`the browser's last output (all of it in ${join(args.out, "browser.log")}):\n${logTail(browserLog)}`);
    }
  };

  // The whole run's deadline. Normally every call below has its own and a
  // run takes seconds; this catches anything that still never returns.
  // Exiting kills the browser (Playwright closes what it launched).
  const stopWatchdog = startWatchdog(args.runTimeout * 1000, async () => {
    await finish(EXIT.app, "browser-hang", `the run did not finish within ${args.runTimeout}s (--run-timeout) while ${step}`);
    verdict();
    process.exit(outcome);
  });

  const server = await serve(root);
  const url = `http://127.0.0.1:${server.address().port}/`;
  let browser;
  let gameContext;
  let chromeTracing = false;
  try {
    const { options } = launchConfig(args.browser, process.platform, args);
    report.launchOptions = options;
    try {
      browser = await LAUNCHERS[args.browser].launch(options);
    } catch (e) {
      // A browser that isn't installed is "unsupported" here; one that is
      // installed and won't start is a failure, never a quiet pass.
      const kind = launchFailureKind(e.message, process.env);
      throw new Failure(kind, `could not launch headless ${args.browser}: ${e.message.split("\n").slice(0, 6).join(" | ")}`);
    }
    report.browser = `${args.browser} ${browser.version()}`;
    log(`${report.browser}, ${report.platform}`);

    // Software GL contexts occasionally fail to start; ask again before
    // calling a backend unsupported.
    step = "probing the browser's WebGPU and WebGL2";
    for (let attempt = 1; ; attempt++) {
      const probeContext = await browser.newContext();
      report.capabilities = await probe(probeContext, url, args.backend);
      await probeContext.close();
      report.probeAttempts = attempt;
      if (backendAvailable(report.capabilities, args.backend) || attempt === 3) break;
      log(`probe ${attempt} lacked the backend: ${JSON.stringify(report.capabilities)}; retrying`);
      await sleep(1000);
    }
    log(`WebGPU adapter: ${JSON.stringify(report.capabilities.webgpu)}; WebGL2: ${JSON.stringify(report.capabilities.webgl2)}`);
    const reason = unsupportedReason(report.capabilities, args.backend, args.browser);
    if (reason) throw new Failure("unsupported", reason);

    const context = await browser.newContext({ viewport: { width: 800, height: 600 } });
    gameContext = context;
    if (args.trace) {
      // Every action, request and (as screenshots) the page over time, for
      // `npx playwright show-trace trace.zip`.
      await context.tracing.start({ screenshots: true, snapshots: false, sources: false, title: `${args.fixture ?? "site"} ${args.browser} ${args.backend}` });
    }
    if (args.backend === "webgl2") {
      // Hide WebGPU so the engine takes its WebGL2 fallback.
      await context.addInitScript(() => {
        Object.defineProperty(Navigator.prototype, "gpu", { get: () => undefined, configurable: true });
      });
    }
    if (args.music === "required") await context.addInitScript(watchAudio);
    if (args.case === "missing-asset") {
      await context.route(`**/assets/${args.asset}`, (route) => route.fulfill({ status: 404, body: "gone" }));
    }
    const page = await context.newPage();
    // A page whose content process died: Playwright's pending calls on it
    // may never return, so fail at once.
    const crashed = new Promise((_, reject) => {
      page.once("crash", () => reject(new Failure("browser-crash", `the page crashed (the browser's content process died) while ${step}`)));
    });
    crashed.catch(() => {});
    /** A browser call with a deadline, failing at once if the page crashes. */
    const call = (promise, ms = CALL_TIMEOUT_MS) => withDeadline(Promise.race([promise, crashed]), ms, step);
    page.on("console", (m) => report.console.push({ type: m.type(), text: m.text(), at: elapsed() }));
    page.on("pageerror", (e) => report.pageErrors.push({ message: String(e.stack ?? e), at: elapsed() }));
    page.on("response", (r) => {
      const path = new URL(r.url()).pathname;
      report.requests.push({ path, status: r.status() });
    });
    page.on("requestfailed", (r) =>
      report.requests.push({ path: new URL(r.url()).pathname, status: "failed", error: r.failure()?.errorText }),
    );

    if (args.chromeTrace && args.browser === "chromium") {
      // Chromium's own trace of the GPU process and compositor frames.
      await browser.startTracing(page, {
        categories: ["-*", "gpu", "viz", "disabled-by-default-gpu.service", "benchmark", "cc"],
        screenshots: false,
      });
      chromeTracing = true;
    }

    const query = args.case === "panic" || args.case === "stall" ? `?smoke_case=${args.case}` : "";
    log(`loading ${url}${query}`);
    step = "loading the page";
    await call(page.goto(url + query));

    // An app failure shows as a panic in the console, an uncaught page
    // error, the page's error banner or a failed asset request.
    const appFailure = (status) => {
      const badAsset = report.requests.find((r) => r.path.includes("/assets/") && r.status !== 200);
      if (badAsset) return new Failure("asset", `asset request ${badAsset.path} answered ${badAsset.status}`);
      const panic = report.console.find((m) => m.text.includes("panicked at"));
      if (panic) return new Failure("panic", panic.text.split("\n").slice(0, 3).join(" | "));
      if (report.pageErrors.length) return new Failure("page-error", report.pageErrors[0].message.split("\n")[0]);
      if (status) return new Failure("page-error", status);
      return null;
    };
    // A page error can arrive a moment before the event that explains it:
    // the answer to a failed asset request, or the panic's console line.
    // Give those a moment, so the failure is named by its cause.
    const settled = async (failure) => {
      if (failure.kind !== "page-error") return failure;
      await sleep(500);
      return appFailure(null) ?? failure;
    };

    // 1. Startup: the first frame publishes the state.
    step = "waiting for the first frame";
    const deadline = Date.now() + args.timeout * 1000;
    let snapshot;
    for (;;) {
      snapshot = await call(smokeState(page));
      const failure = appFailure(snapshot.status);
      if (failure) throw await settled(failure);
      if (snapshot.state && snapshot.state.ticks > 0) break;
      if (Date.now() > deadline) {
        throw new Failure("startup-timeout", `no running frame within ${args.timeout}s (state: ${JSON.stringify(snapshot.state)})`);
      }
      await sleep(100);
    }
    report.startupSeconds = elapsed();
    report.backend = await call(canvasBackend(page));
    log(`started after ${report.startupSeconds.toFixed(1)}s on ${report.backend}: ${JSON.stringify(snapshot.state)}`);
    if (args.backend !== "auto" && report.backend !== args.backend) {
      throw new Failure("backend", `asked for ${args.backend} but the canvas uses ${report.backend}`);
    }

    // 1b. The engine's own account of the GPU it opened (Graphics::gpu_capabilities,
    //     published by the fixture): the adapter, the backend, the limits and
    //     what the renderer falls back on. It must agree with the canvas.
    report.engine = { gpu: snapshot.gpu };
    if (snapshot.gpu) {
      const { adapter, limits } = snapshot.gpu;
      const missing = snapshot.gpu.features.filter((f) => !f.available).map((f) => `${f.feature}: ${f.fallback ?? "-"}`);
      report.engine.summary = { adapter, limits, unavailable: missing };
      log(`engine GPU: ${adapter.backend} ${adapter.device_type} "${adapter.name}"; ${missing.length} features fall back`);
    }
    const mismatch = checkEngineGpu(snapshot.gpu, report.backend, args.engineGpu === "required");
    if (mismatch) throw new Failure("backend", mismatch);

    // 2. Assets: the index and the file were fetched (and a texture decoded).
    for (const path of ["/assets/index.json", `/assets/${args.asset}`]) {
      if (!report.requests.some((r) => r.path === path && r.status === 200)) {
        throw new Failure("asset", `${path} was never fetched`);
      }
    }
    if (args.assetSize !== "none") {
      const [assetWidth, assetHeight] = args.assetSize.split("x").map(Number);
      if (JSON.stringify(snapshot.state.asset) !== JSON.stringify([assetWidth, assetHeight])) {
        throw new Failure("asset", `${args.asset} decoded as ${JSON.stringify(snapshot.state.asset)}, not ${args.assetSize}`);
      }
    }
    // Assets from the game's in-memory asset source load by name in the
    // browser too, and are never fetched (fixtures with `assetSource`).
    if (args.assetSource !== "none") {
      const [sourceWidth, sourceHeight] = args.assetSource.split("x").map(Number);
      if (JSON.stringify(snapshot.state.source) !== JSON.stringify([sourceWidth, sourceHeight])) {
        throw new Failure("asset", `the asset source's image decoded as ${JSON.stringify(snapshot.state.source)}, not ${args.assetSource}`);
      }
      if (snapshot.state.stream !== true) {
        throw new Failure("asset", "streamed music from the asset source did not load");
      }
      const fetchedSource = report.requests.find((r) => r.path.includes("/assets/embedded/"));
      if (fetchedSource) {
        throw new Failure("asset", `${fetchedSource.path} was fetched, but it comes from an asset source`);
      }
    }

    // 3. Progress: the fixed-step simulation keeps ticking. The bound is
    //    coarse, not a frame-rate assertion: 60 Hz for 3 s gives ~180 ticks,
    //    and the bound must stay above STALL_AFTER in examples/web_smoke.rs,
    //    since a `stall` run seen early (at tick 1) still advances that far.
    step = "checking that the simulation keeps running";
    const before = snapshot.state;
    await sleep(3000);
    snapshot = await call(smokeState(page));
    let failure = appFailure(snapshot.status);
    if (failure) throw await settled(failure);
    report.progress = { before, after: snapshot.state };
    log(`after 3s: ${JSON.stringify(snapshot.state)}`);
    if (snapshot.state.frames <= before.frames) {
      throw new Failure("stall", `frames stopped at ${before.frames}`);
    }
    if (snapshot.state.ticks < before.ticks + MIN_PROGRESS_TICKS) {
      throw new Failure("stall", `the simulation stalled: ${before.ticks} -> ${snapshot.state.ticks} ticks in 3s`);
    }

    // 3b. Music asked for at setup waits for the first click: the engine
    //     opens no AudioContext before it (a browser would keep it suspended).
    if (args.music === "required") {
      step = "checking that no audio started before the first click";
      report.audio = { beforeClick: await call(audioSeen(page)) };
      const early = checkMusicBeforeClick(snapshot.state, report.audio.beforeClick);
      if (early) throw new Failure("audio", early);
    }

    // 4. Input: a click on the canvas reaches the game.
    step = "clicking the canvas (a game's first click also opens its audio)";
    const box = await call(page.locator(GAME_CANVAS).boundingBox());
    await call(page.mouse.click(box.x + box.width / 2, box.y + box.height / 2));
    step = "waiting for the game to see the click";
    const clickDeadline = Date.now() + 10_000;
    for (;;) {
      snapshot = await call(smokeState(page));
      failure = appFailure(snapshot.status);
      if (failure) throw await settled(failure);
      if (snapshot.state.clicks >= 1) break;
      if (Date.now() > clickDeadline) throw new Failure("input", "the game never saw the click");
      await sleep(100);
    }
    log(`click seen: ${JSON.stringify(snapshot.state)}`);

    // 4a. The click lets the page make sound, and the music that waited
    //     for it plays.
    if (args.music === "required") {
      step = "waiting for the music after the click";
      const musicDeadline = Date.now() + 10_000;
      for (;;) {
        snapshot = await call(smokeState(page));
        report.audio.afterClick = await call(audioSeen(page));
        const waiting = musicAfterClick(snapshot.state, report.audio.afterClick);
        if (!waiting) break;
        if (Date.now() > musicDeadline) throw new Failure("audio", `the music never played after the click: ${waiting}`);
        await sleep(100);
      }
      log(`music heard after the click: ${JSON.stringify(report.audio.afterClick)}`);
    }

    // 4b. Keys: each --press key reaches the game and changes the state it
    //     reports (a game with screens: Enter starts a round, Escape pauses).
    for (const { key, field, value } of args.presses) {
      step = `pressing ${key}`;
      await call(page.keyboard.press(key));
      const pressDeadline = Date.now() + 10_000;
      for (;;) {
        snapshot = await call(smokeState(page));
        failure = appFailure(snapshot.status);
        if (failure) throw await settled(failure);
        if (String(snapshot.state[field]) === value) break;
        if (Date.now() > pressDeadline) {
          throw new Failure("input", `after ${key}, ${field} is ${JSON.stringify(snapshot.state[field])}, not ${value}`);
        }
        await sleep(100);
      }
      log(`${key} seen: ${field} is ${value}`);
    }

    // 4c. Fields the fixture must report at least (--min): a 3D scene that
    //     loaded but draws no models and no triangles is not running.
    step = "reading the game's final state";
    snapshot = await call(smokeState(page));
    const tooLow = checkMinimums(snapshot.state, args.mins);
    if (tooLow) throw new Failure("state", tooLow);

    // 4d. Storage (--after-reload): once the game says its writes are stored
    //     (`saving` false), reload the page; the new page must find what the
    //     old one saved in the browser's storage (coin_dash's settings).
    if (args.afterReload.length) {
      step = "waiting for the game's writes, then reloading";
      const savedDeadline = Date.now() + 10_000;
      while ((await call(smokeState(page))).state?.saving === true) {
        if (Date.now() > savedDeadline) throw new Failure("storage", "the game's writes never finished");
        await sleep(100);
      }
      await call(page.reload());
      const reloadDeadline = Date.now() + args.timeout * 1000;
      for (;;) {
        snapshot = await call(smokeState(page));
        failure = appFailure(snapshot.status);
        if (failure) throw await settled(failure);
        if (snapshot.state && snapshot.state.ticks > 0) break;
        if (Date.now() > reloadDeadline) throw new Failure("startup-timeout", "no running frame after the reload");
        await sleep(100);
      }
      for (const { field, value } of args.afterReload) {
        if (String(snapshot.state[field]) !== value) {
          throw new Failure("storage", `after the reload, ${field} is ${JSON.stringify(snapshot.state[field])}, not ${value}`);
        }
      }
      log(`after the reload: ${args.afterReload.map(({ field, value }) => `${field} is ${value}`).join(", ")}`);
    }

    // 5. Rendering: the canvas shows the background and the sprites.
    step = "taking the canvas screenshot";
    const png = await call(page.locator(GAME_CANVAS).screenshot());
    await writeFile(join(args.out, "canvas.png"), png);
    const blank = (stats) => stats.colors < 3 || stats.foreground < 0.001;
    report.canvas = { screenshot: await call(canvasStats(page, png)) };
    if (blank(report.canvas.screenshot)) {
      report.canvas.readback = await call(canvasStats(page, null));
    }
    log(`canvas: ${JSON.stringify(report.canvas)}`);
    if (Object.values(report.canvas).every(blank)) {
      if (report.backend === "webgpu" && report.capabilities.webgpu?.canvas !== true) {
        throw new Failure("unsupported", `blank WebGPU canvas, but a plain WebGPU canvas fails here too: ${report.capabilities.webgpu?.canvasProblem ?? "the browser found no WebGPU adapter"}`);
      }
      throw new Failure("blank", `the canvas looks blank: ${JSON.stringify(report.canvas)}`);
    }
    failure = appFailure(null);
    if (failure) throw await settled(failure);
    report.finalState = snapshot.state;

    if (CASES[args.case]) {
      await finish(EXIT.app, "missed", `--case ${args.case} should have failed with "${CASES[args.case]}" but passed`);
    } else {
      await finish(EXIT.pass, "pass", `passed on ${report.backend}`);
    }
  } catch (e) {
    const known = e instanceof Failure || e instanceof BrowserHang;
    const kind = known ? e.kind : "harness";
    const message = known ? e.message : String(e.stack ?? e);
    // A hung or crashed browser can't take a screenshot (it would hang too).
    if (browser?.isConnected() && kind !== "browser-hang" && kind !== "browser-crash") {
      for (const page of browser.contexts().flatMap((c) => c.pages())) {
        await withDeadline(page.screenshot({ path: join(args.out, "page.png") }), CALL_TIMEOUT_MS, "the failure screenshot").catch(() => {});
      }
    }
    if (kind === "unsupported") {
      await finish(args.allowUnsupported ? EXIT.pass : EXIT.unsupported, kind, message);
    } else if (CASES[args.case]) {
      const expected = CASES[args.case] === kind;
      await finish(expected ? EXIT.pass : EXIT.app, expected ? "expected-failure" : kind,
        expected ? `detected the expected ${kind} failure: ${message}` : message);
    } else {
      await finish(EXIT.app, kind, message);
    }
  } finally {
    // Cleanup talks to the browser too, so it gets deadlines as well; the
    // watchdog stays armed until it is done.
    step = "closing the browser";
    if (chromeTracing) {
      const trace = await withDeadline(browser.stopTracing(), CALL_TIMEOUT_MS, "stopping the Chromium trace").catch(() => null);
      if (trace) await writeFile(join(args.out, "chrome-trace.json"), trace);
    }
    if (gameContext && args.trace) {
      await withDeadline(gameContext.tracing.stop({ path: join(args.out, "trace.zip") }), CALL_TIMEOUT_MS, "saving the trace").catch(() => {});
    }
    await withDeadline(browser?.close() ?? Promise.resolve(), CALL_TIMEOUT_MS, "closing the browser").catch(() => {});
    server.close();
  }
  stopWatchdog();
  // Again, now with the browser's exit in it.
  await writeFile(join(args.out, "browser.log"), browserLog.join("\n") + "\n");
  verdict();
  process.exit(outcome);
}

// Run as a program; web/smoke/gallery.mjs imports the helpers above.
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
