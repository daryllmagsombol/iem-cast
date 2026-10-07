//! Frozen Task 1 protocol contract.
//!
//! Native structs keep `snake_case`; wire DTOs use `camelCase`. Realtime identities/counters live
//! in [`crate::ids`]. `SessionContext` is internal to the host; wire forms that must carry its
//! fields flatten them explicitly (e.g. `ListenArm`, `MixApplied`) rather than nesting the context.

use std::net::IpAddr;
use std::time::Instant;

use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::ids::{
    ArmNonce, AudioEpoch, CatalogRevision, ChannelMapRevision, HostEpoch, MixRevision, RequestId,
    SafetyGeneration, SessionEpoch, SourceId, AUDIO_SLOT_SAMPLES, BOUNDED_PACKET_BYTES,
    MAX_SOURCES, STEREO_PCM_SAMPLES,
};

// ---------------------------------------------------------------------------------------------
// Fixed storage
// ---------------------------------------------------------------------------------------------

/// One preallocated multichannel capture slot, moved through the SPSC pool and never reallocated.
pub type AudioSlot = Box<[f32; AUDIO_SLOT_SAMPLES]>;

/// One stereo PCM buffer; interleaved L/R, capacity for 240 stereo frames.
pub type StereoPcm = [f32; STEREO_PCM_SAMPLES];

/// Allocate a silent capture slot.
pub fn silent_audio_slot() -> AudioSlot {
    Box::new([0.0f32; AUDIO_SLOT_SAMPLES])
}

/// A bounded RTP packet; `len` is the valid prefix of `bytes`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BoundedPacket {
    pub len: u16,
    pub bytes: [u8; BOUNDED_PACKET_BYTES],
}

impl BoundedPacket {
    /// An empty packet with a full-size zeroed backing buffer.
    pub const fn empty() -> Self {
        Self {
            len: 0,
            bytes: [0u8; BOUNDED_PACKET_BYTES],
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Session context
// ---------------------------------------------------------------------------------------------

/// The exact identity tuple a session carries through control, DSP, and encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub session_epoch: SessionEpoch,
    pub audio_epoch: AudioEpoch,
    pub safety_generation: SafetyGeneration,
}

impl SessionContext {
    /// Placeholder used when decoding a wire payload whose envelope carries the session identity.
    pub const NONE: Self = Self {
        session_epoch: SessionEpoch(Uuid::nil()),
        audio_epoch: AudioEpoch(Uuid::nil()),
        safety_generation: SafetyGeneration(0),
    };

    /// A placeholder context carrying only a known audio epoch (wire decode of context-free payloads).
    pub const fn placeholder(audio_epoch: AudioEpoch) -> Self {
        Self {
            session_epoch: SessionEpoch(Uuid::nil()),
            audio_epoch,
            safety_generation: SafetyGeneration(0),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Capture
// ---------------------------------------------------------------------------------------------

/// Small wire enum summarizing the CPAL sample format of a device.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SampleFormat {
    F32,
    I16,
    U16,
}

/// A capture input device as reported to the operator UI.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    pub is_default: bool,
    pub input_channels: u16,
    pub sample_formats: Vec<SampleFormat>,
    pub sample_rate_hz: Option<u32>,
    pub buffer_min_frames: Option<u32>,
    pub buffer_max_frames: Option<u32>,
}

/// A request to start capture from one device at an explicitly verified rate.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    pub device_id: String,
    pub sample_rate_hz: u32,
    pub buffer_frames: u32,
}

/// One preallocated capture block with fixed metadata.
#[derive(Clone, PartialEq, Debug)]
pub struct CaptureBlock {
    pub audio_epoch: AudioEpoch,
    pub start_sample: u64,
    pub frame_count: u32,
    pub sample_rate_hz: u32,
    pub channel_count: u16,
    pub channel_map_revision: ChannelMapRevision,
    pub samples: AudioSlot,
}

/// Explicit randomness failure; never silently falls back.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EntropyError {
    Unavailable,
}

// ---------------------------------------------------------------------------------------------
// Catalog / config
// ---------------------------------------------------------------------------------------------

/// The role a physical input plays in the mix.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SourceRole {
    InputChannel,
    MasterLr,
}

