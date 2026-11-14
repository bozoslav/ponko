//! Motion vectors from C: dropping one model's motion (`Graphics::reset_motion`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;
use std::ptr::null;

#[test]
fn one_entitys_motion_is_reset_and_a_stale_entity_is_refused() {
    run_ctx(|ctx| unsafe {
        let w = ponko_ctx_world(ctx);
        let mut e = 0;
        assert_eq!(ponko_spawn(w, null(), 0, &mut e), PONKO_OK);
        assert_eq!(ponko_ctx_reset_motion(ctx, e), PONKO_OK, "{}", last_error());
        assert_eq!(ponko_despawn(w, e), PONKO_OK);
        assert_eq!(ponko_ctx_reset_motion(ctx, e), PONKO_INVALID_ARGUMENT);
        assert!(last_error().contains("no such entity"), "{}", last_error());
    });
}
