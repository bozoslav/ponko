//! 3D navigation using the pure Rust Recast/Detour port `navmesh-rs`.
//!
//! The crate supplies voxelization, clearance/ledge filtering, erosion, regions,
//! tiled polygons, A* and funnel queries without Bevy, C++ or an async runtime.
//! Landmass needs a separate baker; Polyanya needs adaptation for 3D surfaces;
//! oxidized_navigation is a Bevy plugin. Recast bindings need a native toolchain.
//! This wrapper keeps the established algorithm in one dependency.
//!
//! Dynamic AABBs conservatively disable overlapping polygon bounds (expanded by
//! the baked agent clearance). Opt-in carving (`NavBakeSettings::carving`, then
//! `NavObstacle3D::carve`) rebuilds cached heightfield layers.
//! Register `systems::step_navmesh3d` as a sim system, including on headless servers.
//! Agents directly own their Transform3D: do not also attach a physics body or
//! character controller. Crowd avoidance is opt-in. Bake for the largest
//! agent radius/height; agents larger than that radius are stopped.

use crate::ctx::Ctx;
use crate::d3::Transform as Transform3D;
use crate::render::MeshData;
use glam::Vec3;
use navmesh_rs::{Limits, Workspace, bake, crowd, detour, recast, tilecache};
use ponko_ecs::Component;
use ponko_ecs::{Entity, World};
use std::sync::atomic::{AtomicU64, Ordering};

mod carve;

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

/// World-space, counter-clockwise triangle soup; include walls and ceilings too.
/// ```
/// use ponko::prelude::*;
/// let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(20.0), ponko::d3::Transform::default());
/// assert_eq!(source.triangles.len(), 2);
/// ```
#[derive(Clone, Debug, Default)]
pub struct NavSource {
    /// World-space triangles, facing up for walkable floors.
    /// ```
    /// use ponko::prelude::*;
    /// let source = ponko::nav::NavSource { triangles: vec![[Vec3::ZERO, Vec3::Z, Vec3::X]] };
    /// assert_eq!(source.triangles.len(), 1);
    /// ```
    pub triangles: Vec<[Vec3; 3]>,
}
impl NavSource {
    /// Transforms mesh triangles to world space, preserving winding under mirrors.
    /// Invalid indices are rejected later by returning an invalid source to `bake`.
    /// ```
    /// use ponko::prelude::*;
    /// let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(8.0), ponko::d3::Transform::at(Vec3::Y));
    /// assert_eq!(source.triangles[0][0].y, 1.0);
    /// ```
    #[must_use]
    pub fn from_mesh(mesh: &MeshData, transform: Transform3D) -> Self {
        let mut triangles = Vec::with_capacity(mesh.indices.len() / 3);
        if !mesh.indices.len().is_multiple_of(3) {
            return Self {
                triangles: vec![[Vec3::NAN; 3]],
            };
        }
        for indices in mesh.indices.as_chunks::<3>().0 {
            let mut tri = [Vec3::ZERO; 3];
            for (v, &index) in tri.iter_mut().zip(indices) {
                let Some(&p) = mesh.positions.get(index as usize) else {
                    return Self {
                        triangles: vec![[Vec3::NAN; 3]],
                    };
                };
                *v = transform.position + transform.rotation * (p * transform.scale);
            }
            if transform.scale.element_product() < 0.0 {
                tri.swap(1, 2);
            }
            triangles.push(tri);
        }
        Self { triangles }
    }
}

/// Bake clearances in world units, with slopes in degrees. +Y is up.
/// Height/radius round up to cells; climb rounds down, so clearance is conservative.
/// ```
/// use ponko::prelude::*;
/// let settings = ponko::nav::NavBakeSettings { agent_radius: 0.4, ..Default::default() };
/// assert!(settings.agent_height > 1.0);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct NavBakeSettings {
    /// Horizontal clearance; defaults to 0.3.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().agent_radius, 0.3);
    /// ```
    pub agent_radius: f32,
    /// Minimum free height; defaults to 1.8.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().agent_height, 1.8);
    /// ```
    pub agent_height: f32,
    /// Largest traversable step; defaults to 0.4.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().max_climb, 0.4);
    /// ```
    pub max_climb: f32,
    /// Maximum walkable slope in degrees, strictly below 90; defaults to 45.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().max_slope, 45.0);
    /// ```
    pub max_slope: f32,
    /// Horizontal voxel width; defaults to 0.2.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().cell_size, 0.2);
    /// ```
    pub cell_size: f32,
    /// Vertical voxel height; defaults to 0.1.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().cell_height, 0.1);
    /// ```
    pub cell_height: f32,
    /// Tile width in cells (8..=255); smaller tiles localize carving. Defaults to
    /// 32 (6.4 m at the default cell size). A navmesh holds at most 16,384 tiles,
    /// so this is a minimum: the baker grows it, up to 255, for levels that would
    /// need more (about 0.8 km square at the defaults, up to 6.5 km). Beyond that,
    /// raise `cell_size`.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::nav::NavBakeSettings::default().tile_size, 32);
    /// ```
    pub tile_size: u32,
    /// Keep per-tile heightfield layers so `NavObstacle3D::carve` can cut real
    /// holes at runtime; defaults to false.
    ///
    /// Layers cost extra bake time and memory, so they are built only when asked
    /// for. Without them a `carve` obstacle falls back to the conservative
    /// polygon-AABB blocking of ordinary obstacles. With them the whole mesh comes
    /// from the layer pipeline (no separate polygon bake), so a carving bake costs
    /// about one pass over the level, not two.
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(!ponko::nav::NavBakeSettings::default().carving);
    /// ```
    pub carving: bool,
}
impl Default for NavBakeSettings {
    fn default() -> Self {
        Self {
            agent_radius: 0.3,
            agent_height: 1.8,
            max_climb: 0.4,
            max_slope: 45.0,
            cell_size: 0.2,
            cell_height: 0.1,
            tile_size: 32,
            carving: false,
        }
    }
}

