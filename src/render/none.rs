//! Builds without the `window` feature have no GPU: `Renderer` has no
//! values, so `Ctx::renderer` is always `None` and these are never called.

use glam::Mat4;

use super::batch::DrawList;
use super::{FrameStatus, RenderStats, Screenshot};
use crate::color::Color;

pub(crate) enum Renderer {}

impl Renderer {
    pub fn stats(&self) -> RenderStats {
        match *self {}
    }
    pub fn pipeline_stats(&self) -> super::PipelineStats {
        match *self {}
    }
    pub fn take_screenshot(&mut self) -> Option<Screenshot> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn mesh_costs(&self) -> Vec<(u32, usize, usize, usize, usize)> {
        match *self {}
    }
    pub fn set_draw_hidden(&mut self, _on: bool, _always: bool) {
        match *self {}
    }
    pub fn detail(&self) -> super::RenderDetail {
        match *self {}
    }
    pub fn gpu_passes(&self) -> &[crate::profiler::GpuPass] {
        match *self {}
    }
    pub fn gpu_frame(&self) -> Option<(&[crate::profiler::PassSpan], u64)> {
        match *self {}
    }
    pub fn capabilities(&self) -> &super::GpuCapabilities {
        match *self {}
    }
    pub fn display_wait_ms(&self) -> f32 {
        match *self {}
    }
    pub fn max_texture_size(&self) -> u32 {
        match *self {}
    }
    pub fn residency(&self) -> Option<crate::inventory::GpuResidency> {
        match *self {}
    }
    pub fn create_texture(&mut self, _width: u32, _height: u32, _rgba: &[u8]) -> u32 {
        match *self {}
    }
    #[cfg(feature = "compressed-textures")]
    pub fn compressed_formats(&self) -> crate::compressed::GpuFormats {
        match *self {}
    }
    #[cfg(feature = "compressed-textures")]
    pub fn create_compressed_texture(&mut self, _image: &crate::compressed::CompressedImage) -> u32 {
        match *self {}
    }
    #[cfg(feature = "compressed-textures")]
    pub fn replace_compressed_texture(&mut self, _id: u32, _image: &crate::compressed::CompressedImage) {
        match *self {}
    }
    #[cfg(all(feature = "3d", feature = "compressed-textures"))]
    pub fn create_compressed_texture_array(&mut self, _layers: &[&crate::compressed::CompressedImage]) -> u32 {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_render_target(&mut self, _width: u32, _height: u32) -> u32 {
        match *self {}
    }
    pub fn update_texture(&mut self, _id: u32, _width: u32, _height: u32, _rgba: &[u8]) {
        match *self {}
    }
    pub fn set_filter(&mut self, _id: u32, _nearest: bool) {
        match *self {}
    }
    pub fn update_texture_region(&mut self, _id: u32, _at: [u32; 2], _size: [u32; 2], _rgba: &[u8]) {
        match *self {}
    }
    pub fn replace_texture(&mut self, _id: u32, _width: u32, _height: u32, _rgba: &[u8]) {
        match *self {}
    }
    pub fn free_texture(&mut self, _id: u32) {
        match *self {}
    }
    pub fn set_sprite_shader(
        &mut self,
        _id: u32,
        _name: &str,
        _composed: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    pub fn free_sprite_shader(&mut self, _id: u32) {
        match *self {}
    }
    pub fn set_sprite_material(
        &mut self,
        _id: u32,
        _shader: Option<u32>,
        _params: &[glam::Vec4; 4],
        _textures: [Option<u32>; 2],
    ) {
        match *self {}
    }
    pub fn free_sprite_material(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn free_material(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn free_shader(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn free_compute(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn free_texture_array(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_material(&mut self, _desc: &crate::three::GpuMaterialDesc) -> u32 {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn update_material(
        &mut self,
        _id: u32,
        _desc: &crate::three::GpuMaterialDesc,
        _batches: &[Option<crate::three::instances::Batch>],
    ) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn supports_decals(&self) -> bool {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn warm_pipelines(&mut self) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn supports_gi(&self) -> bool {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn max_texture_array_size(&self) -> (u32, u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_texture_array(&mut self, _width: u32, _height: u32, _layers: &[&[u8]], _linear: bool) -> u32 {
        match *self {}
    }

    #[cfg(feature = "3d")]
    pub fn create_gpu_buffer(&mut self, _: u32, _: u32, _: &[glam::Vec4]) {}
    #[cfg(feature = "3d")]
    pub fn free_gpu_buffer(&mut self, _: u32) {}
    #[cfg(feature = "3d")]
    pub fn write_gpu_buffer(&mut self, _: u32, _: u32, _: &[glam::Vec4]) {}
    #[cfg(feature = "3d")]
    pub fn set_instance_batch(&mut self, _: u32, _: Option<&crate::three::instances::Batch>) {}
    pub fn set_mesh_gpu_buffer(&mut self, _: u32, _: u32, _: u32) {}
    #[cfg(feature = "3d")]
    pub fn create_compute(&mut self, _name: &str, _source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn replace_compute(
        &mut self,
        _id: u32,
        _name: &str,
        _source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_effect(&mut self, _name: &str, _source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn replace_effect(
        &mut self,
        _id: u32,
        _name: &str,
        _source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_shader(&mut self, _name: &str, _source: &str) -> Result<u32, Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn replace_shader(
        &mut self,
        _id: u32,
        _name: &str,
        _source: &str,
    ) -> Result<(), Box<crate::render::ShaderDiagnostic>> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn create_mesh(
        &mut self,
        _id: u32,
        _data: &crate::render::MeshData,
        _parts: &[(std::ops::Range<u32>, crate::render::Material)],
        _skinned: bool,
    ) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn free_mesh(&mut self, _id: u32) {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub fn take_occlusion(&mut self) -> Option<crate::three::occlusion::OcclusionGrid> {
        match *self {}
    }
    #[cfg(feature = "3d")]
    pub(crate) fn quality_sample(&self) -> Option<crate::three::autotune::Sample> {
        None
    }
    #[cfg(feature = "3d")]
    pub fn prepare_3d(&mut self, _scene: &crate::three::Scene3D<'_>, _collect_ms: f32) {
        match *self {}
    }
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        _world: &mut DrawList,
        _ui: &mut DrawList,
        _panels: &mut [super::batch::PanelDraw],
        _clear: Color,
        _world_view: Mat4,
        _ui_view: Mat4,
        _capture: bool,
        _lights: &crate::light2d::Lighting2D,
    ) -> FrameStatus {
        match *self {}
    }
}