/// Maps an interleaved physical index to a stable source identity and role.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelMapEntry {
    pub physical_index: u16,
    pub source_id: SourceId,
    pub stereo_pair: Option<SourceId>,
    pub role: SourceRole,
}

/// Versioned persisted configuration (labels, mapping, personal settings only).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub version: u32,
    pub device_id: Option<String>,
    pub selected_interface: Option<String>,
    pub channel_map: Vec<ChannelMapEntry>,
    pub labels: Vec<(SourceId, String)>,
}

/// Catalog view of one source as shown to the operator.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub source_id: SourceId,
    pub physical_index: u16,
    pub label: String,
    pub role: SourceRole,
    pub authorized: bool,
    pub available: bool,
    pub stereo_pair: Option<SourceId>,
}

// ---------------------------------------------------------------------------------------------
// Mix
// ---------------------------------------------------------------------------------------------

/// One per-source gain setting; `Copy` and allocation-free so it can move through audio frames.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceGain {
    pub source_id: SourceId,
    pub gain_db: f32,
    pub muted: bool,
}

impl SourceGain {
    /// The unused/muted filler for slots beyond the active length.
    pub const SILENT: Self = Self {
        source_id: SourceId(Uuid::nil()),
        gain_db: 0.0,
        muted: true,
    };
}

/// Fixed 24-entry per-source gain matrix; serialized as a sequence of exactly `len` entries.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SourceGainMatrix {
    pub entries: [SourceGain; MAX_SOURCES],
    pub len: u16,
}

impl SourceGainMatrix {
    /// An empty matrix.
    pub const fn empty() -> Self {
        Self {
            entries: [SourceGain::SILENT; MAX_SOURCES],
            len: 0,
        }
    }

    /// Build from a slice, truncating anything beyond the native maximum of 24.
    pub fn from_slice(entries: &[SourceGain]) -> Self {
        let mut out = Self::empty();
        let take = entries.len().min(MAX_SOURCES);
        out.entries[..take].copy_from_slice(&entries[..take]);
        out.len = take as u16;
        out
    }

    /// The active entries.
    pub fn as_slice(&self) -> &[SourceGain] {
        &self.entries[..(self.len as usize).min(MAX_SOURCES)]
    }
}

impl Serialize for SourceGainMatrix {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(self.as_slice().iter())
    }
}

struct SourceGainMatrixVisitor;

impl<'de> Visitor<'de> for SourceGainMatrixVisitor {
    type Value = SourceGainMatrix;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a sequence of at most {MAX_SOURCES} source gains")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut out = SourceGainMatrix::empty();
        let mut len = 0usize;
        while let Some(item) = seq.next_element::<SourceGain>()? {
            if len >= MAX_SOURCES {
                return Err(de::Error::custom("more than 24 sources in the gain matrix"));
            }
            out.entries[len] = item;
            len += 1;
        }
        out.len = len as u16;
        Ok(out)
    }
}

impl<'de> Deserialize<'de> for SourceGainMatrix {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_seq(SourceGainMatrixVisitor)
    }
}

/// A complete personal mix snapshot.
///
/// On the wire the internal [`SessionContext`] is omitted; the envelope/event carries the session
/// identity. Decoding produces [`SessionContext::NONE`] for the context.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MixSnapshot {
    pub context: SessionContext,
    pub catalog_revision: CatalogRevision,
    pub mix_revision: MixRevision,
    pub sources: SourceGainMatrix,
    pub master_db: f32,
    pub master_muted: bool,
}

