# Reflection: components and resources as JSON

Reflection lets code that doesn't know your types read and change them by
name: the agent bridge (MCP), the inspector, and save/load all use it. It's
opt-in per type and built on [serde](https://serde.rs).

```rust
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[derive(Component)]
struct Health { current: f32, max: f32 }

Game::new("Arena", 1280, 720)
    .setup(|ctx| {
        ctx.world.reflect::<Health>();
        let e = ctx.world.spawn((ponko::d2::Transform::default(), Health { current: 10.0, max: 10.0 }));

        // Read: {"current": 10.0, "max": 10.0}
        let hp = ctx.world.get_json(e, "Health").unwrap();

        // Write only some fields; the rest keep their values.
        ctx.world.set_json(e, "Health", json!({ "current": 3.0 })).unwrap();
    })
    .run();
```

Add `serde = { version = "1", features = ["derive"] }` to your `Cargo.toml`.

## C and C++ components

C++ has no serde, so a C++ component lists its fields with
`world.reflect<T>`. From then on it is a reflected component like any other:
agents read and write it through `ponko-mcp`, and saves include it, under
the type's name (`Unit`, or `game::Unit` in a namespace) or the one you pass
first: `world.reflect<Unit>("Soldier", {...})`.

```cpp
{{#include ../../examples/cpp/guide.cpp:reflect}}
```

Fields can be `float`, `double`, the fixed-width integers, `bool`,
`ponko::prelude::vec2` (`[x, y]` in JSON) and `ponko::color` (`{"r", "g", "b", "a"}`).
Entities are `uint64_t`, which is how Rust saves an `Entity` too. Bytes
no field covers (padding, fields you leave out) are zero in values made
from JSON, and editing a field leaves them alone.

In C, `ponko_component_reflect(world, id, fields, count)` takes the same
list as `PonkoField { name, kind, offset }`, with `offsetof` for the
offset. The Rust side of this is `World::reflect_fields`, for components
that are registered at runtime and have no Rust type.

## Resources

Game-wide state (a score, the current round, rules an agent might want to
tweak) lives in resources. Reflect them the same way:

<div class="lang-pair">
<div>

```rust
# use serde::{Deserialize, Serialize};
# use ponko::reflect::json::json;
#[derive(Serialize, Deserialize)]
struct Score { red: u32, blue: u32 }

world.reflect_resource::<Score>();
world.insert_resource(Score { red: 0, blue: 0 });

world.resource_json("Score")?;                          // {"red": 0, "blue": 0}
world.set_resource_json("Score", json!({ "blue": 3 }))?; // only blue changes
```

</div>
<div>

```cpp
{{#include ../../examples/cpp/guide.cpp:reflect_resource}}
```

</div>
</div>

Reflected resources show up in `ponko-mcp` (`list_resources`,
`get_resource`, `set_resource`) and are part of [world saves](determinism.md#saving-and-loading-the-world).
Resources hold whatever Rust type you like, so the ones that can't be JSON
(a nav grid, a GPU handle) just stay unreflected. A C++ resource is saved
under its type name with namespaces (`game::Score`); C names it in
`ponko_resource_reflect`.

## Names

A component's name is its type name without the module path:
`my_game::units::Health` is `"Health"`. Reflecting two different types with
the same short name panics and names both, so a name always means one type.

`Transform2D`, `Sprite`, `Animation`, `ParticleEmitter2D` and `Attached` are
reflected in every game already. A sprite's texture is written as the name
of its image in `assets/` (`"texture": "crab.png"`), and setting one loads the
image; a [sprite-sheet animation](tiles.md#animation) names its sheet's image
the same way. Sprites of textures made in code (`create_texture`) have no
file to name, so they're left out of saves. Whole entities with `NotSaved`
are left out of `save()` too ([levels and saves](tiles.md#levels-and-saves)).

## The methods

All of them come from `WorldReflect`, which `use ponko::prelude::*;` brings in.

| Method | Does |
|---|---|
| `reflect::<T>()` | makes `T` visible to reflection |
| `reflected_components()` | names of all reflected components |
| `get_json(e, name)` | one component as JSON |
| `set_json(e, name, value)` | writes it; adds it if missing; objects merge field by field |
| `remove_json(e, name)` | removes it; `Ok(true)` if it was there |
| `entity_json(e)` | all reflected components of `e`, keyed by name |
| `spawn_json(value)` | spawns from the `entity_json` format; on error spawns nothing |
| `reflect_resource::<R>()` | makes resource `R` visible to reflection |
| `reflected_resources()` | names of all reflected resources |
| `resource_json(name)` | one resource as JSON |
| `set_resource_json(name, value)` | writes it; inserts it if missing; objects merge field by field |

`entity_json` and `spawn_json` round-trip, so a snapshot of an entity can be
copied, saved, or sent over the wire:

```rust
# let e = world.spawn((ponko::d2::Transform::default(),));
let snapshot = world.entity_json(e)?;   // {"ponko::d2::Transform": {...}, "Health": {...}}
let copy = world.spawn_json(snapshot)?;
```

Components that aren't reflected are left out of `entity_json`.

## Errors

Every call returns a `ReflectError` that says what went wrong:
`UnknownComponent`, `NoSuchEntity`, `MissingComponent`, `UnknownResource`,
`MissingResource`, or `BadValue` (with serde's message, such as
`invalid type: string "seven", expected struct Health`).

## Tips

- `#[serde(default)]` on a struct lets `set_json` add the component from a
  partial value. `Transform2D` has it.
- Reflection is for tools, not hot loops: it goes through JSON. Use queries
  inside systems.
