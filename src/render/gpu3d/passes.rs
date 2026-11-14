//! What each part of a 3D frame reads and writes, and the rules its order
//! must keep.
//!
//! `Renderer3D::draw` records a frame as a list of parts (see `Part`), each
//! into its own command buffer, on several cores, submitted in list order.
//! This module describes those parts as data. It is a description of the
//! encoder code, not a render graph: it doesn't schedule, allocate or record
//! anything, and nothing outside the renderer sees it.
//!
//! - A [`Pass`] is one part: its name, the passes it runs after, when it is
//!   in a frame, and its steps.
//! - A [`Step`] is one piece of work inside a part (a render or compute
//!   pass, a copy), with the condition it is recorded under ([`When`], over
//!   the frame's [`Features`]) and what it uses.
//! - A [`Use`] is one [`Resource`] and how the step touches it
//!   ([`Access`]): as a color or depth attachment, a resolve target, a
//!   sampled texture, a copy's source or destination.
//! - Each resource has a [`Desc`]: how long its contents last, its format
//!   and sample count, and the resource whose memory it shares, if any.
//!
//! The checks:
//! - [`check_passes`] checks a table once. Names are unique and the `after`
//!   lists form no cycle. Inside a step, attachments agree on the sample
//!   count, a resolve has a multisampled source of its format, a copy's two
//!   sides match, and nothing is sampled while it is rendered to. Between
//!   passes, every read is ordered against every writer, last frame's
//!   contents are read before they are overwritten, and two writers of a
//!   resource are ordered. For every combination of features, a read of a
//!   resource that only lasts a frame has an earlier writer. Resources that
//!   share memory have the same format and sample count.
//! - [`check_frame`] checks one frame: its list of parts and its features.
//!   The parts are the ones the features call for, none comes before a pass
//!   it runs after, every read of a frame resource follows a write, and
//!   resources that share memory aren't alive at the same time. `draw` runs
//!   it in debug builds, so every offscreen render test checks the frames it
//!   draws. It doesn't allocate.
//! - [`check_recorded`] compares the steps a part really recorded (each
//!   calls [`mark`] where it is recorded) with the ones its pass declares
//!   for the frame's features, in debug builds.
//! - [`read_later`] tells whether a later step reads a resource: an
//!   attachment may only be discarded when none does.
//!
//! To add a pass: add its entry to [`FRAME_PASSES`] (what it runs after,
//! when it runs, its steps and their uses), a `Part` for it, push the part
//! where `draw` builds its list, and call [`mark`] where each step is
//! recorded. To add work to a part: add a step (and a feature, if a new
//! condition decides it) and its `mark`. The tests at the bottom and the
//! debug checks fail if the declarations contradict each other or the code.

use std::cell::Cell;
use std::fmt;

/// A GPU resource that steps of a frame share.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Resource {
    /// The GPU-culled draws of static batches, for every view.
    VisibleDraws,
    /// Skinned models' posed vertices.
    SkinnedVertices,
    /// The buffers compute passes update in place.
    StorageBuffers,
    /// What a texture compute pass writes, before it is copied to its target.
    ComputeScratch,
    /// The textures compute passes run on.
    ComputeTargets,
    /// The buffers scene compute passes update in place, once the lit
    /// frame is settled: what models draw from them is last frame's.
    SceneStorageBuffers,
    /// The textures scene compute passes run on: materials read last
    /// frame's, effects this frame's.
    SceneComputeTargets,
    /// The copy of the elements the last frame drew, in GPU buffers that
    /// keep one: the motion pass reads where each element was.
    BufferPasts,
    /// The sky in every direction.
    SkyTexture,
    /// The filtered sky and probe maps that image-based lighting reads.
    Environment,
    /// The sun's shadow cascades. Cascades not redrawn are kept from an
    /// earlier frame.
    SunShadowMap,
    /// The shadow atlas of point and spot lights.
    LightShadowAtlas,
    /// Target cameras' (multisampled) depth.
    TargetDepth,
    /// Target cameras' multisampled color.
    TargetColor,
    /// Target cameras' textures, as materials and probe captures read them.
    TargetImage,
    /// The sun shadow map of global illumination's probe views.
    GiShadowMap,
    /// The probe views' depth.
    GiViewDepth,
    /// The probe views' multisampled color.
    GiViewColor,
    /// The probe views, resolved, for the probes' update.
    GiViewImage,
    /// Global illumination's probes.
    GiProbes,
    /// The main camera's (multisampled) depth.
    SceneDepth,
    /// A copy of the scene depth that transparent surfaces read where the
    /// depth attachment can't be read-only while it is sampled (the GL
    /// backend: see [`Features::DEPTH_COPY`]).
    SceneDepthCopy,
    /// The main camera's multisampled color.
    SceneColor,
    /// With temporal antialiasing: the lit frame before it is resolved over
    /// time.
    TaaInput,
    /// The lit frame that screen effects, bloom and tone mapping read.
    Hdr,
    /// The copy of the opaque scene that transparent surfaces read.
    SceneCopy,
    /// What screen effects swap with the lit frame. It is the scene copy's
    /// texture, free once the transparent surfaces are drawn.
    EffectScratch,
    /// Multisampled motion vectors of the models that moved.
    VelocityColor,
    /// Motion vectors, resolved.
    Velocity,
    /// Ambient occlusion before it is denoised.
    AoRaw,
    /// Temporal ambient occlusion's accumulated image.
    AoHistory,
    /// Ambient occlusion, as the opaque pass reads it without reflections.
    AmbientOcclusion,
    /// The image the opaque pass reads reflections (and, with them, ambient
    /// occlusion) from.
    ScreenImage,
    /// The reflections' filtered levels, before they are copied into the
    /// screen image.
    ReflectionLevels,
    /// Last frame's opaque image, which reflections trace against.
    ReflectionHistory,
    /// Temporal antialiasing's accumulated image.
    TaaHistory,
    /// The depth grid reduced for CPU occlusion culling.
    OcclusionGrid,
    /// The buffer the depth grid is read back through.
    OcclusionReadback,
    /// The render targets target effects write.
    EffectTargets,
    /// Bloom's chain of ever smaller images.
    BloomMips,
    /// The tone mapped frame before FXAA.
    Ldr,
    /// The tone mapped frame before SMAA.
    SmaaInput,
    /// SMAA's edges and blending weights.
    SmaaScratch,
    /// With a render scale: the finished frame at the scene's size.
    Upscaled,
    /// The window or render target the frame ends in.
    Output,
}

/// How long a resource's contents last.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Lifetime {
    /// Written fresh every frame it is used in: reading it before a step
    /// writes it reads garbage, and nothing reads it after the frame.
    Frame,
    /// Kept across frames: a frame may read it without writing it.
    Persistent,
    /// Owned outside the 3D renderer.
    External,
}

/// A resource's format, by the role that decides it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Format {
    /// A buffer, not a texture.
    Buffer,
    /// The depth format.
    Depth,
    /// The lit frame's 16-bit float color.
    Hdr,
    /// Motion vectors' two channels.
    Velocity,
    /// Ambient occlusion's two 8-bit channels.
    Occlusion,
    /// The image the opaque pass reads reflections from.
    Screen,
    /// The window's (or render target's) color format.
    Output,
    /// Decided by the game or by another module: not checked.
    Any,
}

/// How many samples a pixel of a resource has.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Samples {
    One,
    /// The frame's multisampling: one too, without [`Features::MSAA`].
    Scene,
}

/// What a resource is.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Desc {
    pub name: &'static str,
    pub lifetime: Lifetime,
    pub format: Format,
    pub samples: Samples,
    /// The resource whose memory this one is: the two are never alive at
    /// the same time.
    pub shares: Option<Resource>,
}

impl Resource {
    /// Every resource, in declaration order.
    pub(crate) const ALL: [Resource; 45] = {
        use Resource::*;
        [
            VisibleDraws,
            SkinnedVertices,
            StorageBuffers,
            ComputeScratch,
            ComputeTargets,
            SceneStorageBuffers,
            SceneComputeTargets,
            BufferPasts,
            SkyTexture,
            Environment,
            SunShadowMap,
            LightShadowAtlas,
            TargetDepth,
            TargetColor,
            TargetImage,
            GiShadowMap,
            GiViewDepth,
            GiViewColor,
            GiViewImage,
            GiProbes,
            SceneDepth,
            SceneDepthCopy,
            SceneColor,
            TaaInput,
            Hdr,
            SceneCopy,
            EffectScratch,
            VelocityColor,
            Velocity,
            AoRaw,
            AoHistory,
            AmbientOcclusion,
            ScreenImage,
            ReflectionLevels,
            ReflectionHistory,
            TaaHistory,
            OcclusionGrid,
            OcclusionReadback,
            EffectTargets,
            BloomMips,
            Ldr,
            SmaaInput,
            SmaaScratch,
            Upscaled,
            Output,
        ]
    };

