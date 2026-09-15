//! End-to-end domain games: no private fields or fabricated mid-game snapshots.
use game_core::{
    BoardPosition, Seat,
    actions::{ActionError, ActionEvent, GameAction},
    geometry::{Orientation, distinct_orientations},
    rules::{CUSTOM_V1, PATCHES, patch},
    state::{ActionPhase, GameSnapshot, Lifecycle, Outcome, PieceId, SpecialPatchStatus},
};
use std::collections::BTreeSet;

fn choose(state: &GameSnapshot, variation: usize, buy_patches: bool) -> GameAction {
    let actor = state.input_actor().unwrap();
    let player = state.player(actor);
    if let ActionPhase::Special { .. } = state.action_phase() {
        for y in 0..9 {
            for x in 0..9 {
                let position = BoardPosition::new(x, y).unwrap();
                if player.board().at(position).is_none() {
                    return GameAction::PlaceSpecialPatch { position };
                }
            }
        }
        panic!("full board queue should have been discarded automatically");
    }
    if buy_patches {
        let mut candidates = state.supply().candidates();
        // Two policies exercise different purchases rather than always exhausting the same path.
        if variation.is_multiple_of(2) {
            candidates.sort_by_key(|&id| patch(id).unwrap().button_cost);
        } else {
            candidates.reverse();
        }
        for id in candidates {
            if u32::from(patch(id).unwrap().button_cost) > player.buttons() {
                continue;
            }
            let mut poses = distinct_orientations(id).unwrap();
            let shift = variation % poses.len();
            poses.rotate_left(shift);
            for (orientation, _) in poses {
                for y in 0..9 {
                    for x in 0..9 {
                        let position = BoardPosition::new(x, y).unwrap();
                        if player
                            .board()
                            .preview_placement(PieceId::Normal(id), position, orientation)
                            .is_ok()
                        {
                            return GameAction::BuyAndPlace {
                                patch_id: id,
                                position,
                                orientation,
                            };
                        }
                    }
                }
            }
        }
    }
    GameAction::Advance
}