/// Invalid input, resource limits or a Recast/Detour failure. No partial mesh is returned.
/// ```
/// use ponko::prelude::*;
/// let error = ponko::d3::NavMesh::bake(&[], ponko::nav::NavBakeSettings::default()).err().unwrap();
/// assert!(!error.to_string().is_empty());
/// ```
#[derive(Debug)]
pub struct NavBakeError(String);
impl std::fmt::Display for NavBakeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NavBakeError {}
fn error(value: impl std::fmt::Display) -> NavBakeError {
    NavBakeError(value.to_string())
}

/// Most tiles one navmesh holds (the backend's tile reference layout).
const MAX_TILES: usize = 1 << 14;
/// Working memory one tile may use while baking.
const TILE_WORKING_BYTES: usize = 64 << 20;
/// Work units (span, cell and vertex visits) one tile may use.
const TILE_WORK: u64 = 100_000_000;
/// Headroom for input bucketing and bookkeeping, before the per-tile output.
const SETUP_BYTES: usize = 2 << 30;

/// Turns a backend bake failure into a message naming the limit and the setting.
fn bake_error(e: &bake::BakeError, s: &NavBakeSettings) -> NavBakeError {
    use navmesh_rs::Error;
    let at = e.tile.map_or_else(String::new, |[x, z]| format!(" in tile ({x}, {z})"));
    let advice = format!(
        "lower NavBakeSettings::tile_size (now {}) or raise cell_size (now {})",
        s.tile_size, s.cell_size
    );
    match &e.error {
        Error::LimitExceeded => error(format!(
            "navigation bake limit exceeded{at}: a tile needs more than {} MiB of working memory, \
             {} million work units or its output size cap; {advice}",
            TILE_WORKING_BYTES >> 20,
            TILE_WORK / 1_000_000
        )),
        Error::FormatLimit(what) => error(format!("navigation bake format limit{at}: {what}; {advice}")),
        Error::SizeOverflow => error(format!("navigation bake size overflow{at}; {advice}")),
        other => error(format!("navigation bake failed{at}: {other}")),
    }
}

struct PolygonBounds {
    reference: detour::PolyRef,
    min: Vec3,
    max: Vec3,
}

/// Tiled navmesh resource. Replace it with a new bake to invalidate agent paths.
/// Queries reject partial or truncated paths. Same ordered inputs yield the same
/// results; the backend's `libm` feature removes platform libm differences.
/// ```
/// use ponko::prelude::*;
/// let floor = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(20.0), ponko::d3::Transform::default());
/// let nav = ponko::d3::NavMesh::bake(&[floor], ponko::nav::NavBakeSettings::default()).unwrap();
/// assert!(nav.find_path(Vec3::ZERO, Vec3::X).is_some());
/// ```
pub struct NavMesh3D {
    // Keep the ECS resource's box in place while querying components. Moving this
    // owned state out and back avoids a boxed resource allocation on every tick.
    state: Option<NavState>,
}

