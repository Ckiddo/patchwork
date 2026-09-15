//! Authoritative game commands run under the same participant/room locks as room mutations.
use super::{Database, StoreError as E, friends::*, transaction::TxError};
use game_core::{
    BoardPosition, Seat,
    actions::{ActionError, ActionEvent, GameAction},
    geometry::{Orientation, PlacementError},
    rules::{CUSTOM_RULES_VERSION, PatchId},
    state::{GameSnapshot, Lifecycle, Outcome as GameOutcome, ResultReason},
};
use prost::Message;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use util_lib::protocol::v1::{self, game_request::Action};
use uuid::Uuid;

pub fn fingerprint(user: Uuid, request: &v1::GameRequest) -> Vec<u8> {
    let mut hash = Sha256::new();
    hash.update(b"patchwork-game-command-v1");
    hash.update(user.as_bytes());
    hash.update(request.encode_to_vec());
    hash.finalize().to_vec()
}

pub(crate) fn core_error(error: ActionError) -> E {
    match error {
        ActionError::UnknownPlayer => E::Permission,
        ActionError::NotYourTurn => E::NotYourTurn,
        ActionError::GameFinished | ActionError::NotRunning => E::GameNotRunning,
        ActionError::WrongPhase => E::WrongActionPhase,
        ActionError::InsufficientButtons { .. } => E::InsufficientButtons,
        ActionError::Placement(
            PlacementError::Overlap | PlacementError::OutOfBounds | PlacementError::AlreadyPlaced,
        ) => E::InvalidPlacement,
        ActionError::Placement(_) => E::InvalidInput,
        ActionError::InvalidState(_) | ActionError::ArithmeticOverflow => E::Unavailable,
    }
}
pub(crate) fn decode(game: &GameView) -> Result<GameSnapshot, E> {
    if game.rules != CUSTOM_RULES_VERSION {
        return Err(E::RulesNotImplemented);
    }
    let state: GameSnapshot =
        serde_json::from_value(game.state.clone()).map_err(|_| E::Unavailable)?;
    if state.game_id() != game.id.to_string() || phase(&state) != game.phase {
        return Err(E::Unavailable);
    }
    Ok(state)
}
pub(crate) fn phase(state: &GameSnapshot) -> &'static str {
    match state.lifecycle() {
        Lifecycle::Running => "playing",
        Lifecycle::Paused => "paused",
        Lifecycle::Finished
            if state
                .result()
                .is_some_and(|r| r.reason == ResultReason::Abandoned) =>
        {
            "abandoned"
        }
        Lifecycle::Finished => "finished",
    }
}
fn position(value: Option<&v1::BoardPosition>) -> Result<BoardPosition, E> {
    let p = value.ok_or(E::InvalidInput)?;
    BoardPosition::new(p.x, p.y).map_err(|_| E::InvalidPlacement)
}
pub fn action(request: &v1::GameRequest) -> Result<Option<GameAction>, E> {
    Ok(Some(
        match request.action.as_ref().ok_or(E::InvalidInput)? {
            Action::Advance(_) => GameAction::Advance,
            Action::PlaceSpecialPatch(p) => GameAction::PlaceSpecialPatch {
                position: position(Some(p))?,
            },
            Action::BuyAndPlace(p) => {
                let id: u32 = p.patch_id.parse().map_err(|_| E::InvalidInput)?;
                if p.patch_id != id.to_string() {
                    return Err(E::InvalidInput);
                }
                let turns = u8::try_from(p.quarter_turns).map_err(|_| E::InvalidInput)?;
                GameAction::BuyAndPlace {
                    patch_id: PatchId(id),
                    position: position(p.position.as_ref())?,
                    orientation: Orientation::new(turns, p.flipped).map_err(|_| E::InvalidInput)?,
                }
            }
            Action::Resign(_) => return Ok(None),
        },
    ))
}

pub(super) async fn receipt(
    c: &mut PgConnection,
    m: &RoomMutation,
    request: &v1::GameRequest,
) -> Result<Option<Outcome>, TxError> {
    let game = Uuid::parse_str(&request.game_id).map_err(|_| E::InvalidInput)?;
    let row = sqlx::query("SELECT payload_hash,response FROM patchwork.command_receipts WHERE game_id=$1 AND user_id=$2 AND request_id=$3")
        .bind(game).bind(m.permit.user).bind(&m.request_id).fetch_optional(c).await?;
    row.map(|r| {
        if r.get::<Vec<u8>, _>("payload_hash") != m.fingerprint {
            return Err(E::RequestIdConflict.into());
        }
        serde_json::from_value(r.get("response")).map_err(|_| E::Unavailable.into())
    })
    .transpose()
}

