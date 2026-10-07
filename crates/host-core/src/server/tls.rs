//! TLS identity loading.
//!
//! The host serves WSS/HTTPS with an operator-configured certificate. A missing or unreadable
//! certificate/key is a [`ControlError::TlsIdentityInvalid`], distinct from the other structured
//! errors, so the operator is told to reconfigure rather than silently falling back to plaintext.

use std::path::{Path, PathBuf};

use axum_server::tls_rustls::RustlsConfig;

use crate::contract::ControlError;

/// Paths to a PEM certificate chain and private key.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TlsIdentity {
    /// Certificate chain path.
    pub certificate_path: PathBuf,
    /// Private key path.
    pub key_path: PathBuf,
}

impl TlsIdentity {
    /// Build an identity from operator-provided paths.
    pub fn new(certificate_path: impl Into<PathBuf>, key_path: impl Into<PathBuf>) -> Self {
        Self {
            certificate_path: certificate_path.into(),
            key_path: key_path.into(),
        }
    }

    /// Load the identity into an axum-server rustls config.
    ///
    /// Never falls back to plaintext; any read/parse failure is `TlsIdentityInvalid`.
    pub async fn load(&self) -> Result<RustlsConfig, ControlError> {
        self.validate_paths()?;
        RustlsConfig::from_pem_file(&self.certificate_path, &self.key_path)
            .await
            .map_err(|_| ControlError::TlsIdentityInvalid)
    }

    /// Validate that both files exist and are readable before attempting a parse.
    pub fn validate_paths(&self) -> Result<(), ControlError> {
        for path in [&self.certificate_path, &self.key_path] {
            if !is_readable_file(path) {
                return Err(ControlError::TlsIdentityInvalid);
            }
        }
        Ok(())
    }
}

fn is_readable_file(path: &Path) -> bool {
    std::fs::File::open(path).is_ok()
}
