//! Shared animation graphs and per-entity playback.
use super::{AnimationClip, Skeleton, Transform3D};
use glam::{Quat, Vec2, Vec3};
use ponko_ecs::Component;
use std::fmt;
use std::sync::Arc;

/// A state of an [`AnimationGraph`]: its position in the graph's state list,
/// from 0.
/// ```
/// let walk = ponko::anim::StateId(1);
/// assert_eq!(walk.0, 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StateId(pub usize);

/// A parameter of an [`AnimationGraph`]: its position in the graph's
/// parameter list, from 0.
/// ```
/// let speed = ponko::anim::ParamId(0);
/// assert_eq!(speed.0, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ParamId(pub usize);

/// An overlay of an [`Animator`], as [`Animator::add_layer`] returns it:
/// overlays are numbered from 0 in the order they were added (the base
/// layer has none).
/// ```
/// let first = ponko::anim::LayerId(0);
/// assert_eq!(first.0, 0);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LayerId(pub usize);

/// Why [`AnimationGraph::new`] refused a graph.
/// ```
/// use ponko::anim::{AnimationGraph, AnimationGraphError, StateId};
/// let err = AnimationGraph::new(vec![], vec![], vec![], StateId(0)).unwrap_err();
/// assert_eq!(err, AnimationGraphError::InvalidInitialState(StateId(0)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AnimationGraphError {
    /// The initial state is not in the state list.
    InvalidInitialState(StateId),
    /// A float parameter's default is NaN or infinite.
    NonFiniteParameter(ParamId),
    /// A state's speed is negative or not finite.
    InvalidSpeed(StateId),
    /// A state's event time is outside 0..1.
    InvalidEventTime(StateId),
    /// A state's blend tree has a child time scale that is not positive and finite.
    InvalidTimeScale(StateId),
    /// A state's 1D blend has no children, or thresholds that are not
    /// finite and strictly increasing.
    InvalidThresholds(StateId),
    /// A state's 2D blend has no children, or a point that is not finite.
    InvalidPoints(StateId),
    /// A transition (by its position in the list) names a state that is not
    /// in the state list.
    InvalidTransitionState(usize),
    /// A transition's duration, offset or exit time is negative or not finite.
    InvalidTransitionClock(usize),
    /// A transition's condition threshold is not finite.
    InvalidThreshold(usize),
    /// A parameter is used as a kind it isn't (or is not in the list).
    WrongParameterKind {
        /// The parameter.
        parameter: ParamId,
        /// The kind it is used as: `"float"`, `"bool"` or `"trigger"`.
        expected: &'static str,
    },
}

impl fmt::Display for AnimationGraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInitialState(s) => write!(f, "initial state {} is not in the graph", s.0),
            Self::NonFiniteParameter(p) => write!(f, "parameter {} has a non-finite default", p.0),
            Self::InvalidSpeed(s) => write!(f, "state {} has an invalid speed", s.0),
            Self::InvalidEventTime(s) => write!(f, "state {} has an event time outside 0..1", s.0),
            Self::InvalidTimeScale(s) => write!(f, "state {} has an invalid child time scale", s.0),
            Self::InvalidThresholds(s) => write!(f, "state {} has invalid 1D blend thresholds", s.0),
            Self::InvalidPoints(s) => write!(f, "state {} has invalid 2D blend points", s.0),
            Self::InvalidTransitionState(t) => write!(f, "transition {t} names a state that is not in the graph"),
            Self::InvalidTransitionClock(t) => write!(f, "transition {t} has an invalid duration, offset or exit time"),
            Self::InvalidThreshold(t) => write!(f, "transition {t} has a non-finite threshold"),
            Self::WrongParameterKind { parameter, expected } => {
                write!(f, "parameter {} is not a {expected} parameter", parameter.0)
            }
        }
    }
}

impl std::error::Error for AnimationGraphError {}

/// Parameter defaults; a [`ParamId`] (the position in the graph's list)
/// identifies each.
/// ```
/// use ponko::anim::AnimationParameter;
/// let speed = ponko::anim::AnimationParameter::Float(0.0);
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum AnimationParameter {
    /// Floating point value.
    Float(f32),
    /// Persistent boolean value.
    Bool(bool),
    /// One-shot flag, consumed only by a selected transition.
    Trigger(bool),
}

/// A recursive motion. Trees use seconds, with each child scaling its clock.
/// Tree duration is the longest child duration divided by its time scale.
/// 2D uses normalized inverse squared distance; an exact point selects only
/// that child (the first if points coincide). Quaternion blends use ordered
/// incremental slerp, so child order matters for more than two rotations.
/// ```
/// use ponko::prelude::*;
/// let clip = ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![]);
/// let motion = ponko::anim::Motion::Clip(clip);
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Motion {
    /// A single clip.
    Clip(AnimationClip),
    /// Sorted, distinct thresholds on a float parameter. Ends clamp.
    Blend1D {
        /// Float parameter.
        parameter: ParamId,
        /// Threshold and child pairs, in increasing order.
        children: Vec<(f32, MotionChild)>,
    },
    /// Freeform points on two float parameters.
    Blend2D {
        /// X float parameter.
        x: ParamId,
        /// Y float parameter.
        y: ParamId,
        /// Point and child pairs.
        children: Vec<(Vec2, MotionChild)>,
    },
}

/// A motion inside a tree; positive time scale multiplies seconds.
/// ```
/// use ponko::prelude::*;
/// let child = ponko::anim::MotionChild { motion: ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])), time_scale: 1.0 };
/// ```
#[derive(Debug, Clone)]
pub struct MotionChild {
    /// Nested clip or tree.
    pub motion: Motion,
    /// Positive, finite clock multiplier.
    pub time_scale: f32,
}

/// A named event on a state's normalized clock.
/// ```
/// use ponko::anim::AnimationEvent;
/// let step = ponko::anim::AnimationEvent { time: 0.5, name: "step".into() };
/// ```
#[derive(Debug, Clone)]
pub struct AnimationEvent {
    /// Normalized time in [0, 1); zero fires on loop wrap, not initial entry.
    pub time: f32,
    /// Shared event name; reporting clones the Arc without allocating.
    pub name: Arc<str>,
}

/// State playback settings. Use [`AnimationState::new`] for looping defaults.
/// ```
/// use ponko::prelude::*;
/// let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
/// assert!(s.looping);
/// ```
#[derive(Debug, Clone)]
pub struct AnimationState {
    /// Motion to sample.
    pub motion: Motion,
    /// Nonnegative playback multiplier.
    pub speed: f32,
    /// Optional float parameter multiplying speed; negative values stop time.
    pub speed_parameter: Option<ParamId>,
    /// Wrap time when true, otherwise clamp at the motion's end.
    pub looping: bool,
    /// Events, reported in time order (ties retain definition order).
    pub events: Vec<AnimationEvent>,
}
impl AnimationState {
    /// Loop a motion at normal speed, without events.
    /// ```
    /// use ponko::prelude::*;
    /// let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// assert_eq!(s.speed, 1.0);
    /// ```
    pub fn new(motion: Motion) -> Self {
        Self {
            motion,
            speed: 1.,
            speed_parameter: None,
            looping: true,
            events: Vec::new(),
        }
    }
}

/// Transition predicates, all of which must hold.
/// ```
/// use ponko::anim::{AnimationCondition, ParamId};
/// let moving = AnimationCondition::Greater(ParamId(0), 0.05);
/// ```
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub enum AnimationCondition {
    /// Float parameter strictly greater than threshold.
    Greater(ParamId, f32),
    /// Float parameter strictly less than threshold.
    Less(ParamId, f32),
    /// Boolean parameter equals the given value.
    Bool(ParamId, bool),
    /// Trigger parameter is set.
    Trigger(ParamId),
}