struct NavState {
    mesh: detour::NavMesh,
    settings: NavBakeSettings,
    polygons: Vec<PolygonBounds>,
    obstacles: Vec<NavObstacle3D>,
    scratch: Vec<NavObstacle3D>,
    revision: u64,
    crowd: Option<crowd::Crowd>,
    crowd_revision: u64,
    crowd_seen: Vec<bool>,
    crowd_owners: Vec<Option<Entity>>,
    crowd_workspace: Workspace,
    /// Present only when baked with `NavBakeSettings::carving`.
    cache: Option<tilecache::TileCache>,
    carve_handles: Vec<tilecache::ObstacleRef>,
    carved: Vec<NavObstacle3D>,
    pending: bool,
    carve_failed: bool,
}
impl NavMesh3D {
    /// Bakes ordered world triangles with bounded backend memory/work budgets.
    /// Empty triangle soups and nonfinite inputs are errors; an entirely steep
    /// surface successfully produces an empty, unqueryable mesh.
    /// ```
    /// use ponko::prelude::*;
    /// let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// assert!(ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).is_ok());
    /// ```
    pub fn bake(sources: &[NavSource], settings: NavBakeSettings) -> Result<Self, NavBakeError> {
        NavState::bake(sources, settings).map(|state| Self { state: Some(state) })
    }
    /// Nearest unblocked surface within a Euclidean distance, including Y.
    /// ```
    /// use ponko::prelude::*;
    /// # let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// # let nav = ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap();
    /// assert!(nav.sample_position(Vec3::Y, 2.0).is_some());
    /// ```
    #[must_use]
    pub fn sample_position(&self, point: Vec3, max_distance: f32) -> Option<Vec3> {
        self.state.as_ref()?.sample_position(point, max_distance)
    }
    /// Complete string-pulled corners; endpoints must be within two cells of a
    /// surface horizontally and `max_climb + two cell heights` vertically.
    /// Unreachable endpoints return None instead of Detour's partial route.
    /// ```
    /// use ponko::prelude::*;
    /// # let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// # let nav = ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap();
    /// assert!(nav.find_path(Vec3::ZERO, Vec3::X * 2.0).is_some());
    /// ```
    #[must_use]
    pub fn find_path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        self.state.as_ref()?.find_path(from, to)
    }
    /// Replaces world-space blocking boxes, restoring polygons when boxes leave.
    /// Invalid boxes fail closed by blocking every polygon. Overlap tests use
    /// polygon AABBs, so thin boxes can close a much larger area. Carving boxes
    /// instead queue cached-layer rebuilds; call `update_carving` to advance them.
    /// The sim system owns this list when registered: it gathers obstacle components.
    /// ```
    /// use ponko::prelude::*;
    /// # let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// # let mut nav = ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap();
    /// nav.set_obstacles(&[ponko::d3::NavObstacle::new(Vec3::ZERO, Vec3::splat(20.0))]);
    /// assert!(nav.find_path(Vec3::ZERO, Vec3::X).is_none());
    /// nav.set_obstacles(&[]);
    /// ```
    pub fn set_obstacles(&mut self, obstacles: &[NavObstacle3D]) {
        if let Some(state) = &mut self.state {
            state.set_obstacles(obstacles);
        }
    }
    /// Rebuilds at most `budget` cached layers; returns the number of update steps.
    /// Call once per frame for manually managed meshes. The sim system uses one.
    /// ```
    /// # use ponko::prelude::*;
    /// let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// let mut nav = ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap();
    /// assert_eq!(nav.update_carving(1).unwrap(), 0);
    /// ```
    pub fn update_carving(&mut self, budget: usize) -> Result<usize, NavBakeError> {
        self.state.as_mut().map_or(Ok(0), |s| s.update_carving(budget))
    }
    /// Whether queued carving updates remain.
    /// ```
    /// # use ponko::prelude::*;
    /// let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(10.0), ponko::d3::Transform::default());
    /// let nav = ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap();
    /// assert!(!nav.carving_pending());
    /// ```
    pub fn carving_pending(&self) -> bool {
        self.state.as_ref().is_some_and(|s| s.pending || s.carving_changed())
    }
}

impl NavState {
    fn bake(sources: &[NavSource], settings: NavBakeSettings) -> Result<Self, NavBakeError> {
        let s = settings;
        if ![
            s.agent_radius,
            s.agent_height,
            s.max_climb,
            s.max_slope,
            s.cell_size,
            s.cell_height,
        ]
        .iter()
        .all(|x| x.is_finite())
            || s.agent_radius < 0.0
            || s.agent_height <= 0.0
            || s.max_climb < 0.0
            || !(0.0..90.0).contains(&s.max_slope)
            || s.cell_size <= 0.0
            || s.cell_height <= 0.0
            || !(8..=255).contains(&s.tile_size)
        {
            return Err(error("invalid navigation bake settings"));
        }
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        for source in sources {
            for triangle in &source.triangles {
                for p in triangle {
                    if !p.is_finite() {
                        return Err(error("nonfinite source triangle"));
                    }
                    tris.push(i32::try_from(verts.len() / 3).map_err(error)?);
                    verts.extend(p.to_array());
                }
            }
        }
        if tris.is_empty() {
            return Err(error("no source triangles"));
        }
        let mut config = bake::BakeConfig::default();
        config.recast.cs = s.cell_size;
        config.recast.ch = s.cell_height;
        config.recast.walkable_height = (s.agent_height / s.cell_height).ceil() as i32;
        config.recast.walkable_radius = (s.agent_radius / s.cell_size).ceil() as i32;
        config.recast.walkable_climb = (s.max_climb / s.cell_height).floor() as i32;
        config.recast.walkable_slope_angle = s.max_slope;
        config.recast.min_region_area = 0;
        config.recast.merge_region_area = 0;
        config.recast.detail_sample_dist = s.cell_size * 6.0;
        config.recast.detail_sample_max_error = s.cell_height;
        config.walkable_height = s.agent_height;
        config.walkable_radius = s.agent_radius;
        config.walkable_climb = s.max_climb;
        config.strict_triangulation = true;
        let grid = carve::TileGrid::new(&verts, &s)?;
        config.recast.tile_size = grid.tile_size as i32;
        config.tile_limits.max_bytes = TILE_WORKING_BYTES;
        config.tile_limits.max_work = TILE_WORK;
        config.tile_limits.max_tile_bytes = grid.max_tile_bytes();
        // The backend charges every tile's byte cap up front, so the total grows
        // with the tile count instead of capping the level size.
        config.max_total_bytes = SETUP_BYTES + grid.count * config.tile_limits.max_tile_bytes;
        let (mesh, cache) = if s.carving {
            let (mesh, cache) = carve::bake_layers(&verts, &tris, &config, grid.layout())?;
            (mesh, Some(cache))
        } else {
            let baked = bake::tiles(&bake::Input::new(&verts, &tris), &config, &mut Workspace::new())
                .map_err(|e| bake_error(&e, &s))?;
            (
                baked.into_navmesh(Limits::default()).map_err(|e| bake_error(&e, &s))?,
                None,
            )
        };
        let mut polygons = Vec::new();
        for i in 0..mesh.max_tiles() as usize {
            let Some(tile) = mesh.tile(i) else {
                continue;
            };
            let base = mesh.poly_ref_base(i).expect("resident tile has reference");
            for (j, poly) in tile.polys.iter().enumerate() {
                let mut min = Vec3::splat(f32::INFINITY);
                let mut max = Vec3::splat(f32::NEG_INFINITY);
                for &v in &poly.verts[..poly.vert_count as usize] {
                    let at = v as usize * 3;
                    let p = Vec3::from_slice(&tile.verts[at..at + 3]);
                    min = min.min(p);
                    max = max.max(p);
                }
                polygons.push(PolygonBounds {
                    reference: base | j as detour::PolyRef,
                    min,
                    max,
                });
            }
        }
        Ok(Self {
            mesh,
            settings,
            polygons,
            obstacles: Vec::new(),
            scratch: Vec::new(),
            revision: NEXT_REVISION.fetch_add(1, Ordering::Relaxed),
            crowd: None,
            crowd_revision: 0,
            crowd_seen: Vec::new(),
            crowd_owners: Vec::new(),
            crowd_workspace: Workspace::new(),
            cache,
            carve_handles: Vec::new(),
            carved: Vec::new(),
            pending: false,
            carve_failed: false,
        })
    }

