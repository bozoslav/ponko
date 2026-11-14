//! Static instance batches: persistent instance records, culled per view
//! and distance level, drawn with as few commands as the device allows.
//!
//! A batch takes one of two paths, chosen when it is made
//! ([`Renderer3D::set_instance_batch`]):
//!
//! - **GPU-driven** (devices with compute culling, batches the measured
//!   crossover or the game's [`BatchCulling`] sends here: one level, at
//!   least [`GPU_BATCH_MIN`] instances, or [`GPU_BATCH_TRIANGLES`]
//!   triangles over all instances, and no see-through material): the batch's records and
//!   cells live in a [`Pool`] shared by all such batches, and each of its
//!   views (a batch level seen by a camera or a shadow view) has a visible
//!   list in one of the pool's [`Page`]s. Every frame a four-stage compute
//!   sequence per page culls all its views (a table of *jobs* says which), writing
//!   each job's visible instance indices and the instance counts of its
//!   indirect draw arguments. A level is then drawn with one indirect draw
//!   per material, whatever is visible. A pool gets another page when its
//!   views outgrow one buffer, so a view always has a place.
//! - **CPU** (WebGL2 and other devices without compute culling, small
//!   batches, and batches that exceed a device limit): the batch's cells are
//!   culled on the CPU and the instances drawn straight from a vertex
//!   buffer, one draw per run of neighbouring visible instances. Distances
//!   are tested per instance where a cell straddles a band's edge, so both
//!   paths draw the same instances but for those outside the view or hidden
//!   behind others, which leave no pixels.
//!
//! Every device limit a batch can hit is checked before the GPU is asked:
//! a batch or a view that doesn't fit falls back or is skipped, and says so
//! once (`traffic::too_big`), instead of failing wgpu's validation.
use super::*;
use crate::render::{BatchCulling, InstanceBatchStats, InstanceLevel};
use crate::three::instances::{Batch, Cell};

use crate::render::capabilities::{GPU_BATCH_MIN, GPU_BATCH_TRIANGLES};

/// Bytes of an instance's record in a pool (six vec4s) and of a cell's.
const RECORD: u64 = 96;
const CELL: u64 = 32;
/// Bytes of an instance on the CPU path's vertex buffer.
const VERTEX: u64 = size_of::<Instance3D>() as u64;
/// Bytes of one indirect draw's arguments.
const ARGS: u64 = 20;
/// Words of a job in a page's table, and floats of a view in the frame's.
const JOB: usize = 12;
const VIEW: usize = 28;
/// The first buffer of each kind a pool makes holds this much (it doubles).
const FIRST_BYTES: u64 = 64 << 10;
/// Keep empty buffers across short streaming gaps, then return their memory.
const EMPTY_FRAMES: u8 = 8;

/// Buffer identities survive replacement of the arena, page or pool slot.
fn next_generation() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// Which path the batches made from now on take.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum BatchPath {
    /// The renderer picks: the GPU-driven path where the device has it and
    /// the batch is large enough to gain from it.
    #[default]
    Auto,
    /// CPU cell culling and direct draws, also where compute culling exists.
    #[cfg_attr(not(test), allow(dead_code))]
    Cpu,
    /// Compute culling and indirect draws wherever the device can, also
    /// for small batches.
    #[cfg_attr(not(test), allow(dead_code))]
    Gpu,
}

/// Why a batch is on the path it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// Culled by the compute pass, drawn indirectly.
    Gpu,
    /// On the CPU: under the crossover (fewer than [`GPU_BATCH_MIN`]
    /// instances, a see-through material or several levels), or forced.
    CpuSmall,
    /// On the CPU: the device has no compute culling.
    CpuNoCompute,
    /// On the CPU: its records are more than one storage binding holds.
    CpuOverLimit,
    /// Not drawn: its instances are more than a buffer holds.
    Dropped,
}

/// A call into wgpu from the batch drawing code. In test builds, what the
/// call allocates is wgpu's, not the batch code's: a test asserts that the
/// batch code itself allocates nothing while a frame is recorded.
macro_rules! gpu {
    ($call:expr) => {{
        #[cfg(test)]
        let _wgpu = crate::three::lights::tests::unmarked();
        $call
    }};
}

/// One view's frustum and eye, for this frame's culling of every batch.
#[derive(Clone, Copy, PartialEq)]
struct SlotView {
    planes: [glam::Vec4; 6],
    eye: Vec3,
}

/// The renderer's batch settings and per-frame scratch (kept between
/// frames, so a steady frame allocates nothing).
#[derive(Default)]
pub(crate) struct Batching {
    /// The path batches made from now on take (tests and benchmarks force one).
    pub path: BatchPath,
    /// This frame's views, by slot.
    slots: Vec<SlotView>,
    /// Every live batch level as (mesh, batch, level), sorted: batches of
    /// one mesh are drawn together, so they share its buffers, pipeline
    /// and material.
    order: Vec<(u32, u32, u32)>,
    /// The limits already reported (see `report`), so each is said once.
    reported: u32,
    /// Views not drawn this frame because of a device limit.
    skipped: u32,
    /// Whether a dispatch had more cells or jobs than the device takes.
    clamped: bool,
    /// The batches already reported as over a limit, by id: an update of
    /// such a batch doesn't say it again.
    said: Vec<u32>,
    /// The culling dispatches of the last prepared frame.
    dispatches: u32,
    /// The jobs (batch, view and level) culled in the last prepared frame.
    jobs: u32,
    /// Whether the compute pass tests the camera's jobs against the depth grid.
    occlusion: bool,
    /// Whether the frame had more views than its buffer takes.
    capped: bool,
    /// How often a CPU-path list was built since the renderer was made
    /// (tests watch it: a view at rest builds none).
    built: u32,
}

/// What `Batching::reported` flags.
const SKIPPED_VIEW: u32 = 1;
const GRID_TOO_BIG: u32 = 2;
const TOO_MANY_GROUPS: u32 = 4;
const TOO_MANY_VIEWS: u32 = 8;

/// Views the frame's buffer always has room for, whatever the occlusion
/// grid would like: the grid gives way first (batches then go without
/// occlusion culling, which only draws more).
const MIN_VIEWS: usize = 64;

/// The room in the frame's buffer, of the `most` floats one storage
/// binding takes: the floats before the views (the header and the
/// occlusion grid's depths, `grid` if they fit) and the most views after.
fn frame_room(most: u64, grid: u64) -> (u64, usize) {
    let grid = grid.min(most.saturating_sub((MIN_VIEWS * VIEW) as u64)).max(24);
    let views = most.saturating_sub(grid) / VIEW as u64;
    (grid, usize::try_from(views).unwrap_or(usize::MAX))
}

/// A buffer that hands out ranges, grows by doubling up to the device's
/// limit (keeping its contents), and takes ranges back.
struct Arena {
    buffer: wgpu::Buffer,
    label: &'static str,
    usage: wgpu::BufferUsages,
    capacity: u64,
    /// The most bytes it may grow to: a multiple of `align`.
    most: u64,
    /// Every range starts at a multiple of this and is a multiple long.
    align: u64,
    /// Everything at and after this is free.
    end: u64,
    /// Freed ranges below `end` as (start, bytes), sorted, never touching.
    free: Vec<(u64, u64)>,
    /// Bumped whenever `buffer` is replaced: bind groups made with an
    /// older one are stale.
    generation: u32,
}

impl Arena {
    /// `reserved`: bytes at the start that are never handed out.
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        align: u64,
        reserved: u64,
        first: u64,
    ) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let most = traffic::most_bytes(device, usage) as u64 / align * align;
        let end = reserved.div_ceil(align) * align;
        let capacity = FIRST_BYTES.max((end + first).next_power_of_two()).min(most).max(align);
        Self {
            buffer: Self::make(device, label, usage, capacity),
            label,
            usage,
            capacity,
            most,
            align,
            end,
            free: Vec::new(),
            generation: next_generation(),
        }
    }

    fn make(device: &wgpu::Device, label: &'static str, usage: wgpu::BufferUsages, size: u64) -> wgpu::Buffer {
        device.new_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage,
            mapped_at_creation: false,
        })
    }

    fn rounded(&self, bytes: u64) -> u64 {
        bytes.max(1).div_ceil(self.align) * self.align
    }

    /// A range of `bytes`, or `None` when the device's limit is reached.
    fn alloc(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: u64) -> Option<u64> {
        let size = self.rounded(bytes);
        if let Some(i) = self.free.iter().position(|&(_, s)| s >= size) {
            let (at, s) = self.free[i];
            if s == size {
                self.free.remove(i);
            } else {
                self.free[i] = (at + size, s - size);
            }
            return Some(at);
        }
        let at = self.end;
        let needed = at.checked_add(size)?;
        if needed > self.most {
            return None;
        }
        if needed > self.capacity {
            let capacity = needed.next_power_of_two().min(self.most);
            let bigger = Self::make(device, self.label, self.usage, capacity);
            if self.end > 0 {
                // Writes queued for the old buffer land before this copy.
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_buffer_to_buffer(&self.buffer, 0, &bigger, 0, Some(self.end));
                queue.submit([encoder.finish()]);
            }
            self.buffer = bigger;
            self.capacity = capacity;
            self.generation = next_generation();
            traffic::reallocated();
        }
        self.end = needed;
        Some(at)
    }

    /// Takes the range `alloc(bytes)` returned as `at` back.
    fn release(&mut self, at: u64, bytes: u64) {
        let size = self.rounded(bytes);
        let i = self.free.partition_point(|&(start, _)| start < at);
        self.free.insert(i, (at, size));
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
        if let Some(&(start, s)) = self.free.last()
            && start + s == self.end
        {
            self.end = start;
            self.free.pop();
        }
    }
}

