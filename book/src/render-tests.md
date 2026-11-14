# GPU regression tests

The 3D renderer is protected by offscreen GPU tests: each draws a small,
controlled scene into a texture, reads the pixels back and checks them. They
need no window and no asset files, and they run wherever a GPU adapter
exists. This chapter is for contributors who change the renderer: how to run
the tests, how to read a failure on your hardware, and how a reference image
is changed.

The suites are the `src/render/*_render_test.rs` files. Their shared helpers
are in `src/render/render_test_support.rs`; the scenes of the regression
suites are in `src/render/regression_stage.rs`.

These tests are in Rust only; C and C++ games draw with the same renderer, so
they are covered by the same tests.

## Running them

```sh
# Every offscreen suite (about five seconds on a desktop GPU):
cargo test -p ponko --lib --features 3d render_test

# One suite, with what it measured:
cargo test -p ponko --lib --features 3d regression_render_test -- --nocapture
```

`cargo test --workspace` runs them too.

### A skip is not a pass

A test that can't run returns early and logs why. Nothing was checked, so
before you rely on a green run, look at what ran:

```sh
cargo test -p ponko --lib --features 3d render_test -- --nocapture 2>&1 | grep "render test"
```

```text
render test ran [render::regression_render_test::a_box_shadows_the_floor_on_the_far_side_from_the_sun] on Apple M4 Max (Metal, IntegratedGpu, driver unknown)
render test SKIPPED [render::regression_render_test::a_mirror_floor_reflects_the_glowing_box]: needs ScreenSpaceReflections, which the forced webgl2 tier lacks
```

There are two kinds of skip:

| Skip | When | How to make it a failure |
|---|---|---|
| No adapter or device | A machine or CI runner without a GPU | `PONKO_REQUIRE_GPU=1` |
| A capability is missing | The adapter (or the forced tier) has no compute shaders, too few texture slots, no 2x multisampling | It stays a skip: some adapters really lack the feature. The fallback has its own test |

Set `PONKO_REQUIRE_GPU=1` on any machine that has a GPU, so that a broken
driver or a missing adapter can't pass silently.

### GPU tiers

The engine has three capability tiers (`desktop`, `baseline`, `webgl2`).
`PONKO_GPU_TIER` runs every suite on a device opened with a lower tier's
limits, which is how the fallbacks are tested without other hardware:

```sh
PONKO_GPU_TIER=baseline cargo test -p ponko --lib --features 3d render_test
PONKO_GPU_TIER=webgl2   cargo test -p ponko --lib --features 3d render_test
```

Run both before a pull request that touches the renderer. This forces
*limits* on your adapter; it doesn't run the WebGL2 backend itself.

## What is covered

| Area | Suites |
|---|---|
| Opaque, transparency, alpha cutout, shadows, skinned meshes, reflections, ambient occlusion, global illumination, post-processing, output formats: one scene each, with a reference image | `regression_render_test` |
| Sequences: camera cuts by rotation, teleports, resizes, forced tiers and their fallbacks, shader hot reload | `regression_sequence_render_test` |
| Temporal antialiasing: convergence, motion vectors, ghosting, history resets | `taa_render_test` |
| Motion vectors of skinned and morphing models: bending limbs, morph targets, the CPU fallback, teleports, back into view, a swing under TAA | `skin_motion_render_test` |
| Screen-space effects | `ssr_render_test`, `ao_aa_render_test`, `contact_render_test`, `shafts_render_test`, `decal_render_test` |
| Lighting | `gi_render_test`, `environment_render_test`, `light_shadow_render_test`, `atlas_render_test`, `shadow_cache_render_test`, `effect_shadow_render_test` |
| Geometry and draw paths | `static_batch_render_test`, `buffer_render_test`, `draw_order_render_test`, `lod_render_test`, `occlusion_render_test`, `far_terrain_render_test`, `skin_render_test`, `single_sample_render_test`, `coverage_render_test` |
| Materials and shaders | `material_render_test`, `model_material_render_test`, `variant_render_test`, `shader_reload_render_test`, `target_effect_render_test`, `post_links_render_test` |
| Capabilities and quality presets | `capabilities_render_test`, `quality_render_test` |
| Resources: uploads, handles, lifetimes, GPU memory | `upload_render_test`, `handle_render_test`, `asset_lifetime_render_test`, `residency_render_test` |
| Text and UI | `text3d_render_test`, `ui_render_test` |
| The rendering baseline's scenes draw and report on any device | `gpu3d::scene_bench::render_test` |

