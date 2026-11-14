// Tests for the browser-free parts of the runner: `node --test web/smoke`.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  BrowserHang,
  UsageError,
  canvasBackendOf,
  checkEngineGpu,
  checkMinimums,
  checkMusicBeforeClick,
  launchConfig,
  launchFailureKind,
  logTail,
  musicAfterClick,
  parseArgs,
  parseFixtures,
  parseExpect,
  parseMin,
  parsePress,
  startWatchdog,
  unsupportedReason,
  withDeadline,
} from "./lib.mjs";

const fixtures = parseFixtures(readFileSync(new URL("./fixtures.json", import.meta.url), "utf8"));
const parse = (...argv) => parseArgs(argv, fixtures);

test("a fixture supplies its example, features, asset and expectations", () => {
  const args = parse("--fixture", "web_smoke_3d");
  assert.equal(args.example, "web_smoke_3d");
  assert.deepEqual(args.features, ["3d"]);
  assert.equal(args.asset, "models/house.glb");
  assert.equal(args.assetSize, "none", "a model has no decoded size to compare");
  assert.equal(args.assetSource, "none", "only the 2D fixture loads from an in-memory asset source");
  assert.deepEqual(args.mins, [
    { field: "models_drawn", min: 1 },
    { field: "triangles", min: 100 },
  ]);
  assert.equal(args.engineGpu, "required");
  assert.match(args.dir, /target.web.web_smoke_3d$/);
});

test("flags on the command line win over the fixture", () => {
  const args = parse("--fixture", "coin_dash", "--asset", "other.png", "--asset-size", "8x8", "--press", "Space=screen:Playing", "--min", "x=2", "--dir", "site");
  assert.equal(args.asset, "other.png");
  assert.equal(args.assetSize, "8x8");
  assert.deepEqual(args.presses, [{ key: "Space", field: "screen", value: "Playing" }]);
  assert.deepEqual(args.mins, [{ field: "x", min: 2 }]);
  assert.equal(args.dir, "site");
});

test("coin_dash keeps its screens walk-through in the fixture", () => {
  const args = parse("--fixture", "coin_dash");
  assert.deepEqual(args.presses.map((p) => `${p.key}=${p.field}:${p.value}`), [
    "Enter=screen:Playing",
    "Escape=screen:Paused",
    "Q=screen:Title",
    "O=screen:Settings",
    "Escape=screen:Title",
  ]);
  assert.deepEqual(args.features, []);
});

test("without a fixture the defaults are the 2D fixture's", () => {
  const args = parse("--dir", "site");
  assert.equal(args.browser, "chromium");
  assert.equal(args.backend, "auto");
  assert.equal(args.asset, "crab.png");
  assert.equal(args.assetSize, "32x32");
  assert.equal(args.assetSource, "4x2");
  assert.equal(args.engineGpu, "optional");
  assert.equal(args.trace, true);
});

test("the 2D fixture expects its in-memory asset source; a flag can change or drop it", () => {
  assert.equal(parse("--fixture", "web_smoke").assetSource, "4x2");
  assert.equal(parse("--fixture", "coin_dash").assetSource, "none");
  assert.equal(parse("--fixture", "web_smoke", "--asset-source", "none").assetSource, "none");
  assert.throws(() => parse("--fixture", "web_smoke", "--asset-source", "big"), /--asset-source must look like/);
});

test("artifacts go to a folder named by fixture, browser, backend and case", () => {
  const args = parse("--fixture", "web_smoke", "--browser", "firefox", "--backend", "webgl2", "--case", "stall");
  assert.match(args.out, /web_smoke-firefox-webgl2-stall$/);
  const full = parse("--fixture", "web_smoke", "--chromium-mode", "new", "--backend", "webgpu");
  assert.match(full.out, /web_smoke-chromium-new-webgpu-ok$/, "the two Chromium modes don't overwrite each other");
});

test("bad arguments are usage errors that name the problem", () => {
  const bad = (message, ...argv) => assert.throws(() => parse(...argv), (e) => e instanceof UsageError && message.test(e.message));
  bad(/--dir or --fixture/);
  bad(/--fixture must be one of/, "--fixture", "nope");
  bad(/--browser must be/, "--dir", "d", "--browser", "ie");
  bad(/--backend must be/, "--dir", "d", "--backend", "vulkan");
  bad(/--case must be/, "--dir", "d", "--case", "boom");
  bad(/--build needs --fixture/, "--dir", "d", "--build");
  bad(/--asset-size/, "--dir", "d", "--asset-size", "big");
  bad(/--press wants/, "--dir", "d", "--press", "Enter");
  bad(/--min wants/, "--dir", "d", "--min", "x");
  bad(/unknown argument/, "--dir", "d", "--frobnicate");
  bad(/needs a value/, "--dir");
});

