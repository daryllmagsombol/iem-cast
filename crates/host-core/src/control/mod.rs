//! Authoritative control boundary (Lane B).
//!
//! The [`ControlActor`] owns safety generations, accepted/installed mix revisions, and the pairing
//! store. Authentication is enforced by the [`crate::server`] boundary; the actor receives an
//! already-authenticated [`crate::ids::SessionEpoch`].

pub mod actor;
pub mod auth;
pub mod revisions;

#[cfg(test)]
mod tests;

pub use actor::ControlActor;
pub use auth::{OriginPolicy, SessionStore};
pub use revisions::{canonicalize_gain, RateLimiter};
