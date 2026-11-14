//! GPU error scopes around the pipelines of the game's shaders (material
//! shaders, screen effects, compute passes).
//!
//! naga checks the WGSL first ([`super::super::diagnose`]); this catches
//! what only the GPU checks when it builds the pipelines (a binding no
//! layout has, a limit the device lacks), so a bad shader comes back as an
//! error instead of breaking the device or the frame.
//!
//! Native GPUs and WebGL2 answer a scope at once: the new pipelines are
//! rejected before they're used, and the last good ones stay. A browser's
//! WebGPU answers later (a promise): the new pipelines are used meanwhile,
//! and if the GPU rejects them, the next frame puts the previous ones back
//! ([`Renderer3D::poll_shader_checks`]) and reports the error.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use super::{Renderer3D, ShaderPipelines, ShadowCache, build_effect, compose_effect, for_samples};
use crate::three::{EFFECT_STAND_IN, ShaderDiagnostic, ShaderKind};

/// The GPU's verdict on one scope.
pub(super) enum Verdict {
    /// Known: the error, if it rejected something.
    Ready(Option<String>),
    /// A browser's WebGPU hasn't answered yet.
    #[cfg(target_arch = "wasm32")]
    Waiting(Pin<Box<dyn Future<Output = Option<wgpu::Error>>>>),
}

impl Verdict {
    /// Polls once, without blocking.
    fn poll(&mut self) -> Option<Option<String>> {
        match self {
            Verdict::Ready(error) => Some(error.take()),
            #[cfg(target_arch = "wasm32")]
            Verdict::Waiting(future) => match poll_once(future.as_mut()) {
                Poll::Ready(error) => Some(error.map(|e| e.to_string())),
                Poll::Pending => None,
            },
        }
    }
}

fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

/// Ends `scope`: `Ok(error)` if the GPU answered now, else the verdict to
/// check on later frames. `defer` (tests) hands even a known answer to
/// later, as a browser's WebGPU does.
pub(super) fn end_scope(scope: wgpu::ErrorScopeGuard, defer: bool) -> Result<Option<String>, Verdict> {
    let mut future = Box::pin(scope.pop());
    let now = match poll_once(future.as_mut()) {
        Poll::Ready(error) => error.map(|e| e.to_string()),
        #[cfg(target_arch = "wasm32")]
        Poll::Pending => return Err(Verdict::Waiting(future)),
        // Native GPUs answer at once; block if one ever doesn't.
        #[cfg(not(target_arch = "wasm32"))]
        Poll::Pending => pollster::block_on(future).map(|e| e.to_string()),
    };
    if defer { Err(Verdict::Ready(now)) } else { Ok(now) }
}

/// A material shader's verdict to come, with the variant it was built for.
pub(super) type LaterVerdict = Option<(String, Verdict)>;

/// A material shader's previous pipelines and shadow traits.
type Previous = (Arc<ShaderPipelines>, super::shadow_cache::ShaderShadow);

/// Which pipelines a check is about, and what to put back if the GPU
/// rejects them (`None`: the shader was new; it gets a stand-in).
pub(super) enum Swap {
    Material {
        new: Arc<ShaderPipelines>,
        old: Option<Previous>,
    },
    Effect {
        new: wgpu::RenderPipeline,
        old: Option<(wgpu::ShaderModule, wgpu::RenderPipeline)>,
    },
    Compute {
        new: wgpu::ComputePipeline,
        old: Option<(Option<wgpu::ComputePipeline>, bool)>,
    },
}

/// Pipelines in use whose GPU check hasn't come back.
pub(super) struct PendingCheck {
    pub id: u32,
    /// Which time this shader's pipelines were put in use (`installed`).
    pub version: u64,
    pub name: String,
    pub variant: String,
    pub swap: Swap,
    pub verdict: Verdict,
}

impl PendingCheck {
    fn kind(&self) -> ShaderKind {
        swap_kind(&self.swap)
    }

    fn reload(&self) -> bool {
        match &self.swap {
            Swap::Material { old, .. } => old.is_some(),
            Swap::Effect { old, .. } => old.is_some(),
            Swap::Compute { old, .. } => old.is_some(),
        }
    }

    fn same_shader(&self, other: &PendingCheck) -> bool {
        self.id == other.id && self.kind() == other.kind()
    }
}

/// A shader whose pipelines the GPU rejected after they were put in use
/// (a browser's WebGPU), now drawing with the previous ones again.
#[derive(Debug)]
pub struct LateShaderError {
    /// It replaced a working version (a hot reload).
    pub reload: bool,
    /// Newer code of the same shader was put in use since: the rejected
    /// version is gone, and the shader's current state stands.
    pub superseded: bool,
    pub diagnostic: Box<ShaderDiagnostic>,
}

/// The diagnostic for pipelines the GPU rejected: no place in the source.
pub(super) fn rejected(name: &str, kind: ShaderKind, variant: &str, error: &str) -> Box<ShaderDiagnostic> {
    let what = if kind == ShaderKind::Material {
        "pipelines"
    } else {
        "pipeline"
    };
    super::super::without_location(name, kind, variant, format!("the GPU rejected its {what}: {error}"))
}