    fn sample_position(&self, point: Vec3, max_distance: f32) -> Option<Vec3> {
        if !point.is_finite() || !max_distance.is_finite() || max_distance < 0.0 {
            return None;
        }
        let query = detour::NavMeshQuery::new(&self.mesh, 1).ok()?;
        let (r, p, _) = query
            .find_nearest_poly(&point.to_array(), &[max_distance; 3], &detour::DefaultFilter::default())
            .ok()?;
        let p = Vec3::from_array(p);
        (r != 0 && p.distance(point) <= max_distance).then_some(p)
    }

    fn find_path(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        if !from.is_finite() || !to.is_finite() {
            return None;
        }
        let mut query = detour::NavMeshQuery::new(&self.mesh, 8192).ok()?;
        let s = self.settings;
        let ext = [s.cell_size * 2.0, s.max_climb + s.cell_height * 2.0, s.cell_size * 2.0];
        let filter = detour::DefaultFilter::default();
        let (a, pa, _) = query.find_nearest_poly(&from.to_array(), &ext, &filter).ok()?;
        let (b, pb, _) = query.find_nearest_poly(&to.to_array(), &ext, &filter).ok()?;
        if a == 0 || b == 0 {
            return None;
        }
        let mut path = Vec::new();
        let status = query.find_path(a, b, &pa, &pb, &filter, &mut path, 8192).ok()?;
        if status.has_detail(
            detour::Status::PARTIAL_RESULT | detour::Status::OUT_OF_NODES | detour::Status::BUFFER_TOO_SMALL,
        ) || path.last() != Some(&b)
        {
            return None;
        }
        let mut corners = Vec::new();
        query.find_straight_path(&pa, &pb, &path, &mut corners, 8192, 0).ok()?;
        if corners.last()?.flags & detour::STRAIGHTPATH_END == 0 {
            return None;
        }
        Some(corners.into_iter().map(|p| Vec3::from_array(p.pos)).collect())
    }

    // Resolve heights through connected polygons, never a global nearest lookup
    // that could jump between stacked floors. This work happens only on replans.
    fn agent_route(&self, from: Vec3, to: Vec3) -> Option<Vec<Vec3>> {
        let corners = self.find_path(from, to)?;
        let mut query = detour::NavMeshQuery::new(&self.mesh, 64).ok()?;
        let filter = detour::DefaultFilter::default();
        let (mut poly, mut position, _) = query
            .find_nearest_poly(&corners.first()?.to_array(), &[self.settings.cell_size; 3], &filter)
            .ok()?;
        if poly == 0 {
            return None;
        }
        let mut route = vec![Vec3::from_array(position)];
        let mut visited = Vec::with_capacity(64);
        for segment in corners.windows(2) {
            let count = (segment[0].distance(segment[1]) / self.settings.cell_size)
                .ceil()
                .max(1.0) as usize;
            // Bound route storage for accidental enormous worlds/tiny cells.
            if count > 1_000_000usize.saturating_sub(route.len()) {
                return None;
            }
            for i in 1..=count {
                let target = segment[0].lerp(segment[1], i as f32 / count as f32);
                let (mut next, status) = query
                    .move_along_surface(poly, &position, &target.to_array(), &filter, &mut visited, 64)
                    .ok()?;
                if status.has_detail(detour::Status::BUFFER_TOO_SMALL | detour::Status::OUT_OF_NODES) {
                    return None;
                }
                poly = *visited.last()?;
                // Surface moves can land a few ulps outside an edge; closest-point
                // projection handles that boundary and reads detail height safely.
                next = query.closest_point_on_poly(poly, &next).ok()?.0;
                let point = Vec3::from_array(next);
                if (point - target).with_y(0.0).length() > self.settings.cell_size {
                    return None;
                }
                route.push(point);
                position = next;
            }
        }
        Some(route)
    }

