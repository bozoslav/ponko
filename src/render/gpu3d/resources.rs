//! The scene's GPU resources: meshes, materials, texture arrays and the
//! game's GPU buffers.
//!
//! [`Resources`] owns them by id, from the call that creates one until the
//! call that frees it; a freed slot stays (drawing nothing, or as the
//! default material) so ids never move. It also owns what binds them: the
//! material bind group layout and sampler, and the storage buffer layouts.
//! It knows nothing about passes or frames: the renderer tells its shadow
//! cache when a change here makes kept shadow maps stale, and the passes
//! read meshes and materials by id while recording.

use std::ops::Range;

use super::skinning;
use crate::render::Material;
use crate::render::gpu::GpuTexture;
use crate::render::traffic::{Counted, NewBuffer, Upload};
use crate::three::{GpuMaterialDesc, MaterialTextures, MeshData, linear};

/// Every mesh, material, texture array and GPU buffer the scene draws with.
pub(super) struct Resources {
    pub(super) meshes: Vec<GpuMesh>,
    pub(super) mesh_arena: super::mesh_arena::MeshArena,
    pub(super) materials: Vec<GpuMaterial>,
    pub(super) material_layout: wgpu::BindGroupLayout,
    pub(super) material_sampler: wgpu::Sampler,
    /// Whether a texture made for sprites can also be read through its
    /// linear (non-sRGB) format: not on WebGL2, which has no view formats.
    pub(super) linear_views: bool,
    /// A 1x1 white texture, for materials without one.
    pub(super) white: u32,
    /// Texture arrays for materials (whole, every layer and mip level).
    pub(super) arrays: Vec<wgpu::TextureView>,
    /// A 1x1 white one-layer array, for materials without one.
    pub(super) white_array: wgpu::TextureView,
    /// The game's GPU buffers, by id, and what binds them: read by material
    /// shaders, written by compute passes. `None` where the GPU has no
    /// storage buffers (WebGL2); `storage_dummy` is bound by draws without one.
    pub(super) storage_read: Option<wgpu::BindGroupLayout>,
    pub(super) storage_write: Option<(wgpu::BindGroupLayout, wgpu::PipelineLayout)>,
    pub(super) storage_buffers: Vec<Option<StorageBuffer>>,
    pub(super) storage_dummy: Option<wgpu::BindGroup>,
}