impl Renderer3D {
    /// Whether to hand GPU verdicts to later frames even when known now.
    pub(super) fn defer_checks(&self) -> bool {
        #[cfg(test)]
        {
            self.defer_checks
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    /// Checks the pipelines waiting on the GPU's verdict: rejected ones are
    /// replaced by what was there before (or a stand-in), and reported in
    /// `out`. Cheap when nothing waits.
    pub fn poll_shader_checks(&mut self, device: &wgpu::Device, out: &mut Vec<LateShaderError>) {
        let mut at = 0;
        while at < self.shader_checks.len() {
            let Some(error) = self.shader_checks[at].verdict.poll() else {
                at += 1;
                continue;
            };
            let check = self.shader_checks.remove(at);
            let Some(error) = error else { continue };
            let (kind, reload) = (check.kind(), check.reload());
            // A rejected shader may already have drawn on asynchronous
            // backends. Its pixels must not survive the rollback.
            self.history.content_changed = true;
            let diagnostic = rejected(&check.name, kind, &check.variant, &error);
            let superseded = self.shader_versions.get(&(kind, check.id)) != Some(&check.version);
            // A newer version of the same shader is in use and waiting too:
            // if that one fails, it goes back to what was good before this.
            if let Some(newer) = self.shader_checks[at..].iter().position(|c| c.same_shader(&check)) {
                // Other shaders made from the rejected code since share it.
                if let Swap::Material { new, .. } = &check.swap {
                    let new = new.clone();
                    self.drop_shared(&new, None);
                }
                hand_down(check.swap, &mut self.shader_checks[at + newer].swap);
            } else {
                self.put_back(device, check.id, check.swap);
            }
            out.push(LateShaderError {
                reload,
                superseded,
                diagnostic,
            });
        }
    }

    /// Puts back what pipelines `swap.new` replaced, where they're still in use.
    fn put_back(&mut self, device: &wgpu::Device, id: u32, swap: Swap) {
        let id = id as usize;
        match swap {
            Swap::Material { new, old } => {
                let back = old.unwrap_or_else(|| (self.shaders.sets[0].clone(), ShadowCache::shader_traits("")));
                self.drop_shared(&new, Some((id, back)));
            }
            Swap::Effect { new, old } => {
                if self.effects.pipelines.get(id) != Some(&new) {
                    return;
                }
                let (module, pipeline) = old.unwrap_or_else(|| {
                    build_effect(
                        device,
                        &self.shaders.layout,
                        "3d effect stand-in",
                        &for_samples(&compose_effect(EFFECT_STAND_IN), self.samples, self.float_depth),
                    )
                });
                self.effects.pipelines[id] = pipeline;
                self.effects.modules[id] = module;
                self.effects.target_pipelines[id] = None;
            }
            Swap::Compute { new, old } => {
                if self.compute_pipelines.get(id) != Some(&Some(new)) {
                    return;
                }
                // A new pass that fails is skipped, as a freed one is.
                let (pipeline, buffer) = old.unwrap_or((None, self.buffer_computes[id]));
                self.compute_pipelines[id] = pipeline;
                self.buffer_computes[id] = buffer;
            }
        }
    }

    /// Every shader drawing with rejected material pipelines `new` goes
    /// back to the built-in shader; `own` (`id`, what it replaced) gets
    /// its previous ones instead. Shaders made from the same code since
    /// share `new` through the material cache.
    fn drop_shared(&mut self, new: &Arc<ShaderPipelines>, own: Option<(usize, Previous)>) {
        let builtin = (self.shaders.sets[0].clone(), ShadowCache::shader_traits(""));
        for (i, slot) in self.shaders.sets.iter_mut().enumerate() {
            if Arc::ptr_eq(slot, new) {
                let (pipelines, shadow) = match &own {
                    Some((id, (back, traits))) if *id == i => (back, *traits),
                    _ => (&builtin.0, builtin.1),
                };
                *slot = pipelines.clone();
                self.shadow_cache.shaders[i] = shadow;
            }
        }
        self.shadow_cache.invalidate();
    }

    /// Notes that shader `id`'s new pipelines are in use now.
    pub(super) fn installed(&mut self, kind: ShaderKind, id: u32) {
        *self.shader_versions.entry((kind, id)).or_default() += 1;
    }

    /// Records pipelines put in use before the GPU's verdict on them.
    pub(super) fn wait_for_verdict(&mut self, id: u32, name: &str, variant: &str, swap: Swap, verdict: Verdict) {
        let version = self.shader_versions.get(&(swap_kind(&swap), id)).copied().unwrap_or(0);
        self.shader_checks.push(PendingCheck {
            id,
            version,
            name: name.to_string(),
            variant: variant.to_string(),
            swap,
            verdict,
        });
    }

    /// How many shaders wait on the GPU's verdict (for tests).
    #[cfg(test)]
    pub(crate) fn pending_shader_checks(&self) -> usize {
        self.shader_checks.len()
    }
}

fn swap_kind(swap: &Swap) -> ShaderKind {
    match swap {
        Swap::Material { .. } => ShaderKind::Material,
        Swap::Effect { .. } => ShaderKind::Effect,
        Swap::Compute { .. } => ShaderKind::Compute,
    }
}

/// `older` was rejected while `newer` (the same shader) waits: `newer`
/// falls back to what `older` replaced.
fn hand_down(older: Swap, newer: &mut Swap) {
    match (older, newer) {
        (Swap::Material { old, .. }, Swap::Material { old: slot, .. }) => *slot = old,
        (Swap::Effect { old, .. }, Swap::Effect { old: slot, .. }) => *slot = old,
        (Swap::Compute { old, .. }, Swap::Compute { old: slot, .. }) => *slot = old,
        _ => {}
    }
}