    fn set_obstacles(&mut self, obstacles: &[NavObstacle3D]) {
        if self.obstacles == obstacles {
            return;
        }
        self.obstacles.clear();
        self.obstacles.extend_from_slice(obstacles);
        self.apply_obstacles();
    }
    fn apply_obstacles(&mut self) {
        let radius = Vec3::new(self.settings.agent_radius, 0.0, self.settings.agent_radius);
        for poly in &self.polygons {
            let blocked = self.carve_failed
                || self.obstacles.iter().any(|o| {
                    !(o.carve && self.cache.is_some())
                        && (!o.center.is_finite()
                            || !o.half_extents.is_finite()
                            || o.half_extents.min_element() < 0.0
                            || {
                                let min = o.center - o.half_extents - radius;
                                let max = o.center + o.half_extents + radius;
                                poly.max.cmpge(min - Vec3::Y * self.settings.agent_height).all()
                                    && poly.min.cmple(max).all()
                            })
                });
            self.mesh
                .set_poly_flags(poly.reference, u16::from(!blocked))
                .expect("cached polygon reference");
        }
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }
}

/// World-space axis-aligned blocking box component; no Transform3D is needed.
/// ```
/// use ponko::prelude::*;
/// let obstacle = ponko::d3::NavObstacle::new(Vec3::ZERO, Vec3::ONE);
/// assert_eq!(obstacle.center, Vec3::ZERO);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Component)]
pub struct NavObstacle3D {
    /// Center in world space.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::NavObstacle::new(Vec3::Y, Vec3::ONE).center, Vec3::Y);
    /// ```
    pub center: Vec3,
    /// Nonnegative half sizes along world X,Y,Z.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::NavObstacle::new(Vec3::ZERO, Vec3::ONE).half_extents, Vec3::ONE);
    /// ```
    pub half_extents: Vec3,
    /// Rebuild cached heightfield layers to cut a real hole. Default false. Needs a
    /// mesh baked with `NavBakeSettings::carving`; otherwise this blocks like any
    /// other obstacle.
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(!ponko::d3::NavObstacle::new(Vec3::ZERO, Vec3::ONE).carve);
    /// ```
    pub carve: bool,
}
impl NavObstacle3D {
    /// Creates an axis-aligned world box. Invalid values block the entire mesh.
    /// ```
    /// # use ponko::prelude::*;
    /// let obstacle = ponko::d3::NavObstacle::new(Vec3::Y, Vec3::ONE);
    /// assert_eq!(obstacle.half_extents.y, 1.0);
    /// ```
    #[must_use]
    pub fn new(center: Vec3, half_extents: Vec3) -> Self {
        Self {
            center,
            half_extents,
            carve: false,
        }
    }
}

/// Agent component; its feet follow the surface, without gravity.
/// Enable `avoidance` to use DetourCrowd instead of the original path follower.
/// The original follower retains path storage and allocates only on replans.
/// Crowd mode retains slots/corridors but the backend allocates query scratch.
/// ```
/// use ponko::prelude::*;
/// let mut agent = ponko::d3::NavAgent::default();
/// agent.destination = Some(Vec3::X * 4.0);
/// ```
#[derive(Clone, Debug, Component)]
pub struct NavAgent3D {
    /// Maximum speed in world units per second. Nonpositive/nonfinite stops.
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(ponko::d3::NavAgent::default().speed > 0.0);
    /// ```
    pub speed: f32,
    /// Agent radius, no larger than the bake radius. Defaults to 0.3.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::NavAgent::default().radius, 0.3);
    /// ```
    pub radius: f32,
    /// Maximum acceleration per second. The original follower turns without inertia.
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(ponko::d3::NavAgent::default().acceleration > 0.0);
    /// ```
    pub acceleration: f32,
    /// Requested feet position; changing it triggers a replan. None stops desired
    /// motion (crowd overlap correction may still displace a stopped agent).
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(ponko::d3::NavAgent::default().destination.is_none());
    /// ```
    pub destination: Option<Vec3>,
    /// Opt into DetourCrowd. Disabled by default to preserve existing movement.
    /// Stopped opted-in agents remain neighbours; opted-out agents are ignored.
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(!ponko::d3::NavAgent::default().avoidance);
    /// ```
    pub avoidance: bool,
    /// Nonnegative separation steering weight (not a right-of-way priority).
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::NavAgent::default().avoidance_weight, 2.0);
    /// ```
    pub avoidance_weight: f32,
    /// Neighbour and wall query distance, in world units; must be positive.
    /// ```
    /// # use ponko::prelude::*;
    /// assert_eq!(ponko::d3::NavAgent::default().neighbour_distance, 3.6);
    /// ```
    pub neighbour_distance: f32,
    crowd_id: Option<crowd::AgentId>,
    crowd_revision: u64,
    path: Vec<Vec3>,
    corner: usize,
    revision: u64,
    planned_destination: Option<Vec3>,
    expected_position: Vec3,
    velocity: f32,
}
impl Default for NavAgent3D {
    fn default() -> Self {
        Self {
            speed: 3.0,
            radius: 0.3,
            acceleration: 10.0,
            destination: None,
            avoidance: false,
            avoidance_weight: 2.0,
            neighbour_distance: 3.6,
            crowd_id: None,
            crowd_revision: 0,
            path: Vec::new(),
            corner: 0,
            revision: 0,
            planned_destination: None,
            expected_position: Vec3::NAN,
            velocity: 0.0,
        }
    }
}