impl MixSnapshot {
    /// A mix snapshot with the given wire-visible fields and a placeholder context.
    pub const fn without_context(
        catalog_revision: CatalogRevision,
        mix_revision: MixRevision,
        sources: SourceGainMatrix,
        master_db: f32,
        master_muted: bool,
    ) -> Self {
        Self {
            context: SessionContext::NONE,
            catalog_revision,
            mix_revision,
            sources,
            master_db,
            master_muted,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MixSnapshotWire<'a> {
    catalog_revision: CatalogRevision,
    mix_revision: MixRevision,
    sources: &'a SourceGainMatrix,
    master_db: f32,
    master_muted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MixSnapshotWireOwned {
    catalog_revision: CatalogRevision,
    mix_revision: MixRevision,
    sources: SourceGainMatrix,
    master_db: f32,
    master_muted: bool,
}

impl Serialize for MixSnapshot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MixSnapshotWire {
            catalog_revision: self.catalog_revision,
            mix_revision: self.mix_revision,
            sources: &self.sources,
            master_db: self.master_db,
            master_muted: self.master_muted,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MixSnapshot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MixSnapshotWireOwned::deserialize(deserializer)?;
        Ok(Self {
            context: SessionContext::NONE,
            catalog_revision: wire.catalog_revision,
            mix_revision: wire.mix_revision,
            sources: wire.sources,
            master_db: wire.master_db,
            master_muted: wire.master_muted,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// DSP frames
// ---------------------------------------------------------------------------------------------

/// One 120-frame stereo codec frame with its session identity.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StereoFrame {
    pub context: SessionContext,
    pub start_sample: u64,
    pub frame_count: u32,
    pub pcm: StereoPcm,
}

impl StereoFrame {
    /// A silent frame for the given context.
    pub fn zeroed(context: SessionContext) -> Self {
        Self {
            context,
            start_sample: 0,
            frame_count: 0,
            pcm: [0.0f32; STEREO_PCM_SAMPLES],
        }
    }
}

/// One encoded Opus packet with its session identity and enqueue timestamp.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct EncodedFrame {
    pub context: SessionContext,
    pub start_sample: u64,
    pub frame_count: u32,
    pub enqueue_instant: Instant,
    pub packet: BoundedPacket,
}

impl EncodedFrame {
    /// An empty encoded frame.
    pub fn zeroed() -> Self {
        Self {
            context: SessionContext::NONE,
            start_sample: 0,
            frame_count: 0,
            enqueue_instant: Instant::now(),
            packet: BoundedPacket::empty(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Events / media
// ---------------------------------------------------------------------------------------------

/// Why a listener was interrupted.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum InterruptReason {
    QueueResidenceExceeded,
    DeviceLost,
    NetworkDisconnected,
    OutputRouteChange,
    UserDisarm,
}

/// Capture-path fault.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum CaptureFault {
    #[error("capture device unavailable")]
    DeviceUnavailable,
    #[error("unsupported sample rate")]
    UnsupportedRate,
    #[error("unsupported sample format")]
    UnsupportedFormat,
    #[error("channel count too large")]
    ChannelCountTooLarge,
    #[error("capture stream error")]
    StreamError,
    #[error("capture device disconnected")]
    Disconnected,
}

/// DSP-path fault.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum AudioFault {
    #[error("internal audio error")]
    Internal,
    #[error("invalid sample")]
    InvalidSample,
    #[error("stale revision")]
    RevisionStale,
    #[error("unmapped source")]
    UnmappedSource,
}

/// Encoder-path fault.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum EncodeFault {
    #[error("encoder initialization failed")]
    EncoderInit,
    #[error("encode failed")]
    EncodeFailed,
}

/// Categorized fault published on the audio event channel.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum FaultCode {
    #[error(transparent)]
    Capture(CaptureFault),
    #[error(transparent)]
    Audio(AudioFault),
}

/// Media negotiation/transport failure.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum MediaFailureCode {
    #[error("negotiation failed")]
    NegotiationFailed,
    #[error("ICE failed")]
    IceFailed,
    #[error("mDNS unresolved")]
    MdnUnresolved,
    #[error("no audio section")]
    NoAudioSection,
    #[error("extra media rejected")]
    ExtraMediaRejected,
    #[error("peer disconnected")]
    PeerDisconnected,
}

/// Control-plane error; maps to the five structured error strings off this enum.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum ControlError {
    #[error("revision conflict")]
    RevisionConflict,
    #[error("source forbidden")]
    SourceForbidden,
    #[error("stale epoch")]
    StaleEpoch,
    #[error("unauthorized")]
    Unauthorized,
    #[error("unmute forbidden while unarmed")]
    UnarmedUnmuteForbidden,
    #[error("rate limited")]
    RateLimited,
    #[error("TLS identity invalid")]
    TlsIdentityInvalid,
    #[error("internal control error")]
    Internal,
}

/// Events published by the DSP/audio worker after applying state.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AudioEvent {
    MixApplied {
        applied_revision: MixRevision,
        context: SessionContext,
        start_sample: u64,
        snapshot: MixSnapshot,
    },
    ArmApplied {
        context: SessionContext,
        arm_nonce: ArmNonce,
    },
    Interrupted {
        session_epoch: SessionEpoch,
        reason: InterruptReason,
    },
    Fault {
        code: FaultCode,
    },
}