Not covered, so say so when a change touches it: no test runs on a second
*hardware* GPU (CI's adapter is a software rasteriser), none runs the real
WebGL2 or browser WebGPU backends (the web smoke test starts a game there
without comparing pixels). Motion vectors of skinned and morphing models
are tested on one character-like bar; a real imported character is not.

## Writing a test

- **Open the device with `render_test_support::gpu` or `gpu_with`**, never
  with `request_adapter`: the helpers log the adapter, apply
  `PONKO_GPU_TIER` and honour `PONKO_REQUIRE_GPU`. This holds for a GPU
  test anywhere, also one in a `mod tests` beside the code it checks; a
  test reads every source file and fails on a device opened another way. A test that needs a feature calls `needs(&device, feature)`
  and returns when it says no.
- **Assert what must hold on any GPU first.** "The floor behind the box is
  in shadow", "the near box hides the far one", "the reflection is red".
  Give each a negative control that shows the assertion can fail (shadows
  off: the floor is evenly lit). These assertions name the broken behaviour
  when they fail; a reference image only says that something changed.
- **Compare images with a tolerance**, not byte equality: `diff` and
  `Tolerance` (a per-channel slack plus the share of pixels allowed beyond
  it). Two frames from one device may be compared tightly; frames from two
  adapters may not.
- **Print the numbers** with `eprintln!`. A failure on hardware you don't
  have is diagnosed from the log.
- **End with `stage.finish()`** in the regression suites: the stage records
  every validation error the device reports, and a frame that drew with one
  fails.
- Build scenes from engine primitives (boxes, planes, WGSL in the test).
  No asset files: the repository is public and the tests must run from a
  bare checkout.

## Reference images

A reference image is a whole-frame baseline: a small PNG in
`tests/fixtures/render/<name>.png` with its metadata in `<name>.json`.

```json
{
  "name": "regression_shadows",
  "scene": "A white box (1 x 1.5 x 1) on a grey floor, white sun at 45 degrees ...",
  "width": 128,
  "height": 128,
  "format": "rgba8-srgb",
  "tolerance": { "channel": 12, "outliers": 0.0075, "edge": 1 },
  "adapter": "Apple M4 Max",
  "backend": "Metal",
  "driver": "unknown",
  "checksum": "…",
  "reason": "New reference: the image regression suite's first baselines (#135)."
}
```

`check_reference(name, &image, tolerance, scene, &adapter)` compares a frame
with it. The comparison fails when:

- more than `outliers` of the pixels differ by more than `channel` levels in
  any channel, after the allowance for edges (below);
- the tolerance or the scene description in the code is not the one in the
  metadata (so loosening a tolerance is a visible change, not a quiet one);
- the PNG is not the file the metadata was made with (its checksum);
- the reference or its metadata is missing.

On a failure the frame and a difference image (red where it is beyond the
tolerance) are written to `target/render-test-output/<name>.actual.png` and
`<name>.diff.png`, and the message names the adapter that made the reference
and the one that drew the frame.

Every comparison prints what it measured, passed or failed, and appends it
to `target/render-test-output/comparisons.log`:

```text
reference regression_shadows: max channel difference 3 (0 off edges), mean 0.021, 0 of 16384 pixels (0.00%) beyond the tolerance, 4.2% of pixels on edges; allowed Tolerance { channel: 12, outliers: 0.0075, edge: 1 } (made on ..., Metal; drawn on ..., Dx12)
```

That line is the margin on the adapter that drew the frame: how far the
largest difference is from `channel` once edges are allowed for, and how
much of the outlier budget is used. Read it after the first run on new
hardware, also when everything passed.

`PONKO_RENDER_TEST_IMAGES=1` also keeps every image a regression test read
back (`target/render-test-output/<test>.<n>.png`), which is the quickest way
to see what a failing semantic assertion looked at.

### What the tolerance allows, and why

Two adapters draw the same scene with the same shades in flat areas and
differ along edges: a silhouette or a shadow's border lands up to a pixel
apart, multisampled edges resolve to other in-between shades, and shadow
depth bias moves a shadow's border a little. Along an edge that difference
is the edge's whole contrast, far beyond any `channel`. Counting those
pixels as outliers needs a budget as large as the scene's share of edge
pixels (4% to 27% in the regression scenes), and a budget that large also
lets a real regression through: measured against it, "ambient occlusion
off", "reflections off" and "the skinned bar not posed" all passed as their
references.

So edges are allowed for by position, not by count. With `edge: 1` a pixel
is within the tolerance when each channel lies within `channel` of the
range spanned by the other image's pixels up to one pixel away, and the
same holds the other way round (a thin line missing from either image still
counts). An edge a pixel apart, or resolved to another shade between its
two sides, passes; an edge two pixels apart, a changed shade in a flat area
or a missing object doesn't. `outliers` then only pays for what is not an
edge, and is small: 0.75% of the frame for lit scenes (`REFERENCE`), 1.5%
with 16 levels for screen-space effects, global illumination and the post
chain (`EFFECT_REFERENCE`), whose shades come out of longer filter chains.

Each reference has **negative controls**: frames of the same scene with
what the reference protects taken away, which must fail the comparison by
at least twice the budget (`reference_rejects`). Measured on the adapter
the references were made on, as the share of pixels beyond the tolerance:

| Reference | Budget | Controls that must fail |
|---|---|---|
| `regression_opaque` | 0.75% | sun off 68%, camera 0.15 to the side 3.7% |
| `regression_transparency` | 0.75% | one pane missing 9.8%, camera 0.15 to the side 5.4% |
| `regression_cutout` | 0.75% | stripes instead of holes 12%, shadows off 6.2%, camera 0.15 to the side 6.1% |
| `regression_shadows` | 0.75% | box moved 23%, shadows off 1.7%, camera 0.15 to the side 1.5% |
| `regression_skinned` | 0.75% | bar not posed 2.9%, bent 0.5 radians less 1.7% |
| `regression_ssr` | 1.5% | reflections off 3.2% |
| `regression_ao` | 1.5% | ambient occlusion off 3.4% |
| `regression_gi` | 1.5% | global illumination off 87%, frame 8 instead of 80 76% |
| `regression_post` | 1.5% | bloom off 59%, another tone curve 33% |

The budgets are set from this table (half of the mildest control or less),
not from a run on a second adapter: none was available when they were
made. They may turn out too tight for an adapter whose shadow filtering or
effect noise differs more; the margin lines of its first run say by how
much, and that is decided in review (see the table of failures below).

A reference image protects only what is large enough in it. The skinned
bar's shadow is some 70 pixels, under the budget: "shadows off" passes as
`regression_skinned`, so that claim is carried by the test's assertion
("the shadow bent with the bar"), not by the image. This is the general
rule: the semantic assertions carry correctness, the reference catches what
nobody thought to assert.

### Changing a reference

Nothing writes to `tests/fixtures/render` unless you ask:

```sh
PONKO_UPDATE_REFERENCES=1 \
PONKO_REFERENCE_REASON="Sun shadows use the new filter kernel (#1234)." \
  cargo test -p ponko --lib --features 3d regression_render_test
```

1. The run rewrites only the references that no longer match. One that is
   still within its tolerance is left alone, so an update run never churns
   images you didn't change.
2. For each rewritten reference it saves `<name>.before.png`,
   `<name>.after.png` and `<name>.diff.png` in `target/render-test-output/`
   and prints the difference in numbers. **Look at them.** If more changed
   than you meant to change, you have found a regression, not a new
   baseline.
3. The reason goes into the metadata. An update without
   `PONKO_REFERENCE_REASON`, or with `CI` set, is refused.
4. Run the tests again without the variables, and under `PONKO_GPU_TIER=
   baseline` and `webgl2`.
5. Commit the PNG and the JSON together and put the before and after images
   in the pull request's description.

On the pull request, the **Reference images** job checks every reference
against its metadata, lists the changed ones in its summary and uploads
their before and after images as the `reference-image-evidence` artifact
(open its `index.html`). The job fails, and holds the merge, until a
maintainer has looked at that artifact and added the `references-reviewed`
label; re-running the job then passes. A changed image with an unchanged
reason, or a changed image with unchanged metadata, fails outright.

The label stands for the images the maintainer saw:

- A later push that changes the references again takes the label off, and
  the job fails until the new images are reviewed. A push that leaves the
  references alone keeps it.
- A label put on when the pull request is opened, before any evidence
  exists, doesn't count either.
- The full run that the `full-ci` label starts on the branch asks for the
  same label: it looks up the branch's pull request. Only `main`, where
  the change has merged, is checked against the metadata alone.

To check the same things locally:

```sh
python3 scripts/check-reference-images.py
python3 scripts/check-reference-images.py --changed origin/main --evidence target/reference-evidence
```

Prefer a semantic assertion to a new reference image. Add a reference when
the whole picture is the contract, keep it at 128x128 or smaller, and never
raise a tolerance to make a failure go away: the script rejects tolerances
that compare nothing (more than 32 levels, 15% of the pixels, or edges more
than 2 pixels apart), and a new reference gets a negative control.

## What a failure means on your hardware

The references were made on one adapter (Apple Metal). Rasterisers differ in
how they resolve multisampled edges, filter shadow maps and round, so a
frame from another adapter is a few levels off in flat areas and different
along edges. The tolerances allow for that (see "What the tolerance allows,
and why"). Read a failure by its kind:

| What fails | What it means | What to do |
|---|---|---|
| A semantic assertion ("the shadow is dark") | The behaviour is broken on this adapter, whatever the reference says | A real regression or a driver bug. Reproduce with `PONKO_RENDER_TEST_IMAGES=1`, attach the image and the `render test ran` line |
| A reference, with a percent or two of pixels beyond the tolerance, as thin lines along edges or shadow borders in the diff image, the semantic assertions passing | Rasterisation or shadow bias differs on this adapter by more than a pixel | Not a regression in itself. Report the adapter, driver and the comparison's line; the fix is a wider tolerance or a second look at the scene, decided in review with the negative controls still failing, never a regenerated image from the failing machine alone |
| `... is not at least twice beyond the tolerance of the reference` | A negative control no longer fails its reference clearly: the tolerance was loosened too far, or the scene changed so the reference no longer shows the effect | Tighten the tolerance or make the effect larger in the scene |
| A reference, with a region or the whole frame different | The picture changed | A regression, unless your change means it: then update the reference with the workflow above |
| A reference on every adapter after your change, semantic assertions still passing | Your change moved pixels without breaking the behaviour (a new filter, a new tone curve) | Update the references, with the before and after images |
| Frames of one device compared with each other ("a resize against a fresh renderer") | State leaked between frames: history, a cached shadow, a target of the old size | A regression: these comparisons are on one adapter and don't depend on the hardware |
| A tier comparison ("the opaque scene on the webgl2 tier against the desktop device") | A fallback path draws something else than the main path | A regression in the fallback |
| `the GPU reported errors` from `stage.finish()` | A validation error: a binding, format or limit the device rejects | A regression, usually on a lower tier. The message is wgpu's |
| `no GPU adapter ..., and PONKO_REQUIRE_GPU is set` | The machine was expected to have a GPU and doesn't | Fix the runner or driver, not the test |
| Only under `PONKO_GPU_TIER=webgl2` or `baseline` | The lower tier's path is broken or the test assumes a feature | Guard the test with `needs`, or fix the fallback |

When you report a failure from hardware the maintainers don't have, include
the `render test ran` line (adapter, backend, driver), the test's printed
numbers, and the `actual` and `diff` images.

A passing run proves what it ran on. A pull request that changes rendering
says which adapter, backend and driver its tests ran on, which tiers were
forced, and what was skipped.

## In CI

Pull request runs don't run tests; run them locally. After a merge (or with
the `full-ci` label):

| Lane | Adapter | Missing GPU |
|---|---|---|
| Windows unit shards | Software rasteriser (WARP, DirectX 12) | Fails (`PONKO_REQUIRE_GPU=1`) |
| Windows GPU tiers (`baseline`, `webgl2`) | The same, with forced limits | Fails, and the job checks that devices were opened on the tier |
| Linux, macOS | Whatever the runner has | Tests skip with a logged reason |

Windows is the lane where the render tests are required to run. When an
image comparison fails in any lane, the job uploads
`target/render-test-output` (the `render-test-output-*` artifact) with the
frame and the difference image. Every lane that runs tests also prints the
comparisons' measured differences in its "Margins of the image comparisons"
step, passed or failed.
