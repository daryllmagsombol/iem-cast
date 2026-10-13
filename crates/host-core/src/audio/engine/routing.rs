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
    ///
    /// Structural validity is resolved to a **fixpoint** before any audible route is assigned.
    /// Ambiguous metadata (duplicate source identities, duplicate physical bindings, orphan sides,
    /// one-sided or non-reciprocal links, ambiguous peer matches, and peers outside the bounded map)
    /// marks its implicated entries invalid, and invalidity then **propagates through reciprocal
    /// partners** (a valid pair whose partner is invalid for any reason is itself closed) until
    /// stable. Only entries that survive — and whose pair peer also survives — are routed. Unrelated
    /// valid sources are preserved.
    pub fn build(channel_map: &[ChannelMapEntry]) -> Self {
        let map: &[ChannelMapEntry] = &channel_map[..channel_map.len().min(MAX_SOURCES)];
        let count = map.len();
        let mut invalid = vec![false; count];

        // Duplicate source identities: every entry sharing an identity is ambiguous.
        for i in 0..count {
            if (0..count).any(|j| j != i && map[j].source_id == map[i].source_id) {
                invalid[i] = true;
            }
        }
        // Duplicate physical bindings: every entry on the shared index is ambiguous.
        for i in 0..count {
            if (0..count).any(|j| j != i && map[j].physical_index == map[i].physical_index) {
                invalid[i] = true;
            }
        }

        // Unique peer lookup: `None` when the peer id is absent **or ambiguous** (multiple entries
        // share it). An ambiguous match is an invalid link, never a first-match guess.
        let peer_of = |i: usize| -> Option<usize> {
            let peer_id = map[i].stereo_pair?;
            let mut found: Option<usize> = None;
            for (j, entry) in map.iter().enumerate() {
                if j != i && entry.source_id == peer_id {
                    if found.is_some() {
                        return None; // ambiguous identity match
                    }
                    found = Some(j);
                }
            }
            found
        };

        // Base link validity: an orphan side, an absent/ambiguous peer, or a non-reciprocal / role-
        // mismatched / same-side link closes the endpoint(s) it implicates.
        for i in 0..count {
            match map[i].stereo_pair {
                None => {
                    if map[i].stereo_side.is_some() {
                        invalid[i] = true;
                    }
                }
                Some(_) => match peer_of(i) {
                    Some(j) => {
                        let peer = &map[j];
                        let valid = peer.stereo_pair == Some(map[i].source_id)
                            && peer.role == map[i].role
                            && map[i].stereo_side.is_some()
                            && peer.stereo_side.is_some()
                            && map[i].stereo_side != peer.stereo_side;
                        if !valid {
                            invalid[i] = true;
                            invalid[j] = true;
                        }
                    }
                    None => invalid[i] = true,
                },
            }
        }

        // Propagate invalidity to a fixpoint through reciprocal partners. A conflicting incoming
        // claim (e.g. C -> A alongside a valid reciprocal A <-> B) leaves A invalid; that must close
        // B as well, not leave B audible. Bounded `n <= MAX_SOURCES`, no graph framework.
        loop {
            let mut changed = false;
            for i in 0..count {
                if map[i].stereo_pair.is_none() {
                    continue;
                }
                match peer_of(i) {
                    Some(j) => {
                        if invalid[i] || invalid[j] {
                            if !invalid[i] {
                                invalid[i] = true;
                                changed = true;
                            }
                            if !invalid[j] {
                                invalid[j] = true;
                                changed = true;
                            }
                        }
                    }
                    None => {
                        if !invalid[i] {
                            invalid[i] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }

        // Assign audible routes only to entries (and pair endpoints) that survived validation.
        let mut routes = vec![Route::Silent; count];
        let mut assigned: Vec<Option<u8>> = vec![None; count];
        let mut group_anchor: Vec<SourceId> = Vec::with_capacity(count);

        for (index, entry) in map.iter().enumerate() {
            if invalid[index] {
                continue;
            }
            // An unpaired legacy master keeps the parity split unchanged and ignores pan.
            if entry.stereo_pair.is_none() && entry.role == SourceRole::MasterLr {
                routes[index] = if entry.physical_index % 2 == 0 {
                    Route::ParityLeft
                } else {
                    Route::ParityRight
                };
                continue;
            }
            match entry.stereo_pair {
                None => {
                    // Unpaired, no side: a mono pan source with its own shared ramp group.
                    let group = group_anchor.len() as u8;
                    group_anchor.push(entry.source_id);
                    assigned[index] = Some(group);
                    routes[index] = Route::MonoPan { group };
                }
                Some(_) => {
                    if let Some(group) = assigned[index] {
                        routes[index] = side_route(entry.stereo_side, group);
                        continue;
                    }
                    // A validated pair has a unique in-map peer; require **both** endpoints valid
                    // before assigning the shared group.
                    let peer_index = peer_of(index)
                        .expect("a valid pair entry has a unique in-map peer");
                    if invalid[peer_index] {
                        continue;
                    }
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