/// State, event batch and result persist together; caller holds the room lock and bumps room version.
pub(crate) async fn persist(
    c: &mut PgConnection,
    game: &GameView,
    state: &GameSnapshot,
    events: &[ActionEvent],
    user: Option<Uuid>,
    source: &str,
    result_reason: Option<&str>,
) -> Result<(), TxError> {
    let version = game.version.checked_add(1).ok_or(E::InvalidInput)?;
    let seq = game.seq.checked_add(1).ok_or(E::InvalidInput)?;
    let value = serde_json::to_value(state).map_err(|_| E::Unavailable)?;
    let changed = sqlx::query("UPDATE patchwork.games SET snapshot=$2,phase=$3,state_version=$4,event_seq=$5,updated_at=now() WHERE game_id=$1 AND state_version=$6")
        .bind(game.id).bind(&value).bind(phase(state)).bind(version).bind(seq).bind(game.version).execute(&mut *c).await?;
    if changed.rows_affected() != 1 {
        return Err(E::VersionConflict.into());
    }
    // One atomic transition = one seq/version, even if it crosses multiple markers.
    sqlx::query("INSERT INTO patchwork.game_events(game_id,seq,state_version,user_id,payload) VALUES($1,$2,$3,$4,$5)")
        .bind(game.id).bind(seq).bind(version).bind(user)
        .bind(json!({"kind":"game_transition_v1","game_id":game.id,"rules_version":game.rules,"source":source,"phase":phase(state),"events":events,"state":value}))
        .execute(&mut *c).await?;
    if let Some(result) = state.result() {
        let winner = match result.outcome {
            GameOutcome::Won { winner } => Some(winner.index() as i16),
            _ => None,
        };
        let reason = result_reason.ok_or(E::Unavailable)?;
        let score0 = i32::try_from(result.scores[0].total).map_err(|_| E::Unavailable)?;
        let score1 = i32::try_from(result.scores[1].total).map_err(|_| E::Unavailable)?;
        sqlx::query("INSERT INTO patchwork.game_results(game_id,score0,score1,winner_seat,reason) VALUES($1,$2,$3,$4,$5)")
            .bind(game.id).bind(score0).bind(score1).bind(winner).bind(reason).execute(&mut *c).await?;
    }
    Ok(())
}

pub(super) async fn execute(
    c: &mut PgConnection,
    m: &RoomMutation,
    request: &v1::GameRequest,
    presence: &super::recovery::Presence,
) -> Result<Outcome, TxError> {
    let id = Uuid::parse_str(&request.game_id).map_err(|_| E::InvalidInput)?;
    if m.fingerprint != fingerprint(m.permit.user, request)
        || m.expected_version
            != i64::try_from(request.expected_version).map_err(|_| E::InvalidInput)?
    {
        return Err(E::InvalidInput.into());
    }
    let row = sqlx::query(
        "SELECT room_id,player0,player1 FROM patchwork.games WHERE game_id=$1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *c)
    .await?
    .ok_or(E::NotFound)?;
    let users = [row.get::<Uuid, _>("player0"), row.get::<Uuid, _>("player1")];
    if row.get::<Uuid, _>("room_id") != m.room || !users.contains(&m.permit.user) {
        return Err(E::Permission.into());
    }
    if let Some(original) = receipt(c, m, request).await? {
        return Ok(original);
    }
    let game = read_game(c, Some(id)).await?.ok_or(E::NotFound)?;
    let state = decode(&game)?;
    if game.version != m.expected_version {
        return Err(E::VersionConflict.into());
    }
    if users.iter().enumerate().any(|(seat, user)| {
        state
            .player(if seat == 0 { Seat::First } else { Seat::Second })
            .user_id()
            != user.to_string()
    }) {
        return Err(E::Unavailable.into());
    }
    let command = action(request)?;
    let transition = if let Some(command) = command {
        if !presence.stable_for(&users) {
            return Err(E::Unavailable.into());
        }
        for user in users {
            if presence.synchronized.get(&user) != Some(&id) {
                return Err(E::SyncRequired.into());
            }
            let permit = presence.online.get(&user).ok_or(E::SyncRequired)?;
            fence(c, permit).await?;
        }
        state
            .apply_action(&m.permit.user.to_string(), command)
            .map_err(core_error)?
    } else {
        let (player, _) = state
            .perspective(&m.permit.user.to_string())
            .ok_or(E::Permission)?;
        state
            .terminate(ResultReason::Forfeit {
                loser: player.seat(),
            })
            .map_err(core_error)?
    };
    let reason = transition.state.result().map(|_| {
        if command.is_none() {
            "resigned"
        } else {
            "completed"
        }
    });
    persist(
        c,
        &game,
        &transition.state,
        &transition.events,
        Some(m.permit.user),
        if command.is_none() {
            "resign"
        } else {
            "action"
        },
        reason,
    )
    .await?;
    let changed = sqlx::query("UPDATE patchwork.rooms SET version=version+1,phase=CASE WHEN $2 THEN 'finished' ELSE phase END,updated_at=now() WHERE room_id=$1 AND version<9223372036854775807")
        .bind(m.room).bind(transition.state.result().is_some()).execute(&mut *c).await?;
    if changed.rows_affected() != 1 {
        return Err(E::VersionConflict.into());
    }
    if !m.permit.live() || (command.is_some() && !presence.stable_for(&users)) {
        return Err(E::Permission.into());
    }
    let room = read_room(c, m.room).await?;
    let game = read_game(c, Some(id)).await?;
    let outcome = Outcome {
        room,
        game,
        recipients: users.to_vec(),
    };
    sqlx::query("INSERT INTO patchwork.command_receipts(game_id,user_id,request_id,payload_hash,state_version,response) VALUES($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(m.permit.user).bind(&m.request_id).bind(&m.fingerprint).bind(m.expected_version+1)
        .bind(serde_json::to_value(&outcome).map_err(|_| E::Unavailable)?).execute(c).await?;
    Ok(outcome)
}

impl Database {
    pub async fn game_room(&self, game: Uuid) -> Result<Uuid, E> {
        sqlx::query_scalar("SELECT room_id FROM patchwork.games WHERE game_id=$1")
            .bind(game)
            .fetch_optional(self.pool())
            .await
            .map_err(|_| E::Unavailable)?
            .ok_or(E::NotFound)
    }
}
