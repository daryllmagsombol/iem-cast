//! Bounded mDNS (`.local`) candidate resolution for the selected LAN.
//!
//! `str0m`'s ICE candidate parser accepts IP literals, not `.local` hostnames. A browser on a
//! local network may emit mDNS candidates such as `3f2a...local`. We resolve those through an
//! injected, bounded resolver *before* handing the candidate to `str0m`. Resolution is
//! best-effort and is explicitly **not** a Bonjour guarantee.

use std::net::IpAddr;

use crate::contract::MediaFailureCode;

/// A resolver that turns a `.local` hostname into an IP address.
///
/// Injected so the transport logic is testable offline and so a production build can choose the
/// system resolver. Returning `Err` is surfaced as `MdnUnresolved`, never silently ignored.
pub trait MdnsResolver: Send + Sync {
    fn resolve(&self, hostname: &str) -> Result<IpAddr, MediaFailureCode>;
}

/// Bounds on resolution work, to keep a hostile or broken peer from causing unbounded work.
#[derive(Clone, Copy, Debug)]
pub struct MdnsLimits {
    /// Maximum `.local` candidates resolved per session.
    pub max_resolves: usize,
}

impl Default for MdnsLimits {
    fn default() -> Self {
        Self { max_resolves: 16 }
    }
}

/// Classify a raw ICE candidate string.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CandidateClass {
    /// The candidate's connection address is already a literal IP; pass through unchanged.
    IpLiteral,
    /// The candidate uses a `.local` hostname that must be resolved first.
    LocalHostname(String),
    /// Not a candidate line we recognise.
    Unrecognised,
}

/// Extract the connection address token from an ICE candidate line.
///
/// Candidate syntax: `candidate:<foundation> <component> <transport> <priority> <address>
/// <port> typ <type> ...`. We only inspect the address field.
pub fn classify_candidate(candidate: &str) -> CandidateClass {
    let rest = match candidate.strip_prefix("candidate:") {
        Some(rest) => rest,
        None => return CandidateClass::Unrecognised,
    };
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // foundation component transport priority address port -> address is index 4
    let Some(address) = fields.get(4) else {
        return CandidateClass::Unrecognised;
    };
    if address.parse::<IpAddr>().is_ok() {
        return CandidateClass::IpLiteral;
    }
    let lower = address.to_ascii_lowercase();
    if lower.ends_with(".local") {
        return CandidateClass::LocalHostname((*address).to_string());
    }
    CandidateClass::Unrecognised
}

/// Resolve a candidate to an IP-literal candidate string, using the injected resolver only for
/// `.local` hostnames. IP literals are returned unchanged and cause **no** resolver call.
pub fn resolve_candidate(
    candidate: &str,
    resolver: &dyn MdnsResolver,
    limits: &mut MdnsLimits,
) -> Result<String, MediaFailureCode> {
    match classify_candidate(candidate) {
        CandidateClass::IpLiteral => Ok(candidate.to_string()),
        CandidateClass::Unrecognised => Err(MediaFailureCode::MdnUnresolved),
        CandidateClass::LocalHostname(hostname) => {
            if limits.max_resolves == 0 {
                return Err(MediaFailureCode::MdnUnresolved);
            }
            limits.max_resolves -= 1;
            let ip = resolver.resolve(&hostname)?;
            Ok(replace_candidate_address(candidate, &ip.to_string()))
        }
    }
}

/// Replace the address field (index 4) of a candidate line, preserving everything else.
fn replace_candidate_address(candidate: &str, new_address: &str) -> String {
    let Some(rest) = candidate.strip_prefix("candidate:") else {
        return candidate.to_string();
    };
    let mut fields: Vec<String> = rest.split_whitespace().map(str::to_string).collect();
    if fields.len() > 4 {
        fields[4] = new_address.to_string();
    }
    format!("candidate:{}", fields.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingResolver {
        calls: AtomicUsize,
        result: Result<IpAddr, MediaFailureCode>,
    }

    impl MdnsResolver for CountingResolver {
        fn resolve(&self, _hostname: &str) -> Result<IpAddr, MediaFailureCode> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result
        }
    }

    fn ip(v: &str) -> IpAddr {
        v.parse().unwrap()
    }

    #[test]
    fn valid_selected_lan_ip_candidate_is_accepted_without_resolver_call() {
        let resolver = CountingResolver {
            calls: AtomicUsize::new(0),
            result: Ok(ip("192.168.1.9")),
        };
        let mut limits = MdnsLimits::default();
        let candidate = "candidate:1 1 udp 2122260223 192.168.1.5 50000 typ host";
        assert_eq!(
            resolve_candidate(candidate, &resolver, &mut limits),
            Ok(candidate.to_string())
        );
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn local_hostname_uses_resolver_and_rewrites_address() {
        let resolver = CountingResolver {
            calls: AtomicUsize::new(0),
            result: Ok(ip("192.168.1.77")),
        };
        let mut limits = MdnsLimits::default();
        let candidate = "candidate:1 1 udp 2122260223 3f2a.local 50000 typ host";
        let resolved = resolve_candidate(candidate, &resolver, &mut limits).unwrap();
        assert!(resolved.contains("192.168.1.77"));
        assert!(!resolved.contains(".local"));
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn mdn_resolution_failure_is_visible_and_escalates() {
        let resolver = CountingResolver {
            calls: AtomicUsize::new(0),
            result: Err(MediaFailureCode::MdnUnresolved),
        };
        let mut limits = MdnsLimits::default();
        let candidate = "candidate:1 1 udp 2122260223 9ab1.local 50000 typ host";
        assert_eq!(
            resolve_candidate(candidate, &resolver, &mut limits),
            Err(MediaFailureCode::MdnUnresolved)
        );
    }

    #[test]
    fn unrecognised_candidate_is_rejected() {
        let resolver = CountingResolver {
            calls: AtomicUsize::new(0),
            result: Ok(ip("192.168.1.9")),
        };
        let mut limits = MdnsLimits::default();
        assert_eq!(
            resolve_candidate("not-a-candidate", &resolver, &mut limits),
            Err(MediaFailureCode::MdnUnresolved)
        );
    }
}
