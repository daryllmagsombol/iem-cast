//! Realtime identity and counter newtypes.
//!
//! Realtime identities passed through frames are `Copy` newtypes over [`Uuid`]; their wire form
//! is an opaque UUID string. Counters are `Copy` newtypes over `u64` whose wire form is a
//! decimal-only string so they remain exact in JavaScript. Formatting happens off-thread.

use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

/// Native maximum number of physical sources (a device reporting more is rejected).
pub const MAX_SOURCES: usize = 24;
/// Native maximum capture block width.
pub const MAX_BLOCK_FRAMES: usize = 256;
/// Interleaved sample count of one stereo PCM buffer (240 stereo frames).
pub const STEREO_PCM_SAMPLES: usize = 480;
/// Fixed byte capacity of a bounded RTP packet.
pub const BOUNDED_PACKET_BYTES: usize = 1275;
/// Interleaved sample capacity of one capture pool slot.
pub const AUDIO_SLOT_SAMPLES: usize = MAX_BLOCK_FRAMES * MAX_SOURCES;

macro_rules! identity_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Construct from raw bytes (used by tests and native fixtures).
            pub const fn from_bytes(bytes: [u8; 16]) -> Self {
                Self(Uuid::from_bytes(bytes))
            }

            /// Borrow the underlying UUID.
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let text = String::deserialize(deserializer)?;
                Uuid::parse_str(&text).map(Self).map_err(de::Error::custom)
            }
        }
    };
}

identity_newtype! {
    /// Global host control identity; created per host process restart and not carried in audio frames.
    HostEpoch
}
identity_newtype! {
    /// Capture generation identity; a new value invalidates all prior audio/timeline references.
    AudioEpoch
}
identity_newtype! {
    /// Per-listener/connection identity; a new value invalidates that listener's prior messages.
    SessionEpoch
}
identity_newtype! {
    /// Fresh nonce minted per arm attempt; a cancelled nonce is permanently invalid for that attempt.
    ArmNonce
}
identity_newtype! {
    /// Stable identity of one catalog source, preserved across renames.
    SourceId
}

macro_rules! counter_newtype {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub u64);

        impl $name {
            /// Raw native value.
            pub const fn get(&self) -> u64 {
                self.0
            }
        }

        impl From<u64> for $name {
            fn from(value: u64) -> Self {
                Self(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let text = String::deserialize(deserializer)?;
                text.parse::<u64>()
                    .map(Self)
                    .map_err(|_| de::Error::custom("expected a decimal-only u64 string"))
            }
        }
    };
}

counter_newtype! {
    /// Monotonic source-catalog/label/pairing revision.
    CatalogRevision
}
counter_newtype! {
    /// Monotonic accepted-mix revision.
    MixRevision
}
counter_newtype! {
    /// Monotonic safety generation advanced on every arm/disarm.
    SafetyGeneration
}
counter_newtype! {
    /// Revision of the interleaved-index-to-source channel map.
    ChannelMapRevision
}

/// A capture frame count serialized as a decimal string constrained to the `u32` range.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct FrameCountWire(pub u32);

impl FrameCountWire {
    /// Raw native value.
    pub const fn get(&self) -> u32 {
        self.0
    }
}

impl Serialize for FrameCountWire {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for FrameCountWire {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        let value = text
            .parse::<u64>()
            .map_err(|_| de::Error::custom("expected a decimal-only u32 string"))?;
        u32::try_from(value)
            .map(FrameCountWire)
            .map_err(|_| de::Error::custom("frameCount exceeds the u32 range"))
    }
}

/// Opaque, unique per-client request identifier.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

impl RequestId {
    /// Borrow the request id text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
