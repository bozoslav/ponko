//! 3D physics for big worlds from C (the `physics3d` feature): collision
//! meshes built on any thread, the `TerrainCollider` and `ColliderFocus`
//! components, and collider streaming. Without the feature every call here
//! fails with an error naming it; the value helpers still work.
#![cfg_attr(not(feature = "physics3d"), allow(unused_variables))]

use crate::physics3d::{PonkoCollisionGroups, PonkoCollisionMesh};
use crate::three::PonkoVec3;
use crate::{PonkoCtx, PonkoStatus, PonkoWorld};

/// A convex hull, triangle mesh or heightfield built on the CPU (Rust
/// `CollisionMeshData`): make it on any thread with
/// `ponko_collision_mesh_data_heightfield` and friends, then add it on the
/// game's with `ponko_ctx_physics3d_add_mesh`.
pub struct PonkoCollisionMeshData {
    #[cfg(feature = "physics3d")]
    data: ponko::d3::CollisionMeshData,
}

/// The built-in `TerrainCollider` component: makes a terrain tile's entity
/// solid, with a collider of `mesh` at its `PonkoTransform3D` (turned and
/// scaled with it). Start from `ponko_terrain_collider(mesh)`. Not a
/// query column.
/// ```
/// let t = ponko_ffi::ponko_terrain_collider(ponko_ffi::PonkoCollisionMesh { id: 3 });
/// assert_eq!((t.mesh.id, t.friction), (3, 0.5));
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTerrainCollider {
    /// The heightfield (any collision mesh works).
    pub mesh: PonkoCollisionMesh,
    /// 0 ice, 1 rubber (default 0.5).
    pub friction: f32,
    /// Bounciness: 0 none (the default), 1 keeps all its speed.
    pub restitution: f32,
    /// Which colliders it touches (default all).
    pub groups: PonkoCollisionGroups,
}

/// The built-in `ColliderFocus` component: keeps static colliders within
/// `radius` of its entity in the physics world (on the ground), until
/// they're beyond `keep_radius` of every focus, once collider streaming is
/// on (`ponko_ctx_physics3d_stream_colliders`). Same layout as Rust's.
/// ```
/// let f = ponko_ffi::ponko_collider_focus(150.0, 200.0);
/// assert_eq!(f.keep_radius, 200.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoColliderFocus {
    /// Colliders nearer than this come in.
    pub radius: f32,
    /// Colliders beyond this from every focus go.
    pub keep_radius: f32,
}

/// A terrain collider of `mesh` with the default surface (Rust
/// `TerrainCollider::new`).
/// ```
/// let t = ponko_ffi::ponko_terrain_collider(Default::default());
/// assert_eq!(t.groups.memberships, u32::MAX);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_terrain_collider(mesh: PonkoCollisionMesh) -> PonkoTerrainCollider {
    PonkoTerrainCollider {
        mesh,
        friction: 0.5,
        restitution: 0.0,
        groups: crate::physics3d::ponko_collision_groups_all(),
    }
}

/// A collider focus: colliders within `radius` come in, and go beyond
/// `keep_radius` (Rust `ColliderFocus::new`).
/// ```
/// assert_eq!(ponko_ffi::ponko_collider_focus(1.0, 2.0).radius, 1.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_collider_focus(radius: f32, keep_radius: f32) -> PonkoColliderFocus {
    PonkoColliderFocus { radius, keep_radius }
}

/// The id of the built-in `TerrainCollider` component
/// (`PonkoTerrainCollider`), registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_terrain_collider_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_terrain_collider_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, false) }
}

/// The id of the built-in `ColliderFocus` component (`PonkoColliderFocus`,
/// also a query column), registering it if needed.
/// # Safety
/// Live handle and writable output.
/// ```
/// use ponko_ffi::*;
/// let w = ponko_world_new();
/// unsafe { let mut id = 99; assert_eq!(ponko_collider_focus_component(w, &mut id), PonkoStatus::PONKO_OK); ponko_world_free(w); }
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collider_focus_component(world: *mut PonkoWorld, out_id: *mut u32) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::component(world, out_id, true) }
}

