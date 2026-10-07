//! Single-use pairing credentials.
//!
//! Tokens are 256 bits of OS randomness, valid for 120 seconds, single-use, removed from history
//! on exchange, and never logged. The store is driven by injected [`Clock`]/[`Entropy`] so tests
//! control time and randomness.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::contract::{Clock, Entropy, EntropyError};

/// Pairing credential lifetime.
pub const PAIRING_TTL: Duration = Duration::from_secs(120);
/// Raw token length in bytes (256 bits).
pub const PAIRING_TOKEN_BYTES: usize = 32;

/// Operator-facing pairing credential (camelCase on the wire).
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingCredential {
    /// Join URL with the single-use fragment token embedded. Never logged.
    pub join_url: String,
    /// Remaining lifetime in whole seconds.
    pub expires_in_seconds: u64,
}

/// Why a pairing token could not be exchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PairError {
    /// Unknown, already used, or expired token.
    UnknownOrExpired,
    /// Randomness was unavailable when minting the token.
    EntropyUnavailable,
}

#[derive(Clone, Copy, Debug)]
struct Pending {
    expires_at: Instant,
}

/// In-memory single-use pairing store.
pub struct PairStore {
    entropy: Arc<dyn Entropy>,
    clock: Arc<dyn Clock>,
    ttl: Duration,
    join_base: String,
    pending: HashMap<String, Pending>,
}

impl std::fmt::Debug for PairStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render tokens, even when the store is logged for diagnostics.
        f.debug_struct("PairStore")
            .field("pending_count", &self.pending.len())
            .field("join_base", &self.join_base)
            .finish()
    }
}

impl PairStore {
    /// Create a store with the default 120 s lifetime and join base.
    pub fn new(entropy: Arc<dyn Entropy>, clock: Arc<dyn Clock>) -> Self {
        Self {
            entropy,
            clock,
            ttl: PAIRING_TTL,
            join_base: "https://host.local:8443".to_string(),
            pending: HashMap::new(),
        }
    }

    /// Override the join base URL used in issued credentials.
    pub fn set_join_base(&mut self, base: impl Into<String>) {
        self.join_base = base.into();
    }

    fn mint_token(&self) -> Result<String, PairError> {
        let mut buf = [0u8; PAIRING_TOKEN_BYTES];
        self.entropy
            .fill(&mut buf)
            .map_err(|_: EntropyError| PairError::EntropyUnavailable)?;
        Ok(crate::control::auth::hex(&buf))
    }

    /// Mint a fresh single-use token. The plaintext token is returned to the caller only.
    pub fn issue(&mut self) -> String {
        match self.mint_token() {
            Ok(token) => {
                let expires_at = self.clock.now() + self.ttl;
                self.pending.insert(token.clone(), Pending { expires_at });
                token
            }
            Err(_) => panic!("OS entropy unavailable while minting a pairing token"),
        }
    }

    /// Mint an operator credential embedding the token in the join URL fragment.
    pub fn issue_credential(&mut self) -> PairingCredential {
        let token = self.issue();
        PairingCredential {
            join_url: format!("{}/join#t={token}", self.join_base),
            expires_in_seconds: self.ttl.as_secs(),
        }
    }

    /// Exchange a token. Single-use: it is removed from history whether or not it succeeds.
    pub fn exchange(&mut self, token: &str) -> Result<(), PairError> {
        // Remove on first sight so a token can never be used twice, even under a race.
        let Some(pending) = self.pending.remove(token) else {
            return Err(PairError::UnknownOrExpired);
        };
        if self.clock.now() > pending.expires_at {
            return Err(PairError::UnknownOrExpired);
        }
        Ok(())
    }

    /// Number of outstanding (unused) tokens.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}