    pub(crate) const fn desc(self) -> Desc {
        use Format as F;
        use Lifetime::{External, Frame, Persistent};
        use Samples::{One, Scene};
        const fn desc(name: &'static str, lifetime: Lifetime, format: Format, samples: Samples) -> Desc {
            Desc {
                name,
                lifetime,
                format,
                samples,
                shares: None,
            }
        }
        match self {
            Resource::VisibleDraws => desc("visible static draws", Frame, F::Buffer, One),
            Resource::SkinnedVertices => desc("skinned vertices", Frame, F::Buffer, One),
            Resource::StorageBuffers => desc("storage buffers", Persistent, F::Buffer, One),
            Resource::ComputeScratch => desc("compute scratch", Frame, F::Any, One),
            Resource::ComputeTargets => desc("compute targets", Persistent, F::Any, One),
            Resource::SceneStorageBuffers => desc("scene storage buffers", Persistent, F::Buffer, One),
            Resource::SceneComputeTargets => desc("scene compute targets", Persistent, F::Any, One),
            Resource::BufferPasts => desc("GPU buffers' last frame", Persistent, F::Buffer, One),
            Resource::SkyTexture => desc("sky texture", Persistent, F::Hdr, One),
            Resource::Environment => desc("environment maps", Persistent, F::Hdr, One),
            Resource::SunShadowMap => desc("sun shadow map", Persistent, F::Depth, One),
            Resource::LightShadowAtlas => desc("light shadow atlas", Persistent, F::Depth, One),
            Resource::TargetDepth => desc("target camera depth", Frame, F::Depth, Scene),
            Resource::TargetColor => desc("target camera color", Frame, F::Hdr, Scene),
            Resource::TargetImage => desc("target camera image", Persistent, F::Hdr, One),
            Resource::GiShadowMap => desc("gi shadow map", Frame, F::Depth, One),
            Resource::GiViewDepth => desc("gi view depth", Frame, F::Depth, Scene),
            Resource::GiViewColor => desc("gi view color", Frame, F::Hdr, Scene),
            Resource::GiViewImage => desc("gi view image", Frame, F::Hdr, One),
            Resource::GiProbes => desc("gi probes", Persistent, F::Hdr, One),
            Resource::SceneDepth => desc("scene depth", Frame, F::Depth, Scene),
            Resource::SceneDepthCopy => desc("scene depth copy", Frame, F::Depth, Scene),
            Resource::SceneColor => desc("scene color", Frame, F::Hdr, Scene),
            Resource::TaaInput => desc("taa input", Frame, F::Hdr, One),
            Resource::Hdr => desc("lit frame", Frame, F::Hdr, One),
            Resource::SceneCopy => desc("scene copy", Frame, F::Hdr, One),
            Resource::EffectScratch => Desc {
                shares: Some(Resource::SceneCopy),
                ..desc("effect scratch", Frame, F::Hdr, One)
            },
            Resource::VelocityColor => desc("motion vector color", Frame, F::Velocity, Scene),
            Resource::Velocity => desc("motion vectors", Frame, F::Velocity, One),
            Resource::AoRaw => desc("raw ambient occlusion", Frame, F::Occlusion, One),
            Resource::AoHistory => desc("ambient occlusion history", Persistent, F::Hdr, One),
            Resource::AmbientOcclusion => desc("ambient occlusion", Frame, F::Occlusion, One),
            Resource::ScreenImage => desc("screen image", Frame, F::Screen, One),
            Resource::ReflectionLevels => desc("reflection levels", Frame, F::Screen, One),
            Resource::ReflectionHistory => desc("reflection history", Persistent, F::Hdr, One),
            Resource::TaaHistory => desc("taa history", Persistent, F::Hdr, One),
            Resource::OcclusionGrid => desc("occlusion grid", Frame, F::Buffer, One),
            Resource::OcclusionReadback => desc("occlusion readback", Persistent, F::Buffer, One),
            Resource::EffectTargets => desc("effect targets", Persistent, F::Any, One),
            Resource::BloomMips => desc("bloom", Frame, F::Hdr, One),
            Resource::Ldr => desc("frame before fxaa", Frame, F::Output, One),
            Resource::SmaaInput => desc("frame before smaa", Frame, F::Any, One),
            Resource::SmaaScratch => desc("smaa edges and weights", Frame, F::Any, One),
            Resource::Upscaled => desc("scaled frame", Frame, F::Output, One),
            Resource::Output => desc("output", External, F::Output, One),
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        self.desc().name
    }

    pub(crate) const fn lifetime(self) -> Lifetime {
        self.desc().lifetime
    }
}

/// What a frame does, as a set of flags: each decides whether some steps
/// are recorded. `Renderer3D::frame_features` reads them off the renderer.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Features(u64);

macro_rules! features {
    ($($(#[$doc:meta])* $name:ident = $bit:expr, $text:expr;)*) => {
        impl Features {
            $($(#[$doc])* pub(crate) const $name: Features = Features(1 << $bit);)*
            /// Each flag with its name.
            const NAMES: &'static [(Features, &'static str)] = &[$((Features::$name, $text)),*];
        }
    };
}

features! {
    /// More than one sample per pixel.
    MSAA = 0, "multisampling";
    /// Static batches culled on the GPU.
    STATIC_CULL = 1, "gpu culling";
    /// Skinned models to pose.
    SKINNING = 2, "skinning";
    /// Compute passes over GPU buffers.
    BUFFER_COMPUTE = 3, "buffer compute";
    /// Compute passes over textures.
    TEXTURE_COMPUTE = 4, "texture compute";
    /// The sky texture is drawn again.
    SKY_TEXTURE = 5, "sky texture";
    /// The environment's lookup table is still to draw.
    SKY_BRDF = 6, "environment lookup";
    /// The environment's sky maps are filtered again.
    SKY_MAPS = 7, "sky maps";
    /// Any shadow pass (derived).
    SHADOWS = 8, "shadows";
    /// The sun's cascades (in a shadow part: this part draws one).
    SUN_SHADOWS = 9, "sun shadows";
    /// Light views (in a shadow part: this part draws one).
    LIGHT_SHADOWS = 10, "light shadows";
    /// The target cameras' part (derived).
    VIEWS = 11, "views";
    TARGET_CAMERAS = 12, "target cameras";
    /// A target camera draws cut-out surfaces (with multisampling).
    TARGET_CUTOUTS = 13, "target camera cut-outs";
    /// Environment probes captured or loaded this frame.
    PROBES = 14, "probes";
    /// Global illumination probes updated this frame.
    GI = 15, "global illumination";
    /// The main camera draws cut-out surfaces (with multisampling).
    CUTOUTS = 16, "cut-outs";
    /// Every solid surface goes through the depth prepass on this GPU.
    SOLID_PREPASS = 17, "solid prepass";
    /// The depth grid is read back for occlusion culling.
    OCCLUSION = 18, "occlusion readback";
    /// Motion vectors.
    VELOCITY = 19, "motion vectors";
    AO = 20, "ambient occlusion";
    /// Screen-space reflections.
    SSR = 21, "reflections";
    /// A transparent surface reads the scene: the transparent pass is split
    /// from the opaque one.
    SPLIT = 22, "scene-reading transparents";
    TAA = 23, "temporal antialiasing";
    TARGET_EFFECTS = 24, "target effects";
    /// At least one screen effect.
    EFFECTS = 25, "screen effects";
    /// At least two screen effects: one writes back into the lit frame.
    EFFECTS_BACK = 26, "two screen effects";
    /// An odd number of screen effects: the frame ends in the scratch.
    EFFECTS_ODD = 27, "odd screen effects";
    BLOOM = 28, "bloom";
    FXAA = 29, "fxaa";
    SMAA = 30, "smaa";
    /// A render scale other than 1.
    UPSCALE = 31, "render scale";
    /// Ambient occlusion accumulated over frames: it keeps a history.
    AO_TEMPORAL = 32, "temporal ambient occlusion";
    /// The reflections' image has filtered levels.
    SSR_LEVELS = 33, "reflection levels";
    /// Global illumination's probes are updated from their views.
    GI_UPDATE = 34, "global illumination update";
    /// Scene compute passes over GPU buffers.
    SCENE_BUFFER_COMPUTE = 35, "scene buffer compute";
    /// Scene compute passes over textures.
    SCENE_TEXTURE_COMPUTE = 36, "scene texture compute";
    /// Scene-reading transparent surfaces read a copy of the scene depth:
    /// the GL backend can't sample a depth attachment, even a read-only one.
    DEPTH_COPY = 37, "scene depth copy";
    /// GPU buffers keep the elements this frame drew, for the next
    /// frame's motion vectors.
    BUFFER_PAST = 38, "buffer motion copy";
}

impl Features {
    pub(crate) const NONE: Features = Features(0);

    /// A frame with `flag` has `needs` too.
    const IMPLIES: &'static [(Features, Features)] = &[
        (Features::SUN_SHADOWS, Features::SHADOWS),
        (Features::LIGHT_SHADOWS, Features::SHADOWS),
        (Features::TARGET_CAMERAS, Features::VIEWS),
        (Features::PROBES, Features::VIEWS),
        (Features::TARGET_CUTOUTS, Features::TARGET_CAMERAS),
        (Features::TARGET_CUTOUTS, Features::MSAA),
        (Features::CUTOUTS, Features::MSAA),
        (Features::TAA, Features::VELOCITY),
        (Features::EFFECTS_BACK, Features::EFFECTS),
        (Features::EFFECTS_ODD, Features::EFFECTS),
        (Features::AO_TEMPORAL, Features::AO),
        (Features::AO_TEMPORAL, Features::VELOCITY),
        (Features::SSR_LEVELS, Features::SSR),
        (Features::GI_UPDATE, Features::GI),
    ];

    /// No frame has both.
    const EXCLUDES: &'static [(Features, Features)] = &[
        (Features::FXAA, Features::SMAA),
        (Features::FXAA, Features::TAA),
        (Features::SMAA, Features::TAA),
    ];

    pub(crate) const fn with(self, other: Features) -> Features {
        Features(self.0 | other.0)
    }

    pub(crate) const fn without(self, other: Features) -> Features {
        Features(self.0 & !other.0)
    }

    pub(crate) const fn contains(self, other: Features) -> bool {
        self.0 & other.0 == other.0
    }

    pub(crate) const fn intersects(self, other: Features) -> bool {
        self.0 & other.0 != 0
    }

    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `self` if `on`, else nothing: for building a frame's features.
    pub(crate) const fn only_if(self, on: bool) -> Features {
        if on { self } else { Features::NONE }
    }

    /// With every flag these imply.
    pub(crate) fn closed(self) -> Features {
        let mut all = self;
        // The implications are one level deep, except cut-outs -> target
        // cameras -> views: two rounds reach everything.
        for _ in 0..2 {
            for &(flag, needs) in Features::IMPLIES {
                if all.contains(flag) {
                    all = all.with(needs);
                }
            }
        }
        all
    }

    /// Whether a frame can have exactly these: every implied flag is set
    /// and no two exclude each other.
    pub(crate) fn check(self) -> Result<(), PassError> {
        for &(flag, needs) in Features::IMPLIES {
            if self.contains(flag) && !self.contains(needs) {
                return Err(PassError::MissingFeature { flag, needs });
            }
        }
        for &(a, b) in Features::EXCLUDES {
            if self.contains(a) && self.contains(b) {
                return Err(PassError::ExclusiveFeatures { a, b });
            }
        }
        Ok(())
    }

    /// The single flags set, lowest first.
    fn flags(self) -> impl Iterator<Item = Features> + Clone {
        (0..64)
            .map(|bit| Features(1 << bit))
            .filter(move |&f| self.intersects(f))
    }
}

impl fmt::Display for Features {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_empty() {
            return write!(f, "nothing");
        }
        let mut first = true;
        for flag in self.flags() {
            let name = Features::NAMES
                .iter()
                .find(|(named, _)| *named == flag)
                .map_or("?", |(_, name)| name);
            write!(f, "{}{name}", if first { "" } else { ", " })?;
            first = false;
        }
        Ok(())
    }
}

