//! Owned world-space SDF text from C. Text is copied, never a query column.
use crate::ctx::{ctx_access, out, text};
use crate::media::{font_from_c, font_to_c};
use crate::types::to_c;
use crate::{PonkoColor, PonkoCtx, PonkoFont, PonkoStatus, boundary, error, invalid};
use ponko::d3::Text as Text3D;
use ponko::d3::TextAlign as TextAlign3D;
use ponko::prelude::Entity;
use std::ffi::c_char;

/// How lines line up inside a world text block.
/// ```
/// assert_eq!(ponko_ffi::PonkoTextAlign3D::PONKO_TEXT_ALIGN3D_CENTER as u32, 1);
/// ```
#[repr(u32)]
#[derive(Clone, Copy, Debug)]
#[allow(non_camel_case_types)]
pub enum PonkoTextAlign3D {
    /// Left edge.
    PONKO_TEXT_ALIGN3D_LEFT = 0,
    /// Center (default).
    PONKO_TEXT_ALIGN3D_CENTER = 1,
    /// Right edge.
    PONKO_TEXT_ALIGN3D_RIGHT = 2,
}

/// World text settings. Start with `ponko_text3d_default`. Size is meters
/// per line; outline width and softness are fractions of a line, clamped
/// by the renderer to about 0.12. Text needs a d3::Transform to be drawn.
/// ```
/// let settings: ponko_ffi::PonkoText3D = ponko_ffi::ponko_text3d_default();
/// assert!(!settings.billboard);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PonkoText3D {
    /// Loaded font, or id 0 for the built-in font.
    pub font: PonkoFont,
    /// Line height in meters (default 0.25).
    pub size: f32,
    /// Fill color.
    pub color: PonkoColor,
    /// Outline color (transparent by default).
    pub outline: PonkoColor,
    /// Outline reach in lines.
    pub outline_width: f32,
    /// Edge softness in lines.
    pub softness: f32,
    /// PonkoTextAlign3D as uint32_t.
    pub align: u32,
    /// Face the camera instead of following the transform's rotation.
    pub billboard: bool,
}
fn settings(t: &Text3D) -> PonkoText3D {
    PonkoText3D {
        font: font_to_c(t.font),
        size: t.size,
        color: to_c(t.color),
        outline: to_c(t.outline),
        outline_width: t.outline_width,
        softness: t.softness,
        align: t.align as u32,
        billboard: t.billboard,
    }
}
/// Default settings: white, centered, quarter-meter lines, no outline.
/// ```
/// assert_eq!(ponko_ffi::ponko_text3d_default().size, 0.25);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_text3d_default() -> PonkoText3D {
    settings(&Text3D::new(""))
}

/// Adds or replaces world text. Copies UTF-8 text; newlines start lines.
/// Rejects unknown fonts, invalid alignment and non-finite/negative sizes
/// and reaches. Failed validation leaves the component unchanged.
/// # Safety
/// `ctx` null or live; `value` null or readable; `string` null or a C string.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_text3d(std::ptr::null_mut(), 0, c"Hi".as_ptr(), &ponko_ffi::ponko_text3d_default()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_text3d(
    ctx: *mut PonkoCtx,
    entity: u64,
    string: *const c_char,
    value: *const PonkoText3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = Entity::from_bits(entity);
            if !c.world.is_alive(e) {
                return Err(invalid("no such entity"));
            }
            let v = value.as_ref().ok_or_else(|| invalid("settings is null"))?;
            let string = text(string)?;
            let font = font_from_c(v.font);
            if !c.gfx.is_live(font) {
                return Err(invalid("unknown font"));
            }
            let align = match v.align {
                0 => TextAlign3D::Left,
                1 => TextAlign3D::Center,
                2 => TextAlign3D::Right,
                _ => return Err(invalid("alignment must be 0 to 2")),
            };
            if [v.size, v.outline_width, v.softness]
                .iter()
                .any(|x| !x.is_finite() || *x < 0.0)
            {
                return Err(invalid("size and reaches must be finite and nonnegative"));
            }
            c.world.insert(
                e,
                Text3D {
                    text: string.to_owned(),
                    font,
                    size: v.size,
                    color: to_c(v.color),
                    outline: to_c(v.outline),
                    outline_width: v.outline_width,
                    softness: v.softness,
                    align,
                    billboard: v.billboard,
                },
            );
            Ok(())
        })
    })
}

/// Reads settings. Returns NOT_FOUND if a live entity has no world text.
/// # Safety
/// `ctx` null or live; output null or writable.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_text3d(std::ptr::null_mut(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_text3d(ctx: *mut PonkoCtx, entity: u64, output: *mut PonkoText3D) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = Entity::from_bits(entity);
            if !c.world.is_alive(e) {
                return Err(invalid("no such entity"));
            }
            let t = c
                .world
                .get::<Text3D>(e)
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity has no world text"))?;
            out(output, settings(t))
        })
    })
}

/// Copies text into a NUL-terminated buffer; length excludes NUL. Query
/// length with null buffer and capacity 0. If capacity <= length, writes
/// an empty string (when capacity > 0); allocate length + 1 and retry.
/// Returns NOT_FOUND for a live entity without world text.
/// # Safety
/// `ctx` null or live; buffer writable for capacity bytes (null only with
/// capacity 0); length null or writable. Outputs must not overlap.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_text3d_text(std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, &mut 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_text3d_text(
    ctx: *mut PonkoCtx,
    entity: u64,
    buffer: *mut c_char,
    capacity: usize,
    length: *mut usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract; outputs validated before writes.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            if length.is_null() || (buffer.is_null() && capacity != 0) {
                return Err(invalid("output is null"));
            }
            let e = Entity::from_bits(entity);
            if !c.world.is_alive(e) {
                return Err(invalid("no such entity"));
            }
            let t = c
                .world
                .get::<Text3D>(e)
                .ok_or_else(|| error(PonkoStatus::PONKO_NOT_FOUND, "entity has no world text"))?;
            length.write(t.text.len());
            if capacity > t.text.len() {
                std::ptr::copy_nonoverlapping(t.text.as_ptr(), buffer.cast(), t.text.len());
                buffer.add(t.text.len()).write(0);
            } else if capacity > 0 {
                buffer.write(0);
            }
            Ok(())
        })
    })
}

/// Removes world text. A live entity without text is unchanged.
/// # Safety
/// `ctx` null or live.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_text3d(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_text3d(ctx: *mut PonkoCtx, entity: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = Entity::from_bits(entity);
            if !c.world.is_alive(e) {
                return Err(invalid("no such entity"));
            }
            c.world.remove::<Text3D>(e);
            Ok(())
        })
    })
}