/// An ordered transition. The graph's first matching transition wins, with at
/// most one taken per update; any-state transitions ignore exit time and do
/// not reenter their current target. Exit time is a minimum on the state's
/// unwrapped normalized clock, checked against the end of this update.
///
/// New fields may be added in any release: build one with
/// [`new`](Self::new) or [`from_any`](Self::from_any) and the `with_*`
/// methods.
/// ```
/// use ponko::anim::{AnimationCondition, AnimationTransition, ParamId, StateId};
/// let t = AnimationTransition::new(StateId(0), StateId(1))
///     .with_condition(AnimationCondition::Greater(ParamId(0), 0.5))
///     .with_exit_time(1.0)
///     .with_duration(0.12);
/// assert_eq!(t.from, Some(StateId(0)));
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AnimationTransition {
    /// Source state, or None for any state.
    pub from: Option<StateId>,
    /// Target state.
    pub to: StateId,
    /// Required predicates; an empty list always matches.
    pub conditions: Vec<AnimationCondition>,
    /// Optional minimum normalized time, ignored for any-state transitions.
    pub exit_time: Option<f32>,
    /// Crossfade duration in seconds, independent of playback speed.
    pub duration: f32,
    /// Target normalized starting time.
    pub offset: f32,
}

impl AnimationTransition {
    /// A transition from state `from` to `to`, with no conditions (it always
    /// matches), no exit time, no crossfade and no offset.
    /// ```
    /// use ponko::anim::{AnimationTransition, StateId};
    /// let t = AnimationTransition::new(StateId(0), StateId(1));
    /// assert_eq!((t.duration, t.exit_time), (0.0, None));
    /// ```
    #[must_use]
    pub fn new(from: StateId, to: StateId) -> Self {
        Self {
            from: Some(from),
            ..Self::from_any(to)
        }
    }

    /// A transition from any state to `to` (not reentering `to` itself),
    /// otherwise as [`new`](Self::new).
    /// ```
    /// use ponko::anim::{AnimationTransition, StateId};
    /// assert_eq!(AnimationTransition::from_any(StateId(2)).from, None);
    /// ```
    #[must_use]
    pub fn from_any(to: StateId) -> Self {
        Self {
            from: None,
            to,
            conditions: Vec::new(),
            exit_time: None,
            duration: 0.,
            offset: 0.,
        }
    }

    /// Adds a predicate that must hold (all of them must).
    #[must_use]
    pub fn with_condition(mut self, condition: AnimationCondition) -> Self {
        self.conditions.push(condition);
        self
    }

    with_fields! {
        /// Sets [`exit_time`](Self::exit_time), the minimum normalized time
        /// before it is taken (`None`: any time).
        exit_time: Option<f32> => with_exit_time;
        /// Sets [`duration`](Self::duration), the crossfade in seconds.
        duration: f32 => with_duration;
        /// Sets [`offset`](Self::offset), the target's normalized starting time.
        offset: f32 => with_offset;
    }
}

