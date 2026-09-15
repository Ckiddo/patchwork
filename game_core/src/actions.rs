//! Pure transitions. A successful result must be persisted atomically by the T38 authority.
//! No request IDs, connection generations or database versions belong in this layer.
use crate::{
    BoardPosition, Seat,
    geometry::{Orientation, PlacementError, PlacementRequest},
    rules::{CUSTOM_V1, PatchId, patch},
    state::{
        ActionPhase, DiscardReason, GameResult, GameSnapshot, Lifecycle, Outcome,
        PendingSpecialPatch, PieceId, PlacedPiece, ResultReason, ScoreBreakdown,
        SpecialPatchStatus, StateError,
    },
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Domain intents, adapted from the existing protocol (not another wire command format).
/// The actor and target board always come from the authenticated user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameAction {
    Advance,
    BuyAndPlace {
        patch_id: PatchId,
        position: BoardPosition,
        orientation: Orientation,
    },
    /// The saved queue determines which special patch this places.
    PlaceSpecialPatch {
        position: BoardPosition,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionError {
    InvalidState(StateError),
    UnknownPlayer,
    NotRunning,
    GameFinished,
    NotYourTurn,
    WrongPhase,
    InsufficientButtons { required: u32, available: u32 },
    Placement(PlacementError),
    ArithmeticOverflow,
}
impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid game action: {self:?}")
    }
}
impl std::error::Error for ActionError {}
impl From<StateError> for ActionError {
    fn from(value: StateError) -> Self {
        Self::InvalidState(value)
    }
}
impl From<PlacementError> for ActionError {
    fn from(value: PlacementError) -> Self {
        Self::Placement(value)
    }
}

