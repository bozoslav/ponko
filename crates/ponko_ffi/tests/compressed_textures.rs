//! Compressed textures from C: a KTX2 file loads through the ordinary
//! texture loader with the `compressed-textures` feature, and fails with
//! an unsupported error naming it without.
mod common;
use common::*;
use ponko_ffi::*;

use PonkoStatus::*;

const CARD_BC7: &[u8] = include_bytes!("../../../tests/fixtures/textures/card.ktx2");

#[test]
fn ktx2_files_load_through_the_texture_loader() {
    run_ctx(|ctx| unsafe {
        let source = ponko_memory_source_new(c"textures".as_ptr());
        assert_eq!(
            ponko_memory_source_insert(source, c"card.ktx2".as_ptr(), CARD_BC7.as_ptr(), CARD_BC7.len()),
            PONKO_OK
        );
        assert_eq!(ponko_ctx_add_memory_source(ctx, source), PONKO_OK);
        ponko_memory_source_free(source);
        let mut card = PonkoTexture {
            id: 0,
            width: 0,
            height: 0,
        };
        let status = ponko_ctx_try_load_texture(ctx, c"card.ktx2".as_ptr(), &mut card);
        if cfg!(feature = "compressed-textures") {
            assert_eq!(status, PONKO_OK, "{}", last_error());
            assert_eq!((card.width, card.height), (64, 64));
        } else {
            assert_ne!(status, PONKO_OK);
            assert_eq!(ponko_last_error_kind(), PonkoErrorKind::PONKO_ERROR_UNSUPPORTED);
            assert!(last_error().contains("compressed-textures"), "{}", last_error());
        }
    });
}