/// Register with `Game::sim_system`, just like the exposed physics step.
/// No resource means no-op. Each tick gathers obstacle components, invalidates
/// paths on mesh/obstacle/destination changes or teleports, then moves agents.
/// Stalled agents retry when those inputs change. No window/GPU is required.
/// ```
/// use ponko::prelude::*;
/// Game::new("server", 1, 1).sim_system(systems::step_navmesh3d).run_headless(1, |ctx| {
///     let source = ponko::nav::NavSource::from_mesh(&ponko::render::MeshData::plane(20.0), ponko::d3::Transform::default());
///     ctx.world.insert_resource(ponko::d3::NavMesh::bake(&[source], ponko::nav::NavBakeSettings::default()).unwrap());
///     ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::NavAgent::default()));
/// });
/// ```
pub fn step_navmesh3d(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    step_world(&mut ctx.world, dt);
}

/// What [`step_navmesh3d`] uses, as a declaration: it writes the
/// [`NavMesh3D`] resource (obstacles, carving and the crowd live in it),
/// reads the timestep ([`Service::Time`](crate::access::Service::Time)),
/// reads [`NavObstacle3D`] and writes [`d3::Transform`](crate::d3::Transform) and [`NavAgent3D`].
/// Nothing else: no events, commands, physics or drawing.
///
/// Register the step with it to have
/// [`Game::access_report`](crate::game::Game::access_report) list what it touches
/// and debug builds check that it stays inside it. The step keeps `&mut Ctx`
/// and its place in the order; see [`access`](crate::access).
///
/// ```
/// use ponko::prelude::*;
/// use ponko::systems;
///
/// let game = Game::new("server", 1, 1).add(
///     Phase::AfterPhysics,
///     systems::step_navmesh3d.access(systems::step_navmesh3d_access()),
/// );
/// # let _ = game;
/// ```
///
/// The [`Service`](crate::access::Service) entries of the declaration are
/// documentation: they appear in the report, but services are not recorded,
/// so nothing checks them at run time. Components, resources and events are
/// checked in debug builds.
#[must_use]
pub fn step_navmesh3d_access() -> crate::access::SystemAccess {
    use crate::access::{Service, SystemAccess};
    SystemAccess::new()
        .read_service(Service::Time)
        .write_resource::<NavMesh3D>()
        .read::<NavObstacle3D>()
        .write::<Transform3D>()
        .write::<NavAgent3D>()
}

/// [`step_navmesh3d`] without a `Ctx`: the [`NavMesh3D`] resource, the agent
/// and obstacle components, and the timestep are all it uses.
///
/// The baked state moves out of the resource for the step, so [`step`] can
/// borrow it alongside the world's agent queries, and goes back right after.
/// While it is out the resource reads as empty, but nothing else runs then:
/// `step` doesn't touch resources.
fn step_world(world: &mut World, dt: f32) {
    if !dt.is_finite() || dt <= 0.0 {
        return;
    }
    let Some(mut nav) = world.get_resource_mut::<NavMesh3D>().and_then(|nav| nav.state.take()) else {
        return;
    };
    step(&mut nav, world, dt);
    world.resource_mut::<NavMesh3D>().state = Some(nav);
}