test("fixtures.json rejects unknown fields and bad presses", () => {
  assert.throws(() => parseFixtures('{"a":{"example":"a","typo":1}}'), /unknown field "typo"/);
  assert.throws(() => parseFixtures('{"a":{}}'), /needs an "example"/);
  assert.throws(() => parseFixtures('{"a":{"example":"a","presses":["Enter"]}}'), /--press wants/);
});

test("presses and minimums parse", () => {
  assert.deepEqual(parsePress("Enter=screen:Playing"), { key: "Enter", field: "screen", value: "Playing" });
  assert.deepEqual(parseMin("triangles=100"), { field: "triangles", min: 100 });
  assert.deepEqual(parseExpect("settings_at_start:Loaded"), { field: "settings_at_start", value: "Loaded" });
  assert.throws(() => parseExpect("Loaded"), /--after-reload wants/);
  assert.throws(() => parseFixtures('{"a":{"example":"a","afterReload":["x"]}}'), /--after-reload wants/);
});

test("the engine's backend names map to the canvas's context", () => {
  assert.equal(canvasBackendOf("gl"), "webgl2");
  assert.equal(canvasBackendOf("webgpu"), "webgpu");
  assert.equal(canvasBackendOf("vulkan"), null);
});

const gpu = (backend) => ({ adapter: { backend } });

test("the engine's adapter must agree with the canvas", () => {
  assert.equal(checkEngineGpu(gpu("gl"), "webgl2", true), null);
  assert.equal(checkEngineGpu(gpu("webgpu"), "webgpu", true), null);
  assert.match(checkEngineGpu(gpu("gl"), "webgpu", true), /engine says gl .*webgpu context/);
  assert.match(checkEngineGpu(gpu("metal"), "webgl2", true), /not a browser backend/);
  assert.equal(checkEngineGpu(gpu("gl"), "unknown", true), null, "an unknown canvas can't contradict it");
});

test("a missing engine report fails only where the fixture promised one", () => {
  assert.match(checkEngineGpu(null, "webgl2", true), /published no GPU report/);
  assert.equal(checkEngineGpu(null, "webgl2", false), null);
});

test("minimum fields must be numbers at or above the bound", () => {
  const mins = [{ field: "models_drawn", min: 1 }];
  assert.equal(checkMinimums({ models_drawn: 8 }, mins), null);
  assert.match(checkMinimums({ models_drawn: 0 }, mins), /models_drawn is 0, below the required 1/);
  assert.match(checkMinimums({}, mins), /models_drawn is undefined/);
  assert.match(checkMinimums({ models_drawn: "8" }, mins), /below the required/);
});

