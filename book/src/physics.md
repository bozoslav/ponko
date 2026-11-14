# Physics: bodies that fall, stack and bounce

Crates that tumble, balls that bounce, doors that push things, triggers that
notice who walked in: 2D rigid-body physics, built on
[rapier](https://rapier.rs). It's behind the `physics` feature:

```toml
ponko = { version = "0.1", features = ["physics"] }
```

(The C library, `ponko_ffi`, has it on by default.) For 3D bodies, terrain
and a walking character, see [3D physics](physics3d.md).

Turn it on with gravity in pixels per second² (y is down) and add the
systems that react to it:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/physics.rs:setup}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:physics_setup}}
```

</div>
</div>

The physics step is a simulation system in the `Physics`
[phase](systems.md#phases), wherever you call `physics`. Systems that move
things for this tick's step (forces, velocities) go in `BeforePhysics` with
`.add(Phase::BeforePhysics, f)` (C++: `.add(ponko::phase::before_physics, f)`);
`sim_system`s run after the step and see its result.

## Bodies and colliders

An entity becomes a physics body with three components: a `d2::Transform`, a
`RigidBody` (how it moves) and a `Collider` (its shape, in pixels):

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/physics.rs:bodies}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:physics_bodies}}
```

</div>
</div>

| Rust | C++ | Meaning |
|---|---|---|
| `RigidBody::dynamic()` | `ponko::dynamic_body()` | moved by gravity and collisions |
| `RigidBody::fixed()` | `ponko::fixed_body()` | never moves: ground, walls |
| `RigidBody::kinematic()` | `ponko::kinematic_body()` | moved only by you; pushes, isn't pushed |
| `Collider::circle(r)` | `ponko::circle_collider(r)` | |
| `Collider::rect(size)` | `ponko::rect_collider(size)` | |
| `Collider::capsule(h, r)` | `ponko::capsule_collider(h, r)` | vertical, good for characters |
| `Collider::polygon(&points)` | `ponko::polygon_collider({points...})` | convex, 3 to 8 corners: ramps, rocks, ships |
| `.density(d)`, `.friction(f)`, `.restitution(r)` | fields of the same name | mass per pixel², 0 ice..1 rubber, bounciness |
| `.sensor()` | `.sensor = true` | reports overlaps, doesn't push |
| `.offset(v)` | `.offset = v` | shape moved from the entity's position |

A polygon is the convex hull of its corners, given around the entity's
position in any order; `collider.shape.points()` reads them back (C++:
`world.collider_points(e)`). For a concave shape, such as an L, split it
into convex pieces on separate bodies.

A rigid body also has `gravity_scale`, `linear_damping`, `angular_damping`,
`lock_rotation` (characters that shouldn't tip over) and `ccd` (continuous
collision detection, so bullets don't pass through thin walls).

**`d2::Transform` stays the source of truth.** After each step, dynamic bodies
write their new position and rotation into it, so sprites, attachments, the
spatial grid and everything else just follow. Set a transform yourself and
the body teleports there. Change a `RigidBody` or `Collider` and the body is
rebuilt; despawn the entity (or remove either component) and it's gone.

Add a `Velocity` component to read a body's speed after every step, or to
set it (a jump, a thrown crate, a moving platform):

```rust
# let player = ctx.world.spawn((ponko::d2::Transform::default(),));
ctx.world.insert(player, ponko::d2::Velocity::linear(vec2(0.0, -400.0))); // jump: y is down
```

In C++, `ponko::velocity` is a plain struct, so it also works as a query
column. Bodies and colliders need converting, so like sprites they're read
and written with `get`/`set`, not queried.

## Collisions

When two colliders start or stop touching, the step sends a
`CollisionStarted` or `CollisionStopped` event. `sensor` tells overlaps with
a sensor apart from real contacts:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/physics.rs:collisions}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:physics_collisions}}
```

</div>
</div>

Like all [events](events.md), each system sees each collision once.

## Rays, overlaps and pushes

`ctx.physics` answers questions about the world as of the last step, and
gives bodies a push:

<div class="lang-pair">
<div>

```rust
{{#include ../../examples/physics.rs:queries}}
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:physics_queries}}
```

</div>
</div>

- `cast_ray(origin, direction, max_distance, filter)`: the first collider hit,
  with the point, the surface normal and the distance.
- `overlap_point(point, filter, &mut out)` and
  `overlap_circle(center, r, filter, &mut out)`: everything there. `out` is
  reused, so no allocation per call.

A `d2::QueryFilter` sees solid colliders by default; `.with_sensors()` adds
sensors and `.exclude(entity)` skips one body. The C and C++ `cast_ray`,
`entities_at` and `entities_in_circle` take no filter and see sensors too.
- `apply_impulse(e, impulse)`: a one-off push in mass × pixels/s (multiply by
  `mass(e)` to get a speed change).
- `set_gravity(g)`: change the pull for everyone.
- `set_paused(true)`: hold the step while the other sim systems run, for a
  pause screen ([Pausing only physics](sim-clock.md#pausing-only-physics)).

New bodies join at the next step, so a query in the tick you spawned them
won't see them yet.

## Determinism and cost

The same start gives the same result, tick after tick, on one machine: the
physics step runs inside the [simulation clock](sim-clock.md) with a fixed
time step. For bit-identical results across different CPUs and operating
systems (lockstep multiplayer, replays), use the `physics-determinism`
feature instead, which turns on rapier's cross-platform determinism at a
small speed cost.

Bodies cost only where they exist: entities without a `RigidBody` are never
looked at, so the million-entity paths stay as fast as before. Compared with
calling rapier directly, Ponko's syncing adds about 3–8%
(`cargo run --release -p ponko_bench -- physics`):

| Boxes falling into a pile, 300 ticks | Ponko | rapier alone |
|---|---|---|
| 1,000 | 35.1 ms | 32.6 ms |
| 10,000 | ~1.9 s | ~1.8 s |

Try it: `cargo run --release --example physics --features physics`. Left
click drops crates, right click balls, Space blasts things away.