/// One navigation tick on explicit inputs: the baked state, the world whose
/// [`NavObstacle3D`] it reads and whose [`d3::Transform`](crate::d3::Transform) / [`NavAgent3D`] it
/// moves, and a positive, finite timestep.
fn step(nav: &mut NavState, world: &mut World, dt: f32) {
    nav.scratch.clear();
    world.query::<&NavObstacle3D>().for_each(|o| nav.scratch.push(*o));
    if nav.scratch != nav.obstacles {
        let obstacles = std::mem::take(&mut nav.scratch);
        nav.set_obstacles(&obstacles);
        nav.scratch = obstacles;
    }
    if nav.update_carving(1).is_err() {
        nav.carve_failed = true;
        nav.apply_obstacles();
    }
    step_crowd(world, nav, dt);
    world.query::<(&mut Transform3D, &mut NavAgent3D)>().for_each(|(t, a)| {
        if a.avoidance {
            return;
        }
        a.crowd_id = None;
        let valid = a.radius.is_finite()
            && a.radius >= 0.0
            && a.radius <= nav.settings.agent_radius
            && a.speed.is_finite()
            && a.speed > 0.0
            && a.acceleration.is_finite()
            && a.acceleration > 0.0
            && a.destination.is_some_and(|p| p.is_finite())
            && t.position.is_finite();
        if !valid || a.destination.is_none() {
            a.velocity = 0.0;
            a.revision = 0;
            return;
        }
        if a.revision != nav.revision || a.destination != a.planned_destination || t.position != a.expected_position {
            a.path.clear();
            a.corner = 0;
            a.velocity = 0.0;
            if let Some(path) = nav.agent_route(t.position, a.destination.unwrap()) {
                a.path.extend(path);
            }
            a.revision = nav.revision;
            a.planned_destination = a.destination;
        }
        a.velocity = (a.velocity + a.acceleration * dt).min(a.speed);
        let mut remaining = a.velocity * dt;
        while let Some(&target) = a.path.get(a.corner) {
            let delta = target - t.position;
            let distance = delta.length();
            if distance <= remaining {
                t.position = target;
                remaining -= distance;
                a.corner += 1;
            } else {
                t.position += delta / distance * remaining;
                break;
            }
        }
        if a.corner == a.path.len() {
            a.velocity = 0.0;
        }
        a.expected_position = t.position;
    });
}

