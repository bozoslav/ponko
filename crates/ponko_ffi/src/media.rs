//! Shapes, UI widgets, fonts, sound, screenshots and turns: the rest of `Ctx`.
use std::ffi::c_char;

use ponko::prelude::{Font, Playing, Sound, SoundParams, TextParams};
use ponko::ui::UiStyle;

use crate::ctx::{ctx_access, out, text};
use crate::types::*;
use crate::{PonkoCtx, PonkoStatus, boundary, invalid};

/// A font from `ponko_ctx_load_font`; `{0}` is the built-in font.
/// ```
/// assert_eq!(ponko_ffi::PonkoFont::default().id, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoFont {
    /// Engine-internal id; 0 is the built-in font.
    pub id: u32,
}

/// Options for `ponko_ctx_draw_text_ex`. Start from `ponko_text_params_default()`.
/// ```
/// assert_eq!(ponko_ffi::ponko_text_params_default().size, 16.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTextParams {
    /// Which font. Default the built-in one.
    pub font: PonkoFont,
    /// Height of a line in window pixels. Default 16.
    pub size: f32,
    /// Default white.
    pub color: PonkoColor,
    /// Draw order among screen-space drawing; higher is on top. Default 0.
    pub z: f32,
}

/// The default text options: built-in font, 16 px, white.
/// ```
/// assert_eq!(ponko_ffi::ponko_text_params_default().font.id, 0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_text_params_default() -> PonkoTextParams {
    let p = TextParams::default();
    PonkoTextParams {
        font: font_to_c(p.font),
        size: p.size,
        color: to_c(p.color),
        z: p.z,
    }
}

/// Colors and sizes of the UI widgets (`ponko_ctx_ui_*`).
/// ```
/// assert_eq!(ponko_ffi::ponko_ui_style_default().text_size, 16.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoUiStyle {
    /// Panel background.
    pub panel: PonkoColor,
    /// Button background.
    pub button: PonkoColor,
    /// Button under the mouse.
    pub button_hovered: PonkoColor,
    /// Button held down.
    pub button_pressed: PonkoColor,
    /// Highlights, such as a hovered button's outline.
    pub accent: PonkoColor,
    /// Label and button text.
    pub text: PonkoColor,
    /// Text height in pixels.
    pub text_size: f32,
    /// Space between a panel's edge and its contents, in pixels.
    pub padding: f32,
}

fn style_to_c(s: UiStyle) -> PonkoUiStyle {
    PonkoUiStyle {
        panel: to_c(s.panel),
        button: to_c(s.button),
        button_hovered: to_c(s.button_hovered),
        button_pressed: to_c(s.button_pressed),
        accent: to_c(s.accent),
        text: to_c(s.text),
        text_size: s.text_size,
        padding: s.padding,
    }
}

/// The engine's default UI style.
/// ```
/// assert_eq!(ponko_ffi::ponko_ui_style_default().padding, 10.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_ui_style_default() -> PonkoUiStyle {
    style_to_c(UiStyle::default())
}

/// A sound from `ponko_ctx_load_sound` or `ponko_ctx_create_sound`.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoSound>(), 8);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSound {
    /// Engine-internal id. Never make one up.
    pub id: u32,
    /// Length in seconds.
    pub seconds: f32,
}

/// One playback of a sound, for `ponko_ctx_stop_sound`.
/// ```
/// assert_eq!(std::mem::size_of::<ponko_ffi::PonkoPlaying>(), 8);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoPlaying {
    /// Engine-internal id; 0 in headless runs.
    pub id: u64,
}

/// Options for `ponko_ctx_play_sound`. Start from `ponko_sound_params_default()`.
/// ```
/// assert_eq!(ponko_ffi::ponko_sound_params_default().volume, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoSoundParams {
    /// 0 is silent, 1 (default) is as recorded.
    pub volume: f32,
    /// Playback speed, which also changes the pitch. Default 1.
    pub speed: f32,
    /// Start over at the end. Default false.
    pub looping: bool,
}

/// Default playback: full volume, normal speed, once.
/// ```
/// assert!(!ponko_ffi::ponko_sound_params_default().looping);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_sound_params_default() -> PonkoSoundParams {
    let p = SoundParams::default();
    PonkoSoundParams {
        volume: p.volume,
        speed: p.speed,
        looping: p.looping,
    }
}