/// The records and cells of a group of GPU-driven batches, and the pages
/// their views' lists are in.
struct Pool {
    records: Arena,
    pages: Vec<Page>,
    /// Live batches in the pool.
    batches: u32,
    /// Whether it ever held a batch: an emptied pool gives its memory back.
    used: bool,
    idle: u8,
}

/// Bytes at the start of a page's visibility buffer kept for its root.
const ROOT: u64 = 32;

/// Visible lists and indirect draw arguments of batch views that one
/// dispatch culls together. A pool's views fill its pages one after
/// another: a view that doesn't fit the pages there are gets a new one.
struct Page {
    /// The root, this frame's jobs, and the views' visible lists.
    visibility: Arena,
    args: Arena,
    /// The culling pass's bind group, and the generations of the four
    /// buffers it was made with (records, visibility, arguments, frame).
    bind: Option<(wgpu::BindGroup, [u32; 4])>,
    /// This frame's jobs.
    table: Vec<u32>,
    /// Where the jobs are in `visibility`: (start, bytes). It always has
    /// room for a job per view of the page, so a frame never has to find
    /// room for it.
    table_at: (u64, u64),
    /// What the root and the jobs hold on the GPU, and the visibility
    /// generation and table start they were written to.
    root_mirror: Mirror,
    table_mirror: Mirror,
    mirrored: (u32, u64),
    /// This frame's jobs and the workgroups they take together.
    jobs: u32,
    groups: u32,
    scans: u32,
    /// The views that have their lists here.
    views: u32,
    /// Whether it ever held a view: an emptied page gives its memory back.
    used: bool,
    idle: u8,
}

impl Page {
    fn new(device: &wgpu::Device, align: u64, first: u64) -> Self {
        let storage = wgpu::BufferUsages::STORAGE;
        Self {
            visibility: Arena::new(device, "static batch visibility", storage, align, ROOT, first),
            args: Arena::new(
                device,
                "static batch indirect arguments",
                storage | wgpu::BufferUsages::INDIRECT,
                4,
                0,
                0,
            ),
            bind: None,
            table: Vec::new(),
            table_at: (0, 0),
            root_mirror: Mirror::default(),
            table_mirror: Mirror::default(),
            mirrored: (0, 0),
            jobs: 0,
            groups: 0,
            scans: 0,
            views: 0,
            used: false,
            idle: 0,
        }
    }

    /// Room for one more view: a visible list of `list` bytes, `arguments`
    /// bytes of draw arguments and a job in the table. (list start,
    /// arguments start), or `None` when the page is full.
    fn place(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, list: u64, arguments: u64) -> Option<(u64, u64)> {
        let at = self.visibility.alloc(device, queue, list)?;
        let Some(args) = self.args.alloc(device, queue, arguments) else {
            self.visibility.release(at, list);
            return None;
        };
        let jobs = u64::from(self.views + 1) * (JOB as u64 * 4);
        if jobs > self.table_at.1 {
            // Room to spare, so a few more views don't move it; just
            // enough when the page is nearly full.
            let grown = [jobs.next_power_of_two().max(1024), jobs]
                .into_iter()
                .find_map(|size| self.visibility.alloc(device, queue, size).map(|at| (at, size)));
            let Some(grown) = grown else {
                self.visibility.release(at, list);
                self.args.release(args, arguments);
                return None;
            };
            if self.table_at.1 > 0 {
                self.visibility.release(self.table_at.0, self.table_at.1);
            }
            self.table_at = grown;
        }
        self.views += 1;
        self.used = true;
        self.idle = 0;
        Some((at, args))
    }

    fn bytes(&self) -> u64 {
        self.visibility.capacity + self.args.capacity
    }
}

impl Pool {
    fn new(device: &wgpu::Device, align: u64, first: u64) -> Self {
        Self {
            records: Arena::new(
                device,
                "static batch records",
                wgpu::BufferUsages::STORAGE,
                align,
                0,
                first,
            ),
            pages: Vec::new(),
            batches: 0,
            used: false,
            idle: 0,
        }
    }

    fn bytes(&self) -> u64 {
        self.records.capacity + self.pages.iter().map(Page::bytes).sum::<u64>()
    }
}

/// One batch level in one view: where its visible list and its draw
/// arguments are in the pool, and the bind group its draws read through.
pub(crate) struct BatchView {
    /// The batch's records and this view's visible list, and the buffer
    /// generations (records, visibility) it was made with.
    read: Option<(wgpu::BindGroup, [u32; 2])>,
    /// Which of the pool's pages holds the two ranges below.
    page: usize,
    /// (start, bytes) in the page's visibility buffer.
    visibility: (u64, u64),
    /// Start in the page's argument buffer, in bytes.
    args: u64,
    /// The level's distance band this frame.
    band: (f32, f32),
    slot: usize,
    level: usize,
    active: bool,
}

/// A GPU-driven batch's place in its pool.
struct Pooled {
    pool: usize,
    /// (start, bytes) of its records and cells.
    records: (u64, u64),
}

/// What a CPU-path list was built from (the view and the level's band;
/// `None` when it has to be built again), and whether this frame drew it.
type Seen = (Option<(SlotView, (f32, f32))>, bool);

pub(crate) struct StaticBatch {
    pub levels: Vec<InstanceLevel>,
    /// The CPU path's instance vertices. GPU-driven draws bind one
    /// zero-stride marker instead and read the pool.
    vertices: Option<wgpu::Buffer>,
    pooled: Option<Pooled>,
    pub views: Vec<BatchView>,
    cells: Vec<Cell>,
    min: Vec3,
    max: Vec3,
    /// The CPU path's draws by view and level: runs of instances.
    pub visible: Vec<Vec<(u32, u32)>>,
    /// What each list of `visible` was culled for (a view and a band),
    /// when that alone decided it: an unchanged view keeps its list (a
    /// light's shadow views, a camera at rest). And whether the list is
    /// this frame's.
    seen: Vec<Seen>,
    /// Four centres/radii (legacy) or origins (levels) at a time. Prepared
    /// once; boundary cells evaluate exact distances with SIMD, without gathers.
    distances: Vec<DistanceQuad>,
    pub(super) compute: bool,
    pub(crate) route: Route,
    arguments: Vec<Vec<u32>>,
    pub options: crate::render::InstanceBatchOptions,
    legacy_distance: bool,
    pub count: u32,
}

struct DistanceQuad {
    x: glam::Vec4,
    y: glam::Vec4,
    z: glam::Vec4,
    radius: glam::Vec4,
}
impl DistanceQuad {
    fn distances(&self, eye: Vec3) -> glam::Vec4 {
        let x = self.x - glam::Vec4::splat(eye.x);
        let y = self.y - glam::Vec4::splat(eye.y);
        let z = self.z - glam::Vec4::splat(eye.z);
        (x * x + y * y + z * z).sqrt() - self.radius
    }
}

impl StaticBatch {
    pub(super) fn view(&self, slot: usize, level: usize) -> Option<&BatchView> {
        self.views
            .iter()
            .find(|v| v.slot == slot && v.level == level && v.active && v.read.is_some())
    }
    pub(super) fn casts(&self) -> bool {
        self.options.cast_shadows && self.levels.iter().any(|l| l.shadows)
    }
    pub(super) fn shadow_distance(&self) -> bool {
        self.casts()
            && if self.legacy_distance {
                self.options.max_distance.is_finite()
            } else {
                self.levels
                    .iter()
                    .any(|l| l.shadows && (l.min_distance > 0.0 || l.max_distance.is_finite()))
            }
    }
    /// Whether the level has anything to draw in the view.
    fn draws(&self, slot: usize, level: usize) -> bool {
        if self.compute {
            self.view(slot, level).is_some()
        } else {
            self.visible
                .get(slot * self.levels.len() + level)
                .is_some_and(|v| !v.is_empty())
        }
    }
}