/// Immutable graph shared by animators with independent parameters and clocks.
/// ```
/// use ponko::prelude::*;
/// let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
/// let graph = ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph");
/// let animator = ponko::anim::Animator::new(graph.clone());
/// ```
#[derive(Debug)]
pub struct AnimationGraph {
    parameters: Vec<AnimationParameter>,
    states: Vec<AnimationState>,
    transitions: Vec<AnimationTransition>,
    initial: usize,
}
impl AnimationGraph {
    /// Checks ids, parameter kinds, finite clocks and nonempty trees, and
    /// returns the shared graph, or what is wrong with it.
    /// ```
    /// use ponko::prelude::*;
    /// use ponko::anim::StateId;
    /// let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// let graph = ponko::anim::AnimationGraph::new(vec![], vec![s], vec![], StateId(0)).expect("a valid graph");
    /// ```
    pub fn new(
        parameters: Vec<AnimationParameter>,
        mut states: Vec<AnimationState>,
        transitions: Vec<AnimationTransition>,
        initial: StateId,
    ) -> Result<Arc<Self>, AnimationGraphError> {
        if initial.0 >= states.len() {
            return Err(AnimationGraphError::InvalidInitialState(initial));
        }
        for (i, p) in parameters.iter().enumerate() {
            if let AnimationParameter::Float(v) = p
                && !v.is_finite()
            {
                return Err(AnimationGraphError::NonFiniteParameter(ParamId(i)));
            }
        }
        for (i, s) in states.iter_mut().enumerate() {
            let id = StateId(i);
            s.motion.validate(&parameters, id)?;
            if !(s.speed.is_finite() && s.speed >= 0.) {
                return Err(AnimationGraphError::InvalidSpeed(id));
            }
            if let Some(p) = s.speed_parameter {
                expect_kind(&parameters, p, "float")?;
            }
            if !s
                .events
                .iter()
                .all(|e| e.time.is_finite() && (0. ..1.).contains(&e.time))
            {
                return Err(AnimationGraphError::InvalidEventTime(id));
            }
            s.events.sort_by(|a, b| a.time.total_cmp(&b.time));
        }
        for (i, t) in transitions.iter().enumerate() {
            if !(t.to.0 < states.len() && t.from.is_none_or(|s| s.0 < states.len())) {
                return Err(AnimationGraphError::InvalidTransitionState(i));
            }
            if !(t.duration.is_finite()
                && t.duration >= 0.
                && t.offset.is_finite()
                && t.offset >= 0.
                && t.exit_time.is_none_or(|v| v.is_finite() && v >= 0.))
            {
                return Err(AnimationGraphError::InvalidTransitionClock(i));
            }
            for c in &t.conditions {
                match *c {
                    AnimationCondition::Greater(p, v) | AnimationCondition::Less(p, v) => {
                        expect_kind(&parameters, p, "float")?;
                        if !v.is_finite() {
                            return Err(AnimationGraphError::InvalidThreshold(i));
                        }
                    }
                    AnimationCondition::Bool(p, _) => expect_kind(&parameters, p, "bool")?,
                    AnimationCondition::Trigger(p) => expect_kind(&parameters, p, "trigger")?,
                }
            }
        }
        Ok(Arc::new(Self {
            parameters,
            states,
            transitions,
            initial: initial.0,
        }))
    }
}
/// `Ok` when parameter `p` exists and is of kind `expected`.
fn expect_kind(
    parameters: &[AnimationParameter],
    p: ParamId,
    expected: &'static str,
) -> Result<(), AnimationGraphError> {
    let kind = match parameters.get(p.0) {
        Some(AnimationParameter::Float(_)) => "float",
        Some(AnimationParameter::Bool(_)) => "bool",
        Some(AnimationParameter::Trigger(_)) => "trigger",
        None => "",
    };
    if kind == expected {
        Ok(())
    } else {
        Err(AnimationGraphError::WrongParameterKind { parameter: p, expected })
    }
}
fn float_parameter(p: &[AnimationParameter], i: ParamId) -> f32 {
    match p.get(i.0) {
        Some(AnimationParameter::Float(v)) => *v,
        _ => panic!("AnimationGraph: expected float parameter"),
    }
}
impl Motion {
    fn validate(&self, p: &[AnimationParameter], state: StateId) -> Result<(), AnimationGraphError> {
        match self {
            Self::Clip(_) => (),
            Self::Blend1D { parameter, children } => {
                expect_kind(p, *parameter, "float")?;
                if children.is_empty()
                    || !children.iter().all(|(v, _)| v.is_finite())
                    || !children.windows(2).all(|c| c[0].0 < c[1].0)
                {
                    return Err(AnimationGraphError::InvalidThresholds(state));
                }
                for (_, c) in children {
                    c.validate(p, state)?;
                }
            }
            Self::Blend2D { x, y, children } => {
                expect_kind(p, *x, "float")?;
                expect_kind(p, *y, "float")?;
                if children.is_empty() || !children.iter().all(|(v, _)| v.is_finite()) {
                    return Err(AnimationGraphError::InvalidPoints(state));
                }
                for (_, c) in children {
                    c.validate(p, state)?;
                }
            }
        }
        Ok(())
    }
    fn duration(&self) -> f32 {
        match self {
            Self::Clip(c) => c.duration(),
            Self::Blend1D { children, .. } => children
                .iter()
                .map(|(_, c)| c.motion.duration() / c.time_scale)
                .fold(0., f32::max),
            Self::Blend2D { children, .. } => children
                .iter()
                .map(|(_, c)| c.motion.duration() / c.time_scale)
                .fold(0., f32::max),
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn sample(
        &self,
        time: f32,
        looping: bool,
        weight: f32,
        p: &[AnimationParameter],
        output: &mut Skeleton,
        scratch: &mut Skeleton,
        total: &mut f32,
    ) {
        if weight <= 0. {
            return;
        }
        match self {
            Self::Clip(c) => {
                scratch.reset_pose();
                let t = if looping && c.duration() > 0. {
                    time.rem_euclid(c.duration())
                } else {
                    time
                };
                c.sample(t, scratch);
                *total += weight;
                mix(&mut output.pose, &scratch.pose, weight / *total);
                output.morphs.mix(&scratch.morphs, weight / *total);
            }
            Self::Blend1D { parameter, children } => {
                let v = float_parameter(p, *parameter);
                let hi = children.partition_point(|(t, _)| *t <= v);
                if hi == 0 {
                    children[0].1.sample(time, looping, weight, p, output, scratch, total);
                } else if hi == children.len() {
                    children[hi - 1]
                        .1
                        .sample(time, looping, weight, p, output, scratch, total);
                } else {
                    let f = (v - children[hi - 1].0) / (children[hi].0 - children[hi - 1].0);
                    children[hi - 1]
                        .1
                        .sample(time, looping, weight * (1. - f), p, output, scratch, total);
                    children[hi]
                        .1
                        .sample(time, looping, weight * f, p, output, scratch, total);
                }
            }
            Self::Blend2D { x, y, children } => {
                let v = Vec2::new(float_parameter(p, *x), float_parameter(p, *y));
                if let Some((_, c)) = children.iter().find(|(point, _)| *point == v) {
                    c.sample(time, looping, weight, p, output, scratch, total);
                } else {
                    // f64 avoids overflow near points and at large finite parameters.
                    let distance = |point: Vec2| {
                        let dx = f64::from(point.x) - f64::from(v.x);
                        let dy = f64::from(point.y) - f64::from(v.y);
                        dx * dx + dy * dy
                    };
                    let sum: f64 = children.iter().map(|(point, _)| 1. / distance(*point)).sum();
                    for (point, c) in children {
                        c.sample(
                            time,
                            looping,
                            weight * ((1. / distance(*point)) / sum) as f32,
                            p,
                            output,
                            scratch,
                            total,
                        );
                    }
                }
            }
        }
    }
}
impl Motion {
    /// Horizontal root translation and yaw between two unwrapped clock times,
    /// counting whole loops, blended with the same weights as the pose.
    pub(crate) fn root_delta(
        &self,
        from: f32,
        to: f32,
        looping: bool,
        p: &[AnimationParameter],
        skeleton: &Skeleton,
        root: usize,
    ) -> (Vec3, f32) {
        let mut sum = (Vec3::ZERO, 0.);
        self.add_root_delta(from, to, looping, 1., p, skeleton, root, &mut sum);
        sum
    }
    #[allow(clippy::too_many_arguments)]
    fn add_root_delta(
        &self,
        from: f32,
        to: f32,
        looping: bool,
        weight: f32,
        p: &[AnimationParameter],
        skeleton: &Skeleton,
        root: usize,
        sum: &mut (Vec3, f32),
    ) {
        if weight <= 0. {
            return;
        }
        let child = |c: &MotionChild, w: f32, sum: &mut (Vec3, f32)| {
            c.motion.add_root_delta(
                from * c.time_scale,
                to * c.time_scale,
                looping,
                w,
                p,
                skeleton,
                root,
                sum,
            );
        };
        match self {
            Self::Clip(c) => {
                let at = |t: f32| {
                    let pose = c.sample_joint(t, skeleton, root);
                    (pose.position * Vec3::new(1., 0., 1.), yaw(pose.rotation))
                };
                let d = c.duration();
                let between = |a: f32, b: f32| {
                    let (pa, ya) = at(a);
                    let (pb, yb) = at(b);
                    (pb - pa, angle_delta(ya, yb))
                };
                let (position, angle) = if looping && d > 0. {
                    let loops = (to / d).floor() - (from / d).floor();
                    let (lp, ly) = between(0., d);
                    let (pp, py) = between(from.rem_euclid(d), to.rem_euclid(d));
                    (pp + lp * loops, py + ly * loops)
                } else {
                    between(from, to)
                };
                sum.0 += position * weight;
                sum.1 += angle * weight;
            }
            Self::Blend1D { parameter, children } => {
                let v = float_parameter(p, *parameter);
                let hi = children.partition_point(|(t, _)| *t <= v);
                if hi == 0 {
                    child(&children[0].1, weight, sum);
                } else if hi == children.len() {
                    child(&children[hi - 1].1, weight, sum);
                } else {
                    let f = (v - children[hi - 1].0) / (children[hi].0 - children[hi - 1].0);
                    child(&children[hi - 1].1, weight * (1. - f), sum);
                    child(&children[hi].1, weight * f, sum);
                }
            }
            Self::Blend2D { x, y, children } => {
                let v = Vec2::new(float_parameter(p, *x), float_parameter(p, *y));
                if let Some((_, c)) = children.iter().find(|(point, _)| *point == v) {
                    child(c, weight, sum);
                } else {
                    let distance = |point: Vec2| {
                        let dx = f64::from(point.x) - f64::from(v.x);
                        let dy = f64::from(point.y) - f64::from(v.y);
                        dx * dx + dy * dy
                    };
                    let total: f64 = children.iter().map(|(point, _)| 1. / distance(*point)).sum();
                    for (point, c) in children {
                        child(c, weight * ((1. / distance(*point)) / total) as f32, sum);
                    }
                }
            }
        }
    }
}
impl MotionChild {
    fn validate(&self, p: &[AnimationParameter], state: StateId) -> Result<(), AnimationGraphError> {
        if !(self.time_scale.is_finite() && self.time_scale > 0.) {
            return Err(AnimationGraphError::InvalidTimeScale(state));
        }
        self.motion.validate(p, state)
    }
    #[allow(clippy::too_many_arguments)]
    fn sample(
        &self,
        time: f32,
        looping: bool,
        weight: f32,
        p: &[AnimationParameter],
        output: &mut Skeleton,
        scratch: &mut Skeleton,
        total: &mut f32,
    ) {
        self.motion
            .sample(time * self.time_scale, looping, weight, p, output, scratch, total);
    }
}
fn mix(output: &mut [Transform3D], source: &[Transform3D], weight: f32) {
    for (a, b) in output.iter_mut().zip(source) {
        a.position = a.position.lerp(b.position, weight);
        a.scale = a.scale.lerp(b.scale, weight);
        a.rotation = a.rotation.slerp(b.rotation, weight);
    }
}
#[derive(Debug, Clone)]
struct Fade {
    source: Option<(usize, f32)>,
    elapsed: f32,
    duration: f32,
}

/// How a layer combines with the lower pose.
/// ```
/// use ponko::anim::LayerBlend;
/// let mode = ponko::anim::LayerBlend::Additive;
/// assert_ne!(mode, ponko::anim::LayerBlend::Override);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayerBlend {
    /// Interpolate toward the layer pose.
    Override,
    /// Add translation/scale differences and postmultiply the reference-relative rotation.
    Additive,
}

/// Joint indices selected for one skeleton layout. Build once, reuse each frame.
/// ```
/// use ponko::prelude::*;
/// let mask = ponko::anim::JointMask::new(vec![0, 2]);
/// assert!(mask.contains(2));
/// ```
#[derive(Debug, Clone)]
pub struct JointMask(Vec<usize>);
impl JointMask {
    /// Select explicit joint indices (duplicates are removed).
    /// ```
    /// use ponko::anim::JointMask;
    /// assert!(ponko::anim::JointMask::new(vec![3, 3]).contains(3));
    /// ```
    pub fn new(mut joints: Vec<usize>) -> Self {
        joints.sort_unstable();
        joints.dedup();
        Self(joints)
    }
    /// Select the named joint and every descendant; None if the name is absent.
    /// ```
    /// use ponko::prelude::*;
    /// assert!(ponko::anim::JointMask::descendants(&ponko::anim::Skeleton::new(vec![]), "arm").is_none());
    /// ```
    pub fn descendants(skeleton: &Skeleton, name: &str) -> Option<Self> {
        let root = skeleton.find(name)?;
        let mut joints = vec![root];
        for (i, joint) in skeleton.joints().iter().enumerate().skip(root + 1) {
            if joint.parent.is_some_and(|p| joints.binary_search(&p).is_ok()) {
                joints.push(i);
            }
        }
        Some(Self(joints))
    }
    /// Whether the joint index is selected.
    /// ```
    /// use ponko::anim::JointMask;
    /// assert!(!ponko::anim::JointMask::new(vec![1]).contains(0));
    /// ```
    pub fn contains(&self, joint: usize) -> bool {
        self.0.binary_search(&joint).is_ok()
    }
}

/// An overlay with its own graph, parameters, events and clock. Layers run even
/// at weight zero. Configure parameters/commands through `animator`; base-layer
/// commands remain on the parent Animator. Nested layers are supported.
/// ```
/// use ponko::prelude::*;
/// let s = ponko::anim::Skeleton::new(vec![]);
/// let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
/// let mut layer = ponko::anim::AnimationLayer::new(ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
/// layer.weight = 0.5;
/// layer.mode = ponko::anim::LayerBlend::Additive;
/// ```
#[derive(Debug, Clone)]
pub struct AnimationLayer {
    /// Independent playback; its root-motion settings do not move the parent entity.
    pub animator: Box<Animator>,
    /// Runtime weight, clamped to [0, 1]; NaN means zero.
    pub weight: f32,
    /// Override or reference-relative additive blending.
    pub mode: LayerBlend,
    /// None selects every joint. Rebuild index masks when changing skeleton layouts.
    pub mask: Option<JointMask>,
    /// Chosen additive reference; None samples the destination motion at time zero
    /// with current parameters. Missing reference joints use the skeleton rest pose.
    pub reference_pose: Option<Vec<Transform3D>>,
    buffers: Option<(Skeleton, Skeleton, Skeleton)>,
}
impl AnimationLayer {
    /// Make a full-weight override layer, without a mask or explicit reference.
    /// ```
    /// use ponko::prelude::*;
    /// let s = ponko::anim::Skeleton::new(vec![]);
    /// let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
    /// let layer = ponko::anim::AnimationLayer::new(ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(layer.weight, 1.0);
    /// ```
    pub fn new(graph: Arc<AnimationGraph>) -> Self {
        Self {
            animator: Box::new(Animator::new(graph)),
            weight: 1.,
            mode: LayerBlend::Override,
            mask: None,
            reference_pose: None,
            buffers: None,
        }
    }
    fn apply(&mut self, dt: f32, skeleton: &mut Skeleton) {
        if self
            .buffers
            .as_ref()
            .is_none_or(|(s, _, _)| !s.shares_joints(skeleton.shared_joints()))
        {
            self.buffers = Some((skeleton.clone(), skeleton.clone(), skeleton.clone()));
        }
        let (pose, reference, scratch) = self.buffers.as_mut().unwrap();
        self.animator.update(dt, pose);
        let w = if self.weight.is_nan() {
            0.
        } else {
            self.weight.clamp(0., 1.)
        };
        if w == 0. {
            return;
        }
        if self.mode == LayerBlend::Additive && self.reference_pose.is_none() {
            reference.reset_pose();
            self.animator.graph.states[self.animator.state].motion.sample(
                0.,
                false,
                1.,
                &self.animator.parameters,
                reference,
                scratch,
                &mut 0.,
            );
        }
        skeleton
            .morphs
            .layer(&pose.morphs, w, self.mode == LayerBlend::Additive);
        let joints = skeleton.shared_joints().clone();
        for (i, (a, b)) in skeleton.pose.iter_mut().zip(&pose.pose).enumerate() {
            if self.mask.as_ref().is_some_and(|m| !m.contains(i)) {
                continue;
            }
            match self.mode {
                LayerBlend::Override => {
                    a.position = a.position.lerp(b.position, w);
                    a.rotation = a.rotation.slerp(b.rotation, w);
                    a.scale = a.scale.lerp(b.scale, w);
                }
                LayerBlend::Additive => {
                    let r = self
                        .reference_pose
                        .as_ref()
                        .map_or_else(|| reference.pose.get(i), |p| p.get(i))
                        .unwrap_or(&joints[i].rest);
                    a.position += (b.position - r.position) * w;
                    a.scale += (b.scale - r.scale) * w;
                    a.rotation = (a.rotation * Quat::IDENTITY.slerp(r.rotation.inverse() * b.rotation, w)).normalize();
                }
            }
        }
    }
}

/// Horizontal translation in skeleton-local axes and yaw for the latest update.
/// Vertical motion and pitch/roll stay in the pose. Apply translation using the
/// entity's pre-update orientation, then postmultiply its rotation.
/// ```
/// use ponko::prelude::*;
/// let delta = ponko::anim::RootMotion::default();
/// assert_eq!(delta.position, Vec3::ZERO);
/// assert_eq!(delta.rotation, Quat::IDENTITY);
/// ```
#[derive(Debug, Clone, Copy)]
pub struct RootMotion {
    /// Local horizontal displacement, before entity scale.
    pub position: Vec3,
    /// Yaw quaternion delta.
    pub rotation: Quat,
}
impl Default for RootMotion {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}
fn yaw(q: Quat) -> f32 {
    let forward = q * Vec3::Z;
    forward.x.atan2(forward.z)
}
fn angle_delta(a: f32, b: f32) -> f32 {
    (b - a + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// Animation component. The engine updates it every frame after the frame
/// systems and before the draw systems: frame systems see the previous
/// frame's pose, draw systems and the renderer this frame's. `run_headless`
/// has no frames, so there (or to pose it at another point) set `manual` and
/// call [`update`](Self::update) yourself, e.g. from a sim system.
/// Buffers allocate on first use or skeleton changes, never in steady state.
/// Interruptions freeze the last displayed mixed pose as the new fade source;
/// ordinary fades continue advancing the outgoing state. Only the destination
/// emits events and tests transitions. Events accumulate until drained, up to
/// 64 entries; excess events are dropped, retaining the earliest events.
/// ```
/// use ponko::prelude::*;
/// let mut s = ponko::anim::Skeleton::new(vec![]);
/// let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
/// let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
/// a.manual = true;
/// a.update(0.016, &mut s);
/// ```
#[derive(Debug, Clone, Component)]
pub struct Animator {
    graph: Arc<AnimationGraph>,
    parameters: Vec<AnimationParameter>,
    state: usize,
    time: f32,
    fade: Option<Fade>,
    pending: Option<(usize, f32, f32)>,
    buffers: Option<(Skeleton, Skeleton, Skeleton)>,
    events: Vec<Arc<str>>,
    layers: Vec<AnimationLayer>,
    root_delta: RootMotion,
    /// Extract base-layer horizontal translation and yaw instead of posing them.
    pub extract_root_motion: bool,
    /// Root joint index, default zero; invalid indices produce an identity delta.
    /// Extraction uses this joint's local axes, so normally choose a hierarchy root.
    pub root_joint: usize,
    /// Apply extracted motion to the entity Transform3D during automatic updates.
    /// Implies extraction, even when extract_root_motion is false.
    pub apply_root_motion: bool,
    /// Skip automatic engine updates; manual calls still work.
    pub manual: bool,
}
impl Animator {
    /// Start at the graph's initial state with its default parameters.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![], vec![s], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.state(), ponko::anim::StateId(0));
    /// ```
    pub fn new(graph: Arc<AnimationGraph>) -> Self {
        Self {
            parameters: graph.parameters.clone(),
            state: graph.initial,
            graph,
            time: 0.,
            fade: None,
            pending: None,
            buffers: None,
            events: Vec::with_capacity(64),
            layers: Vec::new(),
            root_delta: RootMotion::default(),
            extract_root_motion: false,
            root_joint: 0,
            apply_root_motion: false,
            manual: false,
        }
    }
    /// Append an overlay, returning its zero-based overlay index (base excluded).
    /// ```
    /// use ponko::prelude::*;
    /// let s = ponko::anim::Skeleton::new(vec![]);
    /// let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
    /// let g = ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph");
    /// let mut a = ponko::anim::Animator::new(g.clone());
    /// assert_eq!(a.add_layer(ponko::anim::AnimationLayer::new(g)), ponko::anim::LayerId(0));
    /// ```
    pub fn add_layer(&mut self, layer: AnimationLayer) -> LayerId {
        let index = self.layers.len();
        self.layers.push(layer);
        LayerId(index)
    }
    /// Edit an overlay by index; panics for an invalid index.
    /// ```
    /// use ponko::prelude::*;
    /// # let s = ponko::anim::Skeleton::new(vec![]);
    /// # let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
    /// # let g = ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph");
    /// # let mut a = ponko::anim::Animator::new(g.clone());
    /// let index = a.add_layer(ponko::anim::AnimationLayer::new(g));
    /// a.layer_mut(index).weight = 0.25;
    /// ```
    pub fn layer_mut(&mut self, layer: LayerId) -> &mut AnimationLayer {
        &mut self.layers[layer.0]
    }
    /// Number of overlays added with [`add_layer`](Self::add_layer).
    /// ```
    /// use ponko::prelude::*;
    /// # let s = ponko::anim::Skeleton::new(vec![]);
    /// # let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
    /// let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.layer_count(), 0);
    /// ```
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
    /// Latest update's base-layer motion; identity when extraction is disabled.
    /// Reading does not consume it. play/offset changes never add displacement.
    /// Ordinary crossfades blend deltas using the end-of-update fade weight;
    /// interrupted fades blend from zero velocity (their pose source is frozen).
    /// ```
    /// use ponko::prelude::*;
    /// # let s = ponko::anim::Skeleton::new(vec![]);
    /// # let state = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &s, vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![], vec![state], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.extract_root_motion = true;
    /// assert_eq!(a.root_motion().position, Vec3::ZERO);
    /// ```
    pub fn root_motion(&self) -> RootMotion {
        self.root_delta
    }
    /// The current value of a parameter by index, or `None` for an invalid
    /// index. Lets callers check an index and kind before a setter that panics.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![ponko::anim::AnimationParameter::Bool(true)], vec![s], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert!(matches!(a.parameter(ponko::anim::ParamId(0)), Some(ponko::anim::AnimationParameter::Bool(true))));
    /// assert!(a.parameter(ponko::anim::ParamId(1)).is_none());
    /// ```
    pub fn parameter(&self, parameter: ParamId) -> Option<AnimationParameter> {
        self.parameters.get(parameter.0).copied()
    }
    /// Number of states in the graph, so a state index can be checked before
    /// [`crossfade`](Self::crossfade) or [`play`](Self::play) panic on it.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![], vec![s], vec![], ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.state_count(), 1);
    /// ```
    pub fn state_count(&self) -> usize {
        self.graph.states.len()
    }
    /// Set a float by index. Panics on a wrong type/index or non-finite value.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![ponko::anim::AnimationParameter::Float(0.)],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.set_float(ponko::anim::ParamId(0), 1.5);
    /// ```
    pub fn set_float(&mut self, parameter: ParamId, value: f32) {
        assert!(value.is_finite(), "Animator: non-finite float");
        match self.parameters.get_mut(parameter.0) {
            Some(AnimationParameter::Float(v)) => *v = value,
            _ => panic!("Animator: expected float parameter"),
        }
    }
    /// Set a persistent bool by index; panics on a wrong type/index.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![ponko::anim::AnimationParameter::Bool(false)],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.set_bool(ponko::anim::ParamId(0), true);
    /// ```
    pub fn set_bool(&mut self, parameter: ParamId, value: bool) {
        match self.parameters.get_mut(parameter.0) {
            Some(AnimationParameter::Bool(v)) => *v = value,
            _ => panic!("Animator: expected bool parameter"),
        }
    }
    /// Set or reset a trigger by index; panics on a wrong type/index.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![ponko::anim::AnimationParameter::Trigger(false)],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.set_trigger(ponko::anim::ParamId(0), true);
    /// ```
    pub fn set_trigger(&mut self, parameter: ParamId, value: bool) {
        match self.parameters.get_mut(parameter.0) {
            Some(AnimationParameter::Trigger(v)) => *v = value,
            _ => panic!("Animator: expected trigger parameter"),
        }
    }
    /// Destination state index, including while crossfading.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.state(), ponko::anim::StateId(0));
    /// ```
    pub fn state(&self) -> StateId {
        StateId(self.state)
    }
    /// Unwrapped normalized destination clock (clamped for non-looping states).
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.normalized_time(), 0.0);
    /// ```
    pub fn normalized_time(&self) -> f32 {
        let d = self.graph.states[self.state].motion.duration();
        if d > 0. { self.time / d } else { 0. }
    }
    /// Destination weight; one when no crossfade is active.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// assert_eq!(a.fade_weight(), 1.0);
    /// ```
    pub fn fade_weight(&self) -> f32 {
        self.fade.as_ref().map_or(1., |f| (f.elapsed / f.duration).min(1.))
    }
    /// Queue a fixed-time fade; applied on the next update, before advancing.
    /// Multiple commands before an update use the last command. Panics for
    /// invalid state, negative/non-finite duration or normalized offset.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.crossfade(ponko::anim::StateId(0), 0.12, 0.0);
    /// ```
    pub fn crossfade(&mut self, state: StateId, duration: f32, normalized_offset: f32) {
        assert!(
            state.0 < self.graph.states.len()
                && duration.is_finite()
                && duration >= 0.
                && normalized_offset.is_finite()
                && normalized_offset >= 0.,
            "Animator: invalid playback command"
        );
        self.pending = Some((state.0, duration, normalized_offset));
    }
    /// Queue an immediate state change at a normalized time; applied on update.
    /// The pose snaps: with TAA or temporal AO on, call
    /// [`Graphics::reset_motion`](crate::gfx::Graphics::reset_motion) for the
    /// posed model on that frame, so its jump is not read as motion.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.play(ponko::anim::StateId(0), 0.5);
    /// ```
    pub fn play(&mut self, state: StateId, normalized_time: f32) {
        self.crossfade(state, 0., normalized_time);
    }
    /// Takes the oldest unread event, if any (for reading events one at a
    /// time, as the C API does; [`drain_events`](Self::drain_events) takes all).
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// while let Some(name) = a.next_event() { println!("{name}"); }
    /// ```
    pub fn next_event(&mut self) -> Option<Arc<str>> {
        (!self.events.is_empty()).then(|| self.events.remove(0))
    }
    /// Drain retained event names, preserving the buffer's allocation.
    /// ```
    /// # use ponko::prelude::*;
    /// # let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &ponko::anim::Skeleton::new(vec![]), vec![])));
    /// # let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// for name in a.drain_events() { println!("{name}"); }
    /// ```
    pub fn drain_events(&mut self) -> impl Iterator<Item = Arc<str>> + '_ {
        self.events.drain(..)
    }
    fn advance(&self, state: usize, time: f32, dt: f32) -> f32 {
        let s = &self.graph.states[state];
        let speed = s.speed
            * s.speed_parameter
                .map_or(1., |p| float_parameter(&self.parameters, p).max(0.));
        let next = (time + dt * speed).min(f32::MAX);
        if s.looping { next } else { next.min(s.motion.duration()) }
    }
    fn matches(&self, c: &AnimationCondition) -> bool {
        match *c {
            AnimationCondition::Greater(i, v) => float_parameter(&self.parameters, i) > v,
            AnimationCondition::Less(i, v) => float_parameter(&self.parameters, i) < v,
            AnimationCondition::Bool(i, v) => matches!(self.parameters[i.0], AnimationParameter::Bool(b) if b == v),
            AnimationCondition::Trigger(i) => matches!(self.parameters[i.0], AnimationParameter::Trigger(true)),
        }
    }
    /// Advance by nonnegative finite seconds and sample into the skeleton.
    /// Panics on invalid dt. Calls with zero dt apply queued commands and
    /// transitions but emit no events. Warm up once with each skeleton layout
    /// before allocation-sensitive gameplay (clip name binding also warms up).
    /// ```
    /// # use ponko::prelude::*;
    /// let mut skeleton = ponko::anim::Skeleton::new(vec![]);
    /// let s = ponko::anim::AnimationState::new(ponko::anim::Motion::Clip(ponko::anim::AnimationClip::new("idle", &skeleton, vec![])));
    /// let mut a = ponko::anim::Animator::new(ponko::anim::AnimationGraph::new(vec![],vec![s],vec![],ponko::anim::StateId(0)).expect("a valid graph"));
    /// a.update(0.016, &mut skeleton);
    /// ```
    pub fn update(&mut self, dt: f32, skeleton: &mut Skeleton) {
        self.update_base(dt, skeleton);
        for layer in &mut self.layers {
            layer.apply(dt, skeleton);
        }
        if self.extract_root_motion || self.apply_root_motion {
            let rest = skeleton.joints().get(self.root_joint).map(|j| j.rest);
            if let (Some(rest), Some(pose)) = (rest, skeleton.pose.get_mut(self.root_joint)) {
                pose.position.x = rest.position.x;
                pose.position.z = rest.position.z;
                pose.rotation = Quat::from_rotation_y(yaw(rest.rotation) - yaw(pose.rotation)) * pose.rotation;
            }
        }
    }
    fn update_base(&mut self, dt: f32, skeleton: &mut Skeleton) {
        assert!(
            dt.is_finite() && dt >= 0.,
            "Animator: dt must be finite and nonnegative"
        );
        if self
            .buffers
            .as_ref()
            .is_none_or(|(s, _, _)| !s.shares_joints(skeleton.shared_joints()))
        {
            self.buffers = Some((skeleton.clone(), skeleton.clone(), skeleton.clone()));
        }
        let command = self.pending.take();
        let selected = if command.is_none() {
            self.graph.transitions.iter().position(|t| {
                t.from.map_or(t.to.0 != self.state, |s| s.0 == self.state)
                    && (t.from.is_none()
                        || t.exit_time.is_none_or(|exit| {
                            let d = self.graph.states[self.state].motion.duration();
                            d > 0. && self.advance(self.state, self.time, dt) / d >= exit
                        }))
                    && t.conditions.iter().all(|c| self.matches(c))
            })
        } else {
            None
        };
        let command = command.or_else(|| {
            selected.map(|i| {
                let t = &self.graph.transitions[i];
                for c in &t.conditions {
                    if let AnimationCondition::Trigger(p) = *c {
                        self.parameters[p.0] = AnimationParameter::Trigger(false);
                    }
                }
                (t.to.0, t.duration, t.offset)
            })
        });
        if let Some((state, duration, offset)) = command {
            let source = if self.fade.is_some() {
                None
            } else {
                Some((self.state, self.time))
            };
            let (_, _, snapshot) = self.buffers.as_mut().unwrap();
            snapshot.pose.clone_from(&skeleton.pose);
            snapshot.morphs.clone_from(&skeleton.morphs);
            self.fade = if duration > 0. {
                Some(Fade {
                    source,
                    elapsed: 0.,
                    duration,
                })
            } else {
                None
            };
            self.state = state;
            let s = &self.graph.states[state];
            self.time = offset * s.motion.duration();
            if !s.looping {
                self.time = self.time.min(s.motion.duration());
            }
        }
        let old = self.time;
        self.time = self.advance(self.state, self.time, dt);
        self.collect_events(old, self.time);
        self.root_delta = RootMotion::default();
        if (self.extract_root_motion || self.apply_root_motion) && self.root_joint < skeleton.joints().len() {
            let state = &self.graph.states[self.state];
            let mut delta = state.motion.root_delta(
                old,
                self.time,
                state.looping,
                &self.parameters,
                skeleton,
                self.root_joint,
            );
            if let Some(fade) = &self.fade {
                let w = ((fade.elapsed + dt) / fade.duration).min(1.);
                let source = fade.source.map_or((Vec3::ZERO, 0.), |(i, time)| {
                    let s = &self.graph.states[i];
                    s.motion.root_delta(
                        time,
                        self.advance(i, time, dt),
                        s.looping,
                        &self.parameters,
                        skeleton,
                        self.root_joint,
                    )
                });
                delta = (source.0.lerp(delta.0, w), source.1 + (delta.1 - source.1) * w);
            }
            self.root_delta = RootMotion {
                position: delta.0,
                rotation: Quat::from_rotation_y(delta.1),
            };
        }
        if let Some(mut fade) = self.fade.take() {
            fade.elapsed = (fade.elapsed + dt).min(fade.duration);
            if let Some((state, time)) = fade.source {
                fade.source = Some((state, self.advance(state, time, dt)));
            }
            if fade.elapsed < fade.duration {
                self.fade = Some(fade);
            }
        }
        let (output, scratch, snapshot) = self.buffers.as_mut().unwrap();
        let state = &self.graph.states[self.state];
        output.reset_pose();
        state
            .motion
            .sample(self.time, state.looping, 1., &self.parameters, output, scratch, &mut 0.);
        if let Some(f) = &self.fade {
            if let Some((i, time)) = f.source {
                let s = &self.graph.states[i];
                snapshot.reset_pose();
                s.motion
                    .sample(time, s.looping, 1., &self.parameters, snapshot, scratch, &mut 0.);
            }
            mix(&mut output.pose, &snapshot.pose, 1. - f.elapsed / f.duration);
            output.morphs.mix(&snapshot.morphs, 1. - f.elapsed / f.duration);
        }
        skeleton.pose.clone_from(&output.pose);
        skeleton.morphs.clone_from(&output.morphs);
    }
    fn collect_events(&mut self, old: f32, new: f32) {
        let state = &self.graph.states[self.state];
        let d = state.motion.duration();
        if d <= 0. || new <= old || state.events.is_empty() {
            return;
        }
        let start = f64::from(old) / f64::from(d);
        let end = f64::from(new) / f64::from(d);
        let mut cycle = if state.looping { start.floor() } else { 0. };
        while cycle <= end.floor() && self.events.len() < 64 {
            for e in &state.events {
                let at = cycle + f64::from(e.time);
                if at > start && at <= end && self.events.len() < 64 {
                    self.events.push(e.name.clone());
                }
            }
            if !state.looping {
                break;
            }
            let next = cycle + 1.;
            if next == cycle {
                break;
            }
            cycle = next;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::AnimationTrack;
    use crate::anim::Interpolation;
    use crate::anim::Joint;
    use crate::anim::JointAnimation;
    use crate::d3::Transform as Transform3D;
    use glam::Vec4;
    /// A graph that must be valid, with plain indices for brevity.
    fn graph(
        parameters: Vec<AnimationParameter>,
        states: Vec<AnimationState>,
        transitions: Vec<AnimationTransition>,
        initial: usize,
    ) -> Arc<AnimationGraph> {
        AnimationGraph::new(parameters, states, transitions, StateId(initial)).expect("a valid graph")
    }
    fn skeleton() -> Skeleton {
        Skeleton::new(vec![Joint {
            name: "root".into(),
            parent: None,
            rest: Transform3D::default(),
        }])
    }
    fn clip(s: &Skeleton, x: f32) -> AnimationClip {
        AnimationClip::new(
            "pose",
            s,
            vec![JointAnimation {
                joint: 0,
                name: "root".into(),
                translation: Some(AnimationTrack::new(
                    vec![0., 1.],
                    vec![Vec4::new(x, 0., 0., 0.); 2],
                    Interpolation::Linear,
                )),
                rotation: None,
                scale: None,
            }],
        )
    }
    fn near(a: f32, b: f32) {
        assert!((a - b).abs() < 1e-5, "{a} != {b}");
    }
    #[test]
    fn layers_weight_mask_and_additive_rotation() {
        let mut s = Skeleton::new(vec![
            Joint {
                name: "root".into(),
                parent: None,
                rest: Transform3D::default(),
            },
            Joint {
                name: "arm".into(),
                parent: Some(0),
                rest: Transform3D::default(),
            },
        ]);
        let mut a = animator(&s, Motion::Clip(clip(&s, 2.)), vec![]);
        let overlay = animator(&s, Motion::Clip(clip(&s, 10.)), vec![]);
        let mut layer = AnimationLayer::new(overlay.graph.clone());
        layer.mask = Some(JointMask::descendants(&s, "arm").unwrap());
        let i = a.add_layer(layer);
        a.update(0., &mut s);
        near(s.pose[0].position.x, 2.);
        a.layer_mut(i).mask = None;
        a.layer_mut(i).weight = 0.;
        a.update(0., &mut s);
        near(s.pose[0].position.x, 2.);
        a.layer_mut(i).weight = 1.;
        a.update(0., &mut s);
        near(s.pose[0].position.x, 10.);
        let rotation = AnimationClip::new(
            "add",
            &s,
            vec![JointAnimation {
                joint: 0,
                name: "root".into(),
                translation: None,
                scale: None,
                rotation: Some(AnimationTrack::new(
                    vec![0., 1.],
                    vec![Vec4::W, Vec4::from_array(glam::Quat::from_rotation_z(0.2).to_array())],
                    Interpolation::Linear,
                )),
            }],
        );
        let mut layer = AnimationLayer::new(animator(&s, Motion::Clip(rotation), vec![]).graph);
        layer.mode = LayerBlend::Additive;
        layer.animator.graph = graph(
            vec![],
            vec![{
                let mut state = layer.animator.graph.states[0].clone();
                state.looping = false;
                state
            }],
            vec![],
            0,
        );
        a.add_layer(layer);
        a.update(1., &mut s);
        assert!(s.pose[0].rotation.abs_diff_eq(glam::Quat::from_rotation_z(0.2), 1e-5));
    }
    fn walk(s: &Skeleton, distance: f32) -> AnimationClip {
        AnimationClip::new(
            "walk",
            s,
            vec![JointAnimation {
                joint: 0,
                name: "root".into(),
                translation: Some(AnimationTrack::new(
                    vec![0., 1.],
                    vec![Vec4::ZERO, Vec4::new(distance, 0., 0., 0.)],
                    Interpolation::Linear,
                )),
                rotation: None,
                scale: None,
            }],
        )
    }
    #[test]
    fn root_motion_wrap_speed_and_crossfade() {
        let mut s = skeleton();
        let mut fast = AnimationState::new(Motion::Clip(walk(&s, 2.)));
        fast.speed = 2.;
        let mut a = Animator::new(graph(
            vec![],
            vec![AnimationState::new(Motion::Clip(walk(&s, 1.))), fast],
            vec![],
            0,
        ));
        a.extract_root_motion = true;
        let mut total = 0.;
        for _ in 0..10 {
            a.update(0.3, &mut s);
            total += a.root_motion().position.x;
            near(s.pose[0].position.x, 0.);
        }
        near(total, 3.);
        a.crossfade(StateId(1), 1., 0.);
        a.update(0.5, &mut s);
        near(a.root_motion().position.x, 1.25);
        a.update(2., &mut s);
        near(a.root_motion().position.x, 8.);
    }
    #[test]
    fn idle_walk_run_use_ordered_transitions_and_fixed_time_weights() {
        let mut s = skeleton();
        let states = [0., 1., 2.]
            .map(|x| AnimationState::new(Motion::Clip(clip(&s, x))))
            .to_vec();
        let transition = |from, to, condition| AnimationTransition {
            from: Some(StateId(from)),
            to: StateId(to),
            conditions: vec![condition],
            duration: 0.12,
            exit_time: None,
            offset: 0.,
        };
        let g = graph(
            vec![AnimationParameter::Float(0.)],
            states,
            vec![
                transition(0, 1, AnimationCondition::Greater(ParamId(0), 0.05)),
                transition(1, 2, AnimationCondition::Greater(ParamId(0), 1.4)),
            ],
            0,
        );
        let mut a = Animator::new(g);
        a.update(0., &mut s);
        a.set_float(ParamId(0), 1.);
        a.update(0.03, &mut s);
        assert_eq!(a.state(), StateId(1));
        near(a.fade_weight(), 0.25);
        near(s.pose[0].position.x, 0.25);
        a.update(0.09, &mut s);
        near(a.fade_weight(), 1.);
        a.set_float(ParamId(0), 2.);
        a.update(0.06, &mut s);
        assert_eq!(a.state(), StateId(2));
        near(s.pose[0].position.x, 1.5);
    }
    fn animator(_s: &Skeleton, motion: Motion, parameters: Vec<AnimationParameter>) -> Animator {
        Animator::new(graph(parameters, vec![AnimationState::new(motion)], vec![], 0))
    }
    fn child(s: &Skeleton, x: f32) -> MotionChild {
        MotionChild {
            motion: Motion::Clip(clip(s, x)),
            time_scale: 1.,
        }
    }
    #[test]
    fn one_dimensional_blends_match_thresholds_and_interpolate_neighbours() {
        let mut s = skeleton();
        let tree = Motion::Blend1D {
            parameter: ParamId(0),
            children: vec![(0., child(&s, 0.)), (1., child(&s, 10.)), (2., child(&s, 30.))],
        };
        let mut a = animator(&s, tree, vec![AnimationParameter::Float(0.)]);
        for (v, want) in [
            (-1., 0.),
            (0., 0.),
            (0.5, 5.),
            (1., 10.),
            (1.5, 20.),
            (2., 30.),
            (3., 30.),
        ] {
            a.set_float(ParamId(0), v);
            a.update(0., &mut s);
            near(s.pose[0].position.x, want);
        }
    }
    #[test]
    fn two_dimensional_blends_select_cardinals_exactly_and_support_nested_trees() {
        let mut s = skeleton();
        let points = [Vec2::X, Vec2::Y, Vec2::NEG_X, Vec2::NEG_Y];
        let children = points
            .into_iter()
            .enumerate()
            .map(|(i, p)| (p, child(&s, i as f32)))
            .collect();
        let tree = Motion::Blend2D {
            x: ParamId(0),
            y: ParamId(1),
            children,
        };
        let nested = Motion::Blend1D {
            parameter: ParamId(0),
            children: vec![(
                0.,
                MotionChild {
                    motion: tree,
                    time_scale: 1.,
                },
            )],
        };
        let mut a = animator(&s, nested, vec![AnimationParameter::Float(0.); 2]);
        for (i, p) in points.into_iter().enumerate() {
            a.set_float(ParamId(0), p.x);
            a.set_float(ParamId(1), p.y);
            a.update(0., &mut s);
            near(s.pose[0].position.x, i as f32);
        }
        a.set_float(ParamId(0), 0.);
        a.set_float(ParamId(1), 0.);
        a.update(0., &mut s);
        near(s.pose[0].position.x, 1.5);
    }
    fn transition(from: Option<usize>, to: usize, conditions: Vec<AnimationCondition>) -> AnimationTransition {
        AnimationTransition {
            from: from.map(StateId),
            to: StateId(to),
            conditions,
            exit_time: None,
            duration: 0.,
            offset: 0.,
        }
    }
    #[test]
    fn selected_transition_consumes_triggers_but_failed_conditions_do_not() {
        let mut s = skeleton();
        let states = vec![AnimationState::new(Motion::Clip(clip(&s, 0.))); 2];
        let transitions = vec![
            transition(
                Some(0),
                1,
                vec![
                    AnimationCondition::Trigger(ParamId(0)),
                    AnimationCondition::Bool(ParamId(1), true),
                ],
            ),
            transition(Some(1), 0, vec![AnimationCondition::Trigger(ParamId(0))]),
        ];
        let mut a = Animator::new(graph(
            vec![AnimationParameter::Trigger(false), AnimationParameter::Bool(false)],
            states,
            transitions,
            0,
        ));
        a.set_trigger(ParamId(0), true);
        a.update(0., &mut s);
        assert_eq!(a.state(), StateId(0));
        a.set_bool(ParamId(1), true);
        a.update(0., &mut s);
        assert_eq!(a.state(), StateId(1));
        a.update(0., &mut s);
        assert_eq!(a.state(), StateId(1));
        a.set_trigger(ParamId(0), true);
        a.update(0., &mut s);
        assert_eq!(a.state(), StateId(0));
    }
    #[test]
    fn exit_time_waits_until_normalized_threshold_and_any_state_ignores_it() {
        let mut s = skeleton();
        let states = vec![AnimationState::new(Motion::Clip(clip(&s, 0.))); 2];
        let mut t = transition(Some(0), 1, vec![]);
        t.exit_time = Some(0.75);
        let mut a = Animator::new(graph(vec![], states.clone(), vec![t.clone()], 0));
        a.update(0.5, &mut s);
        assert_eq!(a.state(), StateId(0));
        a.update(0.25, &mut s);
        assert_eq!(a.state(), StateId(1));
        t.from = None;
        t.exit_time = Some(100.);
        let mut a = Animator::new(graph(vec![], states, vec![t], 0));
        a.update(0., &mut s);
        assert_eq!(a.state(), StateId(1));
    }
    #[test]
    fn interrupted_crossfade_starts_at_the_last_mixed_pose() {
        let mut s = skeleton();
        let states = [0., 10., 20.]
            .map(|x| AnimationState::new(Motion::Clip(clip(&s, x))))
            .to_vec();
        let mut a = Animator::new(graph(vec![], states, vec![], 0));
        a.update(0., &mut s);
        a.crossfade(StateId(1), 1., 0.);
        a.update(0.5, &mut s);
        near(s.pose[0].position.x, 5.);
        a.crossfade(StateId(2), 1., 0.);
        a.update(0., &mut s);
        near(s.pose[0].position.x, 5.);
        a.update(0.5, &mut s);
        near(s.pose[0].position.x, 12.5);
        a.play(StateId(0), 0.);
        a.update(0., &mut s);
        near(s.pose[0].position.x, 0.);
    }
    #[test]
    fn events_cross_loop_wrap_in_time_order_and_reuse_buffer() {
        let mut s = skeleton();
        let mut state = AnimationState::new(Motion::Clip(clip(&s, 0.)));
        state.events = vec![
            AnimationEvent {
                time: 0.9,
                name: "end".into(),
            },
            AnimationEvent {
                time: 0.1,
                name: "start".into(),
            },
        ];
        let mut a = Animator::new(graph(vec![], vec![state], vec![], 0));
        a.play(StateId(0), 0.8);
        a.update(0.4, &mut s);
        assert_eq!(
            a.drain_events().map(|s| s.to_string()).collect::<Vec<_>>(),
            ["end", "start"]
        );
        a.update(0., &mut s);
        assert_eq!(a.drain_events().count(), 0);
        a.update(100., &mut s);
        assert_eq!(a.drain_events().count(), 64);
    }
    #[test]
    fn child_time_scale_and_parameter_speed_multiply_and_clamping_holds_end() {
        let mut s = skeleton();
        let c = AnimationClip::new(
            "ramp",
            &s,
            vec![JointAnimation {
                joint: 0,
                name: "root".into(),
                translation: Some(AnimationTrack::new(
                    vec![0., 1.],
                    vec![Vec4::ZERO, Vec4::X],
                    Interpolation::Linear,
                )),
                rotation: None,
                scale: None,
            }],
        );
        let mut state = AnimationState::new(Motion::Blend1D {
            parameter: ParamId(0),
            children: vec![(
                0.,
                MotionChild {
                    motion: Motion::Clip(c),
                    time_scale: 2.,
                },
            )],
        });
        state.speed = 2.;
        state.speed_parameter = Some(ParamId(0));
        state.looping = false;
        let mut a = Animator::new(graph(vec![AnimationParameter::Float(0.5)], vec![state], vec![], 0));
        a.update(0.125, &mut s);
        near(s.pose[0].position.x, 0.25);
        near(a.normalized_time(), 0.25);
        a.update(2., &mut s);
        near(s.pose[0].position.x, 1.);
        near(a.normalized_time(), 1.);
        a.set_float(ParamId(0), -1.);
        a.play(StateId(0), 0.);
        a.update(1., &mut s);
        near(s.pose[0].position.x, 0.);
    }
    #[test]
    fn duplicate_thresholds_are_rejected() {
        let s = skeleton();
        let state = AnimationState::new(Motion::Blend1D {
            parameter: ParamId(0),
            children: vec![(0., child(&s, 0.)), (0., child(&s, 1.))],
        });
        let err =
            AnimationGraph::new(vec![AnimationParameter::Float(0.)], vec![state], vec![], StateId(0)).unwrap_err();
        assert_eq!(err, AnimationGraphError::InvalidThresholds(StateId(0)));
        assert_eq!(err.to_string(), "state 0 has invalid 1D blend thresholds");
    }
    #[test]
    fn invalid_graphs_are_errors_naming_the_problem() {
        let s = skeleton();
        let idle = || AnimationState::new(Motion::Clip(clip(&s, 0.)));
        let new = |p, states, t, i| AnimationGraph::new(p, states, t, StateId(i)).unwrap_err();
        assert_eq!(
            new(vec![], vec![idle()], vec![], 1),
            AnimationGraphError::InvalidInitialState(StateId(1))
        );
        assert_eq!(
            new(vec![AnimationParameter::Float(f32::NAN)], vec![idle()], vec![], 0),
            AnimationGraphError::NonFiniteParameter(ParamId(0))
        );
        assert_eq!(
            new(vec![], vec![idle()], vec![transition(Some(0), 3, vec![])], 0),
            AnimationGraphError::InvalidTransitionState(0)
        );
        assert_eq!(
            new(
                vec![AnimationParameter::Bool(false)],
                vec![idle()],
                vec![transition(None, 0, vec![AnimationCondition::Trigger(ParamId(0))])],
                0
            ),
            AnimationGraphError::WrongParameterKind {
                parameter: ParamId(0),
                expected: "trigger"
            }
        );
        let mut fast = idle();
        fast.speed = -1.;
        assert_eq!(
            new(vec![], vec![idle(), fast], vec![], 0),
            AnimationGraphError::InvalidSpeed(StateId(1))
        );
    }
    #[test]
    fn automatic_updates_run_without_a_renderer_and_manual_animators_are_skipped() {
        let mut ctx = crate::ctx::Ctx::new(0.25, Vec2::ONE);
        let s = skeleton();
        let c = clip(&s, 3.);
        let graph = graph(vec![], vec![AnimationState::new(Motion::Clip(c))], vec![], 0);
        let automatic = ctx.world.spawn((Animator::new(graph.clone()), s.clone()));
        let mut manual = Animator::new(graph);
        manual.manual = true;
        let manual = ctx.world.spawn((manual, s));
        ctx.update_animators();
        near(ctx.world.get::<Skeleton>(automatic).unwrap().pose[0].position.x, 3.);
        near(ctx.world.get::<Skeleton>(manual).unwrap().pose[0].position.x, 0.);
    }
    #[test]
    fn first_matching_transition_wins_and_target_offset_is_applied() {
        let mut s = skeleton();
        let states = vec![AnimationState::new(Motion::Clip(clip(&s, 0.))); 3];
        let mut t = transition(Some(0), 1, vec![AnimationCondition::Less(ParamId(0), 1.)]);
        t.offset = 0.5;
        let mut a = Animator::new(graph(
            vec![AnimationParameter::Float(0.)],
            states,
            vec![t, transition(None, 2, vec![])],
            0,
        ));
        a.update(0.1, &mut s);
        assert_eq!(a.state(), StateId(1));
        near(a.normalized_time(), 0.6);
    }
    #[test]
    fn ordinary_fades_keep_advancing_the_outgoing_clip() {
        let mut s = skeleton();
        let ramp = AnimationClip::new(
            "ramp",
            &s,
            vec![JointAnimation {
                joint: 0,
                name: "root".into(),
                translation: Some(AnimationTrack::new(
                    vec![0., 1.],
                    vec![Vec4::ZERO, Vec4::X],
                    Interpolation::Linear,
                )),
                rotation: None,
                scale: None,
            }],
        );
        let mut a = Animator::new(graph(
            vec![],
            vec![
                AnimationState::new(Motion::Clip(ramp)),
                AnimationState::new(Motion::Clip(clip(&s, 2.))),
            ],
            vec![],
            0,
        ));
        a.update(0.25, &mut s);
        a.crossfade(StateId(1), 1., 0.);
        a.update(0.25, &mut s);
        near(s.pose[0].position.x, 0.875);
    }
}
