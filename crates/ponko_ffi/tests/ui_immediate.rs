//! The immediate widgets' font, border width and scale from C
//! (`UiImmediate::font`, `UiImmediate::border_width`, `UiImmediate::scale`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;

#[test]
fn immediate_font_border_and_scale_are_read_and_set() {
    run_ctx(|ctx| unsafe {
        let mut font = PonkoFont { id: 7 };
        assert_eq!(ponko_ctx_ui_immediate_font(ctx, &mut font), PONKO_OK);
        assert_eq!(font, PonkoFont::default(), "the built-in font at first");
        assert_eq!(
            ponko_ctx_set_ui_immediate_font(ctx, PonkoFont::default()),
            PONKO_OK,
            "{}",
            last_error()
        );
        assert_eq!(
            ponko_ctx_set_ui_immediate_font(ctx, PonkoFont { id: 999 }),
            PONKO_INVALID_ARGUMENT
        );

        let mut width = -1.0;
        assert_eq!(ponko_ctx_ui_immediate_border_width(ctx, &mut width), PONKO_OK);
        assert_eq!(width, 0.0);
        assert_eq!(ponko_ctx_set_ui_immediate_border_width(ctx, 2.0), PONKO_OK);
        assert_eq!(ponko_ctx_ui_immediate_border_width(ctx, &mut width), PONKO_OK);
        assert_eq!(width, 2.0);
        for bad in [-1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                ponko_ctx_set_ui_immediate_border_width(ctx, bad),
                PONKO_INVALID_ARGUMENT
            );
        }
        assert_eq!(ponko_ctx_ui_immediate_border_width(ctx, &mut width), PONKO_OK);
        assert_eq!(width, 2.0);

        // The scale follows the screen's UI scale.
        let mut scale = 0.0;
        assert_eq!(ponko_ctx_ui_immediate_scale(ctx, &mut scale), PONKO_OK);
        assert_eq!(scale, 1.0);
        assert_eq!(ponko_ctx_ui_set_scale(ctx, PONKO_UI_SCREEN, 2.0), PONKO_OK);
        assert_eq!(ponko_ctx_ui_immediate_scale(ctx, &mut scale), PONKO_OK);
        assert_eq!(scale, 2.0);
        assert_eq!(
            ponko_ctx_ui_immediate_scale(ctx, std::ptr::null_mut()),
            PONKO_INVALID_ARGUMENT
        );
    });
}