impl fmt::Debug for Features {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{self}]")
    }
}

/// A condition on a frame's features: all of `all` and none of `none`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct When {
    pub all: Features,
    pub none: Features,
}

impl When {
    pub(crate) const ALWAYS: When = When {
        all: Features::NONE,
        none: Features::NONE,
    };

    pub(crate) const fn holds(self, features: Features) -> bool {
        features.contains(self.all) && !features.intersects(self.none)
    }

    const fn and(self, other: When) -> When {
        When {
            all: self.all.with(other.all),
            none: self.none.with(other.none),
        }
    }

    /// Whether no frame satisfies it.
    fn impossible(self) -> bool {
        let all = self.all.closed();
        all.intersects(self.none)
            || Features::EXCLUDES
                .iter()
                .any(|&(a, b)| all.contains(a) && all.contains(b))
    }

    /// Whether every frame that satisfies `self` satisfies `other`.
    fn implies(self, other: When) -> bool {
        self.all.closed().contains(other.all) && self.none.contains(other.none)
    }
}

impl fmt::Display for When {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.all.is_empty(), self.none.is_empty()) {
            (true, true) => write!(f, "any frame"),
            (false, true) => write!(f, "a frame with {}", self.all),
            (true, false) => write!(f, "a frame without {}", self.none),
            (false, false) => write!(f, "a frame with {} and without {}", self.all, self.none),
        }
    }
}

/// In frames with all of `features`.
pub(crate) const fn on(features: Features) -> When {
    When {
        all: features,
        none: Features::NONE,
    }
}

/// How a step touches a resource.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Access {
    /// A color attachment whose old contents don't matter (cleared, or
    /// covered completely).
    Color,
    /// A color attachment drawn over what is there.
    ColorLoad,
    /// The target a multisampled color attachment resolves into.
    Resolve,
    /// A depth attachment, cleared.
    Depth,
    /// A depth attachment, tested and written over what is there.
    DepthLoad,
    /// A read-only depth attachment.
    DepthRead,
    /// Sampled as a single-sampled texture.
    Sample,
    /// Sampled as a multisampled texture.
    SampleMs,
    /// Sampled as the previous frame left it.
    SamplePrevious,
    /// Read (a vertex or storage buffer) as the previous frame left it.
    ReadPrevious,
    /// The source of a copy.
    CopyFrom,
    /// The destination of a copy.
    CopyTo,
    /// Read some other way (a vertex, storage or indirect buffer).
    Read,
    /// Written some other way (by a compute pass).
    Write,
}

impl Access {
    /// Whether it reads what an earlier step of this frame wrote.
    pub(crate) const fn reads(self) -> bool {
        matches!(
            self,
            Access::ColorLoad
                | Access::DepthLoad
                | Access::DepthRead
                | Access::Sample
                | Access::SampleMs
                | Access::CopyFrom
                | Access::Read
        )
    }

    pub(crate) const fn writes(self) -> bool {
        matches!(
            self,
            Access::Color
                | Access::ColorLoad
                | Access::Resolve
                | Access::Depth
                | Access::DepthLoad
                | Access::CopyTo
                | Access::Write
        )
    }

    const fn previous(self) -> bool {
        matches!(self, Access::SamplePrevious | Access::ReadPrevious)
    }

    /// Whether it is an attachment of a render pass (not a resolve target).
    const fn attachment(self) -> bool {
        matches!(
            self,
            Access::Color | Access::ColorLoad | Access::Depth | Access::DepthLoad | Access::DepthRead
        )
    }

    const fn name(self) -> &'static str {
        match self {
            Access::Color => "color attachment",
            Access::ColorLoad => "loaded color attachment",
            Access::Resolve => "resolve target",
            Access::Depth => "depth attachment",
            Access::DepthLoad => "loaded depth attachment",
            Access::DepthRead => "read-only depth attachment",
            Access::Sample => "sampled texture",
            Access::SampleMs => "multisampled texture",
            Access::SamplePrevious => "last frame's texture",
            Access::ReadPrevious => "last frame's buffer",
            Access::CopyFrom => "copy source",
            Access::CopyTo => "copy destination",
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

/// One resource a step uses, how, and in which frames.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Use {
    pub resource: Resource,
    pub access: Access,
    /// On top of the step's own condition.
    pub when: When,
}

impl Use {
    pub(crate) const fn new(resource: Resource, access: Access) -> Use {
        Use {
            resource,
            access,
            when: When::ALWAYS,
        }
    }

    /// Only in frames with all of `features`.
    pub(crate) const fn on(mut self, features: Features) -> Use {
        self.when.all = self.when.all.with(features);
        self
    }

    /// Only in frames with none of `features`.
    pub(crate) const fn off(mut self, features: Features) -> Use {
        self.when.none = self.when.none.with(features);
        self
    }
}

/// One piece of work inside a pass.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Step {
    /// Its name, unique in its pass: what [`mark`] is called with.
    pub name: &'static str,
    /// The frames it is recorded in, on top of its pass's condition.
    pub when: When,
    pub uses: &'static [Use],
}

/// Where a use is in its pass: (its step's index, its index in the step).
/// A step's uses happen in the order they're listed.
type At = (usize, usize);