/// Commands sent into a per-listener media session.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MediaCommand {
    Offer {
        session_epoch: SessionEpoch,
        sdp: String,
    },
    Candidate {
        session_epoch: SessionEpoch,
        candidate: Option<String>,
    },
    Attach {
        session_epoch: SessionEpoch,
    },
    Detach {
        session_epoch: SessionEpoch,
    },
}

/// Events emitted by a per-listener media session.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MediaEvent {
    Answer {
        session_epoch: SessionEpoch,
        sdp: String,
    },
    Readiness {
        session_epoch: SessionEpoch,
        ready: bool,
    },
    Failure {
        session_epoch: SessionEpoch,
        code: MediaFailureCode,
    },
}

// ---------------------------------------------------------------------------------------------
// Wire DTOs
// ---------------------------------------------------------------------------------------------

/// Protocol v1 envelope wrapping a complete payload.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvelopeV1<T> {
    pub v: u8,
    #[serde(rename = "type")]
    pub kind: String,
    pub request_id: RequestId,
    pub host_epoch: HostEpoch,
    pub session_epoch: Option<SessionEpoch>,
    pub payload: T,
}

/// Listener RTC offer carrying the browser's SDP.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RtcOffer {
    pub sdp: String,
}

/// Listener RTC trickle candidate. `None` candidate = end-of-candidates.
///
/// The browser's `RTCIceCandidateInit` uses `sdpMid`/`sdpMLineIndex`; the native fields are
/// `sdp_mid`/`sdp_m_line_index`, renamed to camelCase on the wire.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RtcCandidate {
    pub candidate: Option<String>,
    pub sdp_mid: Option<String>,
    pub sdp_m_line_index: Option<u16>,
}

/// Host RTC answer carrying the host's SDP.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RtcAnswer {
    pub sdp: String,
}

/// Absolute mix intent from a listener.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixPatch {
    pub base_revision: MixRevision,
    pub catalog_revision: CatalogRevision,
    pub sources: SourceGainMatrix,
    pub master_db: f32,
    pub master_muted: bool,
}

/// Accepted mix acknowledgement carrying canonical settings.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MixAck {
    pub accepted_revision: MixRevision,
    pub catalog_revision: CatalogRevision,
    #[serde(rename = "canonicalSettings")]
    pub canonical_snapshot: MixSnapshot,
}

/// Notification that a mix revision was installed by the DSP.
///
/// Wire shape: `appliedRevision`, `audioEpoch`, `startSample`, `settings` — the native
/// [`SessionContext`] is not nested. `startSample` is a decimal-only string on the wire.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MixApplied {
    pub applied_revision: MixRevision,
    pub context: SessionContext,
    pub start_sample: u64,
    pub snapshot: MixSnapshot,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MixAppliedWire<'a> {
    applied_revision: MixRevision,
    audio_epoch: AudioEpoch,
    #[serde(serialize_with = "serialize_dec_u64")]
    start_sample: u64,
    settings: &'a MixSnapshot,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MixAppliedWireOwned {
    applied_revision: MixRevision,
    audio_epoch: AudioEpoch,
    #[serde(deserialize_with = "deserialize_dec_u64")]
    start_sample: u64,
    settings: MixSnapshot,
}

fn serialize_dec_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

fn deserialize_dec_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let text = String::deserialize(deserializer)?;
    text.parse::<u64>()
        .map_err(|_| de::Error::custom("expected a decimal-only u64 string"))
}