/// Which entities take their turn this tick, from `ponko_ctx_turns`:
/// entity `e` does when `e.index % every == now`. `dt` covers a whole round
/// (`every` ticks).
/// ```
/// let t = ponko_ffi::PonkoTurns { every: 4, now: 1, dt: 0.0 };
/// assert_eq!(5 % t.every, t.now);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoTurns {
    /// Ticks in a round.
    pub every: u32,
    /// This tick's place in the round.
    pub now: u32,
    /// Seconds a round covers.
    pub dt: f32,
}

// Font, Sound and Playing are repr(C)/repr(transparent) over these fields.
const _: () = {
    assert!(size_of::<PonkoFont>() == size_of::<Font>() && align_of::<PonkoFont>() == align_of::<Font>());
    assert!(size_of::<PonkoSound>() == size_of::<Sound>() && align_of::<PonkoSound>() == align_of::<Sound>());
    assert!(size_of::<PonkoPlaying>() == size_of::<Playing>() && align_of::<PonkoPlaying>() == align_of::<Playing>());
};
pub(crate) fn font_to_c(f: Font) -> PonkoFont {
    // SAFETY: Font is repr(transparent) over u32 (size checked above).
    unsafe { std::mem::transmute(f) }
}
pub(crate) fn font_from_c(f: PonkoFont) -> Font {
    // SAFETY: as above. Unknown ids make the call fail, they are not UB.
    unsafe { std::mem::transmute(f) }
}
pub(crate) fn sound_to_c(s: Sound) -> PonkoSound {
    // SAFETY: Sound is repr(C) { u32 id, f32 seconds }.
    unsafe { std::mem::transmute(s) }
}
pub(crate) fn sound_from_c(s: PonkoSound) -> Sound {
    // SAFETY: as above. Unknown ids make the call fail, they are not UB.
    unsafe { std::mem::transmute(s) }
}
pub(crate) fn playing_to_c(p: Playing) -> PonkoPlaying {
    // SAFETY: Playing is repr(transparent) over u64.
    unsafe { std::mem::transmute(p) }
}
pub(crate) fn playing_from_c(p: PonkoPlaying) -> Playing {
    // SAFETY: as above; stopping an unknown id does nothing.
    unsafe { std::mem::transmute(p) }
}

// ---- shapes ---------------------------------------------------------------------

/// Fills a rectangle in world space.
/// ```
/// use ponko_ffi::*;
/// let r = PonkoRect::default();
/// let white = PonkoColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ctx_draw_rect(std::ptr::null_mut(), r, white) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_rect(ctx: *mut PonkoCtx, rect: PonkoRect, color: PonkoColor) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.draw_rect(to_c(rect), to_c(color));
            Ok(())
        })
    })
}

/// Draws a line `width` world units thick from `a` to `b`.
/// ```
/// use ponko_ffi::*;
/// let v = PonkoVec2::default();
/// let white = PonkoColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ctx_draw_line(std::ptr::null_mut(), v, v, 1.0, white) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_line(
    ctx: *mut PonkoCtx,
    a: PonkoVec2,
    b: PonkoVec2,
    width: f32,
    color: PonkoColor,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.draw_line(to_c(a), to_c(b), width, to_c(color));
            Ok(())
        })
    })
}

/// Fills a circle in world space.
/// ```
/// use ponko_ffi::*;
/// let white = PonkoColor { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
/// assert_eq!(unsafe { ponko_ctx_draw_circle(std::ptr::null_mut(), PonkoVec2::default(), 4.0, white) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_circle(
    ctx: *mut PonkoCtx,
    center: PonkoVec2,
    radius: f32,
    color: PonkoColor,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.draw_circle(to_c(center), radius, to_c(color));
            Ok(())
        })
    })
}

// ---- UI ---------------------------------------------------------------------------

/// Reads the UI style.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_style(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_style` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_style(ctx: *mut PonkoCtx, out_style: *mut PonkoUiStyle) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_style, style_to_c(c.ui.immediate.style))) })
}

/// Changes the UI style for widgets drawn after this call.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_ui_style(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `style` must be readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_ui_style(ctx: *mut PonkoCtx, style: *const PonkoUiStyle) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if style.is_null() {
                return Err(invalid("style is null"));
            }
            let s = *style;
            c.ui.immediate.style = UiStyle {
                panel: to_c(s.panel),
                button: to_c(s.button),
                button_hovered: to_c(s.button_hovered),
                button_pressed: to_c(s.button_pressed),
                accent: to_c(s.accent),
                text: to_c(s.text),
                text_size: s.text_size,
                padding: s.padding,
            };
            Ok(())
        })
    })
}