/// Builds a heightfield from `count` (n × n) heights `size` wide, as
/// `ponko_ctx_physics3d_heightfield` does. Safe to call from any thread.
/// Null (see `ponko_last_error`) for bad input.
/// ```
/// let h = [0.0f32; 9];
/// let d = unsafe { ponko_ffi::ponko_collision_mesh_data_heightfield(h.as_ptr(), 9, 4.0) };
/// assert!(!d.is_null());
/// unsafe { ponko_ffi::ponko_collision_mesh_data_free(d) };
/// assert!(unsafe { ponko_ffi::ponko_collision_mesh_data_heightfield(h.as_ptr(), 8, 4.0) }.is_null());
/// ```
/// # Safety
/// `heights` holds `count` floats.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collision_mesh_data_heightfield(
    heights: *const f32,
    count: usize,
    size: f32,
) -> *mut PonkoCollisionMeshData {
    // SAFETY: forwarded caller contract.
    unsafe { imp::heightfield(heights, count, size) }
}

/// Builds a triangle mesh: `index_count` indices (three per triangle) into
/// `vertex_count` vertices, as `ponko_ctx_physics3d_trimesh` does. Safe to
/// call from any thread. Null for no triangles, an index out of range or a
/// vertex that isn't finite.
/// ```
/// use ponko_ffi::*;
/// let v = [PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 0.0, y: 0.0, z: 1.0 }];
/// let d = unsafe { ponko_collision_mesh_data_trimesh(v.as_ptr(), 3, [0u32, 2, 1].as_ptr(), 3) };
/// assert!(!d.is_null());
/// unsafe { ponko_collision_mesh_data_free(d) };
/// ```
/// # Safety
/// Readable arrays of the given lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collision_mesh_data_trimesh(
    vertices: *const PonkoVec3,
    vertex_count: usize,
    indices: *const u32,
    index_count: usize,
) -> *mut PonkoCollisionMeshData {
    // SAFETY: forwarded caller contract.
    unsafe { imp::trimesh(vertices, vertex_count, indices, index_count) }
}

/// Builds a convex hull around `count` points, as
/// `ponko_ctx_physics3d_convex_hull` does. Safe to call from any thread.
/// Null if they're flat or not finite.
/// ```
/// use ponko_ffi::*;
/// let p = [PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 1.0, y: 0.0, z: 0.0 }, PonkoVec3 { x: 0.0, y: 1.0, z: 0.0 }];
/// assert!(unsafe { ponko_collision_mesh_data_convex_hull(p.as_ptr(), 3) }.is_null(), "flat");
/// ```
/// # Safety
/// `points` holds `count` values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collision_mesh_data_convex_hull(
    points: *const PonkoVec3,
    count: usize,
) -> *mut PonkoCollisionMeshData {
    // SAFETY: forwarded caller contract.
    unsafe { imp::convex_hull(points, count) }
}

/// Frees collision mesh data. Null is ignored.
/// ```
/// unsafe { ponko_ffi::ponko_collision_mesh_data_free(std::ptr::null_mut()) };
/// ```
/// # Safety
/// `data` must be null or live, and not used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_collision_mesh_data_free(data: *mut PonkoCollisionMeshData) {
    if !data.is_null() {
        // SAFETY: caller passes data from a ponko_collision_mesh_data_* call, once.
        drop(unsafe { Box::from_raw(data) });
    }
}

/// A collision mesh from data built by `ponko_collision_mesh_data_*`
/// (Rust `Physics3D::add_mesh`): quick, the shape is shared. The data stays
/// the caller's to free.
/// # Safety
/// `ctx` null or live; `data` live; output writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_add_mesh(std::ptr::null_mut(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_add_mesh(
    ctx: *mut PonkoCtx,
    data: *const PonkoCollisionMeshData,
    out_mesh: *mut PonkoCollisionMesh,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::add_mesh(ctx, data, out_mesh) }
}

/// Turns collider streaming on (`enabled`, at most `max_new` colliders
/// coming in per step) or off (Rust `Physics3D::stream_colliders`): the
/// colliders of fixed bodies and terrain colliders are then only in the
/// physics world near entities with a `PonkoColliderFocus`.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_stream_colliders(std::ptr::null_mut(), true, 8) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_stream_colliders(
    ctx: *mut PonkoCtx,
    enabled: bool,
    max_new: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::stream(ctx, enabled, max_new) }
}

/// Whether `entity`'s collider is in the physics world now (Rust
/// `Physics3D::is_collider_active`): false for an entity that isn't a body
/// yet, and for a static collider that streaming left out.
/// # Safety
/// `ctx` null or live; output writable.
/// ```
/// let mut active = true;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_physics3d_is_collider_active(std::ptr::null_mut(), 0, &mut active) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_physics3d_is_collider_active(
    ctx: *mut PonkoCtx,
    entity: u64,
    out_active: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    unsafe { imp::is_active(ctx, entity, out_active) }
}

