//! Frozen project rules, independent of rendering and mutable match state.
//! Changes to released data require a new rules version, not edits to custom_v1.

mod catalog;
pub mod registry;

use crate::{BOARD_SIZE, PLAYER_COUNT, Seat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub use catalog::PATCHES;
pub const CUSTOM_RULES_VERSION: &str = "patchwork_custom_v1";
pub const PATCH_COUNT: usize = 33;
pub const STARTING_PATCH_ID: PatchId = PatchId(10);

/// Stable wire ID, never an index into a shuffled supply. Zero is not a normal patch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PatchId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PatchDefinition {
    pub id: PatchId,
    /// Normalized (x, y) coordinates; both minima are zero. No Bevy coordinates.
    pub cells: &'static [(u8, u8)],
    pub button_cost: u8,
    pub time_cost: u8,
    pub income: u8,
}

pub fn patch(id: PatchId) -> Option<&'static PatchDefinition> {
    PATCHES.iter().find(|patch| patch.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonSupply {
    Unlimited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SamePositionTurn {
    OtherThanLastNormalActor,
}

impl SamePositionTurn {
    /// Only the equal-position case. Pending special patches and end-of-game take priority.
    pub fn actor(self, last_normal_actor: Option<Seat>, first_player: Seat) -> Seat {
        match (self, last_normal_actor) {
            (_, None) => first_player,
            (Self::OtherThanLastNormalActor, Some(Seat::First)) => Seat::Second,
            (Self::OtherThanLastNormalActor, Some(Seat::Second)) => Seat::First,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SupplyDirection {
    Clockwise,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnplaceableSpecialPatch {
    DiscardAndRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TiedScore {
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarkerInterval {
    ExcludeDepartureIncludeArrival,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct TimeTrackDefinition {
    pub start: u8,
    pub end: u8,
    pub income_positions: &'static [u8],
    pub special_positions: &'static [u8],
    pub marker_interval: MarkerInterval,
    pub clamp_at_end: bool,
}

impl TimeTrackDefinition {
    /// Definition of crossing, shared by later income/special handling. Overshoot is clamped.
    pub fn crosses(&self, old: u8, new: u16, marker: u8) -> bool {
        self.start <= old
            && old <= self.end
            && marker > old
            && marker <= self.end
            && u16::from(marker) <= new.min(u16::from(self.end))
    }

    pub fn validate(&self) -> Result<(), DefinitionError> {
        if self.start >= self.end || !self.clamp_at_end {
            return Err(DefinitionError::InvalidTrack);
        }
        for positions in [self.income_positions, self.special_positions] {
            if positions.iter().any(|&p| p <= self.start || p > self.end)
                || positions.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(DefinitionError::InvalidTrack);
            }
        }
        // custom_v1 has no simultaneous markers; a future version must define their order.
        if self
            .income_positions
            .iter()
            .any(|p| self.special_positions.contains(p))
        {
            return Err(DefinitionError::InvalidTrack);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct SpecialPatchRules {
    pub cells: &'static [(u8, u8)],
    pub button_cost: u8,
    pub time_cost: u8,
    pub income: u8,
    pub first_claim_only: bool,
    pub resolve_before_next_action_and_scoring: bool,
    pub unplaceable: UnplaceableSpecialPatch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ScoringRules {
    pub bonus_square_side: u8,
    pub bonus_points: u8,
    pub bonus_first_player_only: bool,
    pub bonus_is_spendable: bool,
    pub empty_cell_penalty: u8,
    pub tied_score: TiedScore,
}

impl ScoringRules {
    /// Score arithmetic only; checking/awarding the unique square bonus belongs to the engine.
    pub fn final_score(&self, buttons: u32, owns_bonus: bool) -> u64 {
        u64::from(buttons)
            + if owns_bonus {
                u64::from(self.bonus_points)
            } else {
                0
            }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RulesDefinition {
    pub version: &'static str,
    pub player_count: usize,
    pub board_size: u8,
    pub initial_buttons: u8,
    pub time_tokens_per_player: u8,
    pub button_supply: ButtonSupply,
    pub same_position_turn: SamePositionTurn,
    pub supply_direction: SupplyDirection,
    pub candidate_count: u8,
    pub starting_patch: PatchId,
    pub track: TimeTrackDefinition,
    pub special_patch: SpecialPatchRules,
    pub scoring: ScoringRules,
}

pub const CUSTOM_V1: RulesDefinition = RulesDefinition {
    version: CUSTOM_RULES_VERSION,
    player_count: PLAYER_COUNT,
    board_size: BOARD_SIZE,
    initial_buttons: 5,
    time_tokens_per_player: 1,
    button_supply: ButtonSupply::Unlimited,
    same_position_turn: SamePositionTurn::OtherThanLastNormalActor,
    supply_direction: SupplyDirection::Clockwise,
    candidate_count: 3,
    starting_patch: STARTING_PATCH_ID,
    track: TimeTrackDefinition {
        start: 0,
        end: 53,
        income_positions: &[4, 10, 16, 22, 28, 34, 40, 46, 52],
        special_positions: &[19, 25, 31, 43, 49],
        marker_interval: MarkerInterval::ExcludeDepartureIncludeArrival,
        clamp_at_end: true,
    },
    special_patch: SpecialPatchRules {
        cells: &[(0, 0)],
        button_cost: 0,
        time_cost: 0,
        income: 0,
        first_claim_only: true,
        resolve_before_next_action_and_scoring: true,
        unplaceable: UnplaceableSpecialPatch::DiscardAndRecord,
    },
    scoring: ScoringRules {
        bonus_square_side: 7,
        bonus_points: 7,
        bonus_first_player_only: true,
        bonus_is_spendable: false,
        empty_cell_penalty: 0,
        tied_score: TiedScore::Draw,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DefinitionError {
    WrongPatchCount,
    InvalidOrDuplicateId(PatchId),
    InvalidShape(PatchId),
    InvalidTimeCost(PatchId),
    InvalidStartingPatch,
    InvalidTrack,
}

/// Validate the custom_v1 catalog without relying on vector order or unchecked indices.
pub fn validate_catalog(patches: &[PatchDefinition]) -> Result<(), DefinitionError> {
    if patches.len() != PATCH_COUNT {
        return Err(DefinitionError::WrongPatchCount);
    }
    let mut ids = BTreeSet::new();
    let mut dominoes = Vec::new();
    for patch in patches {
        if patch.id.0 == 0 || patch.id.0 > PATCH_COUNT as u32 || !ids.insert(patch.id) {
            return Err(DefinitionError::InvalidOrDuplicateId(patch.id));
        }
        if patch.time_cost == 0 || patch.time_cost > CUSTOM_V1.track.end {
            return Err(DefinitionError::InvalidTimeCost(patch.id));
        }
        let cells: BTreeSet<_> = patch.cells.iter().copied().collect();
        if cells.len() < 2
            || cells.len() != patch.cells.len()
            || cells
                .iter()
                .any(|&(x, y)| x >= BOARD_SIZE || y >= BOARD_SIZE)
            || cells.iter().map(|p| p.0).min() != Some(0)
            || cells.iter().map(|p| p.1).min() != Some(0)
        {
            return Err(DefinitionError::InvalidShape(patch.id));
        }
        let first = *cells
            .first()
            .ok_or(DefinitionError::InvalidShape(patch.id))?;
        let mut reached = BTreeSet::from([first]);
        let mut pending = vec![first];
        while let Some((x, y)) = pending.pop() {
            for &next in &cells {
                if x.abs_diff(next.0) + y.abs_diff(next.1) == 1 && reached.insert(next) {
                    pending.push(next);
                }
            }
        }
        if reached.len() != cells.len() {
            return Err(DefinitionError::InvalidShape(patch.id));
        }
        if cells.len() == 2 {
            dominoes.push(patch.id);
        }
    }
    if dominoes != [STARTING_PATCH_ID] {
        return Err(DefinitionError::InvalidStartingPatch);
    }
    Ok(())
}