/// One pass of a frame (a `Part`), as data.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pass {
    /// Its name, also its command buffer's label.
    pub name: &'static str,
    /// The passes it must be recorded after, when they're in the frame.
    pub after: &'static [&'static str],
    /// The frames it is in.
    pub when: When,
    /// Its work, in the order it is recorded.
    pub steps: &'static [Step],
}

impl Pass {
    /// Every use of every step, with the step's index and the condition it
    /// happens under (the pass's, the step's and its own).
    fn uses(&self) -> impl Iterator<Item = (At, &'static Step, Use, When)> + Clone {
        let when = self.when;
        self.steps.iter().enumerate().flat_map(move |(s, step)| {
            step.uses
                .iter()
                .enumerate()
                .map(move |(k, u)| ((s, k), step, *u, when.and(step.when).and(u.when)))
        })
    }

    /// The uses that happen in a frame with `features`.
    fn active(&self, features: Features) -> impl Iterator<Item = (At, &'static Step, Use)> + Clone {
        self.uses()
            .filter(move |(_, _, _, when)| when.holds(features))
            .map(|(s, step, u, _)| (s, step, u))
    }
}

use Access as A;
use Features as F;
use Resource::*;

const fn u(resource: Resource, access: Access) -> Use {
    Use::new(resource, access)
}

/// A step's uses: what drawing models reads in every pass (culled draws,
/// posed vertices, what compute passes made, and what scene compute passes
/// made last frame), then the step's own.
macro_rules! models {
    ($($rest:expr),* $(,)?) => {
        &[
            u(VisibleDraws, A::Read).on(F::STATIC_CULL),
            u(SkinnedVertices, A::Read).on(F::SKINNING),
            u(StorageBuffers, A::Read).on(F::BUFFER_COMPUTE),
            u(ComputeTargets, A::Sample).on(F::TEXTURE_COMPUTE),
            u(SceneStorageBuffers, A::ReadPrevious).on(F::SCENE_BUFFER_COMPUTE),
            u(SceneComputeTargets, A::SamplePrevious).on(F::SCENE_TEXTURE_COMPUTE),
            u(BufferPasts, A::ReadPrevious).on(F::BUFFER_PAST),
            $($rest),*
        ]
    };
}

/// [`models!`], lit: with the shadow maps, the sky, the environment and the
/// probes that lighting reads.
macro_rules! lit_models {
    ($($rest:expr),* $(,)?) => {
        models![
            u(SunShadowMap, A::Sample),
            u(LightShadowAtlas, A::Sample),
            u(SkyTexture, A::Sample),
            u(Environment, A::Sample),
            $($rest),*
        ]
    };
}

const fn step(name: &'static str, when: When, uses: &'static [Use]) -> Step {
    Step { name, when, uses }
}

/// The occlusion grid's reduction: after the depth is complete, which is
/// after the prepass on GPUs with a solid prepass, else after the opaque
/// pass.
const OCCLUSION_USES: &[Use] = &[
    u(SceneDepth, A::SampleMs),
    u(OcclusionGrid, A::Write),
    u(OcclusionGrid, A::CopyFrom),
    u(OcclusionReadback, A::CopyTo),
];

/// Index of each part's pass in [`FRAME_PASSES`].
pub(crate) const PRELUDE: usize = 0;
pub(crate) const SHADOW: usize = 1;
pub(crate) const VIEWS: usize = 2;
pub(crate) const GI: usize = 3;
pub(crate) const PREPASS: usize = 4;
pub(crate) const OPAQUE: usize = 5;
pub(crate) const TRANSPARENT: usize = 6;
pub(crate) const POST: usize = 7;