#[cfg(feature = "physics3d")]
mod imp {
    use std::mem::offset_of;

    use ponko::d3::ColliderFocus;
    use ponko::d3::CollisionGroups;
    use ponko::d3::CollisionMeshData;
    use ponko::d3::TerrainCollider;
    use ponko::prelude::Vec3;
    use ponko_ecs::Entity;

    use super::*;
    use crate::ctx::{ctx_access, out};
    use crate::physics3d::{mesh_from_c, mesh_to_c};
    use crate::types::to_c;
    use crate::{CMirror, Failure, Kind, Mirror, boundary, builtin, invalid, mirror};

    const _: () = {
        assert!(size_of::<PonkoColliderFocus>() == size_of::<ColliderFocus>());
        assert!(align_of::<PonkoColliderFocus>() == align_of::<ColliderFocus>());
        assert!(offset_of!(PonkoColliderFocus, radius) == offset_of!(ColliderFocus, radius));
        assert!(offset_of!(PonkoColliderFocus, keep_radius) == offset_of!(ColliderFocus, keep_radius));
    };

    impl CMirror for TerrainCollider {
        type C = PonkoTerrainCollider;
        fn from_c(c: &PonkoTerrainCollider) -> Result<Self, Failure> {
            let amount = |v: f32| {
                if v >= 0.0 && v.is_finite() {
                    Ok(v)
                } else {
                    Err(invalid("friction and restitution must not be negative"))
                }
            };
            Ok(TerrainCollider::new(mesh_from_c(c.mesh))
                .friction(amount(c.friction)?)
                .restitution(amount(c.restitution)?)
                .groups(CollisionGroups::new(c.groups.memberships, c.groups.filter)))
        }
        fn to_c(&self) -> PonkoTerrainCollider {
            PonkoTerrainCollider {
                mesh: mesh_to_c(self.mesh),
                friction: self.friction,
                restitution: self.restitution,
                groups: PonkoCollisionGroups {
                    memberships: self.groups.memberships,
                    filter: self.groups.filter,
                },
            }
        }
    }

    static TERRAIN: Mirror = mirror::<TerrainCollider>();

    pub(crate) fn kind(t: std::any::TypeId) -> Option<Kind> {
        use std::any::TypeId;
        if t == TypeId::of::<TerrainCollider>() {
            Some(Kind::Mirror(&TERRAIN))
        } else if t == TypeId::of::<ColliderFocus>() {
            // repr(C), layout checked against PonkoColliderFocus; any bits
            // are valid (a NaN or negative radius keeps nothing).
            Some(Kind::Plain)
        } else {
            None
        }
    }

