//! HTTPS/WSS app server boundary (Lane B).
//!
//! `GET /join` serves only the static musician page; `POST /api/v1/pair` exchanges a single-use
//! pairing token for a session cookie; `GET /api/v1/ws` is the authenticated WSS control socket.
//! Origin and cookie authorization happen at this boundary, never in the browser.

pub mod http;
pub mod pairing;
pub mod protocol;
pub mod tls;
pub mod ws;

#[cfg(test)]
mod tests;

pub use crate::control::auth::OriginPolicy;
pub use http::{HostServer, PairRequest, PairResponse, POC_ACTIVE_RECEIVER_CAP};
pub use pairing::{PairError, PairStore, PairingCredential};
pub use protocol::{ProtocolError, ServerMessage};
pub use tls::TlsIdentity;
