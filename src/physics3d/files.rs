//! Mesh colliders that scenes name by file: `"Collider3D": {"shape":
//! {"Mesh": {"file": "rock.glb", "scale": [1, 1, 1], "convex": false}}}`.
//! Loading the scene reads the file's mesh, scales it and makes a triangle
//! mesh (or a convex hull) of it, once per description.

use serde_json::Value;

use crate::ctx::Ctx;
use crate::error::{Error, ErrorKind};
use crate::reflect::{ColliderFiles, collider_file, collider_file_key, collider_file_parts};
use glam::Vec3;

use super::CollisionMesh;

/// How [`Ctx::load_collision_mesh_with`] makes a collision mesh of a file:
/// the same choices as a scene's `{"Mesh": {"file": ..., "scale": ...,
/// "convex": ...}}`.
///
/// ```
/// use ponko::prelude::*;
/// let rock = ponko::d3::CollisionMeshOptions { scale: Vec3::splat(2.0), ..Default::default() };
/// assert!(!rock.convex);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionMeshOptions {
    /// Scales the file's corners before the shape is made, as a model's
    /// [`Transform`](crate::d3::Transform) scale would scale its look
    /// (colliders don't take the transform's scale). Default: one.
    pub scale: Vec3,
    /// A convex hull around the corners instead of the triangles: cheaper,
    /// and right for a dynamic body (a triangle mesh gets the mass of its
    /// bounding box). Default: `false`, a triangle mesh.
    pub convex: bool,
}

impl Default for CollisionMeshOptions {
    fn default() -> Self {
        Self {
            scale: Vec3::ONE,
            convex: false,
        }
    }
}

/// Every mesh description under a `"Collider3D"` in `json`.
fn collect<'a>(json: &'a Value, out: &mut Vec<&'a Value>) {
    match json {
        Value::Object(map) => {
            for (k, value) in map {
                if k == "Collider3D"
                    && let Some(file) = collider_file(value)
                {
                    out.push(file);
                } else {
                    collect(value, out);
                }
            }
        }
        Value::Array(list) => list.iter().for_each(|v| collect(v, out)),
        _ => {}
    }
}

impl Ctx {
    /// Makes a glTF file's triangles solid: one collision mesh of every
    /// mesh in the file, as [`load_mesh`](crate::gfx::Graphics::load_mesh)
    /// draws them, for a [`Collider::mesh`](crate::d3::Collider::mesh) on a
    /// fixed body. What you walk on is then exactly what you see. It is what
    /// a scene's `"Collider3D": {"shape": {"Mesh": {"file": "yard.glb"}}}`
    /// makes, and the two share it: each file (with its options) is read
    /// once, and a collider of it is saved by its file like a scene's.
    /// [`load_collision_mesh_with`](Self::load_collision_mesh_with) scales
    /// it or makes a convex hull instead.
    ///
    /// Fails if the file can't be read or has no triangles.
    ///
    /// ```no_run
    /// use ponko::d3::*;
    /// use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let look = ctx.gfx.load_mesh("yard.glb");
    /// let solid = ctx.load_collision_mesh("yard.glb").expect("the yard loads");
    /// ctx.world.spawn((Transform::default(), Model::new(look), RigidBody::fixed(), Collider::mesh(solid)));
    /// # }
    /// ```
    pub fn load_collision_mesh(&mut self, path: &str) -> Result<CollisionMesh, Error> {
        self.load_collision_mesh_with(path, CollisionMeshOptions::default())
    }

    /// [`load_collision_mesh`](Self::load_collision_mesh), scaled or as a
    /// convex hull ([`CollisionMeshOptions`]).
    ///
    /// ```no_run
    /// use ponko::d3::*;
    /// use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let hull = CollisionMeshOptions { convex: true, ..Default::default() };
    /// let rock = ctx.load_collision_mesh_with("rock.glb", hull).expect("the rock loads");
    /// ctx.world.spawn((Transform::default(), RigidBody::dynamic(), Collider::mesh(rock)));
    /// # }
    /// ```
    pub fn load_collision_mesh_with(
        &mut self,
        path: &str,
        options: CollisionMeshOptions,
    ) -> Result<CollisionMesh, Error> {
        // Described as a scene would: only what differs from the defaults.
        let mut file = serde_json::json!({ "file": path });
        if options.scale != Vec3::ONE {
            file["scale"] = serde_json::json!(options.scale.to_array());
        }
        if options.convex {
            file["convex"] = serde_json::json!(true);
        }
        self.collision_mesh_of(&file)
    }

    /// The collision mesh a description names, made the first time.
    fn collision_mesh_of(&mut self, file: &Value) -> Result<CollisionMesh, Error> {
        if self.world.get_resource::<ColliderFiles>().is_none() {
            self.world.insert_resource(ColliderFiles::default());
        }
        let key = collider_file_key(file);
        if let Some(&mesh) = self.world.resource::<ColliderFiles>().by_file.get(&key) {
            return Ok(mesh);
        }
        let (path, scale, convex) = collider_file_parts(file);
        let path = path.ok_or_else(|| Error::new(ErrorKind::Json, "a mesh collider's \"file\" must be a string"))?;
        let data = self.gfx.try_load_mesh_data(path)?;
        let points: Vec<Vec3> = data.positions.iter().map(|&p| p * scale).collect();
        let mesh = if convex {
            self.physics3d.convex_hull(&points)
        } else {
            self.physics3d.trimesh(&points, data.triangles())
        }
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Invalid,
                format!("{path}: no collision shape can be made of it"),
            )
            .with_path(path)
        })?;
        let files = self.world.resource_mut::<ColliderFiles>();
        files.by_file.insert(key, mesh);
        files.by_mesh.insert(mesh, file.clone());
        Ok(mesh)
    }

    /// Makes the collision meshes `json` (a scene, a prefab or components)
    /// names by file, so its colliders can be spawned.
    pub(crate) fn load_collider_files(&mut self, json: &Value) -> Result<(), Error> {
        let mut files = Vec::new();
        collect(json, &mut files);
        for file in files {
            self.collision_mesh_of(file)?;
        }
        Ok(())
    }
}