fn run_game(rotation: usize, first: Seat, buy_patches: bool) -> (GameSnapshot, usize) {
    let mut order: Vec<_> = PATCHES.iter().map(|p| p.id).collect();
    order.rotate_left(rotation);
    if first == Seat::Second {
        order.reverse();
    }
    let initial = GameSnapshot::new(
        "complete-game".into(),
        ["a".into(), "b".into()],
        first,
        order,
    )
    .unwrap();
    let mut state = initial.clone();
    let mut transcript = Vec::new();
    let mut income_markers = [BTreeSet::new(), BTreeSet::new()];
    let mut claimed = BTreeSet::new();
    let mut bought = BTreeSet::new();
    let mut bonus_count = 0;
    let mut result_count = 0;
    let mut button_ledger = [5_u32; 2];
    let mut income_ledger = [0_u32; 2];
    for step in 0..112 {
        // Each normal action advances >=1; at most 106 normal + 5 special.
        if state.result().is_some() {
            break;
        }
        let actor = state
            .input_actor()
            .expect("running game must make progress");
        let user = state.player(actor).user_id().to_owned();
        let action = choose(&state, rotation + step, buy_patches);
        let i = actor.index();
        let old = state.player(actor).time_position();
        let target = match action {
            GameAction::Advance => {
                let target = (state.player(actor.other()).time_position() + 1).min(53);
                button_ledger[i] += u32::from(target - old);
                target
            }
            GameAction::BuyAndPlace { patch_id, .. } => {
                let definition = patch(patch_id).unwrap();
                button_ledger[i] -= u32::from(definition.button_cost);
                income_ledger[i] += u32::from(definition.income);
                assert!(bought.insert(patch_id));
                (old + definition.time_cost).min(53)
            }
            GameAction::PlaceSpecialPatch { .. } => old,
        };
        for &marker in CUSTOM_V1.track.income_positions {
            if old < marker && marker <= target {
                button_ledger[i] += income_ledger[i];
            }
        }
        let before = serde_json::to_vec(&state).unwrap();
        let next = state.apply_action(&user, action).unwrap();
        assert_eq!(serde_json::to_vec(&state).unwrap(), before);
        assert_eq!(next.state.player(actor).time_position(), target);
        assert_eq!(
            next.state.player(actor.other()),
            state.player(actor.other())
        );
        if !matches!(action, GameAction::PlaceSpecialPatch { .. }) {
            assert!(target > old);
        }
        let mut expected_income_positions: BTreeSet<_> = CUSTOM_V1
            .track
            .income_positions
            .iter()
            .copied()
            .filter(|&m| old < m && m <= target)
            .collect();
        for event in &next.events {
            match event {
                ActionEvent::IncomeReceived {
                    actor: owner,
                    track_position,
                    amount,
                } => {
                    assert_eq!(*owner, actor);
                    assert_eq!(*amount, income_ledger[i]);
                    assert!(expected_income_positions.remove(track_position));
                    assert!(
                        income_markers[i].insert(*track_position),
                        "income paid twice"
                    );
                }
                ActionEvent::SpecialClaimed {
                    owner,
                    track_position,
                } => {
                    assert_eq!(*owner, actor);
                    assert!(old < *track_position && *track_position <= target);
                    assert!(claimed.insert(*track_position), "special claimed twice");
                }
                ActionEvent::BonusAwarded { owner, points, .. } => {
                    bonus_count += 1;
                    assert_eq!(*points, 7);
                    assert_eq!(next.state.bonus().owner(), Some(*owner));
                }
                ActionEvent::GameFinished { result } => {
                    result_count += 1;
                    assert_eq!(next.state.result(), Some(result));
                    assert!(next.state.action().pending_specials().is_empty());
                }
                _ => {}
            }
        }
        assert!(expected_income_positions.is_empty());
        for seat in [Seat::First, Seat::Second] {
            let player = next.state.player(seat);
            assert_eq!(player.buttons(), button_ledger[seat.index()]);
            assert_eq!(player.income(), income_ledger[seat.index()]);
        }
        assert!(bonus_count <= 1);
        transcript.push((user, action, next.events));
        // Restore every action, including any special phase; future choices use the restored copy.
        state = serde_json::from_slice(&serde_json::to_vec(&next.state).unwrap()).unwrap();
        assert_eq!(state, next.state);
    }
    assert_eq!(state.lifecycle(), Lifecycle::Finished);
    assert_eq!(result_count, 1);
    assert_eq!(claimed, BTreeSet::from([19, 25, 31, 43, 49]));
    for markers in income_markers {
        assert_eq!(
            markers,
            CUSTOM_V1.track.income_positions.iter().copied().collect()
        );
    }
    assert!(state.special_patches().iter().all(|p| matches!(
        p.status(),
        SpecialPatchStatus::Placed { .. } | SpecialPatchStatus::Discarded { .. }
    )));
    let result = state.result().unwrap();
    for seat in [Seat::First, Seat::Second] {
        let bonus = if state.bonus().owner() == Some(seat) {
            7
        } else {
            0
        };
        let score = result.scores[seat.index()];
        assert_eq!(score.buttons, button_ledger[seat.index()]);
        assert_eq!(score.total, u64::from(button_ledger[seat.index()]) + bonus);
    }
    // Replay identical intents from the same injected initial permutation: no wall time/RNG.
    let mut replay = initial;
    for (user, action, expected_events) in transcript {
        let next = replay.apply_action(&user, action).unwrap();
        assert_eq!(next.events, expected_events);
        replay = next.state;
    }
    assert_eq!(replay, state);
    let settled = state.settle_automatic().unwrap();
    assert_eq!(settled.state, state);
    assert!(settled.events.is_empty());
    assert_eq!(
        state.apply_action("a", GameAction::Advance),
        Err(ActionError::GameFinished)
    );
    (state, bought.len())
}

#[test]
fn sixty_six_complete_games_preserve_balances_claims_and_deterministic_replay() {
    let mut purchases = 0;
    for rotation in 0..PATCHES.len() {
        for first in [Seat::First, Seat::Second] {
            let (_, bought) = run_game(rotation, first, true);
            assert!(bought > 0);
            purchases += bought;
        }
    }
    assert!(purchases > 66);
}

#[test]
fn advance_only_games_end_in_a_draw_without_empty_cell_deductions() {
    for first in [Seat::First, Seat::Second] {
        let (state, bought) = run_game(0, first, false);
        assert_eq!(bought, 0);
        assert_eq!(state.result().unwrap().outcome, Outcome::Draw);
        assert_eq!(state.result().unwrap().scores.map(|s| s.total), [58, 58]);
        assert_eq!(state.supply().remaining_count(), 33);
        assert_eq!(state.bonus().owner(), None);
    }
}

#[test]
fn transformed_purchase_records_the_requested_pose_and_normalized_anchor() {
    let state = GameSnapshot::new(
        "oriented".into(),
        ["a".into(), "b".into()],
        Seat::First,
        PATCHES.iter().map(|p| p.id).collect(),
    )
    .unwrap();
    let orientation = Orientation::new(1, true).unwrap();
    let next = state
        .apply_action(
            "a",
            GameAction::BuyAndPlace {
                patch_id: game_core::rules::PatchId(12),
                position: BoardPosition::new(3, 4).unwrap(),
                orientation,
            },
        )
        .unwrap();
    let placement = &next.state.player(Seat::First).placed_pieces()[0];
    assert_eq!(placement.orientation(), (1, true));
    assert_eq!(
        placement.cells().iter().map(|c| c.coordinates().0).min(),
        Some(3)
    );
    assert_eq!(
        placement.cells().iter().map(|c| c.coordinates().1).min(),
        Some(4)
    );
}
