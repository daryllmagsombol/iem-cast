//! Pure SDP validation and Opus format-parameter adaptation.
//!
//! These functions contain no `str0m` session state so they can be unit-tested offline. They
//! enforce the plan's negotiation rules: exactly one receive-only audio section is accepted,
//! extra media is rejected, the answer is send-only, the negotiated payload type is preserved
//! verbatim, and a mono-only Opus format is refused rather than silently downmixed.

use crate::contract::MediaFailureCode;

/// The parsed shape of an inbound SDP offer, limited to what the POC must validate.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParsedOffer {
    pub audio_sections: usize,
    pub non_audio_sections: usize,
    pub audio_directions: Vec<String>,
    pub audio_rtpmap: Vec<(u8, String)>,
    pub audio_fmtp: Vec<String>,
}

/// Minimal line-oriented SDP parse. This is deliberately not a full SDP implementation; it only
/// extracts the facts needed for the validation rules above.
pub fn parse_offer(sdp: &str) -> ParsedOffer {
    let mut audio_sections = 0usize;
    let mut non_audio_sections = 0usize;
    let mut current_is_audio = false;
    let mut audio_directions = Vec::new();
    let mut audio_rtpmap = Vec::new();
    let mut audio_fmtp = Vec::new();

    for raw in sdp.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("m=") {
            if rest.starts_with("audio") {
                audio_sections += 1;
                current_is_audio = true;
            } else {
                non_audio_sections += 1;
                current_is_audio = false;
            }
            continue;
        }
        if !current_is_audio {
            continue;
        }
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            if let Some((pt, rest)) = rest.split_once(' ') {
                if let Ok(pt) = pt.parse::<u8>() {
                    audio_rtpmap.push((pt, rest.trim().to_string()));
                }
            }
        } else if let Some(rest) = line.strip_prefix("a=fmtp:") {
            audio_fmtp.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("a=") {
            match rest.trim() {
                "sendonly" | "recvonly" | "sendrecv" | "inactive" => {
                    audio_directions.push(rest.trim().to_string())
                }
                _ => {}
            }
        }
    }

    ParsedOffer {
        audio_sections,
        non_audio_sections,
        audio_directions,
        audio_rtpmap,
        audio_fmtp,
    }
}

/// Validate an inbound offer. Rejects anything that is not exactly one receive-only audio
/// section; extra media sections are explicitly rejected rather than ignored.
pub fn validate_offer(sdp: &str) -> Result<ParsedOffer, MediaFailureCode> {
    let parsed = parse_offer(sdp);
    if parsed.non_audio_sections > 0 || parsed.audio_sections > 1 {
        return Err(MediaFailureCode::ExtraMediaRejected);
    }
    if parsed.audio_sections == 0 {
        return Err(MediaFailureCode::NoAudioSection);
    }
    // The browser offers `recvonly` (it receives our audio). Reject other directions.
    if parsed.audio_directions.iter().any(|d| d != "recvonly") {
        return Err(MediaFailureCode::NegotiationFailed);
    }
    Ok(parsed)
}

/// Select the negotiated Opus payload type from the offer, **retaining the actual PT**.
pub fn negotiated_opus_pt(offer: &ParsedOffer) -> Result<u8, MediaFailureCode> {
    offer
        .audio_rtpmap
        .iter()
        .find(|(_, name)| name.to_ascii_lowercase().starts_with("opus/"))
        .map(|(pt, _)| *pt)
        .ok_or(MediaFailureCode::NegotiationFailed)
}

/// Whether the Opus fmtp explicitly restricts the stream to mono (`sprop-stereo=0` with no
/// stereo signalling). The POC refuses this rather than silently downmixing.
pub fn is_mono_only_fmtp(offer: &ParsedOffer) -> bool {
    offer
        .audio_fmtp
        .iter()
        .any(|f| f.replace(' ', "").to_ascii_lowercase().contains("sprop-stereo=0"))
}

/// Validate that the negotiated Opus format keeps stereo, returning the PT on success.
///
/// This is a pure adaptation step: it never rewrites the SDP text, never forces a payload type,
/// and never mutates fingerprints or mids.
pub fn negotiate_stereo(offer: &ParsedOffer) -> Result<u8, MediaFailureCode> {
    if is_mono_only_fmtp(offer) {
        return Err(MediaFailureCode::NegotiationFailed);
    }
    negotiated_opus_pt(offer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recvonly_audio_offer(pt: u8) -> String {
        format!(
            "v=0\r\n\
             m=audio 9 UDP/TLS/RTP/SAVPF {pt}\r\n\
             a=recvonly\r\n\
             a=rtpmap:{pt} opus/48000/2\r\n\
             a=fmtp:{pt} minptime=10;useinbandfec=1;sprop-stereo=1\r\n"
        )
    }

    #[test]
    fn offer_is_recvonly_audio_only() {
        let parsed = validate_offer(&recvonly_audio_offer(111)).unwrap();
        assert_eq!(parsed.audio_sections, 1);
        assert_eq!(parsed.audio_directions, vec!["recvonly".to_string()]);
    }

    #[test]
    fn extra_media_section_is_rejected() {
        let mut sdp = recvonly_audio_offer(111);
        sdp.push_str("m=video 9 UDP/TLS/RTP/SAVPF 96\r\na=recvonly\r\n");
        assert_eq!(validate_offer(&sdp), Err(MediaFailureCode::ExtraMediaRejected));
    }

    #[test]
    fn no_audio_section_is_rejected() {
        let sdp = "v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\na=recvonly\r\n";
        assert_eq!(validate_offer(sdp), Err(MediaFailureCode::ExtraMediaRejected));
    }

    #[test]
    fn fmtp_adaptation_retains_actual_payload_type() {
        let parsed = validate_offer(&recvonly_audio_offer(123)).unwrap();
        assert_eq!(negotiated_opus_pt(&parsed), Ok(123));
        assert_eq!(negotiate_stereo(&parsed), Ok(123));
    }

    #[test]
    fn mono_fmtp_is_explicitly_refused() {
        let sdp = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n\
                   a=recvonly\r\na=rtpmap:111 opus/48000/2\r\n\
                   a=fmtp:111 minptime=10;sprop-stereo=0\r\n";
        let parsed = validate_offer(sdp).unwrap();
        assert_eq!(negotiate_stereo(&parsed), Err(MediaFailureCode::NegotiationFailed));
    }
}
