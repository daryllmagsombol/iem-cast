//! Embedded musician assets.
//!
//! The operator shell embeds **only** the musician bundle. The admin bundle is served to the
//! local operator webview by Tauri's `frontendDist`; it must never be reachable from the LAN.
//! A missing musician bundle fails the build rather than shipping an empty placeholder.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use host_core::contract::AssetProvider;

/// Assets compiled into the shell from `apps/web/dist/musician`.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../web/dist/musician"]
struct MusicianAssets;

/// Caches resolved assets so each path is materialized to `'static` storage at most once.
fn cache() -> &'static Mutex<HashMap<String, &'static [u8]>> {
    static CACHE: OnceLock<Mutex<HashMap<String, &'static [u8]>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// An [`AssetProvider`] over the embedded musician bundle.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmbeddedMusicianAssets;

impl EmbeddedMusicianAssets {
    /// Create the provider.
    pub fn new() -> Self {
        Self
    }

    /// Wrap in an `Arc<dyn AssetProvider>` for host-core composition.
    pub fn into_provider(self) -> Arc<dyn AssetProvider> {
        Arc::new(self)
    }
}

impl AssetProvider for EmbeddedMusicianAssets {
    fn get(&self, path: &str) -> Option<&'static [u8]> {
        let key = match path {
            "/join" | "/" | "" => "index.html",
            other => other.trim_start_matches('/'),
        };
        if let Ok(guard) = cache().lock() {
            if let Some(bytes) = guard.get(key) {
                return Some(*bytes);
            }
        }
        // Materialize once, then hand out the cached static slice. `Box::leak` is intentional
        // and bounded by the finite set of embedded asset paths.
        let file = MusicianAssets::get(key)?;
        let bytes: &'static [u8] = Box::leak(file.data.into_owned().into_boxed_slice());
        if let Ok(mut guard) = cache().lock() {
            guard.insert(key.to_string(), bytes);
        }
        Some(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_assets_serve_the_join_page() {
        let provider = EmbeddedMusicianAssets::new();
        let first = provider.get("/join");
        let second = provider.get("/join");
        assert!(first.is_some(), "musician index must be embedded");
        assert_eq!(first, second, "resolved asset is stable across calls");
    }
}