pub(super) struct Culling {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::ComputePipeline,
    /// Prefixes cell counts and scatters instances in source order.
    scan: wgpu::ComputePipeline,
    prefix: wgpu::ComputePipeline,
    scatter: wgpu::ComputePipeline,
    /// What every page's dispatch reads of the frame: the occlusion
    /// grid's header, its depths, then the views.
    occlusion: wgpu::Buffer,
    /// What `occlusion` holds (only what changed is uploaded).
    header_mirror: Mirror,
    grid_mirror: Mirror,
    views_mirror: Mirror,
    /// Floats of `occlusion` before the views (the header and the most
    /// depths), and how many views fit after them.
    grid_floats: u64,
    view_room: usize,
    /// The most views the buffer takes within one storage binding.
    max_views: usize,
    /// This frame's views, as the buffer holds them.
    views: Vec<f32>,
    /// Bumped whenever `occlusion` is replaced.
    frame_generation: u32,
    /// One attribute marker, shared by all batch draws with zero instance stride.
    pub(super) marker: wgpu::Buffer,
    pools: Vec<Pool>,
    /// What storage bindings start at multiples of.
    align: u64,
    /// The most workgroups one dispatch dimension takes, and how many the
    /// culling dispatch lays out per row and layer.
    max_groups: u32,
    side: u32,
}

impl Culling {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("static culling"),
            entries: &(0..4)
                .map(|i| wgpu::BindGroupLayoutEntry {
                    binding: i,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage {
                            read_only: i == 0 || i == 3,
                        },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                })
                .collect::<Vec<_>>(),
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("static culling"),
            source: wgpu::ShaderSource::Wgsl(include_str!("instances.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry_point| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("static culling"),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry_point),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let limits = device.limits();
        // The whole buffer is one storage binding: it stays within the
        // device's limit, the grid giving way to the views.
        let most = traffic::most_bytes(device, wgpu::BufferUsages::STORAGE) as u64 / 4;
        let (grid_floats, max_views) = frame_room(most, (limits.max_texture_dimension_2d as u64 / 8 + 1) * 160 + 32);
        let view_room = MIN_VIEWS.min(max_views).max(1);
        let occlusion = Self::frame_buffer(device, grid_floats, view_room);
        let mut marker = Instance3D::default();
        marker.model[0][3] = -2.0;
        let marker = device
            .uploading(Upload::StaticBatches)
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("static instance marker"),
                contents: bytemuck::bytes_of(&marker),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let max_groups = limits.max_compute_workgroups_per_dimension;
        Self {
            layout,
            pipeline: pipeline("main"),
            scan: pipeline("scan"),
            prefix: pipeline("prefix"),
            scatter: pipeline("scatter"),
            occlusion,
            header_mirror: Default::default(),
            grid_mirror: Default::default(),
            views_mirror: Default::default(),
            grid_floats,
            view_room,
            max_views,
            views: Vec::new(),
            frame_generation: 0,
            marker,
            pools: Vec::new(),
            // Records are vec4s: their ranges start on one, too.
            align: u64::from(limits.min_storage_buffer_offset_alignment).max(16),
            max_groups,
            side: max_groups.min(256),
        }
    }

    fn frame_buffer(device: &wgpu::Device, grid_floats: u64, views: usize) -> wgpu::Buffer {
        device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("static culling frame"),
            size: (grid_floats + (views * VIEW) as u64) * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | crate::render::traffic::MIRRORED,
            mapped_at_creation: false,
        })
    }

    /// Makes room for `views` views in the frame's buffer.
    fn fit_views(&mut self, device: &wgpu::Device, views: usize) {
        let views = views.min(self.max_views);
        if views > self.view_room {
            self.view_room = views.next_power_of_two().min(self.max_views);
            self.occlusion = Self::frame_buffer(device, self.grid_floats, self.view_room);
            self.header_mirror.forget();
            self.grid_mirror.forget();
            self.views_mirror.forget();
            self.frame_generation += 1;
            traffic::reallocated();
        }
    }

    /// Room for `bytes` of records in a pool: (pool, start). Makes a pool
    /// when the others are full; `None` when no pool could ever hold it.
    fn place(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: u64) -> Option<(usize, u64)> {
        for (i, pool) in self.pools.iter_mut().enumerate() {
            if let Some(at) = pool.records.alloc(device, queue, bytes) {
                return Some((i, at));
            }
        }
        let mut pool = Pool::new(device, self.align, bytes);
        let at = pool.records.alloc(device, queue, bytes)?;
        self.pools.push(pool);
        Some((self.pools.len() - 1, at))
    }

    /// Room for a view of a batch in `pool`: (page, list start, arguments
    /// start). The pool gets another page when the ones it has are full;
    /// `None` only when the view doesn't fit an empty page.
    fn place_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pool: usize,
        list: u64,
        arguments: u64,
    ) -> Option<(usize, u64, u64)> {
        let pool = &mut self.pools[pool];
        for (i, page) in pool.pages.iter_mut().enumerate() {
            if let Some((at, args)) = page.place(device, queue, list, arguments) {
                return Some((i, at, args));
            }
        }
        let mut page = Page::new(device, self.align, list);
        let (at, args) = page.place(device, queue, list, arguments)?;
        pool.pages.push(page);
        Some((pool.pages.len() - 1, at, args))
    }

    /// Gives a freed batch's ranges back to its pool.
    fn release(&mut self, batch: &StaticBatch) {
        let Some(pooled) = &batch.pooled else { return };
        let pool = &mut self.pools[pooled.pool];
        pool.records.release(pooled.records.0, pooled.records.1);
        for view in &batch.views {
            let page = &mut pool.pages[view.page];
            page.visibility.release(view.visibility.0, view.visibility.1);
            page.args
                .release(view.args, batch.arguments[view.level].len() as u64 * 4);
            page.views -= 1;
        }
        pool.batches -= 1;
    }
}

/// `bytes` of `buffer` from `at`, as a binding.
fn range(buffer: &wgpu::Buffer, (at, bytes): (u64, u64)) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer,
        offset: at,
        size: std::num::NonZeroU64::new(bytes),
    })
}

fn planes(matrix: Mat4) -> [glam::Vec4; 6] {
    let r = [matrix.row(0), matrix.row(1), matrix.row(2), matrix.row(3)];
    [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]]
        .map(|p| p / p.truncate().length().max(f32::MIN_POSITIVE))
}
fn intersects(min: Vec3, max: Vec3, planes: &[glam::Vec4; 6], eye: Vec3, near: f32, far: f32) -> bool {
    let center = (min + max) * 0.5;
    let radius = (max - min).length() * 0.5;
    let distance = center.distance(eye);
    distance - radius <= far
        && distance + radius >= near
        && planes.iter().all(|p| p.truncate().dot(center) + p.w >= -radius)
}

/// Adds instances `start..start + count` to a view's draws, joined to the
/// run before them when they follow it.
fn push_run(runs: &mut Vec<(u32, u32)>, start: u32, count: u32) {
    match runs.last_mut() {
        Some(last) if last.0 + last.1 == start => last.1 += count,
        _ => runs.push((start, count)),
    }
}

/// The squared distances from `eye` to the nearest and the farthest point
/// of the box.
fn box_distances(min: Vec3, max: Vec3, eye: Vec3) -> (f32, f32) {
    let near = (min - eye).max(eye - max).max(Vec3::ZERO);
    let far = (min - eye).abs().max((max - eye).abs());
    (near.length_squared(), far.length_squared())
}