/// Whether the mouse is over UI drawn this frame or the last, so clicks on
/// a panel don't also act on the world behind it.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mouse_over_ui(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `out_over` must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mouse_over_ui(ctx: *mut PonkoCtx, out_over: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_over, c.ui.wants_pointer())) })
}

/// Draws a panel background in screen space (window pixels).
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_panel(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_panel(ctx: *mut PonkoCtx, rect: PonkoRect) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.immediate_ui().panel(to_c(rect));
            Ok(())
        })
    })
}

/// Draws text in the UI style at `pos` (screen space); writes its size to
/// `out_size` unless that is null.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_label(std::ptr::null_mut(), Default::default(), c"hi".as_ptr(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `string` NUL-terminated; `out_size` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_label(
    ctx: *mut PonkoCtx,
    pos: PonkoVec2,
    string: *const c_char,
    out_size: *mut PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let size = c.immediate_ui().label(to_c(pos), text(string)?);
            if !out_size.is_null() {
                out_size.write(to_c(size));
            }
            Ok(())
        })
    })
}

/// Draws a button in screen space; `out_clicked` is true on the frame it was clicked.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_ui_button(std::ptr::null_mut(), Default::default(), c"ok".as_ptr(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `label` NUL-terminated; `out_clicked` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_ui_button(
    ctx: *mut PonkoCtx,
    rect: PonkoRect,
    label: *const c_char,
    out_clicked: *mut bool,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let label = text(label)?;
            if out_clicked.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_clicked, c.immediate_ui().button(to_c(rect), label))
        })
    })
}

// ---- fonts --------------------------------------------------------------------------

/// Loads a TTF or OTF font from the assets folder.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_font(std::ptr::null_mut(), c"a.ttf".as_ptr(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `path` NUL-terminated; `out_font` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_font(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_font: *mut PonkoFont,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_font.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_font, font_to_c(c.gfx.load_font(path)))
        })
    })
}

/// Draws text in screen space with a font, size, color and z. An unknown or
/// freed font is an invalid argument.
/// ```
/// let p = ponko_ffi::ponko_text_params_default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_draw_text_ex(std::ptr::null_mut(), c"hi".as_ptr(), Default::default(), &p) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `string` NUL-terminated; `params` readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_draw_text_ex(
    ctx: *mut PonkoCtx,
    string: *const c_char,
    pos: PonkoVec2,
    params: *const PonkoTextParams,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let string = text(string)?;
            if params.is_null() {
                return Err(invalid("params is null"));
            }
            let p = *params;
            if !c.gfx.is_live(font_from_c(p.font)) {
                return Err(invalid("unknown or freed font"));
            }
            c.gfx.draw_text_ex(
                string,
                to_c(pos),
                TextParams::default()
                    .with_font(font_from_c(p.font))
                    .with_size(p.size)
                    .with_color(to_c(p.color))
                    .with_z(p.z),
            );
            Ok(())
        })
    })
}

/// Size in pixels that `ponko_ctx_draw_text_ex` would cover with `font` and
/// `size`. An unknown or freed font is an invalid argument.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_measure_text_ex(std::ptr::null_mut(), c"hi".as_ptr(), Default::default(), 16.0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `string` NUL-terminated; `out_size` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_measure_text_ex(
    ctx: *mut PonkoCtx,
    string: *const c_char,
    font: PonkoFont,
    size: f32,
    out_size: *mut PonkoVec2,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let string = text(string)?;
            if out_size.is_null() {
                return Err(invalid("output is null"));
            }
            if !c.gfx.is_live(font_from_c(font)) {
                return Err(invalid("unknown or freed font"));
            }
            out(out_size, to_c(c.gfx.measure_text_ex(string, font_from_c(font), size)))
        })
    })
}

// ---- sound --------------------------------------------------------------------------

/// Loads a sound (WAV, OGG, MP3 or FLAC) from the assets folder.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_sound(std::ptr::null_mut(), c"a.ogg".as_ptr(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `path` NUL-terminated; `out_sound` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_sound(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_sound: *mut PonkoSound,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_sound.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_sound, sound_to_c(c.audio.load(path)))
        })
    })
}

