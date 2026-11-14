# Web gallery

The examples that run in a browser, as one static site: an index page and a
page per example with the demo, what it shows, its controls, whether it needs
WebGPU, and a link to its source at the release. Each release publishes the
gallery of its version to <https://ponkoengine.github.io/ponko/> (see
[Publishing](#publishing)).

```sh
node web/gallery/build.mjs                  # every example -> target/web/gallery/
node web/gallery/build.mjs --only battle,3d # some of them
python3 -m http.server -d target/web/gallery 8000
```

It needs what [`web/build.sh`](../build.sh) needs (the `wasm32-unknown-unknown`
target and `wasm-bindgen-cli` at the version in `Cargo.lock`, see the guide's
[web chapter](../../book/src/web.md)) and Node. Options:

| Option | Meaning |
|---|---|
| `--out <dir>` | Where the site goes (default `target/web/gallery`). |
| `--ref <tag or branch>` | What the source links point at, and the version the pages name (`v0.2.0` reads "Ponko 0.2.0"). Default `main`. |
| `--only <a,b>` | Build only these examples. |
| `--pages-only` | Rewrite the pages without building (after editing `examples.json` or the page code). |

The site is about 15 MB per example uncompressed (the game's `.wasm` and a
copy of `assets/`); 30 examples come to about 400 MB.

## Adding an example: one line

Add a line to [`examples.json`](examples.json), where the index should show it:

```json
  {"example": "my_example", "title": "My example", "kind": "2d", "webgpu": "optional", "shows": "What it demonstrates, in a sentence.", "controls": "Keys and mouse."},
```

| Field | Meaning |
|---|---|
| `example` | The Cargo example: `examples/<name>.rs` or `examples/<name>/main.rs`. |
| `title` | The name on the index and the page. |
| `kind` | `2d` or `3d`: the index groups by it. |
| `features` | The Cargo features it is built with (optional). At least its `required-features` from `Cargo.toml`; the build and the tests refuse fewer. |
| `webgpu` | `optional`: runs with WebGPU, or WebGL2 where the browser has no WebGPU. `required`: needs WebGPU; without it the page says so and what to try, instead of starting the game. |
| `shows` | What it demonstrates, one or two sentences. |
| `controls` | How to play with it. |

Then build it (`--only my_example`) and check it in a headless browser (below).
`node --test web/gallery` checks the manifest: every example exists, asks for
its required features, and sits on one line.

### Which examples are in, and why the rest aren't

Every example with a window that shows something on its own or with a
mouse and keyboard is listed. Left out: those without a picture (`headless`,
`ecs`, `action_map`, `net_messages`, `net_replication`, `navmesh3d`,
`ffi_bench`; `audio3d` is a sound with an empty scene; `window` draws
nothing), those that need something a gallery page doesn't give them
(`lockstep`: a second player over the network; `hotpatch`: a running `dx`;
`scene_viewer`: a scene file named in the environment; `gamepad`: a
controller, which the headless check can't plug in), the smoke-test fixtures
(`web_smoke`, `web_smoke_3d`) and `bunnymark` (the same picture as
`bunnymark_ecs`, which is listed).

`webgpu: required` is set where WebGL2 can't show the demo:
`compute` and `scene_compute` are about compute passes, which WebGL2 lacks.
The demos that read the scene depth (`terrain`, `shaders`, `target_effect`)
run on WebGL2 through the renderer's GL path for it (see the book's shader
section), and CI checks them there.

## The pages

[`lib.mjs`](lib.mjs) writes the pages, [`gallery.css`](gallery.css) styles
them and [`play.js`](play.js) starts the demo on each. The demo draws into the
page's `<canvas id="ponko">` (the engine uses it, as in `web/index.html`),
16:10 in the page, or the whole window with "Fill window" or `?full` in the
address. Before loading the game, `play.js` asks the browser for a WebGPU
adapter: a `required` demo without one shows the reason and stops there; an
`optional` one says it draws with WebGL2. It reports how the page went in
`<html data-gallery>` (`loading`, `running`, `unsupported`, `error`) for the
check below.

## Checking it in a browser

[`web/smoke/gallery.mjs`](../smoke/gallery.mjs) opens the built gallery in a
headless browser (no window): the index must link every example, and each
page must start its game, keep drawing (draw calls or WebGPU submissions go
on), survive a click and show a canvas that isn't blank, with no panic, page
error or failed request; a `required` demo without WebGPU must show its
message instead. With the runner set up as in
[`web/smoke/README.md`](../smoke/README.md):

```sh
node web/smoke/gallery.mjs --backend webgl2                    # WebGPU hidden: the fallback path
node web/smoke/gallery.mjs --backend webgpu --chromium-mode new  # WebGPU (a Mac's Metal adapter)
node web/smoke/gallery.mjs --only battle,3d --browser webkit
```

Each page leaves `report.json`, `canvas.png` and `console.log` in
`target/web-smoke/gallery-<browser>-<backend>/<example>/`, and
`node web/smoke/summary.mjs target/web-smoke` tables them. Exit codes as for
`smoke.mjs`: 0 passed, 1 a page failed, 2 the browser lacks the backend asked
for, 3 bad arguments or no build. The headless browser can't lock the
pointer, so the check makes `requestPointerLock` succeed without locking.

## Publishing

The release workflow (`.github/workflows/release.yml`) builds the gallery for
the tag (`--ref vX.Y.Z`) and checks every page in headless Chromium on WebGL2
(job `gallery`, which publishes nothing). After the release is published, job
`gallery-pages` deploys it to the repository's GitHub Pages with the built-in
`GITHUB_TOKEN`, for a release (not a candidate) that is the newest version, so a
patch release of an older minor doesn't replace the newer gallery. GitHub
Pages must be set up once by a maintainer (CONTRIBUTING.md, "One-time
setup"); until then the job says so and skips the deployment.