/// The CPU path: the instances of `batch` a view draws at one level, as
/// runs. A cell outside the view (or hidden) is left out whole; one wholly
/// inside the level's distance band is taken whole; in between, each
/// instance's distance decides, as the compute pass decides it.
fn cull_cells(
    batch: &StaticBatch,
    runs: &mut Vec<(u32, u32)>,
    view: &SlotView,
    (near, far): (f32, f32),
    hides: impl Fn(Vec3, Vec3) -> bool,
) {
    let legacy = batch.legacy_distance;
    for cell in &batch.cells {
        let (near2, far2) = box_distances(cell.min, cell.max, view.eye);
        if !legacy && (near2 >= far * far || far2 < near * near) {
            continue;
        }
        if !intersects(
            cell.min,
            cell.max,
            &view.planes,
            view.eye,
            0.0,
            if legacy { far } else { f32::INFINITY },
        ) || hides(cell.min, cell.max)
        {
            continue;
        }
        let whole = if legacy {
            // Every sphere is inside the box: none is farther than it.
            far2 <= far * far
        } else {
            near2 >= near * near && far2 < far * far
        };
        if whole {
            push_run(runs, cell.start, cell.count);
            continue;
        }
        let end = cell.start + cell.count;
        for quad in cell.start / 4..end.div_ceil(4) {
            let d = batch.distances[quad as usize].distances(view.eye);
            let mask = if legacy {
                d.cmple(glam::Vec4::splat(far)).bitmask()
            } else {
                (d.cmpge(glam::Vec4::splat(near)) & d.cmplt(glam::Vec4::splat(far))).bitmask()
            };
            let start = (quad * 4).max(cell.start);
            let stop = (quad * 4 + 4).min(end);
            if mask == 15 {
                push_run(runs, start, stop - start);
            } else {
                for i in start..stop {
                    if mask & (1 << (i % 4)) != 0 {
                        push_run(runs, i, 1);
                    }
                }
            }
        }
    }
}

impl Renderer3D {
    pub fn set_instance_batch(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32, batch: Option<&Batch>) {
        self.shadow_cache.invalidate();
        while self.static_batches.len() <= id as usize {
            self.static_batches.push(None);
        }
        if let (Some(old), Some(culling)) = (self.static_batches[id as usize].take(), &mut self.static_culling) {
            culling.release(&old);
        }
        match batch.filter(|b| !b.instances.is_empty()) {
            Some(batch) => self.static_batches[id as usize] = Some(self.make_batch(device, queue, id, batch)),
            None => self.batching.said.retain(|&said| said != id),
        }
        let order = &mut self.batching.order;
        order.clear();
        for (b, batch) in self.static_batches.iter().enumerate() {
            for (l, level) in batch.iter().flat_map(|b| b.levels.iter().enumerate()) {
                order.push((level.mesh.slot(), b as u32, l as u32));
            }
        }
        order.sort_unstable();
    }

