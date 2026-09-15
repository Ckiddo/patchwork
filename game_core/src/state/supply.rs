use super::StateError;
use crate::rules::{CUSTOM_V1, PATCH_COUNT, PatchId, STARTING_PATCH_ID, patch};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NeutralPosition {
    BeforeSlot { slot: u8 },
    OnVacatedSlot { slot: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SupplyData")]
pub struct SupplyState {
    pub(crate) initial_order: Vec<PatchId>,
    pub(crate) remaining: Vec<Option<PatchId>>,
    pub(crate) neutral: NeutralPosition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SupplyData {
    initial_order: Vec<PatchId>,
    remaining: Vec<Option<PatchId>>,
    neutral: NeutralPosition,
}

impl TryFrom<SupplyData> for SupplyState {
    type Error = StateError;
    fn try_from(value: SupplyData) -> Result<Self, Self::Error> {
        let state = Self {
            initial_order: value.initial_order,
            remaining: value.remaining,
            neutral: value.neutral,
        };
        state.validate()?;
        Ok(state)
    }
}

impl SupplyState {
    /// The server injects one persisted permutation. There is no RNG in the core.
    pub fn new(initial_order: Vec<PatchId>) -> Result<Self, StateError> {
        let slot = initial_order
            .iter()
            .position(|&id| id == STARTING_PATCH_ID)
            .ok_or(StateError::InvalidSupply)?;
        let state = Self {
            remaining: initial_order.iter().copied().map(Some).collect(),
            initial_order,
            neutral: NeutralPosition::BeforeSlot {
                slot: u8::try_from(slot).map_err(|_| StateError::InvalidSupply)?,
            },
        };
        state.validate()?;
        Ok(state)
    }

    pub fn initial_order(&self) -> &[PatchId] {
        &self.initial_order
    }
    pub fn slots(&self) -> &[Option<PatchId>] {
        &self.remaining
    }
    pub fn neutral(&self) -> NeutralPosition {
        self.neutral
    }
    pub fn remaining_count(&self) -> usize {
        self.remaining.iter().flatten().count()
    }

    pub fn candidates(&self) -> Vec<PatchId> {
        let start = match self.neutral {
            NeutralPosition::BeforeSlot { slot } => usize::from(slot),
            NeutralPosition::OnVacatedSlot { slot } => usize::from(slot) + 1,
        };
        // Scan each slot at most once: 1/2 remaining patches must not appear repeatedly.
        (0..PATCH_COUNT)
            .filter_map(|offset| self.remaining[(start + offset) % PATCH_COUNT])
            .take(usize::from(CUSTOM_V1.candidate_count))
            .collect()
    }

    /// Supply transition only. Use GameSnapshot::apply_action for atomic payment and placement.
    pub fn take_candidate(&mut self, id: PatchId) -> Result<u8, StateError> {
        if !self.candidates().contains(&id) {
            return Err(StateError::NotCandidate);
        }
        let slot = self
            .remaining
            .iter()
            .position(|&item| item == Some(id))
            .ok_or(StateError::NotCandidate)?;
        self.remaining[slot] = None;
        self.neutral = NeutralPosition::OnVacatedSlot { slot: slot as u8 };
        Ok(slot as u8)
    }

    pub fn validate(&self) -> Result<(), StateError> {
        if self.initial_order.len() != PATCH_COUNT
            || self.remaining.len() != PATCH_COUNT
            || self.initial_order.iter().any(|&id| patch(id).is_none())
            || self.initial_order.iter().collect::<BTreeSet<_>>().len() != PATCH_COUNT
            || self
                .remaining
                .iter()
                .zip(&self.initial_order)
                .any(|(item, &initial)| item.is_some_and(|id| id != initial))
        {
            return Err(StateError::InvalidSupply);
        }
        let valid = match self.neutral {
            NeutralPosition::BeforeSlot { slot } => {
                self.initial_order.get(usize::from(slot)) == Some(&STARTING_PATCH_ID)
                    && self.remaining_count() == PATCH_COUNT
            }
            NeutralPosition::OnVacatedSlot { slot } => {
                self.remaining.get(usize::from(slot)) == Some(&None)
            }
        };
        if !valid {
            return Err(StateError::InvalidSupply);
        }
        Ok(())
    }
}
