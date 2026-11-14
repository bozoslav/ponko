# Ponko roadmap

Ponko aims to support complete 2D and 3D games through an integrated, high-performance, code-first engine with useful AI development tools. The direction is broad capability with low incidental complexity, preserving the custom archetype ECS, wgpu renderer, headless simulation and Rust/C++ workflows. See [PROJECT_VISION.md](PROJECT_VISION.md) and the current [ARCHITECTURE.md](ARCHITECTURE.md).

This roadmap is directional. Ordering changes with correctness evidence, measurements and real developer needs; a release is defined by its scope, and any date here is a target. An issue describes intended work, not a delivery promise. P1 issues define a release; P2 issues ship when ready and otherwise move to the next one; P3 is research or lower-priority work that never holds a release and may end in rejection. [STABILITY.md](STABILITY.md#releases) says how releases are cut; the [index issue](https://github.com/ponkoengine/ponko/issues/433) lists every planned item.

## Current capability inventory

**Implemented** means the mechanism exists in source, not that every use case is production proven. **Partial** means useful building blocks exist with important integration or coverage gaps. **Missing** means the named workflow was not found in the inspected implementation. **Requires verification** marks claims needing platform/runtime evidence beyond source inspection. This documentation change did not execute runtime or GPU validation.

| Complete-game area | Status | Evidence and remaining work |
|---|---|---|
| ECS and fixed-step simulation | Implemented | [ECS](crates/ponko_ecs/src), [game loop](src/game.rs), [determinism tests](tests/determinism.rs). Systems are sequential by default; view systems can opt into parallel batches ([schedule](src/schedule.rs)). |
| World, scene and prefab management | Implemented | [scene.rs](src/scene.rs), [reflection](src/reflect.rs), [scene tests](tests/scenes.rs). [Ember Hop](examples/ember_hop) spawns its levels' objects as prefabs (one with children) and its title as a scene. Advanced world composition and complete service-state restoration need use-case validation. |
| Asset importing and processing | Partial | [glTF](src/three/gltf.rs), [Unity tooling](crates/ponko_unity), [assets](src/assets.rs). Import support varies by format and feature; it is not source-engine runtime compatibility. |
| Asset streaming and residency | Partial | [Job/CellStreamer](src/stream.rs), [stream tests](tests/stream.rs). Residency budgets for texture mips and meshes shipped in 0.1.0. Instance batches stream as a whole ([#396](https://github.com/ponkoengine/ponko/issues/396)); textures are uncompressed ([#403](https://github.com/ponkoengine/ponko/issues/403)). |
| Meshes, materials, animation and skinning | Implemented | [mesh](src/three/mesh.rs), [clips](src/three/clip.rs), [animator](src/three/animator.rs), [skin](src/three/skin.rs), [morph tests](tests/morph3d.rs). Verify particular imported animation workflows; presence is not arbitrary asset parity. |
| Terrain, vegetation, LOD and large worlds | Partial | [terrain](src/three/terrain.rs), [far terrain](src/three/far_terrain.rs), [instances](src/three/instances.rs), [LOD](src/three/lod.rs), [streaming example](examples/streaming.rs). Budgeted residency and broader incremental extraction remain work. |
| Characters, collision, physics and navigation | Implemented | [physics3d](src/physics3d), [navigation](src/nav3d), [vehicle tests](tests/vehicle3d.rs), [navigation tests](tests/navmesh3d.rs). Features are optional; real game integration and target performance still need evidence. |
| UI, text, actions and gamepads | Implemented | [retained UI](src/ui), [text](src/text.rs), [actions](src/actions.rs), [gamepads](src/gamepad.rs), [UI tests](tests/ui_retained.rs). Input backend and native API coverage vary by target. |
| Accessibility | Requires verification | UI and input building blocks exist; no comprehensive assistive-technology, navigation or accessibility conformance claim is established here. Audit with a playable reference workflow before defining additions. |
| Audio and spatial audio | Implemented | [audio](src/audio.rs), [audio3d example](examples/audio3d.rs), [audio tests](tests/audio.rs). Live output/device behavior requires runtime checks. |
| Multiplayer, synchronization and servers | Partial | [lockstep](src/net.rs), [snapshots](src/net/snapshots.rs), [messages](src/net/messages.rs), [replication tests](tests/net_replication.rs), headless runner. Live transport/relay behavior and a complete deployment/authority workflow require verification. |
| Save/load and serialization | Implemented | [reflection](src/reflect.rs), [scenes](src/scene.rs), [stability contract](STABILITY.md). [Ember Hop](examples/ember_hop) saves and loads its runs as world save slots ([tests](tests/ember_hop.rs)). Applications own custom component migrations and external service persistence. |
| GPU compute and custom graphics | Partial | [compute](src/three/compute.rs), [effects](src/three/effect.rs), [shader example](examples/shaders.rs). Named-stage custom passes shipped in 0.1.0. Stencil on materials ([#397](https://github.com/ponkoengine/ponko/issues/397)) and a custom sky hook ([#398](https://github.com/ponkoengine/ponko/issues/398)) are open. |
| Build tooling, packaging and deployment | Partial | [Cargo manifest](Cargo.toml), [web tooling](web), [release workflow](.github/workflows/release.yml). CI/release scripts are not proof that a complete reference game has shipped through every path. |
| Desktop and browser portability | Requires verification | Target-specific implementation and [CI configuration](.github/workflows/ci.yml) exist; see [platform policy](STABILITY.md#platforms). Full visual/runtime parity and individual GPU feature fallbacks need adapter evidence; mobile is not a validated product commitment. |
| Debugging, profiling and automated testing | Implemented | [tests](tests), [GPU tests](src/render), [performance reports](src/perf_report.rs), [inspector](src/inspector.rs). Broader reproducible matrices and diagnostics remain below. |
| AI development and runtime control | Partial | [MCP](crates/ponko_mcp/src/lib.rs), [control](src/control.rs), [agent guide](book/src/agents.md): state, stepping, launch, screenshots, logs and profiling exist. Reliable autonomous complete-game production is not established. |
| End-to-end shipping reference workflow | Partial | Two packaged, smoke-tested 2D reference games. [Coin Dash](examples/coin_dash): menus, rebinding, sound and its own saves ([packaging](book/src/packaging.md)). [Ember Hop](examples/ember_hop), a platformer: Tiled levels with autotiled ground and tile colliders, 2D physics with sensors as its rules, prefabs with children, a title scene, world save slots at checkpoints, sound files and streamed music, particles, a following camera, keyboard and gamepad ([its chapter](book/src/ember-hop.md)). Both are packaged by `ponko package`, and CI plays each package on Linux, macOS and Windows; both build for the web. [Shard Grove](examples/shard_grove), the 3D reference: a glTF level that is drawn, solid and baked into a navigation mesh, skinned glTF characters with animation state machines, a character controller, enemies on the navmesh, positional sound, menus, a world-space sign, quality presets and saves ([its chapter](book/src/shard-grove.md)); it packages and builds for the web, CI doesn't play its package yet. Open: the web gallery that publishes them ([#373](https://github.com/ponkoengine/ponko/issues/373)), the gaps the 3D reference found ([#375](https://github.com/ponkoengine/ponko/issues/375)), and the gaps the platformer found ([#437](https://github.com/ponkoengine/ponko/issues/437)–[#440](https://github.com/ponkoengine/ponko/issues/440)). |

## After 0.1.0

0.1.0 closed the foundations backlog (#132 to #207): benchmarks and
differential tests, the access-checked parallel scheduler, change detection,
the renderer split into subsystems, incremental extraction, GPU tiers,
residency budgets, custom passes, and the API reshaped around services and
one prelude. What a review of 0.1.0 found missing is mostly what turns a
game into a shipped product, and each release below is aimed at one such
outcome. Each has a GitHub milestone with the live state.

### 0.2.0: ship a small game

A complete 2D game goes from an empty project to a packaged download and a
web build without engine-level code, and releases become routine. Target:
early 2027.

| Outcome | Issues |
|---|---|
| Start and learn | Project template and `new` command [#371](https://github.com/ponkoengine/ponko/issues/371); 2D and 3D tutorials [#372](https://github.com/ponkoengine/ponko/issues/372); a guide per release [#246](https://github.com/ponkoengine/ponko/issues/246); web gallery [#373](https://github.com/ponkoengine/ponko/issues/373) |
| 2D essentials | Image formats [#377](https://github.com/ponkoengine/ponko/issues/377); blend modes and sprite shaders [#378](https://github.com/ponkoengine/ponko/issues/378); 2D particles [#379](https://github.com/ponkoengine/ponko/issues/379); tilemap import, animation and colliders [#381](https://github.com/ponkoengine/ponko/issues/381) |
| Scenes and saves | Prefabs with children [#367](https://github.com/ponkoengine/ponko/issues/367); user data folder, atomic saves, browser storage [#365](https://github.com/ponkoengine/ponko/issues/365); pressed-this-frame input for rebinding [#369](https://github.com/ponkoengine/ponko/issues/369) |
| Test and ship | Headless UI and frame systems [#370](https://github.com/ponkoengine/ponko/issues/370); `package` command [#366](https://github.com/ponkoengine/ponko/issues/366); signed downloads [#282](https://github.com/ponkoengine/ponko/issues/282); a second 2D reference game [#374](https://github.com/ponkoengine/ponko/issues/374); agent acceptance suite [#416](https://github.com/ponkoengine/ponko/issues/416); web audio [#419](https://github.com/ponkoengine/ponko/issues/419) |
| API | One path per type [#383](https://github.com/ponkoengine/ponko/issues/383); `Ctx` shortcuts [#384](https://github.com/ponkoengine/ponko/issues/384); remaining C and C++ bindings [#387](https://github.com/ponkoengine/ponko/issues/387); immediate UI scale and theme [#414](https://github.com/ponkoengine/ponko/issues/414) |
| Releases | Semver checks [#355](https://github.com/ponkoengine/ponko/issues/355); API snapshots [#356](https://github.com/ponkoengine/ponko/issues/356); trusted publishing [#357](https://github.com/ponkoengine/ponko/issues/357); release candidates and patch branches [#358](https://github.com/ponkoengine/ponko/issues/358); migration notes [#359](https://github.com/ponkoengine/ponko/issues/359); dependency audit [#360](https://github.com/ponkoengine/ponko/issues/360); benchmark tracking [#361](https://github.com/ponkoengine/ponko/issues/361) |

### 0.3.0: 3D production and text

A complete 3D game with imported content, text that can be localized, and
scene editing in 3D.

| Outcome | Issues |
|---|---|
| Content | Compressed textures [#403](https://github.com/ponkoengine/ponko/issues/403); glTF extensions [#404](https://github.com/ponkoengine/ponko/issues/404); asset packs [#405](https://github.com/ponkoengine/ponko/issues/405); animation retargeting [#408](https://github.com/ponkoengine/ponko/issues/408) |
| Rendering | Motion vectors for animated meshes [#391](https://github.com/ponkoengine/ponko/issues/391); and for custom vertex functions [#392](https://github.com/ponkoengine/ponko/issues/392); GI coverage [#394](https://github.com/ponkoengine/ponko/issues/394); stencil [#397](https://github.com/ponkoengine/ponko/issues/397); custom sky [#398](https://github.com/ponkoengine/ponko/issues/398); probe refresh [#399](https://github.com/ponkoengine/ponko/issues/399); transparency and particles [#400](https://github.com/ponkoengine/ponko/issues/400); post effects [#401](https://github.com/ponkoengine/ponko/issues/401); instance batch streaming [#396](https://github.com/ponkoengine/ponko/issues/396); GPU memory reports [#393](https://github.com/ponkoengine/ponko/issues/393); 2D lighting [#380](https://github.com/ponkoengine/ponko/issues/380) |
| Text and UI | Shaping, fallback and right-to-left [#409](https://github.com/ponkoengine/ponko/issues/409); IME [#410](https://github.com/ponkoengine/ponko/issues/410); localization [#411](https://github.com/ponkoengine/ponko/issues/411); rich text [#412](https://github.com/ponkoengine/ponko/issues/412); screen readers on Windows and Linux [#413](https://github.com/ponkoengine/ponko/issues/413) |
| Tools | 3D scene editing [#415](https://github.com/ponkoengine/ponko/issues/415); a 3D reference game [#375](https://github.com/ponkoengine/ponko/issues/375); WebGPU gated in CI [#427](https://github.com/ponkoengine/ponko/issues/427) |
| API and simulation | Third-party types in the API [#385](https://github.com/ponkoengine/ponko/issues/385); typed handles [#386](https://github.com/ponkoengine/ponko/issues/386); dependency-based system batches [#388](https://github.com/ponkoengine/ponko/issues/388); save migrations [#368](https://github.com/ponkoengine/ponko/issues/368); navmesh carving [#407](https://github.com/ponkoengine/ponko/issues/407) |

### 0.4.0: multiplayer and platforms

Client prediction [#420](https://github.com/ponkoengine/ponko/issues/420), rollback evaluation [#421](https://github.com/ponkoengine/ponko/issues/421),
multiplayer in the browser [#422](https://github.com/ponkoengine/ponko/issues/422), Steam sockets [#423](https://github.com/ponkoengine/ponko/issues/423),
dedicated servers [#424](https://github.com/ponkoengine/ponko/issues/424), threads on the web [#425](https://github.com/ponkoengine/ponko/issues/425),
the audio mixer [#418](https://github.com/ponkoengine/ponko/issues/418), and the decision on iOS and Android [#426](https://github.com/ponkoengine/ponko/issues/426).

### 1.0: the API freeze

1.0 is declared when the readiness checklist [#432](https://github.com/ponkoengine/ponko/issues/432) is complete, not
on a date: the API has gone two minor releases with only mechanical breaks,
reference games in 2D and 3D and games by other people have shipped on it,
every named platform is gated in CI, and the compatibility policy for after
1.0 is written. Before it come the API freeze review [#389](https://github.com/ponkoengine/ponko/issues/389) and the
C ABI decision [#390](https://github.com/ponkoengine/ponko/issues/390). The last 0.x release carries every remaining
break; 1.0.0 breaks nothing.

### Later

Research and lower-priority work in the
[Later milestone](https://github.com/ponkoengine/ponko/milestone/6) moves
into a release when a real game needs it: fuzzing, more C library targets,
2D skeletal animation, contact-hardening shadows, better screen-space
reflections, skinned instance batches, a binary save format, C and C++ on
the web, HDR output, web build inspection. Ray tracing and mesh shaders were
evaluated for 0.1.0 and stay research.

## How work is chosen

- Complete games decide. A gap found by a reference game or by a game built
  on the engine outranks a feature nobody is waiting for.
- Foundations stay measured: benchmarks, differential tests and render
  regressions come before a redesign, and a measured rejection is a valid
  result.
- The simple path stays simple: `Game::new()`, systems, `.run()`. New power
  is optional and costs nothing when unused; 2D and headless builds do not
  pay for 3D.
- Compatibility is kept as [STABILITY.md](STABILITY.md) describes; a break
  lands in a minor release with its migration note.
- [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) define the
  review and validation workflow. Rust-only contributions are welcome;
  maintainers own guide and C/C++ follow-ups.