/// Makes a mono sound from `len` samples in -1..1 at `sample_rate` per second.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_create_sound(std::ptr::null_mut(), 44100, std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `samples` must hold `len` floats; `out_sound` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_create_sound(
    ctx: *mut PonkoCtx,
    sample_rate: u32,
    samples: *const f32,
    len: usize,
    out_sound: *mut PonkoSound,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if samples.is_null() && len > 0 {
                return Err(invalid("samples is null"));
            }
            if sample_rate == 0 {
                return Err(invalid("the sample rate must not be zero"));
            }
            if out_sound.is_null() {
                return Err(invalid("output is null"));
            }
            let samples = if len == 0 {
                &[][..]
            } else {
                std::slice::from_raw_parts(samples, len)
            };
            out(out_sound, sound_to_c(c.audio.create(sample_rate, samples)))
        })
    })
}

/// Plays a sound; `params` may be null for the defaults. Writes the playback
/// to `out_playing` unless that is null. Silent in headless runs.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_play_sound(std::ptr::null_mut(), s, std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `params` null or readable; `out_playing` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_play_sound(
    ctx: *mut PonkoCtx,
    sound: PonkoSound,
    params: *const PonkoSoundParams,
    out_playing: *mut PonkoPlaying,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let p = if params.is_null() {
                ponko_sound_params_default()
            } else {
                *params
            };
            let playing = c.audio.play_ex(
                sound_from_c(sound),
                SoundParams {
                    volume: p.volume,
                    speed: p.speed,
                    looping: p.looping,
                },
            );
            if !out_playing.is_null() {
                out_playing.write(playing_to_c(playing));
            }
            Ok(())
        })
    })
}

/// Stops a playback. Stopping one that already ended does nothing.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_stop_sound(std::ptr::null_mut(), Default::default()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_stop_sound(ctx: *mut PonkoCtx, playing: PonkoPlaying) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.audio.stop(playing_from_c(playing));
            Ok(())
        })
    })
}

// ---- screenshots and turns ---------------------------------------------------------------

/// Saves this frame as a PNG once it is drawn. Does nothing in headless runs.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_save_screenshot(std::ptr::null_mut(), c"a.png".as_ptr()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; `path` NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_save_screenshot(ctx: *mut PonkoCtx, path: *const c_char) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.gfx.save_screenshot(text(path)?);
            Ok(())
        })
    })
}

/// Textures whose files changed and were reloaded at the start of this frame
/// (hot reload, see `ponko_game_hot_reload`): `*out_textures` points at
/// `*out_count` of them, valid until the system returns. They keep their
/// handles; use this to redo anything computed from an image.
/// ```
/// let (mut p, mut n) = (std::ptr::null(), 0);
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_reloaded_textures(std::ptr::null(), &mut p, &mut n) } as u32, 1);
/// ```
/// # Safety
/// `ctx` must be null or live; outputs writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_reloaded_textures(
    ctx: *const PonkoCtx,
    out_textures: *mut *const PonkoTexture,
    out_count: *mut usize,
) -> PonkoStatus {
    boundary(|| {
        if out_textures.is_null() || out_count.is_null() {
            return Err(invalid("output is null"));
        }
        // SAFETY: forwarded caller contract; outputs checked non-null.
        unsafe {
            ctx_access(ctx, |c| {
                let list = c.assets.reloaded_textures();
                // Texture and PonkoTexture have the same layout (types.rs).
                out_textures.write(list.as_ptr().cast());
                out_count.write(list.len());
                Ok(())
            })
        }
    })
}

/// Spreads work over `every` ticks: see `PonkoTurns`. `every` must be at least 1.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_turns(std::ptr::null(), 4, std::ptr::null_mut()) } as u32, 1);
/// ```
/// Reads only clock values, so it is safe from any thread during the callback.
/// # Safety
/// `ctx` must be null or live; `out_turns` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_turns(ctx: *const PonkoCtx, every: u32, out_turns: *mut PonkoTurns) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        if ctx.is_null() {
            return Err(invalid("ctx is null"));
        }
        if every == 0 {
            return Err(invalid("`every` must be at least 1"));
        }
        let (tick, dt) = (crate::ponko_ctx_sim_tick(ctx), crate::ponko_ctx_dt(ctx));
        out(
            out_turns,
            PonkoTurns {
                every,
                now: (tick % u64::from(every)) as u32,
                dt: dt * every as f32,
            },
        )
    })
}