    pub(super) unsafe fn component(world: *mut PonkoWorld, out_id: *mut u32, focus: bool) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        unsafe {
            if focus {
                builtin::<ColliderFocus>(world, out_id)
            } else {
                builtin::<TerrainCollider>(world, out_id)
            }
        }
    }

    /// `count` values at `ptr`, which may be null only for 0.
    unsafe fn slice<'a, T>(ptr: *const T, count: usize) -> Result<&'a [T], Failure> {
        if count == 0 {
            return Ok(&[]);
        }
        if ptr.is_null() {
            return Err(invalid("array is null"));
        }
        // SAFETY: caller supplies `count` readable values.
        Ok(unsafe { std::slice::from_raw_parts(ptr, count) })
    }

    /// Boxes what `make` builds, or returns null (the error recorded).
    fn made(make: impl FnOnce() -> Result<CollisionMeshData, Failure>) -> *mut PonkoCollisionMeshData {
        let mut made = std::ptr::null_mut();
        let status = boundary(|| {
            made = Box::into_raw(Box::new(PonkoCollisionMeshData { data: make()? }));
            Ok(())
        });
        if status == PonkoStatus::PONKO_OK {
            made
        } else {
            std::ptr::null_mut()
        }
    }

    pub(super) unsafe fn heightfield(heights: *const f32, count: usize, size: f32) -> *mut PonkoCollisionMeshData {
        made(|| {
            // SAFETY: forwarded caller contract.
            let heights = unsafe { slice(heights, count)? };
            let n = (count as f64).sqrt() as usize;
            if n < 2 || n * n != count {
                return Err(invalid("heights must be a square grid of at least 2 x 2"));
            }
            if !(size > 0.0 && size.is_finite()) {
                return Err(invalid("size must be above 0"));
            }
            Ok(CollisionMeshData::heightfield(heights, size))
        })
    }

    pub(super) unsafe fn trimesh(
        vertices: *const PonkoVec3,
        vertex_count: usize,
        indices: *const u32,
        index_count: usize,
    ) -> *mut PonkoCollisionMeshData {
        made(|| {
            if !index_count.is_multiple_of(3) {
                return Err(invalid("three indices per triangle"));
            }
            // SAFETY: forwarded caller contract.
            let (vertices, indices) = unsafe { (slice(vertices, vertex_count)?, slice(indices, index_count)?) };
            let vertices: Vec<Vec3> = vertices.iter().map(|&p| to_c(p)).collect();
            CollisionMeshData::trimesh(&vertices, indices.as_chunks::<3>().0)
                .ok_or_else(|| invalid("a triangle mesh needs triangles, indices in range and finite vertices"))
        })
    }

    pub(super) unsafe fn convex_hull(points: *const PonkoVec3, count: usize) -> *mut PonkoCollisionMeshData {
        made(|| {
            // SAFETY: forwarded caller contract.
            let points: Vec<Vec3> = unsafe { slice(points, count)? }.iter().map(|&p| to_c(p)).collect();
            CollisionMeshData::convex_hull(&points)
                .ok_or_else(|| invalid("a convex hull needs points that aren't flat, all finite"))
        })
    }

    pub(super) unsafe fn add_mesh(
        ctx: *mut PonkoCtx,
        data: *const PonkoCollisionMeshData,
        out_mesh: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe {
            ctx_access(ctx, |c| {
                let Some(data) = data.as_ref() else {
                    return Err(invalid("collision mesh data is null"));
                };
                let mesh = c.physics3d.add_mesh(&data.data);
                out(out_mesh, mesh_to_c(mesh))
            })
        })
    }

    pub(super) unsafe fn stream(ctx: *mut PonkoCtx, enabled: bool, max_new: usize) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe {
            ctx_access(ctx, |c| {
                c.physics3d.stream_colliders(enabled.then_some(max_new));
                Ok(())
            })
        })
    }

    pub(super) unsafe fn is_active(ctx: *mut PonkoCtx, entity: u64, out_active: *mut bool) -> PonkoStatus {
        // SAFETY: forwarded caller contract.
        boundary(|| unsafe {
            ctx_access(ctx, |c| {
                out(out_active, c.physics3d.is_collider_active(Entity::from_bits(entity)))
            })
        })
    }
}

#[cfg(not(feature = "physics3d"))]
mod imp {
    //! Without the feature: every call fails, saying why.
    use super::*;
    use crate::{Failure, Kind, boundary, invalid};

    fn off() -> Result<(), Failure> {
        Err(invalid("ponko_ffi was built without the `physics3d` feature"))
    }
    pub(crate) fn kind(_: std::any::TypeId) -> Option<Kind> {
        None
    }
    pub(super) unsafe fn component(_: *mut PonkoWorld, _: *mut u32, _: bool) -> PonkoStatus {
        boundary(off)
    }
    fn none() -> *mut PonkoCollisionMeshData {
        boundary(off);
        std::ptr::null_mut()
    }
    pub(super) unsafe fn heightfield(_: *const f32, _: usize, _: f32) -> *mut PonkoCollisionMeshData {
        none()
    }
    pub(super) unsafe fn trimesh(
        _: *const PonkoVec3,
        _: usize,
        _: *const u32,
        _: usize,
    ) -> *mut PonkoCollisionMeshData {
        none()
    }
    pub(super) unsafe fn convex_hull(_: *const PonkoVec3, _: usize) -> *mut PonkoCollisionMeshData {
        none()
    }
    pub(super) unsafe fn add_mesh(
        _: *mut PonkoCtx,
        _: *const PonkoCollisionMeshData,
        _: *mut PonkoCollisionMesh,
    ) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn stream(_: *mut PonkoCtx, _: bool, _: usize) -> PonkoStatus {
        boundary(off)
    }
    pub(super) unsafe fn is_active(_: *mut PonkoCtx, _: u64, _: *mut bool) -> PonkoStatus {
        boundary(off)
    }
}

pub(crate) use imp::kind as stream_kind;
