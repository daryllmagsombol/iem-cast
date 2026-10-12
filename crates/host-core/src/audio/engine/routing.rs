//! Precomputed, bounded channel-map routing plan.
//!
//! Built once when an [`AudioEngine`](super::AudioEngine) is created so the per-frame path performs
//! no link searching and no allocation: every mapped source has a fixed [`Route`] and a shared pan/
//! balance ramp group. An unpaired legacy `MasterLr` keeps its `physical_index % 2` parity split;
//! every `InputChannel` and every explicitly linked pair uses the side/balance law. A structurally
//! invalid or incomplete pair is marked [`Route::Silent`], so it fails closed to silence rather than
//! being silently degraded into a mono source.

use crate::contract::{ChannelMapEntry, SourceRole, StereoSide};
use crate::ids::{SourceId, MAX_SOURCES};

/// What one mapped source contributes to the stereo bus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Route {
    /// Structurally invalid or incomplete entry: contributes silence.
    Silent,
    /// Unpaired legacy `MasterLr` with an even physical index (left).
    ParityLeft,
    /// Unpaired legacy `MasterLr` with an odd physical index (right).
    ParityRight,
    /// Unpaired mono `InputChannel`: both sides via the pan law, one shared ramp.
    MonoPan { group: u8 },
    /// Explicitly linked pair left member: left side only, via `L(p)`.
    PairLeft { group: u8 },
    /// Explicitly linked pair right member: right side only, via `R(p)`.
    PairRight { group: u8 },
}

/// One immutable routing decision for a mapped source.
#[derive(Clone, Copy, Debug)]
pub struct RoutingEntry {
    /// Interleaved physical index in the capture block.
    pub physical_index: u16,
    /// Stable source identity this entry resolves gains/pans from.
    pub source_id: SourceId,
    /// The fixed routing decision.
    pub route: Route,
}

/// A bounded routing plan: one entry per mapped source (at most [`MAX_SOURCES`]) and one shared
/// pan/balance ramp group per mono source or linked pair.
pub struct RoutingPlan {
    entries: Vec<RoutingEntry>,
    group_anchor: Vec<SourceId>,
}

impl RoutingPlan {
    /// Build a plan from a channel map, truncating anything beyond [`MAX_SOURCES`].
    pub fn build(channel_map: &[ChannelMapEntry]) -> Self {
        let map: &[ChannelMapEntry] = &channel_map[..channel_map.len().min(MAX_SOURCES)];
        let mut routes = vec![Route::Silent; map.len()];
        let mut assigned: Vec<Option<u8>> = vec![None; map.len()];
        let mut group_anchor: Vec<SourceId> = Vec::with_capacity(map.len());

        for (index, entry) in map.iter().enumerate() {
            // An unpaired legacy master keeps the parity split, unchanged, and ignores pan.
            if entry.role == SourceRole::MasterLr && entry.stereo_pair.is_none() {
                routes[index] = if entry.physical_index % 2 == 0 {
                    Route::ParityLeft
                } else {
                    Route::ParityRight
                };
                continue;
            }

            match entry.stereo_pair {
                None => {
                    // An unpaired source must carry no side. A side without a pair is inconsistent
                    // and fails closed rather than being treated as a mono source.
                    if entry.stereo_side.is_some() {
                        routes[index] = Route::Silent;
                        continue;
                    }
                    let group = group_anchor.len() as u8;
                    group_anchor.push(entry.source_id);
                    assigned[index] = Some(group);
                    routes[index] = Route::MonoPan { group };
                }
                Some(peer_id) => {
                    if let Some(group) = assigned[index] {
                        routes[index] = side_route(entry.stereo_side, group);
                        continue;
                    }
                    let Some(peer_index) = map.iter().position(|c| c.source_id == peer_id) else {
                        routes[index] = Route::Silent;
                        continue;
                    };
                    let peer = &map[peer_index];
                    let valid = peer_index != index
                        && peer.stereo_pair == Some(entry.source_id)
                        && peer.role == entry.role
                        && entry.stereo_side.is_some()
                        && peer.stereo_side.is_some()
                        && entry.stereo_side != peer.stereo_side;
                    if !valid {
                        routes[index] = Route::Silent;
                        continue;
                    }
                    // The pair shares exactly one pan/balance ramp group.
                    let group = group_anchor.len() as u8;
                    group_anchor.push(entry.source_id);
                    assigned[index] = Some(group);
                    assigned[peer_index] = Some(group);
                    routes[index] = side_route(entry.stereo_side, group);
                }
            }
        }

        let entries = map
            .iter()
            .zip(routes.iter())
            .map(|(entry, route)| RoutingEntry {
                physical_index: entry.physical_index,
                source_id: entry.source_id,
                route: *route,
            })
            .collect();

        Self {
            entries,
            group_anchor,
        }
    }

    /// The immutable per-source routing entries.
    pub fn entries(&self) -> &[RoutingEntry] {
        &self.entries
    }

    /// Number of shared pan/balance ramp groups (one per mono source or linked pair).
    pub fn group_count(&self) -> usize {
        self.group_anchor.len()
    }

    /// The representative source for a group (the first member encountered). Pair members are
    /// guaranteed equal canonical pan, so any member is a valid anchor.
    pub fn group_anchor(&self, group: usize) -> Option<SourceId> {
        self.group_anchor.get(group).copied()
    }
}

fn side_route(side: Option<StereoSide>, group: u8) -> Route {
    match side {
        Some(StereoSide::Left) => Route::PairLeft { group },
        Some(StereoSide::Right) => Route::PairRight { group },
        None => Route::Silent,
    }
}