/// The passes of a 3D frame, in the order `draw` records them. Each is
/// present or not depending on the frame (shadow passes repeat: one or more
/// per redrawn cascade or light view).
pub(crate) const FRAME_PASSES: [Pass; 8] = [
    Pass {
        name: "3d prelude",
        after: &[],
        when: When::ALWAYS,
        steps: &[
            step("static cull", on(F::STATIC_CULL), &[u(VisibleDraws, A::Write)]),
            step("skinning", on(F::SKINNING), &[u(SkinnedVertices, A::Write)]),
            step("buffer compute", on(F::BUFFER_COMPUTE), &[u(StorageBuffers, A::Write)]),
            step(
                "texture compute",
                on(F::TEXTURE_COMPUTE),
                &[
                    u(ComputeScratch, A::Write),
                    u(ComputeScratch, A::CopyFrom),
                    u(ComputeTargets, A::CopyTo),
                ],
            ),
            step("sky texture", on(F::SKY_TEXTURE), &[u(SkyTexture, A::Color)]),
            step("environment lookup", on(F::SKY_BRDF), &[u(Environment, A::Color)]),
            step(
                "sky maps",
                on(F::SKY_MAPS),
                &[u(SkyTexture, A::CopyFrom), u(Environment, A::CopyTo)],
            ),
        ],
    },
    Pass {
        name: "3d shadows",
        after: &["3d prelude"],
        when: on(F::SHADOWS),
        steps: &[
            step("sun shadows", on(F::SUN_SHADOWS), models![u(SunShadowMap, A::Depth)]),
            step(
                "light shadows",
                on(F::LIGHT_SHADOWS),
                models![u(LightShadowAtlas, A::Depth)],
            ),
        ],
    },
    Pass {
        name: "3d target cameras",
        after: &["3d prelude", "3d shadows"],
        when: on(F::VIEWS),
        steps: &[
            step("prepass", on(F::TARGET_CAMERAS), models![u(TargetDepth, A::Depth)]),
            step(
                "cut-outs",
                on(F::TARGET_CUTOUTS),
                models![u(TargetColor, A::Color), u(TargetDepth, A::DepthLoad)],
            ),
            step(
                "color",
                on(F::TARGET_CAMERAS),
                lit_models![
                    u(TargetColor, A::Color).on(F::MSAA),
                    u(TargetImage, A::Resolve).on(F::MSAA),
                    u(TargetImage, A::Color).off(F::MSAA),
                    u(TargetDepth, A::DepthLoad),
                ],
            ),
            step(
                "probes",
                on(F::PROBES),
                &[
                    u(TargetImage, A::Sample).on(F::TARGET_CAMERAS),
                    u(Environment, A::Color),
                ],
            ),
        ],
    },
    Pass {
        name: "3d global illumination",
        // After the target cameras: the probes they capture change the
        // environment its views are lit by.
        after: &["3d prelude", "3d shadows", "3d target cameras"],
        when: on(F::GI),
        steps: &[
            step("shadows", When::ALWAYS, models![u(GiShadowMap, A::Depth)]),
            step(
                "cut-out views",
                When::ALWAYS,
                models![u(GiViewColor, A::Color), u(GiViewDepth, A::Depth)],
            ),
            step(
                "views",
                When::ALWAYS,
                models![
                    u(GiShadowMap, A::Sample),
                    u(SkyTexture, A::Sample),
                    u(Environment, A::Sample),
                    u(GiViewColor, A::Color),
                    u(GiViewImage, A::Resolve),
                    u(GiViewDepth, A::DepthLoad),
                ],
            ),
            step(
                "update",
                on(F::GI_UPDATE),
                &[u(GiViewImage, A::Sample), u(GiProbes, A::Write)],
            ),
        ],
    },
    Pass {
        name: "3d depth prepass",
        after: &["3d prelude"],
        when: When::ALWAYS,
        steps: &[
            step("depth", When::ALWAYS, models![u(SceneDepth, A::Depth)]),
            // The cut-out prepass attaches the scene color (cleared, not kept).
            step(
                "cut-outs",
                on(F::CUTOUTS),
                models![u(SceneColor, A::Color), u(SceneDepth, A::DepthLoad)],
            ),
            step(
                "occlusion depth",
                on(F::OCCLUSION.with(F::SOLID_PREPASS)),
                OCCLUSION_USES,
            ),
            step(
                "motion vectors",
                on(F::VELOCITY),
                models![
                    u(VelocityColor, A::Color),
                    u(Velocity, A::Resolve),
                    u(SceneDepth, A::DepthRead),
                ],
            ),
            step(
                "ambient occlusion",
                on(F::AO),
                &[u(SceneDepth, A::SampleMs), u(AoRaw, A::Color)],
            ),
            step(
                "ambient occlusion denoise",
                on(F::AO),
                &[
                    u(AoRaw, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(Velocity, A::Sample).on(F::VELOCITY),
                    u(AoHistory, A::SamplePrevious).on(F::AO_TEMPORAL),
                    u(AoHistory, A::Color).on(F::AO_TEMPORAL),
                    u(AmbientOcclusion, A::Color).off(F::SSR),
                    // With reflections, it is the first level of their image.
                    u(ScreenImage, A::Color).on(F::SSR),
                ],
            ),
            step(
                "reflections",
                on(F::SSR),
                &[
                    u(SceneDepth, A::SampleMs),
                    u(ReflectionHistory, A::SamplePrevious),
                    u(ScreenImage, A::Color),
                ],
            ),
            // The trace's level, filtered into the levels below it.
            step(
                "reflection levels",
                on(F::SSR_LEVELS),
                &[
                    u(ScreenImage, A::Read),
                    u(ReflectionLevels, A::Write),
                    u(ReflectionLevels, A::CopyFrom),
                    u(ScreenImage, A::CopyTo),
                ],
            ),
        ],
    },
    Pass {
        name: "3d opaque",
        after: &[
            "3d prelude",
            "3d shadows",
            "3d target cameras",
            "3d global illumination",
            "3d depth prepass",
        ],
        when: When::ALWAYS,
        steps: &[
            step(
                "color",
                When::ALWAYS,
                lit_models![
                    u(TargetImage, A::Sample),
                    u(GiProbes, A::Sample),
                    u(AmbientOcclusion, A::Sample).on(F::AO).off(F::SSR),
                    u(ScreenImage, A::Sample).on(F::SSR),
                    u(SceneDepth, A::DepthLoad),
                    // The lit frame: resolved from the multisampled color, or
                    // drawn directly with one sample; into temporal
                    // antialiasing's input when that is on.
                    u(SceneColor, A::Color).on(F::MSAA),
                    u(TaaInput, A::Resolve).on(F::MSAA.with(F::TAA)),
                    u(Hdr, A::Resolve).on(F::MSAA).off(F::TAA),
                    u(TaaInput, A::Color).on(F::TAA).off(F::MSAA),
                    u(Hdr, A::Color).off(F::MSAA.with(F::TAA)),
                ],
            ),
            step(
                "reflection history",
                on(F::SSR),
                &[
                    u(TaaInput, A::Sample).on(F::TAA),
                    u(Hdr, A::Sample).off(F::TAA),
                    u(ReflectionHistory, A::Color),
                ],
            ),
            step(
                "occlusion depth",
                When {
                    all: F::OCCLUSION,
                    none: F::SOLID_PREPASS,
                },
                OCCLUSION_USES,
            ),
        ],
    },
    Pass {
        name: "3d transparent",
        after: &["3d opaque"],
        when: on(F::SPLIT),
        steps: &[
            step(
                "scene copy",
                When::ALWAYS,
                &[
                    u(TaaInput, A::CopyFrom).on(F::TAA),
                    u(Hdr, A::CopyFrom).off(F::TAA),
                    u(SceneCopy, A::CopyTo),
                ],
            ),
            step(
                "depth copy",
                on(F::DEPTH_COPY),
                &[u(SceneDepth, A::SampleMs), u(SceneDepthCopy, A::Depth)],
            ),
            step(
                "color",
                When::ALWAYS,
                lit_models![
                    u(TargetImage, A::Sample),
                    u(GiProbes, A::Sample),
                    u(SceneCopy, A::Sample),
                    // Read-only depth: the shaders sample it at the same
                    // time (or, with the depth copy, the copy).
                    u(SceneDepth, A::DepthRead),
                    u(SceneDepth, A::SampleMs).off(F::DEPTH_COPY),
                    u(SceneDepthCopy, A::SampleMs).on(F::DEPTH_COPY),
                    u(SceneColor, A::ColorLoad).on(F::MSAA),
                    u(TaaInput, A::Resolve).on(F::MSAA.with(F::TAA)),
                    u(Hdr, A::Resolve).on(F::MSAA).off(F::TAA),
                    u(TaaInput, A::ColorLoad).on(F::TAA).off(F::MSAA),
                    u(Hdr, A::ColorLoad).off(F::MSAA.with(F::TAA)),
                ],
            ),
        ],
    },
    Pass {
        name: "3d post",
        after: &["3d opaque", "3d transparent"],
        when: When::ALWAYS,
        steps: &[
            step(
                "taa",
                on(F::TAA),
                &[
                    u(TaaInput, A::Sample),
                    u(TaaHistory, A::SamplePrevious),
                    u(Velocity, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(TaaHistory, A::Color),
                    u(Hdr, A::Color),
                ],
            ),
            // Buffers that keep their last frame copy what this frame drew
            // (with motion vectors on), before the scene compute passes.
            step(
                "buffer motion copy",
                on(F::BUFFER_PAST),
                &[
                    u(StorageBuffers, A::Read).on(F::BUFFER_COMPUTE),
                    u(BufferPasts, A::Write),
                ],
            ),
            // The game's scene compute passes read the settled frame and
            // its depth, and update their buffers or textures in place
            // (through the scratch, as the prelude's do).
            step(
                "scene buffer compute",
                on(F::SCENE_BUFFER_COMPUTE),
                &[
                    u(Hdr, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(SceneStorageBuffers, A::Write),
                ],
            ),
            step(
                "scene texture compute",
                on(F::SCENE_TEXTURE_COMPUTE),
                &[
                    u(Hdr, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(ComputeScratch, A::Write),
                    u(ComputeScratch, A::CopyFrom),
                    u(SceneComputeTargets, A::CopyTo),
                ],
            ),
            step(
                "target effects",
                on(F::TARGET_EFFECTS),
                &[
                    u(Hdr, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(SceneComputeTargets, A::Sample).on(F::SCENE_TEXTURE_COMPUTE),
                    u(EffectTargets, A::Color),
                ],
            ),
            // Screen effects swap between the lit frame and the scratch:
            // each reads one and writes the other.
            step(
                "screen effect",
                on(F::EFFECTS),
                &[
                    u(Hdr, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(EffectTargets, A::Sample).on(F::TARGET_EFFECTS),
                    u(SceneComputeTargets, A::Sample).on(F::SCENE_TEXTURE_COMPUTE),
                    u(EffectScratch, A::Color),
                ],
            ),
            step(
                "screen effect back",
                on(F::EFFECTS_BACK),
                &[
                    u(EffectScratch, A::Sample),
                    u(SceneDepth, A::SampleMs),
                    u(EffectTargets, A::Sample).on(F::TARGET_EFFECTS),
                    u(SceneComputeTargets, A::Sample).on(F::SCENE_TEXTURE_COMPUTE),
                    u(Hdr, A::Color),
                ],
            ),
            step(
                "bloom",
                on(F::BLOOM),
                &[
                    u(Hdr, A::Sample).off(F::EFFECTS_ODD),
                    u(EffectScratch, A::Sample).on(F::EFFECTS_ODD),
                    u(BloomMips, A::Color),
                ],
            ),
            step(
                "tone map",
                When::ALWAYS,
                &[
                    u(Hdr, A::Sample).off(F::EFFECTS_ODD),
                    u(EffectScratch, A::Sample).on(F::EFFECTS_ODD),
                    u(BloomMips, A::Sample).on(F::BLOOM),
                    u(Ldr, A::Color).on(F::FXAA),
                    u(SmaaInput, A::Color).on(F::SMAA),
                    u(Upscaled, A::Color).on(F::UPSCALE).off(F::FXAA.with(F::SMAA)),
                    u(Output, A::Color).off(F::UPSCALE.with(F::FXAA).with(F::SMAA)),
                ],
            ),
            step(
                "fxaa",
                on(F::FXAA),
                &[
                    u(Ldr, A::Sample),
                    u(Upscaled, A::Color).on(F::UPSCALE),
                    u(Output, A::Color).off(F::UPSCALE),
                ],
            ),
            step(
                "smaa",
                on(F::SMAA),
                &[
                    u(SmaaInput, A::Sample),
                    u(SmaaScratch, A::Color),
                    u(Upscaled, A::Color).on(F::UPSCALE),
                    u(Output, A::Color).off(F::UPSCALE),
                ],
            ),
            step(
                "upscale",
                on(F::UPSCALE),
                &[u(Upscaled, A::Sample), u(Output, A::Color)],
            ),
        ],
    },
];

/// A contradiction in a table of passes, a frame, or what a part recorded.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum PassError {
    /// Two passes share a name.
    DuplicateName(&'static str),
    /// Two steps of `pass` share a name.
    DuplicateStep { pass: &'static str, step: &'static str },
    /// A pass runs after one that isn't in the table.
    UnknownPass { pass: &'static str, after: &'static str },
    /// The `after` lists loop: each runs after the next, the last after the first.
    Cycle(Vec<&'static str>),
    /// `pass` reads `resource`, which `writer` writes, with no order between
    /// them (for a resource kept across frames: not after it).
    UndeclaredRead {
        pass: &'static str,
        resource: Resource,
        writer: &'static str,
    },
    /// `pass` reads last frame's `resource`, but `writer` (which overwrites
    /// it) doesn't run after it.
    HistoryOverwritten {
        pass: &'static str,
        resource: Resource,
        writer: &'static str,
    },
    /// Two passes write `resource` with no order between them.
    UnorderedWrites {
        resource: Resource,
        first: &'static str,
        second: &'static str,
    },
    /// A step uses `resource` in a way its description rules out.
    InvalidUse {
        pass: &'static str,
        step: &'static str,
        resource: Resource,
        access: Access,
        why: &'static str,
    },
    /// A step renders to `resource` and samples it at the same time.
    Feedback {
        pass: &'static str,
        step: &'static str,
        resource: Resource,
    },
    /// Two resources a step uses together (attachments of one pass, the
    /// two sides of a copy) differ in sample count.
    SampleCount {
        pass: &'static str,
        step: &'static str,
        first: Resource,
        second: Resource,
    },
    /// Two resources a step uses together (a resolve and its source, the
    /// two sides of a copy) differ in format.
    FormatMismatch {
        pass: &'static str,
        step: &'static str,
        first: Resource,
        second: Resource,
    },
    /// In frames that satisfy `frames`, `step` reads `resource` (which only
    /// lasts a frame) and no earlier step writes it.
    Uncovered {
        pass: &'static str,
        step: &'static str,
        resource: Resource,
        frames: When,
    },
    /// `resource` shares `with`'s memory but differs in format or sample count.
    SharedMismatch { resource: Resource, with: Resource },
    /// A frame's features have `flag` without `needs`.
    MissingFeature { flag: Features, needs: Features },
    /// A frame's features have both `a` and `b`.
    ExclusiveFeatures { a: Features, b: Features },
    /// In a frame, `pass` comes before `after`, which it must follow.
    OutOfOrder { pass: &'static str, after: &'static str },
    /// In a frame, `step` of `pass` reads `resource` before anything wrote it.
    ReadBeforeWrite {
        pass: &'static str,
        step: &'static str,
        resource: Resource,
    },
    /// A frame has `pass`, which its features don't call for.
    UnexpectedPass { pass: &'static str, features: Features },
    /// A frame's features call for `pass`, which isn't in it.
    MissingPass { pass: &'static str, features: Features },
    /// In a frame, `resource` and `with` (which share memory) are alive at
    /// the same time.
    SharedOverlap { resource: Resource, with: Resource },
    /// A part recorded `step`, which its pass doesn't declare for the frame.
    UnexpectedStep {
        pass: &'static str,
        step: &'static str,
        features: Features,
    },
    /// A part didn't record `step`, which its pass declares for the frame.
    MissingStep {
        pass: &'static str,
        step: &'static str,
        features: Features,
    },
    /// `step` of `pass` discards `resource`, which `reader` of `reader_pass`
    /// reads later in the frame.
    DiscardedRead {
        pass: &'static str,
        step: &'static str,
        resource: Resource,
        reader_pass: &'static str,
        reader: &'static str,
    },
}

impl fmt::Display for PassError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PassError::DuplicateName(name) => write!(f, "two passes are named \"{name}\""),
            PassError::DuplicateStep { pass, step } => {
                write!(f, "pass \"{pass}\" has two steps named \"{step}\"")
            }
            PassError::UnknownPass { pass, after } => {
                write!(f, "pass \"{pass}\" runs after \"{after}\", which isn't a pass")
            }
            PassError::Cycle(names) => {
                write!(f, "pass dependencies form a cycle: ")?;
                for name in names {
                    write!(f, "\"{name}\" -> ")?;
                }
                write!(f, "\"{}\"", names[0])
            }
            PassError::UndeclaredRead { pass, resource, writer } => write!(
                f,
                "pass \"{pass}\" reads {} but doesn't run after \"{writer}\", which writes it",
                resource.name()
            ),
            PassError::HistoryOverwritten { pass, resource, writer } => write!(
                f,
                "pass \"{pass}\" reads last frame's {} but \"{writer}\", which overwrites it, \
                 doesn't run after it",
                resource.name()
            ),
            PassError::UnorderedWrites {
                resource,
                first,
                second,
            } => write!(
                f,
                "passes \"{first}\" and \"{second}\" both write {} with no order between them",
                resource.name()
            ),
            PassError::InvalidUse {
                pass,
                step,
                resource,
                access,
                why,
            } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" uses {} as a {}, but {why}",
                resource.name(),
                access.name()
            ),
            PassError::Feedback { pass, step, resource } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" renders to {} and samples it at the same time",
                resource.name()
            ),
            PassError::SampleCount {
                pass,
                step,
                first,
                second,
            } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" uses {} ({}) together with {} ({})",
                first.name(),
                samples_name(first.desc().samples),
                second.name(),
                samples_name(second.desc().samples)
            ),
            PassError::FormatMismatch {
                pass,
                step,
                first,
                second,
            } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" uses {} ({:?} format) together with {} ({:?} format)",
                first.name(),
                first.desc().format,
                second.name(),
                second.desc().format
            ),
            PassError::Uncovered {
                pass,
                step,
                resource,
                frames,
            } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" reads {}, which no earlier step writes in {frames}",
                resource.name()
            ),
            PassError::SharedMismatch { resource, with } => write!(
                f,
                "{} shares the memory of {} but differs in format or sample count",
                resource.name(),
                with.name()
            ),
            PassError::MissingFeature { flag, needs } => {
                write!(f, "a frame with {flag} must also have {needs}")
            }
            PassError::ExclusiveFeatures { a, b } => write!(f, "a frame can't have both {a} and {b}"),
            PassError::OutOfOrder { pass, after } => {
                write!(
                    f,
                    "pass \"{pass}\" is recorded before \"{after}\", which it must follow"
                )
            }
            PassError::ReadBeforeWrite { pass, step, resource } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" reads {}, which nothing earlier wrote this frame",
                resource.name()
            ),
            PassError::UnexpectedPass { pass, features } => write!(
                f,
                "pass \"{pass}\" is in a frame that doesn't call for it (the frame has {features})"
            ),
            PassError::MissingPass { pass, features } => write!(
                f,
                "pass \"{pass}\" is missing from a frame that calls for it (the frame has {features})"
            ),
            PassError::SharedOverlap { resource, with } => write!(
                f,
                "{} and {} share memory but are both alive at one point of the frame",
                resource.name(),
                with.name()
            ),
            PassError::UnexpectedStep { pass, step, features } => write!(
                f,
                "pass \"{pass}\" recorded step \"{step}\", which it doesn't declare for this frame \
                 (the frame has {features})"
            ),
            PassError::MissingStep { pass, step, features } => write!(
                f,
                "pass \"{pass}\" didn't record step \"{step}\", which it declares for this frame \
                 (the frame has {features})"
            ),
            PassError::DiscardedRead {
                pass,
                step,
                resource,
                reader_pass,
                reader,
            } => write!(
                f,
                "step \"{step}\" of pass \"{pass}\" discards {}, which step \"{reader}\" of pass \
                 \"{reader_pass}\" reads later",
                resource.name()
            ),
        }
    }
}

