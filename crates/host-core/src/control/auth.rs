//! Control-plane authentication primitives: OS entropy, session cookies, and origin policy.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::contract::{Clock, Entropy, EntropyError};
use crate::ids::SessionEpoch;

/// Session cookie name presented by the musician page.
pub const SESSION_COOKIE_NAME: &str = "iem_session";

/// Draft lifetime of one authenticated listener session.
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// Primary OS randomness source. Failure is surfaced by the [`Entropy`] contract.
pub struct OsEntropy;

impl Entropy for OsEntropy {
    fn fill(&self, buf: &mut [u8]) -> Result<(), EntropyError> {
        getrandom::fill(buf).map_err(|_| EntropyError::Unavailable)
    }
}

fn random_bytes(entropy: &dyn Entropy, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    if entropy.fill(&mut buf).is_err() {
        // Randomness failure is unrecoverable for a security boundary; never fall back silently.
        panic!("OS entropy unavailable while minting an authentication secret");
    }
    buf
}

/// Lowercase hex encoding.
pub fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// One minted session: its epoch and absolute expiry.
#[derive(Clone, Copy, Debug)]
struct SessionRecord {
    session: SessionEpoch,
    expires_at: Instant,
}

/// In-memory cookie-to-session store. Tokens are never persisted.
pub struct SessionStore {
    clock: Arc<dyn Clock>,
    entropy: Arc<dyn Entropy>,
    ttl: Duration,
    by_token: HashMap<String, SessionRecord>,
    by_session: HashMap<SessionEpoch, String>,
    order: VecDeque<SessionEpoch>,
}

impl SessionStore {
    /// Create a store over injected clock/entropy with the given session lifetime.
    pub fn new(clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, ttl: Duration) -> Self {
        Self {
            clock,
            entropy,
            ttl,
            by_token: HashMap::new(),
            by_session: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Mint a new opaque cookie value and its session epoch.
    pub fn issue(&mut self) -> (String, SessionEpoch) {
        let token = hex(&random_bytes(self.entropy.as_ref(), 32));
        let session = SessionEpoch(uuid::Uuid::from_bytes(
            random_bytes(self.entropy.as_ref(), 16).try_into().expect("16 bytes"),
        ));
        let record = SessionRecord {
            session,
            expires_at: self.clock.now() + self.ttl,
        };
        self.by_token.insert(token.clone(), record);
        self.by_session.insert(session, token.clone());
        self.order.push_back(session);
        (token, session)
    }

    /// Resolve a cookie value to a live session, purging it when expired.
    pub fn authenticate(&mut self, token: &str) -> Option<SessionEpoch> {
        let record = *self.by_token.get(token)?;
        if self.clock.now() > record.expires_at {
            self.by_token.remove(token);
            self.by_session.remove(&record.session);
            return None;
        }
        Some(record.session)
    }

    /// Drop all expired sessions.
    pub fn expire_now(&mut self) {
        let now = self.clock.now();
        self.by_token.retain(|_, r| r.expires_at >= now);
        self.by_session
            .retain(|s, _| self.by_token.values().any(|r| r.session == *s));
        self.order.retain(|s| self.by_session.contains_key(s));
    }

    /// Number of live sessions after purging expired entries.
    pub fn active_count(&mut self) -> usize {
        self.expire_now();
        self.by_session.len()
    }

    /// Evict the oldest live session, returning its epoch.
    pub fn evict_oldest(&mut self) -> Option<SessionEpoch> {
        self.expire_now();
        while let Some(session) = self.order.pop_front() {
            if let Some(token) = self.by_session.remove(&session) {
                self.by_token.remove(&token);
                return Some(session);
            }
        }
        None
    }

    /// Revoke one session's cookie.
    pub fn revoke(&mut self, session: SessionEpoch) {
        if let Some(token) = self.by_session.remove(&session) {
            self.by_token.remove(&token);
        }
        self.order.retain(|s| *s != session);
    }
}

/// Build the `Set-Cookie` header value for an authenticated musician.
pub fn set_cookie_value(token: &str, ttl: Duration) -> String {
    format!(
        "{SESSION_COOKIE_NAME}={token}; Path=/; Max-Age={}; HttpOnly; Secure; SameSite=Strict",
        ttl.as_secs()
    )
}

/// Parse one `Cookie` request header and return the session cookie value, if present.
pub fn session_cookie_value(header: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let part = part.trim();
        let (name, value) = part.split_once('=')?;
        (name == SESSION_COOKIE_NAME).then(|| value.to_string())
    })
}

/// Exact-match origin allowlist. Wildcards are deliberately not supported.
#[derive(Clone, Debug, Default)]
pub struct OriginPolicy {
    allowed: Vec<String>,
}

impl OriginPolicy {
    /// Build from an explicit list of allowed origins.
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }

    /// `true` only when the presented origin is byte-for-byte on the list.
    pub fn is_allowed(&self, origin: &str) -> bool {
        self.allowed.iter().any(|a| a == origin)
    }

    /// The configured origins (for diagnostics).
    pub fn allowed(&self) -> &[String] {
        &self.allowed
    }
}