impl Resources {
    /// Makes GPU buffer `id`: a freed one's slot, or the next new one.
    pub(super) fn create_gpu_buffer(&mut self, device: &wgpu::Device, id: u32, len: u32, data: &[glam::Vec4]) {
        let mut slot = None;
        if let Some(read_layout) = &self.storage_read {
            assert!(
                u64::from(len) * 16 <= device.limits().max_storage_buffer_binding_size,
                "GPU buffer exceeds device limit"
            );
            assert!(
                len.div_ceil(64) <= device.limits().max_compute_workgroups_per_dimension,
                "GPU buffer exceeds dispatch limit"
            );
            let zeros;
            let data = if data.is_empty() {
                zeros = vec![glam::Vec4::ZERO; len as usize];
                &zeros
            } else {
                data
            };
            let buffer = device
                .uploading(Upload::Meshes)
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("GPU items"),
                    contents: bytemuck::cast_slice(&data.iter().map(|v| v.to_array()).collect::<Vec<_>>()),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                });
            let indices = device.new_buffer(&wgpu::BufferDescriptor {
                label: Some("non-batch visibility"),
                size: 4,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            });
            let group = |layout, read| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout,
                    entries: &(0..if read { 2 } else { 1 })
                        .map(|binding| wgpu::BindGroupEntry {
                            binding,
                            resource: if binding == 0 {
                                buffer.as_entire_binding()
                            } else {
                                indices.as_entire_binding()
                            },
                        })
                        .collect::<Vec<_>>(),
                })
            };
            let read = group(read_layout, true);
            let write = group(&self.storage_write.as_ref().unwrap().0, false);
            slot = Some(StorageBuffer {
                buffer,
                read,
                write,
                len,
            });
        }
        match self.storage_buffers.get_mut(id as usize) {
            Some(old) => *old = slot,
            None => {
                // Ids are handed out in order: only the next one is new.
                debug_assert_eq!(id as usize, self.storage_buffers.len());
                self.storage_buffers.push(slot);
            }
        }
    }

    /// Frees GPU buffer `id`; passes and meshes using it do nothing from now on.
    pub(super) fn free_gpu_buffer(&mut self, id: u32) {
        if let Some(slot) = self.storage_buffers.get_mut(id as usize) {
            *slot = None;
        }
        for mesh in &mut self.meshes {
            // Still buffer-driven but bound to no buffer: it draws nothing
            // (not once as a plain model), and the id's next buffer isn't
            // picked up until the mesh is bound to it again.
            if mesh.gpu_buffer.is_some_and(|(b, _)| b == id) {
                mesh.gpu_buffer = Some((u32::MAX, 0));
            }
        }
    }

    pub(super) fn write_gpu_buffer(&self, queue: &wgpu::Queue, id: u32, offset: u32, data: &[glam::Vec4]) {
        if let Some(Some(b)) = self.storage_buffers.get(id as usize) {
            queue.uploading(Upload::Meshes).write_buffer(
                &b.buffer,
                u64::from(offset) * 16,
                bytemuck::cast_slice(&data.iter().map(|v| v.to_array()).collect::<Vec<_>>()),
            );
        }
    }

    pub(super) fn set_mesh_gpu_buffer(&mut self, mesh: u32, b: u32, count: u32) {
        assert!(
            self.storage_read.is_some(),
            "GPU-buffer instances require vertex storage buffers on this device; use create_instance_batch for the CPU fallback or draw Model entities"
        );
        if let Some(mesh) = self.meshes.get_mut(mesh as usize) {
            mesh.gpu_buffer = Some((b, count));
        }
    }

    /// Keeps a texture array for materials; returns its id.
    pub(super) fn add_texture_array(&mut self, texture: &wgpu::Texture) -> u32 {
        self.arrays.push(array_view(texture));
        self.arrays.len() as u32 - 1
    }

    pub(super) fn material_texture<'a>(&self, map: Option<u32>, textures: &'a [GpuTexture]) -> &'a wgpu::Texture {
        let id = map.filter(|&t| (t as usize) < textures.len()).unwrap_or(self.white);
        &textures[id as usize].texture
    }

    pub(super) fn material_array(&self, id: Option<u32>) -> &wgpu::TextureView {
        id.and_then(|id| self.arrays.get(id as usize))
            .unwrap_or(&self.white_array)
    }

    pub(super) fn material_bindings(&self, desc: &GpuMaterialDesc, textures: &[GpuTexture]) -> MaterialBindings {
        self.bindings_for(desc.textures, desc.arrays, textures)
    }

    pub(super) fn bindings_for(
        &self,
        maps: crate::three::MaterialTextures,
        array_ids: [Option<u32>; 3],
        textures: &[GpuTexture],
    ) -> MaterialBindings {
        MaterialBindings {
            maps,
            array_ids,
            textures: std::array::from_fn(|i| self.material_texture(maps[i], textures).clone()),
            arrays: std::array::from_fn(|i| self.material_array(array_ids[i]).clone()),
        }
    }

    /// Binds again the materials that read texture `texture` (whose slot in
    /// `textures` now holds the white stand-in) or texture array `array`
    /// (now the white one). A freed normal, ORM or emissive map is switched
    /// off, so the material looks as it would without one.
    pub(super) fn rebind_materials(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        textures: &[GpuTexture],
        texture: Option<u32>,
        array: Option<u32>,
    ) {
        for i in 0..self.materials.len() {
            let material = &self.materials[i];
            let maps = material.bindings.maps;
            let array_ids = material.bindings.array_ids;
            let reads_texture = texture.is_some_and(|t| maps.contains(&Some(t)));
            if !reads_texture && !array.is_some_and(|a| array_ids.contains(&Some(a))) {
                continue;
            }
            if reads_texture {
                // Slots 1 to 3 (normal, ORM, emissive) have flags in `maps`.
                for (slot, &map) in maps.iter().enumerate().take(4).skip(1) {
                    if map == texture {
                        let offset = std::mem::offset_of!(crate::three::MaterialUniform, maps) + 4 * (slot - 1);
                        queue.write_buffer(&material.buffer, offset as u64, &0u32.to_le_bytes());
                    }
                }
            }
            let bindings = self.bindings_for(maps, array_ids, textures);
            let bind_group = self.material_bind_group(device, &material.buffer, &bindings);
            let material = &mut self.materials[i];
            material.bind_group = bind_group;
            material.bindings = bindings;
        }
    }

    /// Binds the materials that read texture `texture` again, keeping their
    /// maps on: its pixels moved to a new GPU texture (a streamed texture
    /// at another resolution). `rebind_materials` is for freed ones.
    pub(super) fn texture_replaced(&mut self, device: &wgpu::Device, textures: &[GpuTexture], texture: u32) {
        for i in 0..self.materials.len() {
            let material = &self.materials[i];
            let (maps, array_ids) = (material.bindings.maps, material.bindings.array_ids);
            if !maps.contains(&Some(texture)) {
                continue;
            }
            let bindings = self.bindings_for(maps, array_ids, textures);
            let bind_group = self.material_bind_group(device, &material.buffer, &bindings);
            let material = &mut self.materials[i];
            material.bind_group = bind_group;
            material.bindings = bindings;
        }
    }

    pub(super) fn material_bindings_match(
        &self,
        bindings: &MaterialBindings,
        desc: &GpuMaterialDesc,
        textures: &[GpuTexture],
    ) -> bool {
        bindings.maps == desc.textures
            && bindings.array_ids == desc.arrays
            && bindings
                .textures
                .iter()
                .zip(desc.textures)
                .all(|(texture, id)| texture == self.material_texture(id, textures))
            && bindings
                .arrays
                .iter()
                .zip(desc.arrays)
                .all(|(view, id)| view == self.material_array(id))
    }

    pub(super) fn material_bind_group(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        bindings: &MaterialBindings,
    ) -> wgpu::BindGroup {
        // Normal and ORM maps hold data, not colors: read them without sRGB decoding.
        let linear = [false, true, true, false, false, false];
        let views: Vec<wgpu::TextureView> = bindings
            .textures
            .iter()
            .zip(linear)
            .map(|(texture, linear)| {
                // Render targets (HDR) are read as they are.
                let image = texture.format() == wgpu::TextureFormat::Rgba8UnormSrgb;
                texture.create_view(&wgpu::TextureViewDescriptor {
                    format: image.then_some(if linear && self.linear_views {
                        wgpu::TextureFormat::Rgba8Unorm
                    } else {
                        wgpu::TextureFormat::Rgba8UnormSrgb
                    }),
                    ..Default::default()
                })
            })
            .collect();
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&self.material_sampler),
            },
        ];
        for (i, view) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        for (i, view) in bindings.arrays.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 8 + i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d material"),
            layout: &self.material_layout,
            entries: &entries,
        })
    }

    /// Makes the next new material; `shaders`: how many material shaders
    /// there are (one naming a later shader draws with the last).
    pub(super) fn create_material(
        &mut self,
        device: &wgpu::Device,
        desc: &GpuMaterialDesc,
        textures: &[GpuTexture],
        shaders: u32,
    ) -> u32 {
        let buffer = device
            .uploading(Upload::Materials)
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("3d material"),
                contents: bytemuck::bytes_of(&desc.uniform),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let bindings = self.material_bindings(desc, textures);
        let bind_group = self.material_bind_group(device, &buffer, &bindings);
        self.materials.push(GpuMaterial {
            buffer,
            bind_group,
            bindings,
            shader: desc.shader.min(shaders - 1),
            transparent: desc.transparent,
            cutout: desc.cutout,
            double_sided: desc.double_sided,
        });
        self.materials.len() as u32 - 1
    }

    /// Changes material `id` in place: every model using it changes.
    pub(super) fn update_material(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        id: u32,
        desc: &GpuMaterialDesc,
        textures: &[GpuTexture],
        shaders: u32,
    ) -> bool {
        let Some(material) = self.materials.get(id as usize) else {
            return false;
        };
        let changed_blend = material.transparent != desc.transparent;
        queue
            .uploading(Upload::Materials)
            .write_buffer(&material.buffer, 0, bytemuck::bytes_of(&desc.uniform));
        if !self.material_bindings_match(&material.bindings, desc, textures) {
            let bindings = self.material_bindings(desc, textures);
            let bind_group = self.material_bind_group(device, &material.buffer, &bindings);
            let material = &mut self.materials[id as usize];
            material.bind_group = bind_group;
            material.bindings = bindings;
        }
        let shader = desc.shader.min(shaders - 1);
        let material = &mut self.materials[id as usize];
        material.shader = shader;
        material.transparent = desc.transparent;
        material.cutout = desc.cutout;
        material.double_sided = desc.double_sided;
        changed_blend
    }

    /// Makes mesh `id`: a freed one's slot, or the next new one.
    pub(super) fn create_mesh_at(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        data: &MeshData,
        parts: &[(Range<u32>, Material)],
        skinned: bool,
        skinning: &mut skinning::Skinning,
    ) {
        self.free_mesh(id);
        let normals = data.normals_or_computed();
        let vertices: Vec<Vertex> = (0..data.positions.len())
            .map(|i| Vertex {
                position: data.positions[i].to_array(),
                normal: normals[i].to_array(),
                uv: data.uvs.get(i).map_or([0.0; 2], |uv| uv.to_array()),
                color: data.colors.get(i).map_or([255; 4], |&c| {
                    linear(c).map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
                }),
                uv1: data.uvs1.get(i).map_or([0.0; 2], |uv| uv.to_array()),
            })
            .collect();
        let skin = (skinned && (!data.joints.is_empty() || !data.morph_targets.is_empty())).then(|| {
            let deltas = data
                .morph_targets
                .iter()
                .flat_map(|t| {
                    (0..vertices.len()).map(|i| skinning::Delta {
                        position: t.positions[i].to_array(),
                        normal: t.normals.get(i).map_or([0.0; 3], |n| n.to_array()),
                    })
                })
                .collect();
            skinning.create(device, &vertices, &data.joints, &data.weights, deltas)
        });
        let allocation = if skin.is_none() {
            self.mesh_arena.upload(device, &vertices, &data.indices)
        } else {
            None
        };
        let buffers = if allocation.is_some() {
            None
        } else {
            let vertices = device
                .uploading(Upload::Meshes)
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("3d vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX
                        | if skin.is_some() {
                            skinning.rest_usage()
                        } else {
                            wgpu::BufferUsages::empty()
                        },
                });
            let indices = device
                .uploading(Upload::Meshes)
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("3d indices"),
                    contents: bytemuck::cast_slice(&data.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
            Some((vertices, indices))
        };
        let offset = allocation.as_ref().map_or(0, |a| a.indices.start);
        let mesh = GpuMesh {
            buffers,
            allocation,
            parts: parts
                .iter()
                .map(|(r, m)| (r.start + offset..r.end + offset, m.id))
                .collect(),
            skin,
            gpu_buffer: None,
        };
        match self.meshes.get_mut(id as usize) {
            Some(slot) => *slot = mesh,
            None => {
                // Ids are handed out in order: only the next one is new.
                debug_assert_eq!(id as usize, self.meshes.len());
                self.meshes.push(mesh);
            }
        }
    }

    /// Meshes with GPU buffers and their bytes, then GPU buffers and
    /// theirs (`Ctx::asset_inventory`).
    pub(super) fn residency(&self) -> (usize, u64, usize, u64) {
        let (mut meshes, mut mesh_bytes) = (0, self.mesh_arena.bytes());
        meshes += self.meshes.iter().filter(|m| m.allocation.is_some()).count();
        for (vertices, indices) in self.meshes.iter().filter_map(|m| m.buffers.as_ref()) {
            meshes += 1;
            mesh_bytes += vertices.size() + indices.size();
        }
        let buffers = self.storage_buffers.iter().flatten();
        (
            meshes,
            mesh_bytes,
            buffers.clone().count(),
            buffers.map(|b| b.buffer.size()).sum(),
        )
    }

    /// Frees mesh `id`'s GPU memory; it draws nothing from now on.
    pub(super) fn free_mesh(&mut self, id: u32) {
        if let Some(mesh) = self.meshes.get_mut(id as usize) {
            if let Some(allocation) = mesh.allocation.take() {
                self.mesh_arena.release(allocation);
            }
            mesh.buffers = None;
            mesh.parts = Vec::new();
            mesh.skin = None;
            mesh.gpu_buffer = None;
        }
    }
}

