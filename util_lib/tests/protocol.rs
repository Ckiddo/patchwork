use prost::Message;
use util_lib::protocol::{
    self, ProtocolError,
    v1::{self, client_envelope::Payload},
};

fn ping() -> v1::ClientEnvelope {
    v1::ClientEnvelope {
        protocol_version: 1,
        request_id: "r1".into(),
        payload: Some(Payload::Ping(v1::Ping { nonce: 42 })),
    }
}

fn error(bytes: &[u8]) -> v1::ErrorCode {
    match protocol::decode_client(bytes) {
        Err(ProtocolError(code)) => code,
        Ok(_) => panic!("invalid wire message was accepted"),
    }
}

#[test]
fn v1_golden_ping_preserves_envelope_field_numbers() {
    let golden = [0x08, 0x01, 0x12, 0x02, b'r', b'1', 0x5a, 0x02, 0x08, 0x2a];
    assert_eq!(ping().encode_to_vec(), golden);
    let decoded = protocol::decode_client(&golden).unwrap();
    assert_eq!(decoded.request_id, "r1");
    assert!(matches!(decoded.payload, Some(Payload::Ping(p)) if p.nonce == 42));
}

#[test]
fn rejects_bad_frames_unknown_commands_and_unsupported_versions() {
    use v1::ErrorCode as E;
    assert_eq!(error(&[0xff]), E::MalformedMessage);
    assert_eq!(error(&[0x08, 1, 0x12, 50, b'x']), E::MalformedMessage);
    assert_eq!(
        error(&vec![0; protocol::MAX_CLIENT_MESSAGE_BYTES + 1]),
        E::MessageTooLarge
    );
    let mut msg = ping();
    msg.protocol_version = 2;
    assert_eq!(error(&msg.encode_to_vec()), E::UnsupportedVersion);
    msg.protocol_version = 1;
    msg.payload = None;
    let mut unknown = msg.encode_to_vec();
    unknown.extend_from_slice(&[0x9a, 0x06, 0x00]); // unknown field 99, length-delimited
    assert_eq!(error(&unknown), E::InvalidRequest);
    msg.payload = Some(Payload::Lobby(v1::LobbyRequest::default()));
    assert_eq!(error(&msg.encode_to_vec()), E::InvalidRequest);
    msg = ping();
    msg.request_id = "unsafe\nrequest".into();
    assert_eq!(error(&msg.encode_to_vec()), E::InvalidRequest);
}

#[test]
fn additive_unknown_fields_are_compatible_and_u64_is_lossless() {
    let mut msg = ping();
    msg.payload = Some(Payload::Ping(v1::Ping { nonce: u64::MAX }));
    let mut wire = msg.encode_to_vec();
    wire.extend_from_slice(&[0xa0, 0x06, 0x01]); // unknown field 100
    let decoded = protocol::decode_client(&wire).unwrap();
    assert!(matches!(decoded.payload, Some(Payload::Ping(p)) if p.nonce == u64::MAX));
}

#[test]
fn error_response_has_stable_code_and_does_not_reflect_input() {
    let wire = ProtocolError(v1::ErrorCode::MalformedMessage)
        .response()
        .encode_to_vec();
    assert_eq!(wire, [0x08, 0x01, 0x52, 0x02, 0x08, 0x01]);
    let decoded = v1::ServerEnvelope::decode(wire.as_slice()).unwrap();
    assert!(decoded.request_id.is_none());
}

#[test]
fn arbitrary_bounded_inputs_never_panic() {
    let mut seed = 7u32;
    for len in 0..512 {
        let bytes: Vec<_> = (0..len)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 24) as u8
            })
            .collect();
        let _ = protocol::decode_client(&bytes);
    }
}
