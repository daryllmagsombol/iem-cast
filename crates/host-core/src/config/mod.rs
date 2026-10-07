//! Versioned persistence boundary (Lane B).
//!
//! Minimal in-memory configuration store for the POC: labels, mapping, and personal settings only,
//! never secrets. The full persistence story is deferred; this keeps the [`Config`] contract
//! usable by the operator composition.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::contract::{ChannelMapEntry, Config};
use crate::ids::SourceId;

/// The current persisted configuration version.
pub const CONFIG_VERSION: u32 = 1;

/// An in-memory configuration store.
#[derive(Clone, Debug)]
pub struct ConfigStore {
    inner: Arc<RwLock<Config>>,
}

impl Default for ConfigStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigStore {
    /// Create a new empty configuration at the current version.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Config {
                version: CONFIG_VERSION,
                device_id: None,
                selected_interface: None,
                channel_map: Vec::new(),
                labels: Vec::new(),
            })),
        }
    }

    /// Snapshot the current configuration.
    pub fn get(&self) -> Config {
        self.inner.read().expect("config").clone()
    }

    /// Select the capture device.
    pub fn set_device_id(&self, device_id: Option<String>) {
        self.inner.write().expect("config").device_id = device_id;
    }

    /// Select the LAN interface by name.
    pub fn set_selected_interface(&self, name: Option<String>) {
        self.inner.write().expect("config").selected_interface = name;
    }

    /// Replace the channel map.
    pub fn set_channel_map(&self, channel_map: Vec<ChannelMapEntry>) {
        self.inner.write().expect("config").channel_map = channel_map;
    }

    /// Set (or replace) a source label.
    pub fn set_label(&self, source_id: SourceId, label: String) {
        let mut config = self.inner.write().expect("config");
        if let Some(entry) = config.labels.iter_mut().find(|(id, _)| *id == source_id) {
            entry.1 = label;
        } else {
            config.labels.push((source_id, label));
        }
    }

    /// Labels as a lookup map.
    pub fn labels(&self) -> HashMap<SourceId, String> {
        self.inner
            .read()
            .expect("config")
            .labels
            .iter()
            .cloned()
            .collect()
    }
}