impl std::error::Error for PassError {}

fn samples_name(samples: Samples) -> &'static str {
    match samples {
        Samples::One => "one sample",
        Samples::Scene => "multisampled",
    }
}

/// The index of the pass named `name` in `table`.
fn index(table: &[Pass], name: &str) -> Option<usize> {
    table.iter().position(|p| p.name == name)
}

/// Whether pass `later` (transitively) runs after pass `earlier`.
fn runs_after(table: &[Pass], later: usize, earlier: usize) -> bool {
    let mut stack = vec![later];
    let mut seen = vec![false; table.len()];
    while let Some(i) = stack.pop() {
        for &name in table[i].after {
            let Some(j) = index(table, name) else { continue };
            if j == earlier {
                return true;
            }
            if !seen[j] {
                seen[j] = true;
                stack.push(j);
            }
        }
    }
    false
}

/// A resource's sample count in frames that satisfy `when`: the frame's
/// multisampling is one sample where `when` rules multisampling out.
fn samples_in(resource: Resource, when: When) -> Samples {
    match resource.desc().samples {
        Samples::Scene if when.none.contains(Features::MSAA) => Samples::One,
        samples => samples,
    }
}

/// Checks what one step uses together (see the module docs).
fn check_step(pass: &Pass, step: &'static Step) -> Result<(), PassError> {
    let invalid = |used: &Use, why| PassError::InvalidUse {
        pass: pass.name,
        step: step.name,
        resource: used.resource,
        access: used.access,
        why,
    };
    let base = pass.when.and(step.when);
    for (i, a) in step.uses.iter().enumerate() {
        let desc = a.resource.desc();
        let depth = desc.format == Format::Depth;
        let buffer = desc.format == Format::Buffer;
        let when = base.and(a.when);
        match a.access {
            A::Depth | A::DepthLoad | A::DepthRead if !depth => return Err(invalid(a, "it isn't a depth texture")),
            A::Color | A::ColorLoad | A::Resolve if depth => return Err(invalid(a, "it is a depth texture")),
            A::Color | A::ColorLoad | A::Resolve | A::Sample | A::SampleMs | A::SamplePrevious if buffer => {
                return Err(invalid(a, "it is a buffer"));
            }
            A::Sample if samples_in(a.resource, when) != Samples::One => {
                return Err(invalid(a, "it is multisampled"));
            }
            A::SampleMs if desc.samples != Samples::Scene => return Err(invalid(a, "it has one sample")),
            A::Resolve if desc.samples != Samples::One => return Err(invalid(a, "it is multisampled")),
            A::SamplePrevious | A::ReadPrevious if desc.lifetime == Lifetime::Frame => {
                return Err(invalid(a, "it isn't kept across frames"));
            }
            A::ReadPrevious if !buffer => return Err(invalid(a, "it isn't a buffer")),
            _ => {}
        }
        // What happens together with `a`: the uses whose conditions some
        // frame satisfies along with its own.
        let together = |b: &&Use| !when.and(b.when).impossible();
        if a.access == A::Resolve {
            let source =
                step.uses.iter().filter(together).find(|b| {
                    matches!(b.access, A::Color | A::ColorLoad) && b.resource.desc().samples == Samples::Scene
                });
            match source {
                None => return Err(invalid(a, "the step has no multisampled color attachment")),
                Some(b) if b.resource.desc().format != desc.format => {
                    return Err(PassError::FormatMismatch {
                        pass: pass.name,
                        step: step.name,
                        first: b.resource,
                        second: a.resource,
                    });
                }
                Some(_) => {}
            }
        }
        for b in step.uses[i + 1..].iter().filter(together) {
            let both = when.and(b.when);
            let other = b.resource.desc();
            let pair = |make: fn(&'static str, &'static str, Resource, Resource) -> PassError| {
                make(pass.name, step.name, a.resource, b.resource)
            };
            let samples_differ = samples_in(a.resource, both) != samples_in(b.resource, both);
            if a.access.attachment() && b.access.attachment() && samples_differ {
                return Err(pair(|pass, step, first, second| PassError::SampleCount {
                    pass,
                    step,
                    first,
                    second,
                }));
            }
            let copy = matches!(
                (a.access, b.access),
                (A::CopyFrom, A::CopyTo) | (A::CopyTo, A::CopyFrom)
            );
            if copy && a.resource != b.resource {
                if samples_differ {
                    return Err(pair(|pass, step, first, second| PassError::SampleCount {
                        pass,
                        step,
                        first,
                        second,
                    }));
                }
                if desc.format != other.format && desc.format != Format::Any && other.format != Format::Any {
                    return Err(pair(|pass, step, first, second| PassError::FormatMismatch {
                        pass,
                        step,
                        first,
                        second,
                    }));
                }
            }
            if a.resource == b.resource {
                let rendered = |x: Access| x.attachment() && x.writes() || x == A::Resolve;
                let sampled = |x: Access| matches!(x, A::Sample | A::SampleMs);
                if rendered(a.access) && sampled(b.access) || rendered(b.access) && sampled(a.access) {
                    return Err(PassError::Feedback {
                        pass: pass.name,
                        step: step.name,
                        resource: a.resource,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Whether, in every frame that satisfies `frames`, one of `writers`'
/// conditions holds. Splits on the flags the writers depend on; `Err` is a
/// kind of frame where none does.
fn covered(frames: When, writers: &[When]) -> Result<(), When> {
    if frames.impossible() || writers.iter().any(|&w| frames.implies(w)) {
        return Ok(());
    }
    let decided = frames.all.closed().with(frames.none);
    let open = writers
        .iter()
        .filter(|w| !frames.and(**w).impossible())
        .flat_map(|w| w.all.with(w.none).flags())
        .find(|&flag| !decided.intersects(flag));
    let Some(flag) = open else {
        return Err(frames);
    };
    covered(
        When {
            all: frames.all.with(flag),
            ..frames
        },
        writers,
    )?;
    covered(
        When {
            none: frames.none.with(flag),
            ..frames
        },
        writers,
    )
}

/// Checks a table of passes (see the module docs).
pub(crate) fn check_passes(table: &[Pass]) -> Result<(), PassError> {
    for (i, pass) in table.iter().enumerate() {
        if table[..i].iter().any(|p| p.name == pass.name) {
            return Err(PassError::DuplicateName(pass.name));
        }
        for &after in pass.after {
            if index(table, after).is_none() {
                return Err(PassError::UnknownPass { pass: pass.name, after });
            }
        }
        for (s, step) in pass.steps.iter().enumerate() {
            if pass.steps[..s].iter().any(|other| other.name == step.name) {
                return Err(PassError::DuplicateStep {
                    pass: pass.name,
                    step: step.name,
                });
            }
        }
    }
    // Cycles: a depth-first walk, with the path on a stack.
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        New,
        OnPath,
        Done,
    }
    fn visit(table: &[Pass], i: usize, marks: &mut [Mark], path: &mut Vec<usize>) -> Result<(), PassError> {
        marks[i] = Mark::OnPath;
        path.push(i);
        for &name in table[i].after {
            let j = index(table, name).unwrap();
            match marks[j] {
                Mark::OnPath => {
                    let start = path.iter().position(|&k| k == j).unwrap();
                    return Err(PassError::Cycle(path[start..].iter().map(|&k| table[k].name).collect()));
                }
                Mark::New => visit(table, j, marks, path)?,
                Mark::Done => {}
            }
        }
        path.pop();
        marks[i] = Mark::Done;
        Ok(())
    }
    let mut marks = vec![Mark::New; table.len()];
    for i in 0..table.len() {
        if marks[i] == Mark::New {
            visit(table, i, &mut marks, &mut Vec::new())?;
        }
    }
    for resource in Resource::ALL {
        if let Some(with) = resource.desc().shares {
            let (a, b) = (resource.desc(), with.desc());
            if a.format != b.format || a.samples != b.samples {
                return Err(PassError::SharedMismatch { resource, with });
            }
        }
    }
    for pass in table {
        for step in pass.steps {
            check_step(pass, step)?;
        }
    }
    // Hazards between passes, now that `runs_after` terminates. Two uses
    // that no frame has together can't clash.
    for (r, reader) in table.iter().enumerate() {
        for (w, writer) in table.iter().enumerate() {
            if r == w {
                continue;
            }
            let after = runs_after(table, r, w);
            let before = runs_after(table, w, r);
            for (_, _, written, write_when) in writer.uses().filter(|(_, _, u, _)| u.access.writes()) {
                let resource = written.resource;
                for (_, _, read, read_when) in reader.uses().filter(|(_, _, u, _)| u.resource == resource) {
                    if read_when.and(write_when).impossible() {
                        continue;
                    }
                    // What lasts a frame may be written again once it was
                    // read (the frame's image, by each pass in turn); what
                    // is kept across frames is read as this frame wrote it.
                    let ordered = after || before && resource.lifetime() == Lifetime::Frame;
                    if read.access.reads() && !ordered {
                        return Err(PassError::UndeclaredRead {
                            pass: reader.name,
                            resource,
                            writer: writer.name,
                        });
                    }
                    if read.access.previous() && !before {
                        return Err(PassError::HistoryOverwritten {
                            pass: reader.name,
                            resource,
                            writer: writer.name,
                        });
                    }
                    if w > r && read.access.writes() && !after && !before {
                        return Err(PassError::UnorderedWrites {
                            resource,
                            first: reader.name,
                            second: writer.name,
                        });
                    }
                }
            }
        }
    }
    for (p, pass) in table.iter().enumerate() {
        for (at, step, read, read_when) in pass.uses() {
            let resource = read.resource;
            // Inside a pass: last frame's contents are read before a step
            // overwrites them (or by the step that does, from the other
            // image of a pair).
            if read.access.previous()
                && pass.uses().any(|(written, _, w, when)| {
                    written.0 < at.0 && w.resource == resource && w.access.writes() && !read_when.and(when).impossible()
                })
            {
                return Err(PassError::HistoryOverwritten {
                    pass: pass.name,
                    resource,
                    writer: pass.name,
                });
            }
            if !read.access.reads() || resource.lifetime() != Lifetime::Frame {
                continue;
            }
            // In every kind of frame the read happens in, an earlier step
            // (of this pass, or of a pass it runs after) writes it.
            let writes = |q: usize, before: At| {
                table[q]
                    .uses()
                    .filter(move |(written, _, w, _)| *written < before && w.resource == resource && w.access.writes())
                    .map(|(_, _, _, when)| when)
            };
            let mut writers: Vec<When> = writes(p, at).collect();
            for q in (0..table.len()).filter(|&q| q != p && runs_after(table, p, q)) {
                writers.extend(writes(q, (usize::MAX, 0)));
            }
            if let Err(frames) = covered(read_when, &writers) {
                return Err(PassError::Uncovered {
                    pass: pass.name,
                    step: step.name,
                    resource,
                    frames,
                });
            }
        }
    }
    Ok(())
}

/// Where a step is in a frame: (its part's place in the frame's order, its
/// index in its pass).
type Place = (usize, usize);

/// The first and last step of the frame that use `resource`.
fn span<I>(table: &[Pass], order: &I, features: Features, resource: Resource) -> Option<(Place, Place)>
where
    I: Iterator<Item = usize> + Clone,
{
    let mut places = order.clone().enumerate().flat_map(|(i, part)| {
        table[part]
            .active(features)
            .filter(move |(_, _, u)| u.resource == resource)
            .map(move |(at, _, _)| (i, at.0))
    });
    let first = places.next()?;
    Some((first, places.last().unwrap_or(first)))
}

/// Checks one frame: `order` lists the frame's parts, as indices in
/// `table`, in the order they're submitted, and `features` is what the
/// frame does (see the module docs). A pass may repeat (the shadow passes
/// do). Doesn't allocate.
pub(crate) fn check_frame<I>(table: &[Pass], order: I, features: Features) -> Result<(), PassError>
where
    I: Iterator<Item = usize> + Clone,
{
    features.check()?;
    for (p, pass) in table.iter().enumerate() {
        if pass.when.holds(features) && !order.clone().any(|part| part == p) {
            return Err(PassError::MissingPass {
                pass: pass.name,
                features,
            });
        }
    }
    for (i, part) in order.clone().enumerate() {
        let pass = &table[part];
        if !pass.when.holds(features) {
            return Err(PassError::UnexpectedPass {
                pass: pass.name,
                features,
            });
        }
        for &after in pass.after {
            if order.clone().skip(i + 1).any(|later| table[later].name == after) {
                return Err(PassError::OutOfOrder { pass: pass.name, after });
            }
        }
        for (at, step, read) in pass.active(features) {
            let resource = read.resource;
            if !read.access.reads() || resource.lifetime() != Lifetime::Frame {
                continue;
            }
            let writes = |u: &Use| u.resource == resource && u.access.writes();
            let written = pass.active(features).any(|(earlier, _, w)| earlier < at && writes(&w))
                || order
                    .clone()
                    .take(i)
                    .any(|earlier| table[earlier].active(features).any(|(_, _, w)| writes(&w)));
            if !written {
                return Err(PassError::ReadBeforeWrite {
                    pass: pass.name,
                    step: step.name,
                    resource,
                });
            }
        }
    }
    for resource in Resource::ALL {
        let Some(with) = resource.desc().shares else { continue };
        if let (Some(a), Some(b)) = (
            span(table, &order, features, resource),
            span(table, &order, features, with),
        ) && a.0 <= b.1
            && b.0 <= a.1
        {
            return Err(PassError::SharedOverlap { resource, with });
        }
    }
    Ok(())
}

/// The first step after step `step` of pass `pass` that reads `resource` as
/// that step left it, in a frame with `features` whose passes are the ones
/// its features call for, in table order: (its pass, its name). A step that
/// writes `resource` without reading it ends the search: what was there is
/// gone.
pub(crate) fn read_later(
    table: &[Pass],
    pass: usize,
    step: &str,
    resource: Resource,
    features: Features,
) -> Option<(&'static str, &'static str)> {
    let from = table[pass].steps.iter().position(|s| s.name == step)?;
    for (p, later) in table.iter().enumerate().skip(pass) {
        if !later.when.holds(features) {
            continue;
        }
        for (s, candidate) in later.steps.iter().enumerate() {
            if p == pass && s <= from || !candidate.when.holds(features) {
                continue;
            }
            let mut uses = candidate
                .uses
                .iter()
                .filter(|u| u.resource == resource && u.when.holds(features));
            if uses.clone().any(|u| u.access.reads()) {
                return Some((later.name, candidate.name));
            }
            if uses.any(|u| u.access.writes()) {
                return None;
            }
        }
    }
    None
}

/// Checks that discarding `resource` at the end of step `step` of pass
/// `pass` loses nothing a later step reads.
pub(crate) fn check_discard(
    table: &[Pass],
    pass: usize,
    step: &'static str,
    resource: Resource,
    features: Features,
) -> Result<(), PassError> {
    match read_later(table, pass, step, resource, features) {
        Some((reader_pass, reader)) => Err(PassError::DiscardedRead {
            pass: table[pass].name,
            step,
            resource,
            reader_pass,
            reader,
        }),
        None => Ok(()),
    }
}

thread_local! {
    /// The pass of the part this thread is recording, and the steps it has
    /// recorded so far (a bit per step of the pass). Debug builds only.
    static RECORDING: Cell<(usize, u64)> = const { Cell::new((usize::MAX, 0)) };
}

/// Notes that the part being recorded on this thread records step `step` of
/// its pass. Call it where the step's commands are recorded, inside
/// whatever condition decides them. Does nothing in release builds, and
/// outside a part (a test that records one pass by hand).
#[inline]
pub(crate) fn mark(step: &'static str) {
    if cfg!(debug_assertions) {
        RECORDING.with(|recording| {
            let (pass, steps) = recording.get();
            let Some(declared) = FRAME_PASSES.get(pass) else {
                return;
            };
            match declared.steps.iter().position(|s| s.name == step) {
                Some(bit) => recording.set((pass, steps | 1 << bit)),
                None => panic!(
                    "invalid 3d frame: pass \"{}\" recorded step \"{step}\", which it doesn't declare",
                    declared.name
                ),
            }
        });
    }
}

/// Starts noting the steps of a part of pass `pass` on this thread. Returns
/// what to hand [`recorded`]: the part this thread was recording before (a
/// thread may record a part while it waits inside another).
pub(crate) fn recording(pass: usize) -> (usize, u64) {
    RECORDING.with(|recording| recording.replace((pass, 0)))
}

/// The steps noted since [`recording`], as a bit per step of the pass.
pub(crate) fn recorded(outer: (usize, u64)) -> u64 {
    RECORDING.with(|recording| recording.replace(outer)).1
}

/// Checks the steps a part of pass `pass` recorded (see [`recorded`])
/// against the ones the pass declares for a frame with `features`.
pub(crate) fn check_recorded(table: &[Pass], pass: usize, features: Features, steps: u64) -> Result<(), PassError> {
    let pass = &table[pass];
    for (bit, step) in pass.steps.iter().enumerate() {
        let declared = pass.when.and(step.when).holds(features);
        let done = steps & 1 << bit != 0;
        if done && !declared {
            return Err(PassError::UnexpectedStep {
                pass: pass.name,
                step: step.name,
                features,
            });
        }
        if declared && !done {
            return Err(PassError::MissingStep {
                pass: pass.name,
                step: step.name,
                features,
            });
        }
    }
    Ok(())
}

/// The frame's passes, one per line, with their steps and what each uses:
/// for reading a frame's structure in tests and reviews.
#[cfg(test)]
pub(crate) fn describe(table: &[Pass]) -> String {
    let mut text = String::new();
    for pass in table {
        text.push_str(&format!(
            "{}: after [{}] in {}\n",
            pass.name,
            pass.after.join(", "),
            pass.when
        ));
        for step in pass.steps {
            let uses: Vec<String> = step
                .uses
                .iter()
                .map(|u| {
                    let when = if u.when == When::ALWAYS {
                        String::new()
                    } else {
                        format!(" in {}", u.when)
                    };
                    format!("{} as {}{when}", u.resource.name(), u.access.name())
                })
                .collect();
            text.push_str(&format!("  {} in {}: {}\n", step.name, step.when, uses.join("; ")));
        }
    }
    text
}

/// How many of `resources` are alive at once, at most, in a frame: the
/// textures they need if every one whose lifetime is over gave its memory
/// to the next.
#[cfg(test)]
pub(crate) fn alive_at_once(table: &[Pass], order: &[usize], features: Features, resources: &[Resource]) -> usize {
    let spans: Vec<(Place, Place)> = resources
        .iter()
        .filter_map(|&r| span(table, &order.iter().copied(), features, r))
        .collect();
    spans
        .iter()
        .map(|a| spans.iter().filter(|b| b.0 <= a.0 && a.0 <= b.1).count())
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