/// Ordered facts for receipts, rendering and accounting. T38 supplies persistence envelopes.
/// These facts are output only: the engine never applies untrusted event bodies as commands.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActionEvent {
    PatchPurchased {
        actor: Seat,
        slot: u8,
        button_cost: u32,
        income_added: u32,
        placement: PlacedPiece,
    },
    AdvanceReward {
        actor: Seat,
        amount: u32,
    },
    TimeMoved {
        actor: Seat,
        from: u8,
        to: u8,
    },
    IncomeReceived {
        actor: Seat,
        track_position: u8,
        amount: u32,
    },
    SpecialClaimed {
        owner: Seat,
        track_position: u8,
    },
    SpecialPlaced {
        owner: Seat,
        track_position: u8,
        position: BoardPosition,
    },
    SpecialDiscarded {
        owner: Seat,
        track_position: u8,
        reason: DiscardReason,
    },
    BonusAwarded {
        owner: Seat,
        top_left: BoardPosition,
        points: u8,
    },
    GameFinished {
        result: GameResult,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionTransition {
    pub state: GameSnapshot,
    pub events: Vec<ActionEvent>,
}

impl GameSnapshot {
    /// Connection lifecycle only; callers must authenticate/fence the system transition.
    pub fn with_connection_pause(&self, paused: bool) -> Result<Self, ActionError> {
        self.validate()?;
        if self.result().is_some() {
            return Err(ActionError::GameFinished);
        }
        let mut state = self.clone();
        state.data.lifecycle = if paused {
            Lifecycle::Paused
        } else {
            Lifecycle::Running
        };
        state.validate()?;
        Ok(state)
    }

    /// Trusted authority hook for resignation, disconnect forfeits and abandonment.
    /// Natural scoring must go through actions/settle_automatic instead.
    pub fn terminate(&self, reason: ResultReason) -> Result<ActionTransition, ActionError> {
        self.validate()?;
        if self.result().is_some() {
            return Err(ActionError::GameFinished);
        }
        let outcome = match reason {
            ResultReason::Forfeit { loser } => Outcome::Won {
                winner: loser.other(),
            },
            ResultReason::Abandoned => Outcome::Abandoned,
            ResultReason::Scored => return Err(ActionError::WrongPhase),
        };
        let mut state = self.clone();
        let scores = state.data.players.each_ref().map(|p| {
            let owns_bonus = state.bonus().owner() == Some(p.seat());
            ScoreBreakdown {
                buttons: p.buttons(),
                bonus_points: if owns_bonus {
                    CUSTOM_V1.scoring.bonus_points
                } else {
                    0
                },
                total: CUSTOM_V1.scoring.final_score(p.buttons(), owns_bonus),
            }
        });
        let result = GameResult {
            scores,
            reason,
            outcome,
        };
        state.data.lifecycle = Lifecycle::Finished;
        state.data.result = Some(result.clone());
        state.validate()?;
        Ok(ActionTransition {
            state,
            events: vec![ActionEvent::GameFinished { result }],
        })
    }

    /// Neither success nor failure mutates self. Do not broadcast before committing this result.
    pub fn apply_action(
        &self,
        authenticated_user_id: &str,
        action: GameAction,
    ) -> Result<ActionTransition, ActionError> {
        self.validate()?;
        let (player, _) = self
            .perspective(authenticated_user_id)
            .ok_or(ActionError::UnknownPlayer)?;
        let actor = player.seat();
        if self.result().is_some() {
            return Err(ActionError::GameFinished);
        }
        if self.lifecycle() != Lifecycle::Running {
            return Err(ActionError::NotRunning);
        }
        let phase = self.action_phase();
        match phase {
            ActionPhase::Normal { actor: expected }
            | ActionPhase::Special {
                actor: expected, ..
            } => {
                if actor != expected {
                    return Err(ActionError::NotYourTurn);
                }
            }
            _ => return Err(ActionError::WrongPhase),
        }
        let mut state = self.clone();
        let mut events = Vec::new();
        match (phase, action) {
            (ActionPhase::Normal { .. }, GameAction::Advance) => {
                let old = player.time_position();
                let target = (u16::from(self.player(actor.other()).time_position()) + 1)
                    .min(u16::from(CUSTOM_V1.track.end)) as u8;
                let amount = u32::from(
                    target
                        .checked_sub(old)
                        .ok_or(ActionError::InvalidState(StateError::InvalidActionState))?,
                );
                state.credit(actor, amount)?;
                events.push(ActionEvent::AdvanceReward { actor, amount });
                state.data.action.last_normal_actor = Some(actor);
                state.move_time(actor, u16::from(target), &mut events)?;
            }
            (
                ActionPhase::Normal { .. },
                GameAction::BuyAndPlace {
                    patch_id,
                    position,
                    orientation,
                },
            ) => {
                let preview = self.preview_placement(
                    authenticated_user_id,
                    PlacementRequest {
                        target_seat: actor,
                        piece: PieceId::Normal(patch_id),
                        anchor: position,
                        orientation,
                    },
                )?;
                let definition = patch(patch_id).ok_or(PlacementError::UnknownPiece)?;
                let cost = u32::from(definition.button_cost);
                if player.buttons() < cost {
                    return Err(ActionError::InsufficientButtons {
                        required: cost,
                        available: player.buttons(),
                    });
                }
                let slot = state.data.supply.take_candidate(patch_id)?;
                let next_player = &mut state.data.players[actor.index()];
                next_player.buttons -= cost;
                let income_added = u32::from(definition.income);
                next_player.income = next_player
                    .income
                    .checked_add(income_added)
                    .ok_or(ActionError::ArithmeticOverflow)?;
                next_player.board = preview.board;
                next_player.placed_pieces.push(preview.placed_piece.clone());
                events.push(ActionEvent::PatchPurchased {
                    actor,
                    slot,
                    button_cost: cost,
                    income_added,
                    placement: preview.placed_piece,
                });
                state.award_bonus(actor, &mut events);
                state.data.action.last_normal_actor = Some(actor);
                state.move_time(
                    actor,
                    u16::from(player.time_position()) + u16::from(definition.time_cost),
                    &mut events,
                )?;
            }
            (
                ActionPhase::Special { track_position, .. },
                GameAction::PlaceSpecialPatch { position },
            ) => {
                let preview = self.preview_placement(
                    authenticated_user_id,
                    PlacementRequest {
                        target_seat: actor,
                        piece: PieceId::Special(track_position),
                        anchor: position,
                        orientation: Orientation::default(),
                    },
                )?;
                let player = &mut state.data.players[actor.index()];
                player.board = preview.board;
                player.placed_pieces.push(preview.placed_piece);
                state.data.action.pending_specials.pop_front();
                state.special_mut(track_position)?.status = SpecialPatchStatus::Placed {
                    owner: actor,
                    position,
                };
                events.push(ActionEvent::SpecialPlaced {
                    owner: actor,
                    track_position,
                    position,
                });
                state.award_bonus(actor, &mut events);
            }
            _ => return Err(ActionError::WrongPhase),
        }
        state.finish_automatic(&mut events)?;
        state.validate()?;
        Ok(ActionTransition { state, events })
    }

    /// Recovery/system hook for full-board queues or a saved AwaitingScoring phase.
    /// Idempotent: finished snapshots return unchanged with no additional result event.
    /// Paused snapshots cannot be settled until the authority resumes them.
    pub fn settle_automatic(&self) -> Result<ActionTransition, ActionError> {
        self.validate()?;
        if self.lifecycle() == Lifecycle::Paused {
            return Err(ActionError::NotRunning);
        }
        let mut state = self.clone();
        let mut events = Vec::new();
        state.finish_automatic(&mut events)?;
        state.validate()?;
        Ok(ActionTransition { state, events })
    }

    fn credit(&mut self, actor: Seat, amount: u32) -> Result<(), ActionError> {
        let player = &mut self.data.players[actor.index()];
        player.buttons = player
            .buttons
            .checked_add(amount)
            .ok_or(ActionError::ArithmeticOverflow)?;
        Ok(())
    }

    fn move_time(
        &mut self,
        actor: Seat,
        target: u16,
        events: &mut Vec<ActionEvent>,
    ) -> Result<(), ActionError> {
        let old = self.player(actor).time_position();
        let new = target.min(u16::from(CUSTOM_V1.track.end)) as u8;
        self.data.players[actor.index()].time_position = new;
        events.push(ActionEvent::TimeMoved {
            actor,
            from: old,
            to: new,
        });
        // At most 53 positions, with income and claims interleaved in physical track order.
        for position in old + 1..=new {
            if CUSTOM_V1.track.income_positions.contains(&position) {
                let amount = self.player(actor).income();
                self.credit(actor, amount)?;
                events.push(ActionEvent::IncomeReceived {
                    actor,
                    track_position: position,
                    amount,
                });
            }
            if CUSTOM_V1.track.special_positions.contains(&position) {
                let special = self.special_mut(position)?;
                if special.status == SpecialPatchStatus::Available {
                    special.status = SpecialPatchStatus::Pending { owner: actor };
                    self.data
                        .action
                        .pending_specials
                        .push_back(PendingSpecialPatch {
                            owner: actor,
                            track_position: position,
                        });
                    events.push(ActionEvent::SpecialClaimed {
                        owner: actor,
                        track_position: position,
                    });
                }
            }
        }
        Ok(())
    }

    fn special_mut(
        &mut self,
        position: u8,
    ) -> Result<&mut crate::state::SpecialPatchState, ActionError> {
        self.data
            .special_patches
            .iter_mut()
            .find(|p| p.track_position == position)
            .ok_or(ActionError::InvalidState(StateError::InvalidSpecialPatches))
    }

    fn award_bonus(&mut self, actor: Seat, events: &mut Vec<ActionEvent>) {
        if self.data.bonus.owner.is_none()
            && let Some(top_left) = self.player(actor).board().completed_bonus_square()
        {
            self.data.bonus.owner = Some(actor);
            events.push(ActionEvent::BonusAwarded {
                owner: actor,
                top_left,
                points: CUSTOM_V1.scoring.bonus_points,
            });
        }
    }

    fn finish_automatic(&mut self, events: &mut Vec<ActionEvent>) -> Result<(), ActionError> {
        if self.result().is_some() {
            return Ok(());
        }
        while let Some(pending) = self.data.action.pending_specials.front().copied() {
            if !self.player(pending.owner).board().is_full() {
                break;
            }
            self.data.action.pending_specials.pop_front();
            self.special_mut(pending.track_position)?.status = SpecialPatchStatus::Discarded {
                owner: pending.owner,
                reason: DiscardReason::BoardFull,
            };
            events.push(ActionEvent::SpecialDiscarded {
                owner: pending.owner,
                track_position: pending.track_position,
                reason: DiscardReason::BoardFull,
            });
        }
        if self.action_phase() == ActionPhase::AwaitingScoring {
            let scores = self.data.players.each_ref().map(|player| {
                let owns_bonus = self.bonus().owner() == Some(player.seat());
                ScoreBreakdown {
                    buttons: player.buttons(),
                    bonus_points: if owns_bonus {
                        CUSTOM_V1.scoring.bonus_points
                    } else {
                        0
                    },
                    total: CUSTOM_V1.scoring.final_score(player.buttons(), owns_bonus),
                }
            });
            let outcome = match scores[0].total.cmp(&scores[1].total) {
                std::cmp::Ordering::Greater => Outcome::Won {
                    winner: Seat::First,
                },
                std::cmp::Ordering::Less => Outcome::Won {
                    winner: Seat::Second,
                },
                std::cmp::Ordering::Equal => Outcome::Draw,
            };
            let result = GameResult {
                scores,
                reason: ResultReason::Scored,
                outcome,
            };
            self.data.result = Some(result.clone());
            self.data.lifecycle = Lifecycle::Finished;
            events.push(ActionEvent::GameFinished { result });
        }
        Ok(())
    }
}