    /// Refresh only automatic batches using a material whose blend mode changed.
    pub fn refresh_batch_material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        material: u32,
        batches: &[Option<Batch>],
    ) {
        for (id, batch) in batches.iter().enumerate() {
            let Some(batch) = batch else { continue };
            if batch.options.culling == BatchCulling::Auto
                && batch.levels.iter().any(|level| {
                    self.resources.meshes[level.mesh.slot() as usize]
                        .parts
                        .iter()
                        .any(|(_, m)| *m == material)
                })
            {
                self.set_instance_batch(device, queue, id as u32, Some(batch));
            }
        }
    }

    /// Picks the batch's path and uploads it there.
    fn make_batch(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: u32, batch: &Batch) -> StaticBatch {
        let count = batch.instances.len() as u32;
        let can_compute = self.resources.storage_read.is_some() && GpuLimits::of(device).compute_culling();
        // What the game asked for decides; without a wish, the crossover.
        let gpu = match (self.batching.path, batch.options.culling) {
            (BatchPath::Gpu, _) => true,
            (BatchPath::Cpu, _) => false,
            (BatchPath::Auto, BatchCulling::PreferGpu) => true,
            (BatchPath::Auto, BatchCulling::PreferCpu) => false,
            // See-through instances blend in the order they are drawn:
            // the CPU path's is the batch's own, the same every frame.
            // Several distance levels stay on the CPU path at any size: the
            // compute pass tests every instance for every level and view,
            // where the CPU skips whole cells out of view or out of a band,
            // and it measured slower at every size up to a million
            // (`crates/ponko_bench/LOD_SPLIT.md`). The crossover below was
            // measured for single-level batches.
            (BatchPath::Auto, _) if batch.levels.len() > 1 => false,
            (BatchPath::Auto, _) => {
                let blends = batch.levels.iter().any(|l| self.blends(l.mesh.slot()));
                let triangles = |l: &InstanceLevel| -> u64 {
                    let parts = &self.resources.meshes[l.mesh.slot() as usize].parts;
                    parts.iter().map(|(r, _)| u64::from(r.end - r.start) / 3).sum()
                };
                let heaviest = batch.levels.iter().map(triangles).max().unwrap_or(0);
                !blends && (count >= GPU_BATCH_MIN || u64::from(count) * heaviest >= GPU_BATCH_TRIANGLES)
            }
        };
        let mut route = if !can_compute {
            Route::CpuNoCompute
        } else if gpu {
            Route::Gpu
        } else {
            Route::CpuSmall
        };
        // Said once per batch, not again when the game updates it.
        let quiet = self.batching.said.contains(&id);
        let mut pooled = None;
        if route == Route::Gpu {
            let culling = self.static_culling.get_or_insert_with(|| Culling::new(device));
            let bytes = 16 + u64::from(count) * RECORD + batch.cells.len() as u64 * CELL;
            let most = traffic::most_bytes(device, wgpu::BufferUsages::STORAGE) as u64 / culling.align * culling.align;
            let fits = bytes.div_ceil(culling.align) * culling.align;
            match (fits <= most).then(|| culling.place(device, queue, bytes)).flatten() {
                Some((pool, at)) => {
                    // The batch's block: a header, its records, its cells.
                    let mut source = Vec::with_capacity(1 + 6 * count as usize + 2 * batch.cells.len());
                    source.push([count as f32, batch.cells.len() as f32, 0.0, 0.0]);
                    for (i, sphere) in batch.instances.iter().zip(&batch.spheres) {
                        source.extend_from_slice(&i.model);
                        source.push(i.color);
                        source.push(*sphere);
                    }
                    for cell in &batch.cells {
                        source.push(cell.min.extend(f32::from_bits(cell.start)).to_array());
                        source.push(cell.max.extend(f32::from_bits(cell.count)).to_array());
                    }
                    let pool_buffers = &mut culling.pools[pool];
                    queue.uploading(Upload::StaticBatches).write_buffer(
                        &pool_buffers.records.buffer,
                        at,
                        bytemuck::cast_slice(&source),
                    );
                    pool_buffers.batches += 1;
                    pool_buffers.used = true;
                    pool_buffers.idle = 0;
                    pooled = Some(Pooled {
                        pool,
                        records: (at, bytes),
                    });
                }
                None => {
                    if !quiet {
                        traffic::too_big(
                            &format!("instance batch {id}'s records"),
                            bytes as usize,
                            most as usize,
                            "it is culled on the CPU",
                        );
                    }
                    route = Route::CpuOverLimit;
                }
            }
        }
        let mut vertices = None;
        if pooled.is_none() {
            let bytes = u64::from(count) * VERTEX;
            let most = traffic::most_bytes(device, wgpu::BufferUsages::VERTEX);
            if bytes as usize <= most {
                vertices = Some(device.uploading(Upload::StaticBatches).create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("static instance vertices"),
                        contents: bytemuck::cast_slice(&batch.instances),
                        usage: wgpu::BufferUsages::VERTEX,
                    },
                ));
            } else {
                if !quiet {
                    traffic::too_big(
                        &format!("instance batch {id}'s instances"),
                        bytes as usize,
                        most,
                        "it is NOT DRAWN (split it into smaller batches)",
                    );
                }
                route = Route::Dropped;
            }
        }
        let said = &mut self.batching.said;
        match route {
            Route::CpuOverLimit | Route::Dropped if !quiet => said.push(id),
            Route::CpuOverLimit | Route::Dropped => {}
            _ => said.retain(|&said| said != id),
        }
        let cpu = vertices.is_some();
        let arguments = batch
            .levels
            .iter()
            .map(|l| {
                self.resources.meshes[l.mesh.slot() as usize]
                    .parts
                    .iter()
                    .flat_map(|(r, _)| [r.end - r.start, 0, r.start, 0, 0])
                    .collect()
            })
            .collect();
        StaticBatch {
            levels: batch.levels.clone(),
            vertices,
            compute: pooled.is_some(),
            pooled,
            views: Vec::new(),
            cells: if route == Route::Dropped {
                Vec::new()
            } else {
                batch.cells.clone()
            },
            min: batch.min,
            max: batch.max,
            visible: Vec::new(),
            seen: Vec::new(),
            distances: if cpu {
                (0..batch.instances.len().div_ceil(4))
                    .map(|quad| {
                        let points: [[f32; 4]; 4] = std::array::from_fn(|lane| {
                            let i = (quad * 4 + lane).min(batch.instances.len() - 1);
                            if batch.legacy_distance {
                                batch.spheres[i]
                            } else {
                                batch.instances[i].model[3]
                            }
                        });
                        DistanceQuad {
                            x: glam::Vec4::from_array(points.map(|p| p[0])),
                            y: glam::Vec4::from_array(points.map(|p| p[1])),
                            z: glam::Vec4::from_array(points.map(|p| p[2])),
                            radius: if batch.legacy_distance {
                                glam::Vec4::from_array(points.map(|p| p[3]))
                            } else {
                                glam::Vec4::ZERO
                            },
                        }
                    })
                    .collect()
            } else {
                Vec::new()
            },
            route,
            arguments,
            options: batch.options,
            legacy_distance: batch.legacy_distance,
            count,
        }
    }

    /// How the live batches are routed, and what the GPU-driven ones hold.
    pub(crate) fn instance_batch_stats(&self) -> InstanceBatchStats {
        let mut stats = InstanceBatchStats::default();
        for batch in self.static_batches.iter().flatten() {
            stats.batches += 1;
            stats.instances += u64::from(batch.count);
            match batch.route {
                Route::Gpu => {
                    stats.gpu_driven += 1;
                    stats.gpu_instances += u64::from(batch.count);
                }
                Route::CpuSmall => stats.cpu_small += 1,
                Route::CpuNoCompute => stats.cpu_no_compute += 1,
                Route::CpuOverLimit => stats.cpu_over_limit += 1,
                Route::Dropped => stats.dropped += 1,
            }
        }
        if let Some(culling) = &self.static_culling {
            // Emptied pools and pages keep their slots; memory is retired
            // after the grace period, and is included in pool_bytes until then.
            stats.pools = culling.pools.iter().filter(|p| p.batches > 0).count() as u32;
            stats.pages = culling
                .pools
                .iter()
                .flat_map(|p| &p.pages)
                .filter(|p| p.views > 0)
                .count() as u32;
            stats.pool_bytes = culling.pools.iter().map(Pool::bytes).sum();
        }
        stats.cull_dispatches = self.batching.dispatches;
        stats.cull_jobs = self.batching.jobs;
        stats.views_skipped = self.batching.skipped;
        stats
    }

    /// Says a limit was hit, once until `clear` is called for it.
    fn report(reported: &mut u32, flag: u32, what: &str, needed: usize, most: usize, then: &str) {
        if *reported & flag == 0 {
            *reported |= flag;
            traffic::too_big(what, needed, most, then);
        }
    }

    pub(super) fn prepare_static(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &crate::three::Frame3D,
        camera: Vec3,
        occlusion: bool,
    ) {
        // Tests count what preparing the batches allocates in a real frame.
        #[cfg(test)]
        let _batch_code = crate::three::lights::tests::marked();
        // Shadow maps drawn while a limit left batch views out lack those
        // batches: once every view fits again they are drawn anew.
        if self.batching.skipped > 0 {
            // Decide before planning this frame's shadows. This also redraws
            // on the very first frame that fits, including dispatch limits.
            self.shadow_cache.invalidate();
        }
        if self.static_shadowless != frame.static_shadowless {
            self.shadow_cache.invalidate();
            self.static_shadowless.clear();
            self.static_shadowless.extend_from_slice(&frame.static_shadowless);
        }
        let batching = &mut self.batching;
        let targets = 1 + MAX_CASCADES + frame.light_views;
        let slots = targets + frame.views.len();
        batching.slots.clear();
        for slot in 0..slots {
            let (matrix, eye) = if slot == 0 {
                (frame.view_proj, camera)
            } else if slot <= MAX_CASCADES {
                (frame.sun_view_proj[slot - 1], camera)
            } else if slot < targets {
                let i = slot - 1 - MAX_CASCADES;
                (frame.lights.shadow_views[i], frame.lights.shadow_positions[i])
            } else {
                let v = &frame.views[slot - targets];
                (v.view_proj, v.camera)
            };
            batching.slots.push(SlotView {
                planes: planes(matrix),
                eye,
            });
        }
        let grid = frame
            .occlusion
            .as_ref()
            .filter(|_| occlusion)
            .and_then(|g| g.base().map(|b| (g, b)));
        // Whether the compute pass has the grid (the CPU path reads it itself).
        let mut gpu_grid = false;
        if let Some(cull) = &mut self.static_culling {
            cull.fit_views(device, slots);
            let mut header = [0.0f32; 24];
            // Where the views start, after the header and the grid's room.
            header[20] = f32::from_bits(cull.grid_floats as u32);
            if let Some((g, (w, h, depth))) = grid {
                let depth: &[u8] = bytemuck::cast_slice(depth);
                if 96 + depth.len() as u64 <= cull.grid_floats * 4 {
                    header[0..4].copy_from_slice(&[*w as f32, *h as f32, g.camera.distance(camera), 1.0]);
                    header[4..20].copy_from_slice(&g.view_proj.to_cols_array());
                    cull.grid_mirror
                        .upload(queue, Upload::StaticBatches, &cull.occlusion, 96, depth);
                    gpu_grid = true;
                    batching.reported &= !GRID_TOO_BIG;
                } else {
                    Self::report(
                        &mut batching.reported,
                        GRID_TOO_BIG,
                        "the occlusion grid's depths for instance batches",
                        96 + depth.len(),
                        (cull.grid_floats * 4) as usize,
                        "GPU-driven batches are not occlusion culled (they draw more, never less)",
                    );
                }
            }
            let header = bytemuck::cast_slice(&header);
            cull.header_mirror
                .upload(queue, Upload::StaticBatches, &cull.occlusion, 0, header);
            cull.views.clear();
            for view in batching.slots.iter().take(cull.max_views) {
                for plane in view.planes {
                    cull.views.extend(plane.to_array());
                }
                cull.views.extend(view.eye.extend(0.0).to_array());
            }
            cull.views_mirror.upload(
                queue,
                Upload::StaticBatches,
                &cull.occlusion,
                cull.grid_floats * 4,
                bytemuck::cast_slice(&cull.views),
            );
            // Nothing is culled until `finish_static` says what.
            for page in cull.pools.iter_mut().flat_map(|p| &mut p.pages) {
                (page.jobs, page.groups, page.scans) = (0, 0, 0);
            }
        }
        // A limit that no longer bites may be reported when it does again.
        if batching.skipped == 0 {
            batching.reported &= !SKIPPED_VIEW;
        }
        if !batching.clamped {
            batching.reported &= !TOO_MANY_GROUPS;
        }
        batching.clamped = false;
        if !batching.capped {
            batching.reported &= !TOO_MANY_VIEWS;
        }
        batching.capped = false;
        let max_views = self.static_culling.as_ref().map_or(usize::MAX, |c| c.max_views);
        (batching.skipped, batching.dispatches, batching.jobs) = (0, 0, 0);
        for batch in self.static_batches.iter_mut().flatten() {
            for view in &mut batch.views {
                view.active = false;
            }
            if batch.route == Route::Dropped {
                continue;
            }
            if !batch.compute {
                while batch.visible.len() < slots * batch.levels.len() {
                    batch.visible.push(Vec::new());
                    batch.seen.push((None, false));
                }
            }
            for seen in &mut batch.seen {
                seen.1 = false;
            }
            for slot in 0..slots {
                let shadow = slot > 0 && slot < targets;
                if shadow && (!batch.casts() || (slot <= MAX_CASCADES && slot > frame.cascades)) {
                    continue;
                }
                let view = batching.slots[slot];
                // Whole-batch rejection precedes jobs, cell tests and draws.
                let enabled = |l: &InstanceLevel| {
                    if shadow {
                        l.shadows && !frame.static_shadowless.contains(&l.mesh.slot())
                    } else {
                        l.camera
                    }
                };
                let far = if batch.legacy_distance {
                    batch.options.max_distance
                } else {
                    batch
                        .levels
                        .iter()
                        .filter(|l| enabled(l))
                        .map(|l| l.max_distance)
                        .fold(0.0, f32::max)
                };
                if !batch.levels.iter().any(enabled)
                    || !intersects(batch.min, batch.max, &view.planes, view.eye, 0.0, far)
                {
                    continue;
                }
                for level in 0..batch.levels.len() {
                    let l = &batch.levels[level];
                    if !enabled(l) {
                        continue;
                    }
                    let band = if batch.legacy_distance {
                        (0.0, batch.options.max_distance)
                    } else {
                        (l.min_distance, l.max_distance)
                    };
                    if !intersects(batch.min, batch.max, &view.planes, view.eye, band.0, band.1) {
                        continue;
                    }
                    if batch.pooled.is_some() && slot >= max_views {
                        // The culling pass has no room for this view.
                        batching.skipped += 1;
                        batching.capped = true;
                        if batching.reported & TOO_MANY_VIEWS == 0 {
                            batching.reported |= TOO_MANY_VIEWS;
                            traffic::too_many(
                                "views (cameras and shadow views) culling instance batches in one frame",
                                slots as u64,
                                max_views as u64,
                                "GPU-driven batches are NOT DRAWN in the views past it",
                            );
                        }
                        continue;
                    }
                    if let (Some(culling), Some(pooled)) = (&mut self.static_culling, &batch.pooled) {
                        let index = match batch.views.iter().position(|v| v.slot == slot && v.level == level) {
                            Some(index) => Some(index),
                            None => {
                                let visibility = (2 * u64::from(batch.count)
                                    + batch.cells.len() as u64
                                    + batch.cells.len().div_ceil(64) as u64
                                    + 1)
                                    * 4;
                                let arguments: &[u8] = bytemuck::cast_slice(&batch.arguments[level]);
                                let placed =
                                    culling.place_view(device, queue, pooled.pool, visibility, arguments.len() as u64);
                                match placed {
                                    Some((page, at, args)) => {
                                        queue.uploading(Upload::StaticBatches).write_buffer(
                                            &culling.pools[pooled.pool].pages[page].args.buffer,
                                            args,
                                            arguments,
                                        );
                                        batch.views.push(BatchView {
                                            read: None,
                                            page,
                                            visibility: (at, visibility),
                                            args,
                                            band,
                                            slot,
                                            level,
                                            active: false,
                                        });
                                        Some(batch.views.len() - 1)
                                    }
                                    None => {
                                        // Its records fit a buffer, so its list
                                        // does: only a mesh of a million parts
                                        // gets here.
                                        batching.skipped += 1;
                                        Self::report(
                                            &mut batching.reported,
                                            SKIPPED_VIEW,
                                            "an instance batch's visible list and draw arguments for one view",
                                            (visibility + arguments.len() as u64) as usize,
                                            traffic::most_bytes(device, wgpu::BufferUsages::STORAGE),
                                            "the batch is NOT DRAWN in that view",
                                        );
                                        None
                                    }
                                }
                            }
                        };
                        if let Some(index) = index {
                            let view = &mut batch.views[index];
                            view.active = true;
                            view.band = band;
                        }
                    } else {
                        let index = slot * batch.levels.len() + level;
                        // The depth grid changes every frame; without it
                        // the view and the band decide the list alone.
                        let key = (slot != 0 || grid.is_none()).then_some((view, band));
                        if key.is_none() || batch.seen[index].0 != key {
                            let mut runs = std::mem::take(&mut batch.visible[index]);
                            runs.clear();
                            let hides = |min: Vec3, max: Vec3| {
                                slot == 0 && grid.is_some_and(|(g, _)| g.hides(min, max, camera))
                            };
                            cull_cells(batch, &mut runs, &view, band, hides);
                            batch.visible[index] = runs;
                            batching.built = batching.built.wrapping_add(1);
                        }
                        batch.seen[index] = (key, true);
                    }
                }
            }
            // A view that draws nothing this frame has an empty list.
            for (runs, seen) in batch.visible.iter_mut().zip(&mut batch.seen) {
                if !seen.1 {
                    runs.clear();
                    seen.0 = None;
                }
            }
        }
        batching.occlusion = occlusion && gpu_grid;
        // Also runs on an entirely empty scene, which can return before
        // finish_static (there are then no passes to record).
        self.retire_static(device);
    }

    /// Writes the culling pass's work for this frame: each page's jobs,
    /// and the bind groups the pass and the draws use. After the shadow
    /// views to redraw are known.
    pub(super) fn finish_static(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(test)]
        let _batch_code = crate::three::lights::tests::marked();
        let batching = &mut self.batching;
        batching.dispatches = 0;
        batching.jobs = 0;
        let (Some(culling), Some(storage_read)) = (&mut self.static_culling, &self.resources.storage_read) else {
            return;
        };
        for page in culling.pools.iter_mut().flat_map(|p| &mut p.pages) {
            page.table.clear();
            (page.jobs, page.groups, page.scans) = (0, 0, 0);
        }
        let targets = 1 + MAX_CASCADES + self.lists.light_views;
        let (redrawn, light_kept) = (self.lists.redrawn, self.shadow_cache.light_kept);
        // A shadow view kept from an earlier frame draws nothing: no job.
        let culled = |slot: usize| {
            slot == 0
                || slot >= targets
                || if slot <= MAX_CASCADES {
                    redrawn & (1 << (slot - 1)) != 0
                } else {
                    light_kept & (1 << (slot - 1 - MAX_CASCADES)) == 0
                }
        };
        // What one dispatch's three dimensions reach, and the reset's one.
        let (side, max_groups) = (culling.side, culling.max_groups);
        let most_groups = u64::from(side) * u64::from(side) * u64::from(max_groups);
        let most_jobs = u64::from(max_groups) * 64;
        let occlusion = batching.occlusion;
        for batch in self.static_batches.iter_mut().flatten() {
            let Some(pooled) = &batch.pooled else { continue };
            let pool = &mut culling.pools[pooled.pool];
            let block = (pooled.records.0 / 16) as u32;
            let cells = batch.cells.len() as u32;
            for view in batch.views.iter_mut().filter(|v| v.active) {
                let page = &mut pool.pages[view.page];
                let generations = [pool.records.generation, page.visibility.generation];
                if view.read.as_ref().is_none_or(|(_, made)| *made != generations) {
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: storage_read,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: range(&pool.records.buffer, pooled.records),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: range(&page.visibility.buffer, view.visibility),
                            },
                        ],
                    });
                    view.read = Some((group, generations));
                }
                if !culled(view.slot) {
                    continue;
                }
                if u64::from(page.groups) + u64::from(cells) > most_groups || u64::from(page.jobs) >= most_jobs {
                    // Not culled, so not drawn: its counts would be stale.
                    view.active = false;
                    batching.skipped += 1;
                    batching.clamped = true;
                    if batching.reported & TOO_MANY_GROUPS == 0 {
                        batching.reported |= TOO_MANY_GROUPS;
                        traffic::too_many(
                            "instance batches' cells to cull in one dispatch",
                            u64::from(page.groups) + u64::from(cells),
                            most_groups,
                            "the batch views past it are NOT DRAWN (use bigger cells or fewer batches)",
                        );
                    }
                    continue;
                }
                let flags = u32::from(batch.legacy_distance) | (u32::from(view.slot == 0 && occlusion) << 1);
                page.table.extend([
                    block,
                    (view.visibility.0 / 4) as u32,
                    (view.args / 4) as u32,
                    (batch.arguments[view.level].len() / 5) as u32,
                    page.groups,
                    cells,
                    view.slot as u32,
                    flags,
                    view.band.0.to_bits(),
                    view.band.1.to_bits(),
                    page.scans,
                    cells.div_ceil(64),
                ]);
                page.jobs += 1;
                page.groups += cells;
                page.scans += cells.div_ceil(64);
            }
        }
        let frame_generation = culling.frame_generation;
        for pool in &mut culling.pools {
            for page in pool.pages.iter_mut().filter(|p| p.jobs > 0) {
                let at = page.table_at.0;
                if page.mirrored != (page.visibility.generation, at) {
                    page.mirrored = (page.visibility.generation, at);
                    page.root_mirror.forget();
                    page.table_mirror.forget();
                }
                let root = [page.jobs, (at / 4) as u32, page.groups, side, page.scans, 0, 0, 0];
                let buffer = &page.visibility.buffer;
                page.root_mirror
                    .upload(queue, Upload::StaticBatches, buffer, 0, bytemuck::cast_slice(&root));
                page.table_mirror.upload(
                    queue,
                    Upload::StaticBatches,
                    buffer,
                    at,
                    bytemuck::cast_slice(&page.table),
                );
                let generations = [
                    pool.records.generation,
                    page.visibility.generation,
                    page.args.generation,
                    frame_generation,
                ];
                if page.bind.as_ref().is_none_or(|(_, made)| *made != generations) {
                    let buffers = [
                        &pool.records.buffer,
                        &page.visibility.buffer,
                        &page.args.buffer,
                        &culling.occlusion,
                    ];
                    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None,
                        layout: &culling.layout,
                        entries: &[0, 1, 2, 3].map(|i| wgpu::BindGroupEntry {
                            binding: i as u32,
                            resource: buffers[i].as_entire_binding(),
                        }),
                    });
                    page.bind = Some((group, generations));
                }
                batching.dispatches += 1;
                batching.jobs += page.jobs;
            }
        }
    }

    fn retire_static(&mut self, device: &wgpu::Device) {
        let Some(culling) = &mut self.static_culling else {
            return;
        };
        // After a short grace period, return memory nothing uses any more:
        // a page whose views are all gone and a pool whose batches are. The slots stay (views and
        // batches hold their indices) with buffers of the smallest size.
        // Here, not when a batch is freed: an updated batch has taken its
        // place again by now.
        let align = culling.align;
        for pool in &mut culling.pools {
            if pool.batches == 0 {
                if pool.used {
                    pool.idle += 1;
                    if pool.idle >= EMPTY_FRAMES {
                        *pool = Pool::new(device, align, 0);
                    }
                }
                continue;
            }
            for page in &mut pool.pages {
                if page.views == 0 && page.used {
                    page.idle += 1;
                    if page.idle >= EMPTY_FRAMES {
                        *page = Page::new(device, align, 0);
                    }
                }
            }
        }
    }

    pub(super) fn cull_static<'t>(&self, encoder: &mut wgpu::CommandEncoder, time: &PassTimer<'_, 't>) {
        let Some(cull) = &self.static_culling else {
            return;
        };
        super::passes::mark("static cull");
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("static instance cull"),
            timestamp_writes: time("static instance cull").map(|t| wgpu::ComputePassTimestampWrites {
                query_set: t.query_set,
                beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                end_of_pass_write_index: t.end_of_pass_write_index,
            }),
        });
        for page in cull.pools.iter().flat_map(|p| &p.pages) {
            let Some((bind, _)) = page.bind.as_ref().filter(|_| page.jobs > 0) else {
                continue;
            };
            pass.set_bind_group(0, bind, &[]);
            let side = cull.side;
            let rows = page.groups.div_ceil(side);
            pass.set_pipeline(&cull.pipeline);
            pass.dispatch_workgroups(page.groups.min(side), rows.min(side), rows.div_ceil(side));
            pass.set_pipeline(&cull.scan);
            let scan_rows = page.scans.div_ceil(side);
            pass.dispatch_workgroups(page.scans.min(side), scan_rows.min(side), scan_rows.div_ceil(side));
            pass.set_pipeline(&cull.prefix);
            pass.dispatch_workgroups(page.jobs.div_ceil(64), 1, 1);
            pass.set_pipeline(&cull.scatter);
            pass.dispatch_workgroups(page.groups.min(side), rows.min(side), rows.div_ceil(side));
        }
    }

    /// Draws every batch's levels that `view` sees, for one pass. Batches
    /// of one mesh are drawn together, so the mesh's buffers, pipeline
    /// and material are bound once for all of them; the transparent pass
    /// keeps the batches' own order.
    pub(super) fn draw_static(&self, pass: &mut wgpu::RenderPass<'_>, kind: Kind, view: usize) {
        if self.batching.order.is_empty() {
            return;
        }
        // Tests count what this code allocates itself (see `gpu!`).
        #[cfg(test)]
        let _batch_code = crate::three::lights::tests::marked();
        let mut state = BatchDraws::default();
        if kind == Kind::Transparent {
            for (b, batch) in self.static_batches.iter().enumerate() {
                for level in 0..batch.as_ref().map_or(0, |b| b.levels.len()) {
                    self.draw_batch_level(pass, &mut state, kind, view, b, level);
                }
            }
        } else {
            for &(_, b, level) in &self.batching.order {
                self.draw_batch_level(pass, &mut state, kind, view, b as usize, level as usize);
            }
        }
        self.state_counters.add(&state.n);
        self.shaders.cache.counters.looked_up(u64::from(state.lookups));
    }

    fn draw_batch_level(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        state: &mut BatchDraws,
        kind: Kind,
        slot: usize,
        b: usize,
        level: usize,
    ) {
        let Some(batch) = &self.static_batches[b] else { return };
        let run_mesh = batch.levels[level].mesh.slot();
        let Some(mesh) = self.resources.meshes.get(run_mesh as usize) else {
            return;
        };
        let Some((vertices, indices)) = mesh.buffers(&self.resources.mesh_arena) else {
            return;
        };
        if !batch.draws(slot, level) {
            return;
        }
        let n = &mut state.n;
        let pooled = batch
            .view(slot, level)
            .zip(batch.pooled.as_ref())
            .zip(self.static_culling.as_ref());
        // Set once per level, whatever its parts and materials.
        let mut level_bound = false;
        for (part, (range, material)) in mesh.parts.iter().enumerate() {
            let material = self.part_material(run_mesh, part, *material);
            let m = &self.resources.materials[material as usize];
            let Some(role) = PipelineRole::for_pass(
                kind.pass(),
                m.cutout,
                m.transparent,
                self.samples > 1,
                self.full_prepass(),
            ) else {
                continue;
            };
            let shader = &self.shaders.sets[m.shader as usize];
            // Batches have no buffer of the game's: only time moves them.
            if kind == Kind::VelocityStill && !super::record::still_moves(shader, false) {
                continue;
            }
            let variant = ShaderVariant::new(
                Sides::of(m.double_sided),
                Instancing::of(false, mesh.gpu_buffer.is_some() || batch.compute),
            );
            state.lookups += 1;
            let Some(pipeline) = shader.variant(variant).for_kind(kind, role) else {
                continue;
            };
            if state.pipeline != Some(pipeline as *const _) {
                gpu!(pass.set_pipeline(pipeline));
                n.pipelines += 1;
                state.pipeline = Some(pipeline as *const _);
            }
            let buffer_key = if mesh.allocation.is_some() {
                0
            } else {
                self.base(run_mesh) + 1
            };
            if state.mesh != buffer_key {
                gpu!(pass.set_vertex_buffer(0, vertices.slice(..)));
                gpu!(pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32));
                n.vertex_buffers += 1;
                n.index_buffers += 1;
                state.mesh = buffer_key;
            }
            if state.material != material {
                gpu!(pass.set_bind_group(1, &m.bind_group, &[]));
                n.bind_groups += 1;
                state.material = material;
            }
            if let Some(((v, pooled), culling)) = pooled {
                if !level_bound {
                    if !state.marker {
                        gpu!(pass.set_vertex_buffer(1, culling.marker.slice(..)));
                        n.vertex_buffers += 1;
                        (state.marker, state.instances) = (true, None);
                    }
                    if let Some((read, _)) = &v.read {
                        gpu!(pass.set_bind_group(3, read, &[]));
                        n.bind_groups += 1;
                        state.dummy = false;
                    }
                    level_bound = true;
                }
                // Neighbouring parts of one material: one multi-draw.
                if part > 0 && mesh.parts[part - 1].1 == mesh.parts[part].1 {
                    continue;
                }
                let parts = mesh.parts[part..]
                    .iter()
                    .take_while(|(_, m)| *m == mesh.parts[part].1)
                    .count();
                let args = &culling.pools[pooled.pool].pages[v.page].args.buffer;
                let at = v.args + part as u64 * ARGS;
                if parts > 1 {
                    gpu!(pass.multi_draw_indexed_indirect(args, at, parts as u32));
                } else {
                    gpu!(pass.draw_indexed_indirect(args, at));
                }
                n.draws += 1;
            } else if let Some(instances) = &batch.vertices {
                if !level_bound {
                    if state.instances != Some(b) {
                        gpu!(pass.set_vertex_buffer(1, instances.slice(..)));
                        n.vertex_buffers += 1;
                        (state.marker, state.instances) = (false, Some(b));
                    }
                    if let Some(dummy) = self.resources.storage_dummy.as_ref().filter(|_| !state.dummy) {
                        gpu!(pass.set_bind_group(3, dummy, &[]));
                        n.bind_groups += 1;
                        state.dummy = true;
                    }
                    level_bound = true;
                }
                for &(start, count) in &batch.visible[slot * batch.levels.len() + level] {
                    gpu!(pass.draw_indexed(range.clone(), 0, start..start + count));
                    n.draws += 1;
                }
            }
        }
    }
}

