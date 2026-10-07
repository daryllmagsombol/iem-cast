//! Desktop composition tests.
//!
//! These assert the shell's privilege boundary and embedded-asset behavior. They do not open
//! audio hardware and do not prove the packaged app captures audio.

use iem_cast_desktop::asset_embed::EmbeddedMusicianAssets;
use iem_cast_desktop::window_guard::authorize_window;
use host_core::contract::AssetProvider;

#[test]
fn asset_provider_trait_allows_host_core_tests_without_frontend_dist() {
    let provider = EmbeddedMusicianAssets::new();
    // The provider is constructed from compiled-in assets; host-core tests never need a
    // `frontendDist` directory to exist.
    let _: &dyn AssetProvider = &provider;
}

#[test]
fn embedded_assets_serve_the_join_page() {
    let provider = EmbeddedMusicianAssets::new();
    assert!(provider.get("/join").is_some());
}

#[test]
fn ipc_command_rejects_non_operator_window_label() {
    assert!(authorize_window("operator").is_ok());
    assert!(authorize_window("musician").is_err());
    assert!(authorize_window("other").is_err());
}