/// One corner of a mesh on the GPU.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Vertex {
    pub(super) position: [f32; 3],
    pub(super) normal: [f32; 3],
    pub(super) uv: [f32; 2],
    /// Linear RGBA, 0..255.
    pub(super) color: [u8; 4],
    /// The second texture coordinate set.
    pub(super) uv1: [f32; 2],
}

pub(super) const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 5] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 8 => Unorm8x4, 9 => Float32x2];

pub(super) const INSTANCE_ATTRIBUTES: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, // model matrix
    7 => Float32x4, // tint
];

/// A moved model for the motion pass: the instance, then last frame's model
/// matrix.
pub(super) const MOVED_ATTRIBUTES: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
    3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, // model matrix
    7 => Float32x4, // tint
    10 => Float32x4, 11 => Float32x4, 12 => Float32x4, 13 => Float32x4, // last frame's matrix
];

/// For the motion pass, in a third vertex buffer: where each corner was in
/// the last frame, before the model matrix (a posed model's last pose; any
/// other mesh's own positions, read from its vertex buffer again).
pub(super) const PREVIOUS_POSITION_ATTRIBUTES: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![14 => Float32x3];

pub(super) struct GpuMesh {
    /// Dedicated buffers for deformable meshes and arena-limit fallback.
    pub(super) buffers: Option<(wgpu::Buffer, wgpu::Buffer)>,
    pub(super) allocation: Option<super::mesh_arena::Allocation>,
    /// Index range and material of each part (arena-global for shared meshes).
    pub(super) parts: Vec<(Range<u32>, u32)>,
    /// What skinning it needs, if it's skinned.
    pub(super) skin: Option<skinning::GpuSkin>,
    pub(super) gpu_buffer: Option<(u32, u32)>,
}

