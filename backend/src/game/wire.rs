use crate::persistence::{StoreError as E, friends::*};
use std::collections::HashMap;
use util_lib::protocol::{
    VERSION,
    v1::{self, server_envelope::Payload},
};
use uuid::Uuid;
pub fn room(r: &RoomView, online: &HashMap<Uuid, Permit>) -> v1::RoomSnapshot {
    v1::RoomSnapshot {
        room_id: r.id.to_string(),
        version: r.version as u64,
        phase: match r.phase.as_str() {
            "waiting" => v1::RoomPhase::Waiting,
            "starting" => v1::RoomPhase::Starting,
            "playing" => v1::RoomPhase::Playing,
            "finished" => v1::RoomPhase::Finished,
            _ => v1::RoomPhase::Closed,
        } as i32,
        members: r
            .members
            .iter()
            .map(|m| v1::RoomMember {
                user_id: m.user.to_string(),
                seat: m.seat,
                ready: m.ready,
                connected: online.get(&m.user).is_some_and(Permit::live),
                nickname: m.nickname.clone(),
            })
            .collect(),
        owner_id: r.owner.map(|u| u.to_string()).unwrap_or_default(),
        rules_version: r.rules.clone(),
        game_id: r.game.map(|g| g.to_string()).unwrap_or_default(),
        code: r.code.clone(),
        mode: r.mode.clone(),
        requires_password: r.protected,
        first_player_seat: r.first_player,
    }
}
pub fn game(g: &GameView) -> v1::GameSnapshot {
    v1::GameSnapshot {
        game_id: g.id.to_string(),
        version: g.version as u64,
        event_seq: g.seq as u64,
        rules_version: g.rules.clone(),
        state_json: g.state.to_string().into_bytes(),
        phase: g.phase.clone(),
    }
}
pub fn envelope(id: Option<String>, payload: Payload) -> v1::ServerEnvelope {
    v1::ServerEnvelope {
        protocol_version: VERSION,
        request_id: id,
        event_seq: 0,
        payload: Some(payload),
    }
}
pub fn error(error: E) -> Payload {
    use v1::ErrorCode as C;
    let code = match error {
        E::InvalidInput => C::InvalidRequest,
        E::Permission => C::Forbidden,
        E::NotFound => C::NotFound,
        E::VersionConflict => C::VersionConflict,
        E::RequestIdConflict => C::RequestIdConflict,
        E::RoomFull => C::RoomFull,
        E::RoomNotJoinable => C::RoomNotJoinable,
        E::NotEnoughPlayers => C::NotEnoughPlayers,
        E::NotReady => C::NotReady,
        E::PlayerBusy => C::PlayerBusy,
        E::BadPassword => C::BadPassword,
        E::RulesNotImplemented => C::NotImplemented,
        E::SyncRequired => C::SyncRequired,
        E::GameNotRunning => C::GameNotRunning,
        E::NotYourTurn => C::NotYourTurn,
        E::InvalidPlacement => C::InvalidPlacement,
        E::InsufficientButtons => C::InsufficientButtons,
        E::WrongActionPhase => C::WrongActionPhase,
        E::Conflict => C::RoomNotJoinable,
        _ => C::ServiceUnavailable,
    };
    Payload::Error(v1::ErrorResponse {
        code: code as i32,
        retryable: matches!(code, C::ServiceUnavailable | C::PlayerBusy),
    })
}