// Persistent slots preserve velocity/corridors; recreating a crowd is limited to
// mesh/obstacle revisions and capacity growth. No entity-index scan per neighbour.
fn step_crowd(world: &mut World, nav: &mut NavState, dt: f32) {
    let count = world.query::<&NavAgent3D>().iter().filter(|a| a.avoidance).count();
    if count == 0 && nav.crowd.is_none() {
        return;
    }
    let capacity = nav.crowd.as_ref().map_or(0, crowd::Crowd::max_agents);
    if nav.crowd_revision != nav.revision || count.min(16383) > capacity {
        let size = count.max(16).next_power_of_two().min(16383);
        nav.crowd = crowd::Crowd::new(
            &mut nav.crowd_workspace,
            &nav.mesh,
            size,
            (nav.settings.agent_radius * 1.3).max(0.001),
        )
        .ok();
        if let Some(c) = &mut nav.crowd {
            c.set_fixes(navmesh_rs::UpstreamFixes::ALL);
        }
        nav.crowd_seen.resize(size, false);
        nav.crowd_owners.clear();
        nav.crowd_owners.resize(size, None);
        nav.crowd_revision = nav.revision;
        // Also invalidate slots on capacity growth without changing mesh paths.
        world.query::<&mut NavAgent3D>().for_each(|a| a.crowd_id = None);
    }
    let Some(c) = &mut nav.crowd else {
        return;
    };
    nav.crowd_seen.fill(false);
    world
        .query::<(Entity, &mut Transform3D, &mut NavAgent3D)>()
        .for_each(|(entity, t, a)| {
            if !a.avoidance {
                return;
            }
            a.revision = 0;
            // Component clones and restored transforms cannot inherit another entity's slot.
            if a.crowd_id
                .is_some_and(|id| nav.crowd_owners.get(id.0) != Some(&Some(entity)))
            {
                a.crowd_id = None;
            }
            if a.crowd_revision != nav.crowd_revision {
                a.crowd_id = None;
            }
            let valid = t.position.is_finite()
                && a.radius.is_finite()
                && a.radius > 0.0
                && a.radius <= nav.settings.agent_radius
                && a.speed.is_finite()
                && a.speed > 0.0
                && a.acceleration.is_finite()
                && a.acceleration > 0.0
                && a.avoidance_weight.is_finite()
                && a.avoidance_weight >= 0.0
                && a.neighbour_distance.is_finite()
                && a.neighbour_distance > 0.0;
            if !valid {
                a.crowd_id = None;
                return;
            }
            if t.position != a.expected_position
                && let Some(id) = a.crowd_id.take()
            {
                let _ = c.remove_agent(id);
            }
            let mut p = crowd::AgentParams::default();
            p.radius = a.radius * 1.3;
            p.height = nav.settings.agent_height;
            p.max_speed = a.speed;
            p.max_acceleration = a.acceleration;
            p.collision_query_range = a.neighbour_distance;
            p.separation_weight = a.avoidance_weight;
            let fresh = a.crowd_id.is_none();
            if fresh {
                a.crowd_id = c
                    .add_agent(&mut nav.crowd_workspace, &nav.mesh, t.position.to_array(), p)
                    .ok();
            }
            let Some(id) = a.crowd_id else {
                return;
            };
            nav.crowd_seen[id.0] = true;
            nav.crowd_owners[id.0] = Some(entity);
            let _ = c.update_agent_parameters(id, p);
            if fresh || a.destination != a.planned_destination {
                let _ = c.reset_move_target(id);
                // Preserve complete-route semantics, including disconnected targets.
                if let Some(to) = a.destination.filter(|to| nav.mesh.max_tiles() > 0 && to.is_finite()) {
                    let query = detour::NavMeshQuery::new(&nav.mesh, 8192).ok();
                    if let Some(mut q) = query {
                        let ext = [
                            nav.settings.cell_size * 2.0,
                            nav.settings.max_climb + nav.settings.cell_height * 2.0,
                            nav.settings.cell_size * 2.0,
                        ];
                        let filter = detour::DefaultFilter::default();
                        if let (Ok((from_ref, from, _)), Ok((to_ref, target, _))) = (
                            q.find_nearest_poly(&t.position.to_array(), &ext, &filter),
                            q.find_nearest_poly(&to.to_array(), &ext, &filter),
                        ) {
                            let mut path = Vec::new();
                            if from_ref != 0
                                && to_ref != 0
                                && q.find_path(from_ref, to_ref, &from, &target, &filter, &mut path, 8192)
                                    .is_ok_and(|s| {
                                        !s.has_detail(
                                            detour::Status::PARTIAL_RESULT
                                                | detour::Status::OUT_OF_NODES
                                                | detour::Status::BUFFER_TOO_SMALL,
                                        )
                                    })
                                && path.last() == Some(&to_ref)
                            {
                                let _ = c.request_move_target(&nav.mesh, id, to_ref, target);
                            }
                        }
                    }
                }
                a.planned_destination = a.destination;
            }
            a.crowd_revision = nav.crowd_revision;
        });
    for (i, seen) in nav.crowd_seen.iter().enumerate() {
        if !seen && c.agent(crowd::AgentId(i)).is_some() {
            let _ = c.remove_agent(crowd::AgentId(i));
            nav.crowd_owners[i] = None;
        }
    }
    // Bound simulation steps for collision prediction during low-rate ticks.
    let steps = (dt / (1.0 / 60.0)).ceil().clamp(1.0, 64.0) as u32;
    for _ in 0..steps {
        if c.update(&mut nav.crowd_workspace, &nav.mesh, dt / steps as f32)
            .is_err()
        {
            break;
        }
    }
    world
        .query::<(Entity, &mut Transform3D, &mut NavAgent3D)>()
        .for_each(|(entity, t, a)| {
            if a.avoidance
                && let Some(agent) = a
                    .crowd_id
                    .filter(|id| nav.crowd_owners[id.0] == Some(entity))
                    .and_then(|id| c.agent(id))
            {
                t.position = Vec3::from_array(agent.npos);
                a.expected_position = t.position;
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec2;

    const DT: f32 = 0.02;

    fn baked() -> NavMesh3D {
        let source = NavSource::from_mesh(&MeshData::plane(20.0), Transform3D::default());
        NavMesh3D::bake(&[source], NavBakeSettings::default()).unwrap()
    }

    /// Path followers and crowd agents crossing the plane, and an obstacle.
    fn agents(world: &mut World) -> Vec<Entity> {
        world.spawn((NavObstacle3D::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0)),));
        (0..12)
            .map(|i| {
                let at = Vec3::new(i as f32 - 6.0, 0.0, -6.0);
                let agent = NavAgent3D {
                    destination: Some(Vec3::new(-at.x, 0.0, 6.0)),
                    avoidance: i % 2 == 0,
                    ..NavAgent3D::default()
                };
                world.spawn((Transform3D::at(at), agent))
            })
            .collect()
    }

    fn positions(world: &World, entities: &[Entity]) -> Vec<[u32; 3]> {
        entities
            .iter()
            .map(|&e| {
                world
                    .get::<Transform3D>(e)
                    .unwrap()
                    .position
                    .to_array()
                    .map(f32::to_bits)
            })
            .collect()
    }

    #[test]
    fn the_core_step_needs_only_its_inputs_and_matches_the_ctx_system() {
        // The core, on explicit state and a world without the resource.
        let mut nav = baked().state.unwrap();
        let mut world = World::new();
        let bare = agents(&mut world);
        let before = positions(&world, &bare);
        for _ in 0..150 {
            step(&mut nav, &mut world, DT);
        }

        // The public system, through a Ctx and the resource.
        let mut ctx = Ctx::new(f64::from(DT), Vec2::ZERO);
        ctx.world.insert_resource(baked());
        let wrapped = agents(&mut ctx.world);
        for _ in 0..150 {
            ctx.set_in_sim(true);
            step_navmesh3d(&mut ctx);
            ctx.set_in_sim(false);
        }

        let moved = positions(&world, &bare);
        assert!(moved.iter().zip(&before).all(|(a, b)| a != b), "every agent walked");
        assert_eq!(moved, positions(&ctx.world, &wrapped));
        assert!(
            ctx.world.resource::<NavMesh3D>().state.is_some(),
            "the state is put back"
        );
    }

    #[test]
    fn the_resource_step_skips_bad_timesteps_and_missing_meshes_and_puts_the_state_back() {
        let mut world = World::new();
        let walker = agents(&mut world)[1];
        step_world(&mut world, DT); // No mesh yet: nothing happens.
        let start = world.get::<Transform3D>(walker).unwrap().position;
        world.insert_resource(baked());
        for dt in [0.0, -DT, f32::NAN, f32::INFINITY] {
            step_world(&mut world, dt);
            assert_eq!(world.get::<Transform3D>(walker).unwrap().position, start);
        }
        step_world(&mut world, DT);
        assert_ne!(world.get::<Transform3D>(walker).unwrap().position, start);
        assert!(world.resource::<NavMesh3D>().state.is_some());
    }
}