test("a browser without WebGPU is unsupported, with the reason, never a pass", () => {
  const noGpu = { webgpu: null, webgl2: { vendor: "v", renderer: "r" }, hasNavigatorGpu: false };
  assert.match(unsupportedReason(noGpu, "webgpu", "webkit"), /headless webkit has no WebGPU \(navigator\.gpu is missing\)/);
  const noAdapter = { webgpu: null, webgl2: {}, hasNavigatorGpu: true };
  assert.match(unsupportedReason(noAdapter, "webgpu"), /no WebGPU adapter/);
  const threw = { webgpu: null, webgpuError: "TypeError: boom", webgl2: {}, hasNavigatorGpu: true };
  assert.match(unsupportedReason(threw, "webgpu"), /threw while asking for a WebGPU adapter: TypeError: boom/);
  const noCanvas = { webgpu: { canvas: false, canvasProblem: "read back rgb(0, 0, 0)" }, webgl2: {}, hasNavigatorGpu: true };
  assert.match(unsupportedReason(noCanvas, "webgpu"), /can't show a WebGPU canvas here: read back rgb\(0, 0, 0\)/);
  const works = { webgpu: { canvas: true }, webgl2: {}, hasNavigatorGpu: true };
  assert.equal(unsupportedReason(works, "webgpu"), null);
});

test("WebGL2 and auto need what they need", () => {
  const none = { webgpu: null, webgl2: null, hasNavigatorGpu: false };
  assert.match(unsupportedReason(none, "webgl2"), /no WebGL2/);
  assert.match(unsupportedReason(none, "auto"), /neither WebGPU nor WebGL2/);
  assert.equal(unsupportedReason({ ...none, webgl2: {} }, "auto"), null);
});

test("launch options per browser", () => {
  const chromium = launchConfig("chromium", "linux", { browserArgs: ["--x"], chromiumMode: "shell" }).options;
  assert.deepEqual(chromium.args, ["--enable-unsafe-swiftshader", "--enable-unsafe-webgpu", "--enable-features=Vulkan", "--x"]);
  assert.equal(chromium.channel, undefined);
  const mac = launchConfig("chromium", "darwin", { chromiumMode: "new" }).options;
  assert.equal(mac.channel, "chromium", "the new headless mode is the full browser");
  assert.ok(!mac.args.includes("--enable-features=Vulkan"));
  assert.equal(launchConfig("firefox", "linux").options.firefoxUserPrefs["dom.webgpu.enabled"], true);
  assert.equal(launchConfig("webkit", "linux").options.headless, true);
});

test("a browser that isn't installed is unsupported on a laptop and a failure in CI", () => {
  const missing = "browserType.launch: Executable doesn't exist at /x/webkit-1/pw_run.sh";
  assert.equal(launchFailureKind(missing, {}), "unsupported");
  assert.equal(launchFailureKind(missing, { CI: "" }), "unsupported");
  assert.equal(launchFailureKind(missing, { CI: "false" }), "unsupported");
  assert.equal(launchFailureKind(missing, { CI: "true" }), "launch", "the CI job installs its browsers: missing is broken");
  assert.equal(launchFailureKind("Failed to launch the browser process", {}), "launch");
  assert.equal(launchFailureKind("Failed to launch the browser process", { CI: "true" }), "launch");
});

test("the 2D fixture checks its music; others and a flag can leave it out", () => {
  assert.equal(parse("--fixture", "web_smoke").music, "required");
  assert.equal(parse("--fixture", "web_smoke_3d").music, "none");
  assert.equal(parse("--fixture", "web_smoke", "--music", "none").music, "none");
  assert.equal(parse("--dir", "site").music, "none");
  assert.throws(() => parse("--fixture", "web_smoke", "--music", "loud"), /--music must be required or none/);
});

test("music must wait for the first click without opening an AudioContext", () => {
  const quiet = { contexts: [], audible: 0 };
  assert.equal(checkMusicBeforeClick({ audio_unlocked: false }, quiet), null);
  assert.match(checkMusicBeforeClick({ audio_unlocked: true }, quiet), /audio_unlocked true/);
  assert.match(checkMusicBeforeClick({}, quiet), /audio_unlocked undefined/);
  assert.match(checkMusicBeforeClick({ audio_unlocked: false }, { contexts: ["suspended"], audible: 0 }), /before the first click/);
});

test("after the click the music is heard in a running AudioContext", () => {
  const unlocked = { audio_unlocked: true };
  assert.equal(musicAfterClick(unlocked, { contexts: ["running"], audible: 3 }), null);
  assert.match(musicAfterClick({ audio_unlocked: false }, { contexts: ["running"], audible: 3 }), /audio_unlocked false/);
  assert.match(musicAfterClick(unlocked, { contexts: ["suspended"], audible: 3 }), /no running AudioContext/);
  assert.match(musicAfterClick(unlocked, { contexts: ["running"], audible: 0 }), /no buffer with sound/);
});

test("a browser call that never returns fails as browser-hang within its deadline", async () => {
  const never = new Promise(() => {});
  const started = Date.now();
  await assert.rejects(withDeadline(never, 200, "clicking the canvas"), (e) => {
    assert.ok(e instanceof BrowserHang);
    assert.equal(e.kind, "browser-hang");
    assert.match(e.message, /clicking the canvas did not return within 0.2s/);
    return true;
  });
  const waited = Date.now() - started;
  assert.ok(waited >= 150 && waited < 2000, `failed after ${waited} ms, not at the 200 ms deadline`);
});

test("a browser call within its deadline keeps its value and its own error", async () => {
  assert.equal(await withDeadline(Promise.resolve(7), 1000, "x"), 7);
  await assert.rejects(withDeadline(Promise.reject(new Error("page closed")), 1000, "x"), /page closed/);
});

test("the run's watchdog fires once at its deadline, and not after it is stopped", async () => {
  let fired = 0;
  startWatchdog(100, () => fired++);
  const stop = startWatchdog(100, () => fired++);
  stop();
  await new Promise((ok) => setTimeout(ok, 300));
  assert.equal(fired, 1);
});

test("a run's deadline is an argument, positive, and covers more than startup", () => {
  assert.equal(parse("--fixture", "web_smoke").runTimeout, 300);
  assert.ok(parse("--fixture", "web_smoke").runTimeout > parse("--fixture", "web_smoke").timeout * 2);
  assert.equal(parse("--fixture", "web_smoke", "--run-timeout", "120").runTimeout, 120);
  assert.throws(() => parse("--fixture", "web_smoke", "--run-timeout", "0"), UsageError);
});

test("a hang's message ends with the browser's last lines", () => {
  const lines = Array.from({ length: 50 }, (_, i) => `line ${i}`);
  const tail = logTail(lines, 3).split("\n");
  assert.deepEqual(tail, ["  | line 47", "  | line 48", "  | line 49"]);
});