impl Serialize for MixApplied {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        MixAppliedWire {
            applied_revision: self.applied_revision,
            audio_epoch: self.context.audio_epoch,
            start_sample: self.start_sample,
            settings: &self.snapshot,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MixApplied {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = MixAppliedWireOwned::deserialize(deserializer)?;
        Ok(Self {
            applied_revision: wire.applied_revision,
            context: SessionContext::placeholder(wire.audio_epoch),
            start_sample: wire.start_sample,
            snapshot: wire.settings,
        })
    }
}

/// Listener arm request.
///
/// Wire shape: `audioEpoch`, `safetyGeneration`, `appliedRevision`, `armNonce`. The native
/// [`SessionContext`] is flattened; `sessionEpoch` comes from the envelope.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ListenArm {
    pub context: SessionContext,
    pub applied_revision: MixRevision,
    pub arm_nonce: ArmNonce,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ListenArmWire {
    audio_epoch: AudioEpoch,
    safety_generation: SafetyGeneration,
    applied_revision: MixRevision,
    arm_nonce: ArmNonce,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListenArmWireOwned {
    audio_epoch: AudioEpoch,
    safety_generation: SafetyGeneration,
    applied_revision: MixRevision,
    arm_nonce: ArmNonce,
}

impl Serialize for ListenArm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ListenArmWire {
            audio_epoch: self.context.audio_epoch,
            safety_generation: self.context.safety_generation,
            applied_revision: self.applied_revision,
            arm_nonce: self.arm_nonce,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ListenArm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ListenArmWireOwned::deserialize(deserializer)?;
        Ok(Self {
            context: SessionContext {
                session_epoch: SessionEpoch(Uuid::nil()),
                audio_epoch: wire.audio_epoch,
                safety_generation: wire.safety_generation,
            },
            applied_revision: wire.applied_revision,
            arm_nonce: wire.arm_nonce,
        })
    }
}

/// Host confirmation that a listener is armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenArmed {
    pub session_epoch: SessionEpoch,
    pub safety_generation: SafetyGeneration,
    pub audio_epoch: AudioEpoch,
    pub arm_nonce: ArmNonce,
}

/// Listener disarm request.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenDisarm {
    pub session_epoch: SessionEpoch,
    pub safety_generation: SafetyGeneration,
}

/// Full server-side view of one listener session.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSnapshot {
    pub session_epoch: SessionEpoch,
    pub context: SessionContext,
    pub phase: ListenerPhase,
    pub catalog_revision: CatalogRevision,
    pub requested_mix: Option<MixSnapshot>,
    pub accepted_mix: Option<MixSnapshot>,
    pub applied_mix: Option<MixSnapshot>,
}

/// Versioned source catalog.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub catalog_revision: CatalogRevision,
    pub sources: Vec<SourceInfo>,
}

/// Condensed listener state for UI updates.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerState {
    pub session_epoch: SessionEpoch,
    pub phase: ListenerPhase,
    pub armed: bool,
}

/// Listener lifecycle phase; kebab-case on the wire (`ready-muted`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListenerPhase {
    Unpaired,
    Paired,
    Negotiating,
    ReadyMuted,
    Armed,
    Interrupted,
    Revoked,
}

// ---------------------------------------------------------------------------------------------
// Start host
// ---------------------------------------------------------------------------------------------

/// A selected LAN interface.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InterfaceInfo {
    pub name: String,
    pub ip_address: IpAddr,
    pub prefix: u8,
}

/// Operator request to start the host on a device and interface.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostRequest {
    pub capture: CaptureRequest,
    pub interface: InterfaceInfo,
    pub certificate_path: String,
    pub key_path: String,
}

/// Result of starting the host.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartHostResult {
    pub host_epoch: HostEpoch,
    pub audio_epoch: AudioEpoch,
    pub join_url: String,
    pub catalog: CatalogSnapshot,
}

// ---------------------------------------------------------------------------------------------
// Injection traits
// ---------------------------------------------------------------------------------------------

/// Monotonic time source; injected so tests control time.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Randomness source; failure is explicit.
pub trait Entropy: Send + Sync {
    fn fill(&self, buf: &mut [u8]) -> Result<(), EntropyError>;
}

/// Embedded asset access for the musician bundle.
pub trait AssetProvider: Send + Sync {
    fn get(&self, path: &str) -> Option<&'static [u8]>;
}

/// Control-to-DSP command surface. `disarm` closes the shared gate immediately.
pub trait DspControl: Send + Sync {
    fn install_snapshot(&self, snapshot: MixSnapshot) -> Result<(), ControlError>;
    fn request_arm(&self, arm: ListenArm) -> Result<(), ControlError>;
    fn disarm(&self, session: SessionEpoch, new_generation: SafetyGeneration);
}

/// Bounded, non-blocking sink the audio worker publishes to only after applying.
pub trait AudioEventSink: Send + Sync {
    fn try_publish(&self, ev: AudioEvent) -> bool;
}
