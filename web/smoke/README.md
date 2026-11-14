# Browser smoke test

Checks that a web build actually runs in a browser, which compiling to
wasm can't: `smoke.mjs` serves a site built by `web/build.sh`, opens it in
a headless browser (Chromium, Firefox or WebKit; no window) and checks, in
order:

1. **Startup**: the game reaches its first frame (no panic, no page error).
   The engine's own GPU report (`Graphics::gpu_capabilities`) must then agree with
   the canvas: an engine that says `gl` on a canvas with a WebGPU context, or
   the reverse, fails.
2. **Assets**: `assets/index.json` and the fixture's asset were fetched
   (HTTP 200); a texture also decoded at its real size. In the 2D fixture, an
   image and streamed music from the game's in-memory asset source loaded by
   name without being fetched.
3. **Progress**: the fixed-step simulation keeps ticking (at least 60 ticks
   in 3 seconds, a third of the 60 Hz rate; a coarse bound, not a frame-rate
   assertion, kept above the `stall` self-test's 20 ticks).
4. **Input**: a click on the canvas reaches the game, then each `--press` key.
   **Music** (`--music required`, the 2D fixture): music the game started
   during setup waits for that click. Before it, the game reports
   `audio_unlocked: false` and the page has no `AudioContext` (a browser keeps
   one opened before a gesture suspended); within 10 s after it, the game
   reports `audio_unlocked: true`, an `AudioContext` is `running`, and buffers
   with sound in them have been played. The runner watches the page's
   `AudioContext` and `AudioBufferSourceNode.start` to see this.
5. **State minimums** (`--min`): fields the fixture must report at least
   (the 3D fixture's models and triangles drawn).
   **Storage** (`--after-reload`): once the game's writes are stored (its
   `saving` field is no longer `true`), the page is reloaded, and the new page
   must report the given fields (`coin_dash`: the settings it saved when the
   settings screen was left are `Loaded` from the browser's storage, which is
   `persistent`).
6. **Rendering**: the canvas is not blank (screenshot, or a copy of the
   canvas for WebGPU).

## Fixtures

A fixture is an example that publishes its state with the helper
`examples/common/web_smoke.rs`: `<html data-smoke>` every frame (ticks,
frames, clicks, size and whatever the fixture adds) and `<html data-smoke-gpu>`
once (the engine's adapter report as JSON). `fixtures.json` says what the
runner needs to know about each one, so `--fixture <name>` is all a run takes:

| Fixture | Example | Features | Checks |
|---|---|---|---|
| `web_smoke` | `examples/web_smoke.rs` | none | `crab.png` fetched and decoded 32x32; an image and streamed music loaded from an in-memory asset source; the music, started at setup, heard after the first click; sprites drawn |
| `web_smoke_3d` | `examples/web_smoke_3d.rs` | `3d` | `models/house.glb` fetched and parsed; a sun with shadows; at least 1 model and 100 triangles drawn |
| `coin_dash` | `examples/coin_dash` | none | the complete game: its picture, keys and the screens they reach (see [A complete game, packaged](../../book/src/packaging.md#the-web-build)); after a reload, the settings it saved come back from the browser's storage |
| `lights2d` | `examples/lights2d.rs` | none | `crab.png` fetched and decoded 32x32; 2D lights with stencil shadows and normal maps drawing on the page's backend (WebGL2 in CI); at least 7 lights

To add one: publish the state with the helper (or the same attributes by hand),
add an entry to `fixtures.json` (`example`, `features`, `asset`, `assetSize`
or none, `assetSource` or none, `presses`, `min`, `afterReload`, `engineGpu`,
`music`), and name it in the CI job. Every
fixture needs `?smoke_case=panic` and `?smoke_case=stall` handled if it is to
run the self-tests; the helper does that.

## The web gallery

`gallery.mjs` checks the [web gallery](../gallery/README.md) the same way,
for pages whose examples publish no state: every page must start its game,
keep drawing, survive a click and show a canvas that isn't blank, or, for a
demo that needs WebGPU on a page without it, say so instead of starting. The
gallery's README has how to run it; `gallery-lib.test.mjs` tests its
decisions.

## Running it

```sh
cd web/smoke && npm ci
# One browser's download is 100 to 200 MB; here inside this folder:
PLAYWRIGHT_BROWSERS_PATH=$PWD/.browsers node node_modules/playwright-core/cli.js install --only-shell chromium
cd ../..
PLAYWRIGHT_BROWSERS_PATH=$PWD/web/smoke/.browsers \
  node web/smoke/smoke.mjs --fixture web_smoke --build --backend webgl2
```

`--build` runs `web/build.sh` for the fixture (with its Cargo features) into
`target/web/<example>`; without it, build that folder yourself (for the 3D
fixture, `web/build.sh --features 3d web_smoke_3d`), or point `--dir` at any
built site. For the other browsers install `firefox` or `webkit` (and
`chromium`, not `--only-shell`, for `--chromium-mode new`), then pass
`--browser firefox`.

Without `PLAYWRIGHT_BROWSERS_PATH`, Playwright uses its usual cache folder.
Remove `web/smoke/.browsers` and `web/smoke/node_modules` when done; both are
git-ignored. The wasm build is a full extra copy of the dependencies:
`rm -rf target/wasm32-unknown-unknown target/web` afterwards.

`node --test web/smoke/*.test.mjs` runs the runner's own tests (no browser or build).

Options:

| Option | Meaning |
|---|---|
| `--fixture <name>` | A fixture of `fixtures.json`: its example, features, asset, key presses and minimums are the defaults (`--build` builds it first). |
| `--dir <site>` | The built site, instead of `target/web/<example>`. |
| `--browser chromium` / `firefox` / `webkit` | The browser (default `chromium`). |
| `--chromium-mode shell` / `new` | Chromium's headless shell (default) or the full browser in its new headless mode, which has the compositor a WebGPU canvas is presented through. |
| `--backend webgl2` | Hide WebGPU from the page, so the engine takes its WebGL2 fallback. |
| `--backend webgpu` | Require WebGPU; "unsupported" if the browser has no adapter or can't show a WebGPU canvas. |
| `--backend auto` | Let the engine choose; the report says which it used. |
| `--case missing-asset` / `panic` / `stall` | Self-tests: answer 404 for the fixture's asset, panic during setup (`?smoke_case=panic`), or pause the simulation (`?smoke_case=stall`). The run passes only if that failure is detected. |
| `--timeout <s>` | Seconds to wait for the first frame (default 90). |
| `--allow-unsupported` | Exit 0 instead of 2 when the browser lacks the backend (the report and the last line still say UNSUPPORTED). |
| `--asset <file>` / `--asset-size <W>x<H>` / `none` | Without a fixture: the file under `assets/` that must be fetched (default `crab.png`), and the size it must decode to. |
| `--asset-source <W>x<H>` / `none` | The size the image from the game's in-memory asset source must decode to (the state's `source`); its streamed music must load too, and nothing under `assets/embedded/` may be fetched. Default `4x2` without a fixture. |
| `--press <Key>=<field>:<value>` | After the click, press the key (Playwright key name: `Enter`, `Escape`, `Q`) and wait up to 10 s for the reported state's `<field>` to become `<value>`. Repeatable, in order. |
| `--min <field>=<n>` | The reported state's numeric `<field>` must be at least `n`. Repeatable. |
| `--after-reload <field>:<value>` | After the presses, reload the page (once the state's `saving` is not `true`); the new page's `<field>` must then be `<value>`. Repeatable. |
| `--engine-gpu required` | Fail when the fixture publishes no engine GPU report (fixtures set this). |
| `--music required` / `none` | Check that music started at setup waits for the first click and is heard after it (the fixture reports `audio_unlocked`). `required` for fixtures with `"music": true`. |
| `--no-trace` / `--chrome-trace` | Skip the Playwright trace, or also write Chromium's GPU trace (it slows a heavy page a lot, enough to trip the stall check on a 3D scene: use it on the 2D fixture, or to look at one run). |
| `--out <dir>` | Where artifacts go (default `target/web-smoke/<fixture>-<browser>-<backend>-<case>`). |
| `--browser-arg <flag>` | Extra browser flag (repeatable). |

## Results

Exit codes: **0** passed, **1** the game failed (or an installed browser would
not start), **2** unsupported (the browser isn't installed, or lacks the
backend asked for), **3** bad arguments or no build. "Unsupported" is decided
without the engine: WebGPU counts as available only if a plain WebGPU canvas,
cleared to red by the runner itself and read back in the same task, is red.
It is never a pass without a reason: the last line, `report.json` and the CI
summary table all say `UNSUPPORTED` and why (no `navigator.gpu`, no adapter,
no canvas), even under `--allow-unsupported`.

Each run writes to its `--out` folder:

- `report.json`: browser version, platform, launch options, the WebGPU adapter
  and WebGL2 renderer the browser offers, the backend the game's canvas used,
  **the engine's own report** (`engine.gpu`: adapter, limits and every renderer
  feature with its fallback; `engine.summary`), startup time, total elapsed
  time, the state before and after the progress check, canvas statistics, every
  request with its status, console messages, page errors and the outcome
  (`kind`: `pass`, `asset`, `panic`, `page-error`, `stall`, `startup-timeout`,
  `input`, `state`, `blank`, `backend`, `launch`, `unsupported`, ...).
- `console.log`: the page's console.
- `trace.zip`: Playwright's trace (actions, requests, screenshots over time);
  open it with `node node_modules/playwright-core/cli.js show-trace trace.zip`.
- `chrome-trace.json` (`--chrome-trace`, Chromium): the GPU process and
  compositor frames, for chrome://tracing or Perfetto.
- `canvas.png`: the canvas after the click; `page.png`: the page on failure.

`node web/smoke/summary.mjs target/web-smoke` prints every run's report as a
table (and appends it to a GitHub job summary).

## What runs where

| Where | Browser | WebGL2 (forced) | WebGPU |
|---|---|---|---|
| CI (`web-smoke` job, `main` and `full-ci`, ubuntu-latest, no GPU) | Chromium (headless shell and full browser), Firefox, WebKit from Playwright | Must pass in Chromium and WebKit, for the 2D and 3D fixtures (software GL); `coin_dash` in Chromium. Firefox is tried and reported: headless Firefox has no WebGL2 on that runner, so it reads "unsupported" | Tried; passes as "unsupported", with the reason, where the browser can't show a WebGPU canvas; where it can, the game must draw through it |
| CI, every pull request (`web-smoke-runner`) | none | the runner's own tests only (and the gallery's) | |
| Release workflow (`gallery` job, ubuntu-latest, no GPU) | Chromium headless shell | Every gallery page must pass (software GL) | Not tried; the demos that need WebGPU must show their message |
| Apple Silicon Mac, the gallery (`gallery.mjs`) | Chromium 156 | Every page passes in the headless shell; the 2 demos that need WebGPU (compute passes) show their message | Every page passes in new headless mode on the Metal adapter |
| Apple Silicon Mac, Chromium new headless mode | Chromium 156 | Passes (ANGLE on Metal) | **Passes** on the Metal adapter: the engine reports `webgpu`, the canvas draws |
| Apple Silicon Mac, Chromium headless shell | same | Passes (ANGLE on SwiftShader) | Unsupported: a SwiftShader fallback adapter exists, but a WebGPU canvas can't be shown |
| Apple Silicon Mac, WebKit (Playwright build) | WebKit 27 | Passes | Passes on its own adapter |

Not covered yet: Firefox on a Mac with its Playwright build here (it would
not start on the development machine, and on CI's Linux runner it has no
WebGL2, so Firefox has not drawn a frame anywhere yet), WebGPU on a
GPU-less Linux runner (the CI job tries it and reports), and mobile
browsers.
