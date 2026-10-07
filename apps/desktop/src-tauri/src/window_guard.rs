//! Caller-window authorization for operator IPC commands.
//!
//! Registered Tauri commands are callable from every application window by default. The operator
//! window is the only surface allowed to drive host setup, so each sensitive command checks the
//! caller's window label. This is defense in depth on top of the capability ACL.

/// The label of the bundled operator window.
pub const OPERATOR_WINDOW_LABEL: &str = "operator";

/// Reasons a caller may be rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowDenied {
    /// The caller is not the operator window.
    NotOperatorWindow,
}

/// Authorize a caller by window label.
///
/// Only the exact bundled operator window label is accepted. Musician LAN pages are never a
/// Tauri window, so they can never pass this check.
pub fn authorize_window(label: &str) -> Result<(), WindowDenied> {
    if label == OPERATOR_WINDOW_LABEL {
        Ok(())
    } else {
        Err(WindowDenied::NotOperatorWindow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_command_rejects_non_operator_window_label() {
        assert!(authorize_window("operator").is_ok());
        assert!(authorize_window("other").is_err());
        assert!(authorize_window("join").is_err());
    }
}
