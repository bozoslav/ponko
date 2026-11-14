//! Positional playback without custom DSP: Kira applies the gain, pan and rate.
use super::{Audio, AudioBus, AudioEffects, Playing, Sound, SoundParams};
use crate::ctx::Ctx;
use crate::d3::Transform as Transform3D;
use glam::{Quat, Vec3};
use ponko_ecs::{Component, Entity};

/// Distance gain curve. Keyed curves use normalized distance and linear gain;
/// keys must be sorted by distance. Values outside the keys use the nearest key.
/// ```
/// use ponko::audio::AudioRolloff;
/// let curve = ponko::audio::AudioRolloff::Keyed(&[(0.0, 1.0), (0.5, 0.3), (1.0, 0.0)]);
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub enum AudioRolloff {
    /// Inverse distance inside the range, silent at the maximum distance.
    #[default]
    Logarithmic,
    /// Linear fade between the minimum and maximum distances.
    Linear,
    /// Sorted (normalized distance, gain) keys; an empty curve is silent.
    Keyed(&'static [(f32, f32)]),
}

/// Settings for a positional voice. Distances are in world units (metres for
/// doppler); pitch is controlled by `sound.speed`. Invalid ranges are clamped.
/// ```
/// use ponko::prelude::*;
/// let settings = ponko::d3::AudioSettings { max_distance: 80.0, doppler: 0.0, ..Default::default() };
/// ```
#[derive(Clone, Copy, Debug)]
pub struct AudioSettings3D {
    /// Volume, pitch/playback speed and looping.
    pub sound: SoundParams,
    /// Routing bus, selected when playback starts. Defaults to `sfx`.
    /// ```
    /// # use ponko::prelude::*;
    /// let settings = ponko::d3::AudioSettings { bus: AudioBus::default(), ..Default::default() };
    /// ```
    pub bus: AudioBus,
    /// Initial per-voice low-pass and shared reverb send. Update live voices
    /// with `Audio::set_effects` (including positional voices).
    /// ```
    /// # use ponko::prelude::*;
    /// let settings = ponko::d3::AudioSettings { effects: ponko::audio::AudioEffects::default(), ..Default::default() };
    /// ```
    pub effects: AudioEffects,
    /// Full-volume radius, clamped to at least 0.001.
    pub min_distance: f32,
    /// Silent beyond this radius; clamped above the minimum radius.
    pub max_distance: f32,
    /// Distance gain curve.
    pub rolloff: AudioRolloff,
    /// Velocity multiplier; zero disables doppler. Default 1, sound speed 343 m/s.
    pub doppler: f32,
}
impl Default for AudioSettings3D {
    fn default() -> Self {
        Self {
            sound: SoundParams::default(),
            bus: AudioBus::default(),
            effects: AudioEffects::default(),
            min_distance: 1.0,
            max_distance: 100.0,
            rolloff: AudioRolloff::default(),
            doppler: 1.0,
        }
    }
}

/// Where the ears are. By default they follow `ctx.scene3d.camera`; set your own with
/// [`Audio::set_listener`]. Velocity (for doppler) is estimated from frame
/// to frame.
/// ```
/// use ponko::prelude::*;
/// let listener = ponko::d3::AudioListener { position: Vec3::ZERO, rotation: Quat::IDENTITY };
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioListener3D {
    /// Ear position in world space.
    pub position: Vec3,
    /// Orientation: +X is right and -Z is forward.
    pub rotation: Quat,
}

/// A sound that plays from an entity, paired with `Transform3D`. It starts by
/// itself and follows the entity every frame. Removing the component or the
/// entity stops it. One-shots don't restart after they finish.
/// ```
/// use ponko::prelude::*;
/// # fn setup(ctx: &mut Ctx, sound: Sound) {
/// ctx.world.spawn((ponko::d3::Transform::default(), ponko::d3::AudioSource::new(sound, ponko::d3::AudioSettings::default())));
/// # }
/// ```
#[derive(Debug, Component)]
pub struct AudioSource3D {
    /// Clip to play. Changes apply when the component is replaced.
    pub sound: Sound,
    /// Live volume, pitch, attenuation, effects and looping settings (looping applies at start).
    pub settings: AudioSettings3D,
    playing: Option<Playing>,
}
impl AudioSource3D {
    /// Creates an unstarted source. Looping follows `settings.sound.looping`.
    /// ```
    /// use ponko::prelude::*;
    /// # fn f(sound: Sound) {
    /// let source = ponko::d3::AudioSource::new(sound, ponko::d3::AudioSettings::default());
    /// # }
    /// ```
    pub fn new(sound: Sound, settings: AudioSettings3D) -> Self {
        Self {
            sound,
            settings,
            playing: None,
        }
    }

    /// Whether the positional update has started this source, for tests of
    /// when in the frame that update runs.
    #[cfg(test)]
    pub(crate) fn started(&self) -> bool {
        self.playing.is_some()
    }
}

struct Voice {
    id: Playing,
    entity: Option<Entity>,
    position: Vec3,
    settings: AudioSettings3D,
    fresh: bool,
    remaining: f32,
}
#[derive(Default)]
pub(crate) struct SpatialState {
    /// The listener of the last update, for its velocity.
    listener: Option<AudioListener3D>,
    /// The game's own listener, instead of the camera.
    custom: Option<AudioListener3D>,
    /// Where `ctx.scene3d.camera` was when the running system started (and when
    /// the frame's positional sound was updated): the default listener.
    pub(crate) camera: AudioListener3D,
    voices: Vec<Voice>,
    next_silent: u64,
}

impl SpatialState {
    pub(super) fn remove(&mut self, id: Playing) {
        self.voices.retain(|voice| voice.id != id);
    }

    /// Where each positional voice was at the last update, for tests of
    /// when in the frame that update runs.
    #[cfg(test)]
    pub(crate) fn voice_positions(&self) -> Vec<Vec3> {
        self.voices.iter().map(|v| v.position).collect()
    }
}

fn gain(distance: f32, settings: AudioSettings3D) -> f32 {
    let min = settings.min_distance.max(0.001);
    let max = settings.max_distance.max(min + 0.001);
    if distance >= max {
        return 0.0;
    }
    if distance <= min {
        return 1.0;
    }
    let t = (distance - min) / (max - min);
    match settings.rolloff {
        AudioRolloff::Linear => 1.0 - t,
        AudioRolloff::Logarithmic => min / distance,
        AudioRolloff::Keyed(keys) => {
            let Some(&(mut x, mut y)) = keys.first() else {
                return 0.0;
            };
            for &(next_x, next_y) in &keys[1..] {
                if t <= next_x {
                    return (y + (next_y - y) * ((t - x) / (next_x - x).max(f32::EPSILON)).clamp(0.0, 1.0)).max(0.0);
                }
                (x, y) = (next_x, next_y);
            }
            y.max(0.0)
        }
    }
}
fn pan(offset: Vec3, rotation: Quat) -> f32 {
    offset.normalize_or_zero().dot(rotation * Vec3::X).clamp(-1.0, 1.0)
}
fn doppler(direction: Vec3, emitter: Vec3, listener: Vec3, factor: f32) -> f32 {
    let factor = factor.max(0.0);
    let radial = |velocity: Vec3| (velocity.dot(direction) * factor).clamp(-300.0, 300.0);
    ((343.0 + radial(listener)) / (343.0 + radial(emitter))).clamp(0.25, 4.0)
}

impl Audio {
    /// Plays a sound from a point in the world. Volume, panning and doppler
    /// follow the listener every frame. A looping sound (`settings.sound.looping`)
    /// plays until `stop_sound`.
    /// ```
    /// use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, sound: Sound) {
    /// ctx.audio.play_at(sound, vec3(3.0, 0.0, -5.0), ponko::d3::AudioSettings::default());
    /// # }
    /// ```
    pub fn play_at(&mut self, sound: Sound, position: Vec3, settings: AudioSettings3D) -> Playing {
        let listener = self.listener();
        let offset = position - listener.position;
        let mut params = settings.sound;
        params.volume *= gain(offset.length(), settings);
        let id = if self.headless {
            self.backend.spatial.next_silent += 1;
            Playing(self.backend.spatial.next_silent)
        } else {
            self.play_on(sound, params, settings.bus, settings.effects)
        };
        self.backend.spatialize(
            id.0,
            params.volume,
            pan(offset, listener.rotation),
            params.speed.max(0.01),
        );
        self.backend.spatial.voices.push(Voice {
            id,
            entity: None,
            position,
            settings,
            fresh: true,
            remaining: sound.seconds(),
        });
        id
    }
}

impl Audio {
    /// Puts the ears somewhere other than the camera, such as on the player in
    /// a third-person game. `None` goes back to following `ctx.scene3d.camera`.
    /// ```
    /// use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, player: Vec3) {
    /// ctx.audio.set_listener(Some(ponko::d3::AudioListener { position: player, rotation: Quat::IDENTITY }));
    /// # }
    /// ```
    pub fn set_listener(&mut self, listener: Option<AudioListener3D>) {
        self.backend.spatial.custom = listener;
    }

    /// Where the ears are: the listener set with
    /// [`set_listener`](Self::set_listener), or else `ctx.scene3d.camera` as it
    /// was when the running system started. The frame's positional sound
    /// update, after the last system, uses the camera's final place.
    /// ```
    /// use ponko::prelude::*;
    /// # fn f(ctx: &Ctx) {
    /// let ears = ctx.audio.listener().position;
    /// # }
    /// ```
    pub fn listener(&self) -> AudioListener3D {
        self.backend.spatial.custom.unwrap_or(self.backend.spatial.camera)
    }
}

/// Moves every positional sound to this frame's listener and emitters. The
/// game runs it after the frame's systems; no allocations unless a voice starts.
pub(crate) fn update_audio3d(ctx: &mut Ctx) {
    ctx.sync_audio_camera();
    let listener = ctx.audio.listener();
    let dt = ctx.time.dt();
    let inverse_dt = if dt > 0.0 { dt.recip() } else { 0.0 };
    let velocity = ctx
        .audio
        .backend
        .spatial
        .listener
        .map_or(Vec3::ZERO, |old| (listener.position - old.position) * inverse_dt);
    ctx.audio.backend.spatial.listener = Some(listener);
    // Split fields rather than gathering the query into an allocated buffer.
    for (entity, transform, source) in ctx.world.query::<(Entity, &Transform3D, &mut AudioSource3D)>().iter() {
        if source.playing.is_none() {
            let offset = transform.position - listener.position;
            let mut params = source.settings.sound;
            params.volume *= gain(offset.length(), source.settings);
            let id = Playing(if ctx.headless {
                ctx.audio.backend.spatial.next_silent += 1;
                ctx.audio.backend.spatial.next_silent
            } else {
                ctx.audio
                    .backend
                    .play_on(source.sound.id, params, source.settings.bus, source.settings.effects)
            });
            source.playing = Some(id);
            ctx.audio.backend.spatial.voices.push(Voice {
                id,
                entity: Some(entity),
                position: transform.position,
                settings: source.settings,
                fresh: true,
                remaining: source.sound.seconds(),
            });
        }
    }
    let mut i = 0;
    while i < ctx.audio.backend.spatial.voices.len() {
        let voice = &mut ctx.audio.backend.spatial.voices[i];
        let mut position = voice.position;
        let mut changed_effects = None;
        if let Some(entity) = voice.entity {
            match (
                ctx.world.get::<Transform3D>(entity),
                ctx.world.get::<AudioSource3D>(entity),
            ) {
                (Some(transform), Some(source)) if source.playing == Some(voice.id) => {
                    position = transform.position;
                    let looping = voice.settings.sound.looping;
                    if source.settings.effects != voice.settings.effects {
                        changed_effects = Some(source.settings.effects);
                    }
                    voice.settings = source.settings;
                    voice.settings.sound.looping = looping;
                }
                _ => {
                    let id = voice.id;
                    ctx.audio.backend.stop(id.0);
                    ctx.audio.backend.spatial.voices.swap_remove(i);
                    continue;
                }
            }
        }
        let emitter_velocity = if voice.fresh {
            Vec3::ZERO
        } else {
            (position - voice.position) * inverse_dt
        };
        let offset = position - listener.position;
        let settings = voice.settings;
        let speed = settings.sound.speed.max(0.01)
            * doppler(offset.normalize_or_zero(), emitter_velocity, velocity, settings.doppler);
        voice.remaining -= dt.max(0.0) * speed;
        voice.position = position;
        voice.fresh = false;
        let (id, volume, panning) = (
            voice.id,
            settings.sound.volume * gain(offset.length(), settings),
            pan(offset, listener.rotation),
        );
        let finished = !settings.sound.looping && voice.remaining <= 0.0;
        if let Some(effects) = changed_effects {
            // Kira smooths the change, so a filter can follow the game (a
            // door closing, the camera moving inside).
            ctx.audio.backend.effects(id.0, effects);
        }
        ctx.audio.backend.spatialize(id.0, volume, panning, speed);
        if finished {
            ctx.audio.backend.spatial.voices.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn audio_distance_curves_and_boundaries() {
        let mut settings = AudioSettings3D {
            min_distance: 1.0,
            max_distance: 11.0,
            rolloff: AudioRolloff::Linear,
            ..Default::default()
        };
        assert_eq!(gain(0.0, settings), 1.0);
        assert_eq!(gain(6.0, settings), 0.5);
        assert_eq!(gain(11.0, settings), 0.0);
        settings.rolloff = AudioRolloff::Logarithmic;
        assert_eq!(gain(2.0, settings), 0.5);
        settings.rolloff = AudioRolloff::Keyed(&[(0.0, 1.0), (1.0, 0.2)]);
        assert!((gain(6.0, settings) - 0.6).abs() < 1e-6);
    }
    #[test]
    fn audio_pan_and_doppler_follow_listener_orientation_and_velocity() {
        assert_eq!(pan(Vec3::X, Quat::IDENTITY), 1.0);
        assert_eq!(pan(-Vec3::X, Quat::IDENTITY), -1.0);
        assert_eq!(pan(Vec3::ZERO, Quat::IDENTITY), 0.0);
        assert!(doppler(Vec3::X, -Vec3::X * 20.0, Vec3::ZERO, 1.0) > 1.0);
        assert!(doppler(Vec3::X, Vec3::X * 20.0, Vec3::ZERO, 1.0) < 1.0);
        assert_eq!(doppler(Vec3::X, Vec3::X, Vec3::ZERO, 0.0), 1.0);
    }
    #[test]
    fn audio_headless_sources_start_update_and_cleanup_without_device() {
        crate::game::Game::new("audio test", 1, 1)
            .system(|ctx| {
                update_audio3d(ctx);
                assert!(ctx.world.query::<&AudioSource3D>().iter().all(|s| s.playing.is_some()));
            })
            .run_headless(2, |ctx| {
                let sound = ctx.audio.create(100, &[0.0; 100]);
                let entity = ctx.world.spawn((
                    Transform3D::default(),
                    AudioSource3D::new(sound, AudioSettings3D::default()),
                ));
                update_audio3d(ctx);
                assert_eq!(ctx.audio.backend.spatial.voices.len(), 1);
                ctx.world.get_mut::<Transform3D>(entity).unwrap().position = Vec3::X * 5.0;
                update_audio3d(ctx);
                assert_eq!(ctx.audio.backend.spatial.voices[0].position, Vec3::X * 5.0);
                ctx.world.despawn(entity);
                update_audio3d(ctx);
                assert!(ctx.audio.backend.spatial.voices.is_empty());
            });
    }
    #[test]
    fn a_scene_load_stops_voices_of_replaced_entities_only() {
        use crate::reflect::WorldReflect;
        crate::game::Game::new("audio test", 1, 1).run_headless(0, |ctx| {
            ctx.world.reflect::<Transform3D>();
            let sound = ctx.audio.create(100, &[0.0; 100]);
            let looping = AudioSettings3D {
                sound: crate::audio::SoundParams {
                    looping: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            // The scene has one entity at index 0, without a sound source.
            let e = ctx.world.spawn((Transform3D::default(),));
            let save = ctx.world.save();
            ctx.world.insert(e, AudioSource3D::new(sound, looping));
            update_audio3d(ctx);
            assert_eq!(ctx.audio.backend.spatial.voices.len(), 1);
            ctx.world.load(&save).unwrap();
            assert!(ctx.world.is_alive(e), "same id, new entity");
            update_audio3d(ctx);
            assert!(
                ctx.audio.backend.spatial.voices.is_empty(),
                "the old entity's voice stops"
            );
        });
    }
}
