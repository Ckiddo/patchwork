//! Transport contract. Decoding validates framing, not identity or game legality.

use prost::Message;

pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/patchwork.v1.rs"));
}

pub const VERSION: u32 = 1;
pub const MAX_CLIENT_MESSAGE_BYTES: usize = 16 * 1024;
pub const MAX_SERVER_MESSAGE_BYTES: usize = 64 * 1024;
pub const DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/protocol.bin"));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProtocolError(pub v1::ErrorCode);

impl ProtocolError {
    pub fn response(self) -> v1::ServerEnvelope {
        // Untrusted request IDs and payloads are deliberately not reflected.
        v1::ServerEnvelope {
            protocol_version: VERSION,
            request_id: None,
            event_seq: 0,
            payload: Some(v1::server_envelope::Payload::Error(v1::ErrorResponse {
                code: self.0 as i32,
                retryable: false,
            })),
        }
    }
}

pub fn decode_client(bytes: &[u8]) -> Result<v1::ClientEnvelope, ProtocolError> {
    use v1::{ErrorCode as E, client_envelope::Payload};
    if bytes.len() > MAX_CLIENT_MESSAGE_BYTES {
        return Err(ProtocolError(E::MessageTooLarge));
    }
    let msg = v1::ClientEnvelope::decode(bytes).map_err(|_| ProtocolError(E::MalformedMessage))?;
    if msg.protocol_version != VERSION {
        return Err(ProtocolError(E::UnsupportedVersion));
    }
    if msg.request_id.is_empty()
        || msg.request_id.len() > 64
        || !msg
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(ProtocolError(E::InvalidRequest));
    }
    match msg.payload.as_ref() {
        None => return Err(ProtocolError(E::InvalidRequest)),
        Some(Payload::Lobby(r)) if r.command.is_none() => {
            return Err(ProtocolError(E::InvalidRequest));
        }
        Some(Payload::Matchmaking(r)) if r.command.is_none() => {
            return Err(ProtocolError(E::InvalidRequest));
        }
        Some(Payload::Game(r)) if r.action.is_none() => {
            return Err(ProtocolError(E::InvalidRequest));
        }
        Some(Payload::Authenticate(r)) if r.access_token.is_empty() => {
            return Err(ProtocolError(E::InvalidRequest));
        }
        _ => {}
    }
    Ok(msg)
}
