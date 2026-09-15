//! Validated, serializable domain state. No network connections, clocks, RNG or rendering IDs.
//! Deserialization checks structural invariants, transformed shapes and bonus square evidence.
mod supply;
#[cfg(test)]
mod tests;
pub use supply::{NeutralPosition, SupplyState};

use crate::{
    BOARD_SIZE, BoardPosition, PLAYER_COUNT, Seat,
    rules::{CUSTOM_RULES_VERSION, CUSTOM_V1, PATCH_COUNT, PatchId, patch, registry::*},
};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{BTreeSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StateError {
    InvalidMetadata,
    InvalidIdentity,
    InvalidPlayer,
    InvalidBoard,
    InvalidBonus,
    InvalidSupply,
    NotCandidate,
    InvalidPartition,
    InvalidSpecialPatches,
    InvalidActionState,
    InvalidResult,
}
impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid game state: {self:?}")
    }
}
impl std::error::Error for StateError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PieceId {
    Normal(PatchId),
    Special(u8),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuiltBoard {
    pub(crate) cells: [[Option<PieceId>; BOARD_SIZE as usize]; BOARD_SIZE as usize],
}
impl Default for QuiltBoard {
    fn default() -> Self {
        Self {
            cells: [[None; BOARD_SIZE as usize]; BOARD_SIZE as usize],
        }
    }
}
impl QuiltBoard {
    pub fn at(&self, position: BoardPosition) -> Option<PieceId> {
        let (x, y) = position.coordinates();
        self.cells[usize::from(y)][usize::from(x)]
    }
    pub fn occupied_count(&self) -> usize {
        self.cells
            .iter()
            .flatten()
            .filter(|cell| cell.is_some())
            .count()
    }
    pub fn is_full(&self) -> bool {
        self.occupied_count() == usize::from(BOARD_SIZE).pow(2)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlacedPiece {
    pub(crate) piece: PieceId,
    pub(crate) cells: Vec<BoardPosition>,
    pub(crate) quarter_turns: u8,
    pub(crate) flipped: bool,
}
impl PlacedPiece {
    pub fn piece(&self) -> PieceId {
        self.piece
    }
    pub fn cells(&self) -> &[BoardPosition] {
        &self.cells
    }
    pub fn orientation(&self) -> (u8, bool) {
        (self.quarter_turns, self.flipped)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayerState {
    pub(crate) user_id: String,
    pub(crate) seat: Seat,
    pub(crate) board: QuiltBoard,
    pub(crate) placed_pieces: Vec<PlacedPiece>,
    pub(crate) buttons: u32,
    pub(crate) income: u32,
    pub(crate) time_position: u8,
}
impl PlayerState {
    pub fn user_id(&self) -> &str {
        &self.user_id
    }
    pub fn seat(&self) -> Seat {
        self.seat
    }
    pub fn board(&self) -> &QuiltBoard {
        &self.board
    }
    pub fn placed_pieces(&self) -> &[PlacedPiece] {
        &self.placed_pieces
    }
    pub fn buttons(&self) -> u32 {
        self.buttons
    }
    pub fn income(&self) -> u32 {
        self.income
    }
    pub fn time_position(&self) -> u8 {
        self.time_position
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscardReason {
    BoardFull,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpecialPatchStatus {
    Available,
    Pending {
        owner: Seat,
    },
    Placed {
        owner: Seat,
        position: BoardPosition,
    },
    Discarded {
        owner: Seat,
        reason: DiscardReason,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpecialPatchState {
    pub(crate) track_position: u8,
    pub(crate) status: SpecialPatchStatus,
}
impl SpecialPatchState {
    pub fn track_position(&self) -> u8 {
        self.track_position
    }
    pub fn status(&self) -> SpecialPatchStatus {
        self.status
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSpecialPatch {
    pub owner: Seat,
    pub track_position: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionState {
    pub(crate) last_normal_actor: Option<Seat>,
    pub(crate) pending_specials: VecDeque<PendingSpecialPatch>,
}
impl ActionState {
    pub fn last_normal_actor(&self) -> Option<Seat> {
        self.last_normal_actor
    }
    pub fn pending_specials(&self) -> &VecDeque<PendingSpecialPatch> {
        &self.pending_specials
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Running,
    Paused,
    Finished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionPhase {
    Normal { actor: Seat },
    Special { actor: Seat, track_position: u8 },
    AwaitingScoring,
    Finished,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BonusState {
    pub(crate) owner: Option<Seat>,
}
impl BonusState {
    pub fn owner(&self) -> Option<Seat> {
        self.owner
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScoreBreakdown {
    pub buttons: u32,
    pub bonus_points: u8,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResultReason {
    Scored,
    Forfeit { loser: Seat },
    Abandoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Won { winner: Seat },
    Draw,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameResult {
    pub scores: [ScoreBreakdown; PLAYER_COUNT],
    pub reason: ResultReason,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SnapshotData {
    pub kind: String,
    pub schema_version: u32,
    pub rules_version: String,
    pub game_id: String,
    pub first_player_seat: Seat,
    pub players: [PlayerState; PLAYER_COUNT],
    pub supply: SupplyState,
    pub special_patches: Vec<SpecialPatchState>,
    pub action: ActionState,
    pub bonus: BonusState,
    pub lifecycle: Lifecycle,
    pub result: Option<GameResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GameSnapshot {
    pub(crate) data: SnapshotData,
}

impl<'de> Deserialize<'de> for GameSnapshot {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let state = Self {
            data: SnapshotData::deserialize(deserializer)?,
        };
        state.validate().map_err(serde::de::Error::custom)?;
        Ok(state)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

impl GameSnapshot {
    pub fn new(
        game_id: String,
        user_ids: [String; PLAYER_COUNT],
        first_player: Seat,
        order: Vec<PatchId>,
    ) -> Result<Self, StateError> {
        let [first, second] = user_ids;
        let player = |user_id, seat| PlayerState {
            user_id,
            seat,
            board: QuiltBoard::default(),
            placed_pieces: Vec::new(),
            buttons: u32::from(CUSTOM_V1.initial_buttons),
            income: 0,
            time_position: CUSTOM_V1.track.start,
        };
        let state = Self {
            data: SnapshotData {
                kind: GAME_SNAPSHOT_KIND.into(),
                schema_version: GAME_SNAPSHOT_SCHEMA,
                rules_version: CUSTOM_RULES_VERSION.into(),
                game_id,
                first_player_seat: first_player,
                players: [player(first, Seat::First), player(second, Seat::Second)],
                supply: SupplyState::new(order)?,
                special_patches: CUSTOM_V1
                    .track
                    .special_positions
                    .iter()
                    .map(|&track_position| SpecialPatchState {
                        track_position,
                        status: SpecialPatchStatus::Available,
                    })
                    .collect(),
                action: ActionState::default(),
                bonus: BonusState::default(),
                lifecycle: Lifecycle::Running,
                result: None,
            },
        };
        state.validate()?;
        Ok(state)
    }

    pub fn game_id(&self) -> &str {
        &self.data.game_id
    }
    pub fn first_player(&self) -> Seat {
        self.data.first_player_seat
    }
    pub fn player(&self, seat: Seat) -> &PlayerState {
        &self.data.players[seat.index()]
    }
    /// (own, opponent): the renderer maps these to (right, left).
    pub fn perspective(&self, user_id: &str) -> Option<(&PlayerState, &PlayerState)> {
        self.data
            .players
            .iter()
            .find(|p| p.user_id == user_id)
            .map(|p| (p, self.player(p.seat.other())))
    }
    pub fn supply(&self) -> &SupplyState {
        &self.data.supply
    }
    pub fn action(&self) -> &ActionState {
        &self.data.action
    }
    pub fn special_patches(&self) -> &[SpecialPatchState] {
        &self.data.special_patches
    }
    pub fn bonus(&self) -> BonusState {
        self.data.bonus
    }
    pub fn lifecycle(&self) -> Lifecycle {
        self.data.lifecycle
    }
    pub fn result(&self) -> Option<&GameResult> {
        self.data.result.as_ref()
    }

    /// Domain phase is independent of connection pause. Special placements precede scoring.
    pub fn action_phase(&self) -> ActionPhase {
        if self.data.result.is_some() {
            return ActionPhase::Finished;
        }
        if let Some(pending) = self.data.action.pending_specials.front() {
            return ActionPhase::Special {
                actor: pending.owner,
                track_position: pending.track_position,
            };
        }
        let [a, b] = self.data.players.each_ref().map(|p| p.time_position);
        if a == CUSTOM_V1.track.end && b == CUSTOM_V1.track.end {
            return ActionPhase::AwaitingScoring;
        }
        let actor = match a.cmp(&b) {
            std::cmp::Ordering::Less => Seat::First,
            std::cmp::Ordering::Greater => Seat::Second,
            std::cmp::Ordering::Equal => CUSTOM_V1.same_position_turn.actor(
                self.data.action.last_normal_actor,
                self.data.first_player_seat,
            ),
        };
        ActionPhase::Normal { actor }
    }

    pub fn input_actor(&self) -> Option<Seat> {
        if self.data.lifecycle != Lifecycle::Running {
            return None;
        }
        match self.action_phase() {
            ActionPhase::Normal { actor } | ActionPhase::Special { actor, .. } => Some(actor),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), StateError> {
        let s = &self.data;
        if snapshot_format(&s.rules_version, &s.kind, Some(s.schema_version))
            != Ok(SnapshotFormat::CustomV1)
        {
            return Err(StateError::InvalidMetadata);
        }
        if !valid_id(&s.game_id)
            || s.players.iter().any(|p| !valid_id(&p.user_id))
            || s.players[0].user_id == s.players[1].user_id
        {
            return Err(StateError::InvalidIdentity);
        }
        s.supply.validate()?;
        let mut normal_ids: BTreeSet<_> = s.supply.remaining.iter().flatten().copied().collect();
        let mut placed_specials = BTreeSet::new();
        for (index, player) in s.players.iter().enumerate() {
            if player.seat.index() != index || player.time_position > CUSTOM_V1.track.end {
                return Err(StateError::InvalidPlayer);
            }
            let mut board = QuiltBoard::default();
            let mut income = 0;
            for placed in &player.placed_pieces {
                if !placed.geometry_is_valid() {
                    return Err(StateError::InvalidBoard);
                }
                let size = match placed.piece {
                    PieceId::Normal(id) => {
                        let definition = patch(id).ok_or(StateError::InvalidBoard)?;
                        if !normal_ids.insert(id) {
                            return Err(StateError::InvalidPartition);
                        }
                        income += u32::from(definition.income);
                        definition.cells.len()
                    }
                    PieceId::Special(position) => {
                        if !CUSTOM_V1.track.special_positions.contains(&position)
                            || !placed_specials.insert(position)
                            || placed.quarter_turns != 0
                            || placed.flipped
                        {
                            return Err(StateError::InvalidSpecialPatches);
                        }
                        1
                    }
                };
                if placed.cells.len() != size {
                    return Err(StateError::InvalidBoard);
                }
                for &cell in &placed.cells {
                    let (x, y) = cell.coordinates();
                    if board.cells[usize::from(y)][usize::from(x)]
                        .replace(placed.piece)
                        .is_some()
                    {
                        return Err(StateError::InvalidBoard);
                    }
                }
            }
            if board != player.board || income != player.income {
                return Err(StateError::InvalidBoard);
            }
        }
        if normal_ids.len() != PATCH_COUNT {
            return Err(StateError::InvalidPartition);
        }
        match s.bonus.owner {
            Some(owner) if self.player(owner).board.completed_bonus_square().is_none() => {
                return Err(StateError::InvalidBonus);
            }
            None if s
                .players
                .iter()
                .any(|p| p.board.completed_bonus_square().is_some()) =>
            {
                return Err(StateError::InvalidBonus);
            }
            _ => {}
        }
        self.validate_specials(&placed_specials)?;
        if s.action.last_normal_actor.is_none()
            && (s.players.iter().any(|p| {
                p.time_position != CUSTOM_V1.track.start
                    || !p.placed_pieces.is_empty()
                    || p.buttons != u32::from(CUSTOM_V1.initial_buttons)
            }) || s.bonus.owner.is_some())
        {
            return Err(StateError::InvalidActionState);
        }
        self.validate_result()
    }

    fn validate_specials(&self, placed: &BTreeSet<u8>) -> Result<(), StateError> {
        let s = &self.data;
        if s.special_patches.len() != CUSTOM_V1.track.special_positions.len() {
            return Err(StateError::InvalidSpecialPatches);
        }
        let max_time = s.players.iter().map(|p| p.time_position).max().unwrap_or(0);
        let mut expected_pending = VecDeque::new();
        for (item, &position) in s
            .special_patches
            .iter()
            .zip(CUSTOM_V1.track.special_positions)
        {
            if item.track_position != position {
                return Err(StateError::InvalidSpecialPatches);
            }
            let owner = match item.status {
                SpecialPatchStatus::Available => {
                    if max_time >= position || placed.contains(&position) {
                        return Err(StateError::InvalidSpecialPatches);
                    }
                    continue;
                }
                SpecialPatchStatus::Pending { owner } => {
                    if placed.contains(&position) || s.action.last_normal_actor != Some(owner) {
                        return Err(StateError::InvalidSpecialPatches);
                    }
                    expected_pending.push_back(PendingSpecialPatch {
                        owner,
                        track_position: position,
                    });
                    owner
                }
                SpecialPatchStatus::Placed {
                    owner,
                    position: cell,
                } => {
                    if !placed.contains(&position)
                        || self.player(owner).board.at(cell) != Some(PieceId::Special(position))
                    {
                        return Err(StateError::InvalidSpecialPatches);
                    }
                    owner
                }
                SpecialPatchStatus::Discarded {
                    owner,
                    reason: DiscardReason::BoardFull,
                } => {
                    if placed.contains(&position) || !self.player(owner).board.is_full() {
                        return Err(StateError::InvalidSpecialPatches);
                    }
                    owner
                }
            };
            if self.player(owner).time_position < position {
                return Err(StateError::InvalidSpecialPatches);
            }
        }
        if s.action.pending_specials != expected_pending {
            return Err(StateError::InvalidActionState);
        }
        Ok(())
    }

    fn validate_result(&self) -> Result<(), StateError> {
        let s = &self.data;
        let Some(result) = &s.result else {
            return if s.lifecycle == Lifecycle::Finished {
                Err(StateError::InvalidResult)
            } else {
                Ok(())
            };
        };
        if s.lifecycle != Lifecycle::Finished {
            return Err(StateError::InvalidResult);
        }
        for player in &s.players {
            let score = result.scores[player.seat.index()];
            let owns_bonus = s.bonus.owner == Some(player.seat);
            if score.buttons != player.buttons
                || score.bonus_points
                    != if owns_bonus {
                        CUSTOM_V1.scoring.bonus_points
                    } else {
                        0
                    }
                || score.total != CUSTOM_V1.scoring.final_score(player.buttons, owns_bonus)
            {
                return Err(StateError::InvalidResult);
            }
        }
        let expected = match result.reason {
            ResultReason::Scored => {
                if s.players
                    .iter()
                    .any(|p| p.time_position != CUSTOM_V1.track.end)
                    || !s.action.pending_specials.is_empty()
                {
                    return Err(StateError::InvalidResult);
                }
                match result.scores[0].total.cmp(&result.scores[1].total) {
                    std::cmp::Ordering::Greater => Outcome::Won {
                        winner: Seat::First,
                    },
                    std::cmp::Ordering::Less => Outcome::Won {
                        winner: Seat::Second,
                    },
                    std::cmp::Ordering::Equal => Outcome::Draw,
                }
            }
            ResultReason::Forfeit { loser } => Outcome::Won {
                winner: loser.other(),
            },
            ResultReason::Abandoned => Outcome::Abandoned,
        };
        if result.outcome != expected {
            return Err(StateError::InvalidResult);
        }
        Ok(())
    }
}