pub(super) struct StorageBuffer {
    pub(super) buffer: wgpu::Buffer,
    pub(super) read: wgpu::BindGroup,
    pub(super) write: wgpu::BindGroup,
    pub(super) len: u32,
}

pub(super) struct GpuMaterial {
    pub(super) buffer: wgpu::Buffer,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) bindings: MaterialBindings,
    pub(super) shader: u32,
    pub(super) transparent: bool,
    /// Whether its depth passes cut out pixels by alpha ([`Blend::Cutout`]).
    pub(super) cutout: bool,
    pub(super) double_sided: bool,
}

// Keep the resource identities as well as the requested handles: mip generation
// and reloads can replace a texture in place. Comparing these fixed arrays does
// not allocate or create views on uniform-only updates.
pub(super) struct MaterialBindings {
    pub(super) maps: MaterialTextures,
    pub(super) array_ids: [Option<u32>; 3],
    pub(super) textures: [wgpu::Texture; 6],
    pub(super) arrays: [wgpu::TextureView; 3],
}

/// A view of a whole texture array, as materials read it.
pub(super) fn array_view(texture: &wgpu::Texture) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("3d texture array"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

impl GpuMesh {
    pub(super) fn buffers<'a>(
        &'a self,
        arena: &'a super::mesh_arena::MeshArena,
    ) -> Option<(&'a wgpu::Buffer, &'a wgpu::Buffer)> {
        if self.allocation.is_some() {
            arena.buffers()
        } else {
            self.buffers.as_ref().map(|(v, i)| (v, i))
        }
    }
}