/// What the batch draws of one pass have bound, so only changes are set
/// again: each `set_*` costs wgpu validation and a backend call.
struct BatchDraws {
    n: StateChanges,
    lookups: u32,
    pipeline: Option<*const wgpu::RenderPipeline>,
    mesh: u32,
    material: u32,
    /// Whether vertex slot 1 holds the marker, or else which batch's instances.
    marker: bool,
    instances: Option<usize>,
    /// Whether group 3 holds the dummy storage group.
    dummy: bool,
}

impl Default for BatchDraws {
    fn default() -> Self {
        Self {
            n: StateChanges::default(),
            lookups: 0,
            pipeline: None,
            mesh: u32::MAX,
            material: u32::MAX,
            marker: false,
            instances: None,
            dummy: false,
        }
    }
}

#[cfg(test)]
impl Renderer3D {
    pub(crate) fn batch_generations(&self) -> Vec<u32> {
        self.static_culling
            .iter()
            .flat_map(|c| &c.pools)
            .flat_map(|p| {
                std::iter::once(p.records.generation).chain(
                    p.pages
                        .iter()
                        .flat_map(|p| [p.visibility.generation, p.args.generation]),
                )
            })
            .collect()
    }
    pub(crate) fn check_batch_order(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        for batch in self.static_batches.iter().flatten() {
            let Some(pooled) = &batch.pooled else { continue };
            let pool = &self.static_culling.as_ref().unwrap().pools[pooled.pool];
            for view in batch.views.iter().filter(|v| v.active) {
                let page = &pool.pages[view.page];
                let count = traffic::read_back(device, queue, &page.args.buffer, view.args + 4, 4);
                let count = u32::from_ne_bytes(count.try_into().unwrap());
                if count == 0 {
                    continue;
                }
                let bytes = traffic::read_back(
                    device,
                    queue,
                    &page.visibility.buffer,
                    view.visibility.0 + 4,
                    u64::from(count) * 4,
                );
                let indices: Vec<u32> = bytes
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_ne_bytes(*b))
                    .collect();
                assert!(
                    indices.windows(2).all(|p| p[0] < p[1]),
                    "visible indices must stay in source order: {indices:?}"
                );
                assert!(indices.last().unwrap() < &batch.count);
            }
        }
    }
    /// The static batches' and their culling's part of `check_mirrors`.
    pub(super) fn check_batch_mirrors(&self, device: &wgpu::Device, queue: &wgpu::Queue) {
        if let Some(cull) = &self.static_culling {
            cull.header_mirror
                .check(device, queue, &cull.occlusion, 0, "static culling header");
            cull.grid_mirror
                .check(device, queue, &cull.occlusion, 96, "static culling depth grid");
            cull.views_mirror.check(
                device,
                queue,
                &cull.occlusion,
                cull.grid_floats * 4,
                "static culling views",
            );
            for page in cull.pools.iter().flat_map(|p| &p.pages) {
                let at = page.table_at.0;
                if page.mirrored == (page.visibility.generation, at) {
                    page.root_mirror
                        .check(device, queue, &page.visibility.buffer, 0, "static culling root");
                    page.table_mirror
                        .check(device, queue, &page.visibility.buffer, at, "static culling jobs");
                }
            }
        }
    }

    /// The batches' whole per-frame preparation, again, for the frame
    /// last prepared: what a test counts allocations of.
    pub(crate) fn prepare_batches(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &crate::three::Frame3D,
        camera: Vec3,
        occlusion: bool,
    ) {
        self.prepare_static(device, queue, frame, camera, occlusion);
        self.finish_static(device, queue);
    }

    /// Makes the culling pass take at most `views` views a frame, as a
    /// device with a small storage binding would.
    pub(crate) fn limit_batch_views(&mut self, views: usize) {
        if let Some(culling) = &mut self.static_culling {
            culling.max_views = views;
        }
    }

    /// How often a CPU-path list was built so far.
    pub(crate) fn batch_lists_built(&self) -> u32 {
        self.batching.built
    }

    /// Which path batch `id` is on, and why.
    pub(crate) fn batch_route(&self, id: u32) -> Option<Route> {
        Some(self.static_batches.get(id as usize)?.as_ref()?.route)
    }

    /// The indirect draw arguments of a GPU-driven batch's `view`th view:
    /// the buffer and where they start.
    pub(crate) fn batch_view_args(&self, id: u32, view: usize) -> Option<(&wgpu::Buffer, u64)> {
        let batch = self.static_batches.get(id as usize)?.as_ref()?;
        let pool = &self.static_culling.as_ref()?.pools[batch.pooled.as_ref()?.pool];
        let view = batch.views.get(view)?;
        Some((&pool.pages[view.page].args.buffer, view.args))
    }

    /// The bytes of a GPU-driven batch's records and cells in its pool.
    pub(crate) fn batch_record_bytes(&self, id: u32) -> Option<u64> {
        let batch = self.static_batches.get(id as usize)?.as_ref()?;
        Some(batch.pooled.as_ref()?.records.1)
    }

    /// (pools, pages, bytes handed out of all their buffers).
    pub(crate) fn batch_pool_use(&self) -> (usize, usize, u64) {
        self.static_culling.as_ref().map_or((0, 0, 0), |c| {
            let used = |a: &Arena| a.end - a.free.iter().map(|f| f.1).sum::<u64>();
            let pages = c.pools.iter().flat_map(|p| &p.pages);
            (
                c.pools.iter().filter(|p| p.batches > 0).count(),
                pages.clone().filter(|p| p.views > 0).count(),
                c.pools.iter().map(|p| used(&p.records)).sum::<u64>()
                    + pages.map(|p| used(&p.visibility) + used(&p.args)).sum::<u64>(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_distances_preserve_scalar_distance_and_boundary_selection() {
        // The SIMD path must retain sqrt/subtraction semantics at exact band
        // edges, rather than approximate them with squared-distance comparisons.
        for n in 0..1024 {
            let p: [Vec3; 4] =
                std::array::from_fn(|i| Vec3::new((n as f32 - 512.0) * 0.13, i as f32 * 1.37, (n % 31) as f32 * -2.11));
            let radii = [0.0, 0.125, 3.0, 20.0];
            let quad = DistanceQuad {
                x: glam::Vec4::from_array(p.map(|v| v.x)),
                y: glam::Vec4::from_array(p.map(|v| v.y)),
                z: glam::Vec4::from_array(p.map(|v| v.z)),
                radius: glam::Vec4::from_array(radii),
            };
            for eye in [Vec3::ZERO, Vec3::new(0.123, -4.5, 7.0), p[0]] {
                let actual = quad.distances(eye).to_array();
                for i in 0..4 {
                    let expected = p[i].distance(eye) - radii[i];
                    assert_eq!(actual[i].to_bits(), expected.to_bits());
                }
            }
        }
    }

    #[test]
    fn replaced_pages_keep_new_buffer_generations() {
        let Some(gpu) = crate::render::render_test_support::gpu() else {
            return;
        };
        // Pages belong to the compute-culling path. WebGL2 has no storage
        // bindings (Metal reports a zero binding limit), so its renderer uses
        // CPU batches and cannot exercise storage-buffer growth here.
        if !crate::render::render_test_support::needs(&gpu.device, crate::render::GpuFeature::ComputeCulling) {
            return;
        }
        let mut page = Page::new(&gpu.device, 16, 0);
        let old = [page.visibility.generation, page.args.generation];
        page = Page::new(&gpu.device, 16, 0);
        assert!(page.visibility.generation > *old.iter().max().unwrap());
        assert!(page.args.generation > page.visibility.generation);
        let generation = page.visibility.generation;
        page.visibility.alloc(&gpu.device, &gpu.queue, FIRST_BYTES * 2).unwrap();
        assert!(page.visibility.generation > generation, "growth also advances identity");
    }

    #[test]
    fn the_frames_buffer_fits_one_binding_and_the_grid_gives_way_to_the_views() {
        // Room for all: the grid as asked, views in what is left.
        assert_eq!(frame_room(1 << 20, 164_032), (164_032, ((1 << 20) - 164_032) / VIEW));
        // A small binding: the grid shrinks, 64 views keep their room.
        let (grid, views) = frame_room(65_536, 164_032);
        assert_eq!((grid, views), (65_536 - (MIN_VIEWS * VIEW) as u64, MIN_VIEWS));
        assert!(grid + (views * VIEW) as u64 <= 65_536);
        // Smaller than the views' room: the header alone, and what fits.
        assert_eq!(frame_room(1_024, 164_032), (24, 1_000 / VIEW));
        assert_eq!(frame_room(0, 164_032), (24, 0));
    }

    #[test]
    fn runs_of_neighbouring_instances_join() {
        let mut runs = Vec::new();
        push_run(&mut runs, 0, 4);
        push_run(&mut runs, 4, 2);
        push_run(&mut runs, 9, 1);
        push_run(&mut runs, 10, 1);
        assert_eq!(runs, [(0, 6), (9, 2)]);
    }

    #[test]
    fn box_distances_bound_every_point_inside() {
        let (min, max) = (Vec3::new(1.0, -1.0, -1.0), Vec3::new(3.0, 1.0, 1.0));
        // Outside: the nearest face and the farthest corner.
        let (near, far) = box_distances(min, max, Vec3::ZERO);
        assert_eq!(near, 1.0);
        assert_eq!(far, 9.0 + 1.0 + 1.0);
        // Inside: nothing is nearer than the eye itself.
        let (near, far) = box_distances(min, max, Vec3::new(2.0, 0.0, 0.0));
        assert_eq!(near, 0.0);
        assert_eq!(far, 3.0);
    }
}
