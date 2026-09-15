use super::*;
use crate::{
    rules::PATCHES,
    state::{NeutralPosition, SpecialPatchState},
};
use serde::Deserialize;

fn cell(x: i32, y: i32) -> BoardPosition {
    BoardPosition::new(x, y).unwrap()
}
fn game() -> GameSnapshot {
    GameSnapshot::new(
        "action-test".into(),
        ["a".into(), "b".into()],
        Seat::First,
        PATCHES.iter().map(|p| p.id).collect(),
    )
    .unwrap()
}

#[test]
fn connection_pause_preserves_pending_action_and_rejects_moves() {
    let mut state = game();
    while state.action().pending_specials().is_empty() {
        let user = state
            .player(state.input_actor().unwrap())
            .user_id()
            .to_string();
        state = state
            .apply_action(&user, GameAction::Advance)
            .unwrap()
            .state;
    }
    let paused = state.with_connection_pause(true).unwrap();
    assert_eq!(paused.lifecycle(), Lifecycle::Paused);
    assert_eq!(paused.action(), state.action());
    assert_eq!(paused.with_connection_pause(false).unwrap(), state);
    let user = paused
        .player(paused.action().pending_specials()[0].owner)
        .user_id();
    assert_eq!(
        paused.apply_action(
            user,
            GameAction::PlaceSpecialPatch {
                position: cell(0, 0)
            }
        ),
        Err(ActionError::NotRunning)
    );
    let restored: GameSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&paused).unwrap()).unwrap();
    assert_eq!(restored, paused);
}

#[test]
fn authority_termination_keeps_actual_scores_and_cannot_settle_twice() {
    let state = game()
        .apply_action("a", GameAction::Advance)
        .unwrap()
        .state
        .with_connection_pause(true)
        .unwrap();
    assert_eq!(
        state.terminate(ResultReason::Scored),
        Err(ActionError::WrongPhase)
    );
    for (reason, outcome) in [
        (
            ResultReason::Forfeit { loser: Seat::First },
            Outcome::Won {
                winner: Seat::Second,
            },
        ),
        (ResultReason::Abandoned, Outcome::Abandoned),
    ] {
        let transition = state.terminate(reason).unwrap();
        let result = transition.state.result().unwrap();
        assert_eq!(result.outcome, outcome);
        assert_eq!(result.scores[0].total, 6);
        assert_eq!(result.scores[1].total, 5);
        assert_eq!(
            transition.events,
            vec![ActionEvent::GameFinished {
                result: result.clone()
            }]
        );
        assert_eq!(
            transition.state.terminate(reason),
            Err(ActionError::GameFinished)
        );
        assert_eq!(
            transition.state.with_connection_pause(false),
            Err(ActionError::GameFinished)
        );
        transition.state.validate().unwrap();
    }
}
fn buy(id: u32, x: i32, y: i32) -> GameAction {
    GameAction::BuyAndPlace {
        patch_id: PatchId(id),
        position: cell(x, y),
        orientation: Orientation::default(),
    }
}
fn perform(state: &GameSnapshot, user: &str, action: GameAction) -> ActionTransition {
    let before = serde_json::to_vec(state).unwrap();
    let result = state.apply_action(user, action).unwrap();
    assert_eq!(serde_json::to_vec(state).unwrap(), before);
    result.state.validate().unwrap();
    let restored: GameSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&result.state).unwrap()).unwrap();
    assert_eq!(restored, result.state);
    let events: Vec<ActionEvent> =
        serde_json::from_slice(&serde_json::to_vec(&result.events).unwrap()).unwrap();
    assert_eq!(events, result.events);
    result
}
fn reject(state: &GameSnapshot, user: &str, action: GameAction, error: ActionError) {
    let before = serde_json::to_vec(state).unwrap();
    assert_eq!(state.apply_action(user, action), Err(error));
    assert_eq!(serde_json::to_vec(state).unwrap(), before);
}

// Boundary fixtures establish validated states, not claimed historical action sequences.
// Complete games through only public actions are tested separately in tests/actions.rs.
fn place_fixture(
    state: &mut GameSnapshot,
    actor: Seat,
    piece: PieceId,
    position: BoardPosition,
    orientation: Orientation,
) {
    let preview = state
        .player(actor)
        .board()
        .preview_placement(piece, position, orientation)
        .unwrap();
    let player = &mut state.data.players[actor.index()];
    player.board = preview.board;
    player.placed_pieces.push(preview.placed_piece);
    if let PieceId::Normal(id) = piece {
        player.income += u32::from(patch(id).unwrap().income);
        let slot = state
            .data
            .supply
            .remaining
            .iter()
            .position(|p| *p == Some(id))
            .unwrap();
        state.data.supply.remaining[slot] = None;
        state.data.supply.neutral = NeutralPosition::OnVacatedSlot { slot: slot as u8 };
    }
}
fn times(a: u8, b: u8, last: Seat) -> GameSnapshot {
    let mut state = game();
    state.data.players[0].time_position = a;
    state.data.players[1].time_position = b;
    state.data.action.last_normal_actor = Some(last);
    for (index, &position) in CUSTOM_V1.track.special_positions.iter().enumerate() {
        if a.max(b) >= position {
            let owner = if a >= position {
                Seat::First
            } else {
                Seat::Second
            };
            let cell = cell(index as i32, 8);
            place_fixture(
                &mut state,
                owner,
                PieceId::Special(position),
                cell,
                Orientation::default(),
            );
            state.data.special_patches[index].status = SpecialPatchStatus::Placed {
                owner,
                position: cell,
            };
        }
    }
    state.validate().unwrap();
    state
}
fn candidates_starting_with(state: &mut GameSnapshot, id: PatchId) {
    // For empty fixture games, customize the injected permutation without changing IDs.
    assert!(
        state
            .data
            .players
            .iter()
            .all(|p| p.placed_pieces.is_empty())
    );
    let mut order = vec![PatchId(10)];
    if id != PatchId(10) {
        order.push(id);
    }
    order.extend(
        PATCHES
            .iter()
            .map(|p| p.id)
            .filter(|p| *p != PatchId(10) && *p != id),
    );
    state.data.supply = crate::state::SupplyState::new(order).unwrap();
}
fn pending(state: &mut GameSnapshot, owner: Seat) {
    state.data.action.last_normal_actor = Some(owner);
    for special in &mut state.data.special_patches {
        if special.track_position <= state.data.players[owner.index()].time_position
            && special.status == SpecialPatchStatus::Available
        {
            special.status = SpecialPatchStatus::Pending { owner };
            state
                .data
                .action
                .pending_specials
                .push_back(PendingSpecialPatch {
                    owner,
                    track_position: special.track_position,
                });
        }
    }
}
#[derive(Deserialize)]
struct FixturePiece {
    id: u32,
    x: i32,
    y: i32,
    turns: u8,
    flipped: bool,
}
fn tiled_fixture(state: &mut GameSnapshot, owner: Seat, name: &str) {
    let fixtures: std::collections::BTreeMap<String, Vec<FixturePiece>> =
        serde_json::from_str(include_str!("../../tests/fixtures/t37_boards.json")).unwrap();
    for p in &fixtures[name] {
        place_fixture(
            state,
            owner,
            PieceId::Normal(PatchId(p.id)),
            cell(p.x, p.y),
            Orientation::new(p.turns, p.flipped).unwrap(),
        );
    }
}

#[test]
fn advance_uses_opponents_next_cell_and_actual_distance() {
    let first = perform(&game(), "a", GameAction::Advance);
    assert_eq!(
        (
            first.state.player(Seat::First).buttons(),
            first.state.player(Seat::First).time_position()
        ),
        (6, 1)
    );
    assert_eq!(first.state.input_actor(), Some(Seat::Second));
    let second = perform(&first.state, "b", GameAction::Advance);
    assert_eq!(
        (
            second.state.player(Seat::Second).buttons(),
            second.state.player(Seat::Second).time_position()
        ),
        (7, 2)
    );
    assert_eq!(
        second.events[0],
        ActionEvent::AdvanceReward {
            actor: Seat::Second,
            amount: 2
        }
    );
}

#[test]
fn purchase_is_atomic_and_a_player_still_behind_can_act_again() {
    let state = times(0, 3, Seat::Second);
    let opponent = state.player(Seat::Second).clone();
    let next = perform(&state, "a", buy(10, 3, 4));
    let a = next.state.player(Seat::First);
    assert_eq!(
        (
            a.buttons(),
            a.income(),
            a.time_position(),
            a.board().occupied_count()
        ),
        (3, 0, 1, 2)
    );
    assert_eq!(next.state.supply().remaining_count(), 32);
    assert_eq!(
        next.state.supply().neutral(),
        NeutralPosition::OnVacatedSlot { slot: 9 }
    );
    assert_eq!(
        next.state.supply().candidates(),
        [PatchId(11), PatchId(12), PatchId(13)]
    );
    assert_eq!(next.state.player(Seat::Second), &opponent);
    assert_eq!(next.state.input_actor(), Some(Seat::First));
    let advanced = perform(&next.state, "a", GameAction::Advance);
    assert_eq!(advanced.state.player(Seat::First).time_position(), 4);
    assert_eq!(advanced.state.player(Seat::First).buttons(), 6);
}

#[test]
fn a_purchase_that_lands_on_equal_time_switches_to_the_other_player() {
    let next = perform(&times(0, 1, Seat::Second), "a", buy(10, 0, 0));
    assert_eq!(next.state.player(Seat::First).time_position(), 1);
    assert_eq!(next.state.action().last_normal_actor(), Some(Seat::First));
    assert_eq!(next.state.input_actor(), Some(Seat::Second));
}

#[test]
fn new_patch_income_is_paid_on_arrival_once_per_patch_not_per_cell() {
    let next = perform(&game(), "a", buy(12, 0, 0));
    let a = next.state.player(Seat::First);
    assert_eq!((a.buttons(), a.income(), a.time_position()), (2, 2, 4)); // 5 - 5 + 2
    assert_eq!(a.board().occupied_count(), 5);
    assert_eq!(
        next.events.last(),
        Some(&ActionEvent::IncomeReceived {
            actor: Seat::First,
            track_position: 4,
            amount: 2
        })
    );
}

#[test]
fn the_same_income_marker_pays_each_players_own_income() {
    let mut state = game();
    let mut order = vec![PatchId(10), PatchId(12), PatchId(18)];
    order.extend(
        PATCHES
            .iter()
            .map(|p| p.id)
            .filter(|id| ![PatchId(10), PatchId(12), PatchId(18)].contains(id)),
    );
    state.data.supply = crate::state::SupplyState::new(order).unwrap();
    let a = perform(&state, "a", buy(12, 0, 0));
    let b = perform(&a.state, "b", buy(18, 0, 0));
    assert_eq!(b.state.player(Seat::First).buttons(), 2);
    assert_eq!(b.state.player(Seat::Second).buttons(), 5); // 5 - 1 + 1
    assert!(b.events.contains(&ActionEvent::IncomeReceived {
        actor: Seat::Second,
        track_position: 4,
        amount: 1
    }));
    assert_eq!(b.state.input_actor(), Some(Seat::First));
}

#[test]
fn advance_pays_multiple_markers_and_does_not_repay_departure_marker() {
    let mut state = times(4, 18, Seat::Second);
    place_fixture(
        &mut state,
        Seat::First,
        PieceId::Normal(PatchId(12)),
        cell(0, 0),
        Orientation::default(),
    );
    state.validate().unwrap();
    let next = perform(&state, "a", GameAction::Advance);
    assert_eq!(next.state.player(Seat::First).buttons(), 24); // 5 + 15 steps + 2 + 2
    let income: Vec<_> = next
        .events
        .iter()
        .filter_map(|e| {
            if let ActionEvent::IncomeReceived {
                track_position,
                amount,
                ..
            } = e
            {
                Some((*track_position, *amount))
            } else {
                None
            }
        })
        .collect();
    assert_eq!(income, [(10, 2), (16, 2)]);
    assert_eq!(
        next.events.last(),
        Some(&ActionEvent::SpecialClaimed {
            owner: Seat::First,
            track_position: 19
        })
    );
    assert_eq!(
        next.state.action_phase(),
        ActionPhase::Special {
            actor: Seat::First,
            track_position: 19
        }
    );
}

#[test]
fn track_processing_interleaves_all_markers_and_queues_claims_in_order() {
    let mut state = game();
    state.data.action.last_normal_actor = Some(Seat::First);
    let mut events = Vec::new();
    // Exercise the shared traversal directly; no catalog patch has a 53-step cost.
    state.move_time(Seat::First, 100, &mut events).unwrap();
    state.finish_automatic(&mut events).unwrap();
    state.validate().unwrap();
    let markers: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ActionEvent::IncomeReceived { track_position, .. }
            | ActionEvent::SpecialClaimed { track_position, .. } => Some(*track_position),
            _ => None,
        })
        .collect();
    assert_eq!(
        markers,
        [4, 10, 16, 19, 22, 25, 28, 31, 34, 40, 43, 46, 49, 52]
    );
    assert_eq!(
        state
            .action()
            .pending_specials()
            .iter()
            .map(|p| p.track_position)
            .collect::<Vec<_>>(),
        [19, 25, 31, 43, 49]
    );
    assert_eq!(state.player(Seat::First).time_position(), 53);
    assert!(state.result().is_none());
}

#[test]
fn purchase_claims_special_before_later_income_on_the_same_move() {
    let next = perform(&times(18, 18, Seat::Second), "a", buy(12, 0, 0));
    assert_eq!(
        &next.events[2..],
        &[
            ActionEvent::SpecialClaimed {
                owner: Seat::First,
                track_position: 19
            },
            ActionEvent::IncomeReceived {
                actor: Seat::First,
                track_position: 22,
                amount: 2
            },
        ]
    );
    assert_eq!(next.state.player(Seat::First).buttons(), 2);
}

#[test]
fn specials_are_free_preserve_normal_actor_and_cannot_be_claimed_again() {
    let advanced = perform(&times(17, 18, Seat::Second), "a", GameAction::Advance);
    let state = advanced.state;
    let resources = state.player(Seat::First).clone();
    reject(&state, "a", GameAction::Advance, ActionError::WrongPhase);
    reject(
        &state,
        "b",
        GameAction::PlaceSpecialPatch {
            position: cell(0, 0),
        },
        ActionError::NotYourTurn,
    );
    let placed = perform(
        &state,
        "a",
        GameAction::PlaceSpecialPatch {
            position: cell(0, 0),
        },
    );
    let a = placed.state.player(Seat::First);
    assert_eq!(
        (a.buttons(), a.income(), a.time_position()),
        (
            resources.buttons(),
            resources.income(),
            resources.time_position()
        )
    );
    assert_eq!(placed.state.action().last_normal_actor(), Some(Seat::First));
    assert_eq!(placed.state.input_actor(), Some(Seat::Second));
    assert_eq!(
        placed.events,
        [ActionEvent::SpecialPlaced {
            owner: Seat::First,
            track_position: 19,
            position: cell(0, 0)
        }]
    );
    let next = perform(&placed.state, "b", GameAction::Advance); // B crosses 19 after A
    assert!(
        !next
            .events
            .iter()
            .any(|e| matches!(e, ActionEvent::SpecialClaimed { .. }))
    );
    assert_eq!(
        next.state.special_patches()[0].status(),
        SpecialPatchStatus::Placed {
            owner: Seat::First,
            position: cell(0, 0)
        }
    );
}

#[test]
fn invalid_identity_phase_candidates_balance_and_geometry_leave_input_unchanged() {
    let state = game();
    reject(
        &state,
        "unknown",
        GameAction::Advance,
        ActionError::UnknownPlayer,
    );
    reject(&state, "b", GameAction::Advance, ActionError::NotYourTurn);
    reject(
        &state,
        "a",
        GameAction::PlaceSpecialPatch {
            position: cell(0, 0),
        },
        ActionError::WrongPhase,
    );
    reject(
        &state,
        "a",
        buy(1, 0, 0),
        ActionError::Placement(PlacementError::NotCandidate),
    );
    reject(
        &state,
        "a",
        buy(0, 0, 0),
        ActionError::Placement(PlacementError::UnknownPiece),
    );
    reject(
        &state,
        "a",
        buy(10, 8, 8),
        ActionError::Placement(PlacementError::OutOfBounds),
    );
    reject(
        &state,
        "a",
        buy(11, 0, 0),
        ActionError::InsufficientButtons {
            required: 10,
            available: 5,
        },
    );
    let next = perform(&state, "a", buy(10, 0, 0)).state;
    reject(
        &next,
        "b",
        buy(10, 5, 5),
        ActionError::Placement(PlacementError::NotAvailable),
    );
    let mut state = times(0, 3, Seat::Second);
    state = perform(&state, "a", buy(10, 0, 0)).state;
    reject(
        &state,
        "a",
        buy(12, 0, 0),
        ActionError::Placement(PlacementError::Overlap),
    );
    state.data.lifecycle = Lifecycle::Paused;
    reject(&state, "a", GameAction::Advance, ActionError::NotRunning);
    assert_eq!(state.settle_automatic(), Err(ActionError::NotRunning));
}

#[test]
fn early_and_late_arithmetic_overflow_cannot_partially_commit() {
    let mut state = times(1, 2, Seat::Second);
    state.data.players[0].buttons = u32::MAX;
    reject(
        &state,
        "a",
        GameAction::Advance,
        ActionError::ArithmeticOverflow,
    );
    let mut state = times(3, 3, Seat::Second);
    state.data.players[0].buttons = u32::MAX;
    candidates_starting_with(&mut state, PatchId(2)); // free purchase, income 1, crosses 4
    reject(&state, "a", buy(2, 0, 0), ActionError::ArithmeticOverflow);
    assert_eq!(state.supply().remaining_count(), 33);
    assert_eq!(state.player(Seat::First).board().occupied_count(), 0);
}

#[test]
fn advance_and_purchase_clamp_at_terminal_without_extra_rewards() {
    let mut state = times(50, 53, Seat::Second);
    place_fixture(
        &mut state,
        Seat::First,
        PieceId::Normal(PatchId(12)),
        cell(0, 0),
        Orientation::default(),
    );
    let next = perform(&state, "a", GameAction::Advance);
    assert_eq!(next.state.player(Seat::First).time_position(), 53);
    assert_eq!(next.state.player(Seat::First).buttons(), 10); // 5 + 3 actual steps + 2 income at 52
    assert_eq!(
        next.events[0],
        ActionEvent::AdvanceReward {
            actor: Seat::First,
            amount: 3
        }
    );
    assert_eq!(
        next.events
            .iter()
            .filter(|e| matches!(e, ActionEvent::IncomeReceived { .. }))
            .count(),
        1
    );
    let mut state = times(50, 53, Seat::Second);
    state.data.players[0].buttons = 10;
    // Leave special ownership intact while moving patch 15 into the initial candidate set.
    let mut order = vec![PatchId(10), PatchId(15)];
    order.extend(
        PATCHES
            .iter()
            .map(|p| p.id)
            .filter(|p| *p != PatchId(10) && *p != PatchId(15)),
    );
    state.data.supply = crate::state::SupplyState::new(order).unwrap();
    let next = perform(&state, "a", buy(15, 0, 0)); // time cost 6: intended 56, ends at 53
    assert_eq!(
        (
            next.state.player(Seat::First).buttons(),
            next.state.player(Seat::First).time_position()
        ),
        (5, 53)
    ); // 10 - 8 + 3
    assert_eq!(
        next.events
            .iter()
            .filter(|e| matches!(e, ActionEvent::IncomeReceived { .. }))
            .count(),
        1
    );
    assert!(
        !next
            .events
            .iter()
            .any(|e| matches!(e, ActionEvent::AdvanceReward { .. }))
    );
}

#[test]
fn arriving_from_52_does_not_repay_52_and_equal_scores_are_a_draw() {
    let mut state = times(52, 53, Seat::Second);
    state.data.players[0].buttons = 4;
    let next = perform(&state, "a", GameAction::Advance);
    let result = next.state.result().unwrap();
    assert_eq!(result.outcome, Outcome::Draw);
    assert_eq!(result.scores.map(|s| s.total), [5, 5]); // no empty-cell penalty
    assert!(
        !next
            .events
            .iter()
            .any(|e| matches!(e, ActionEvent::IncomeReceived { .. }))
    );
}

#[test]
fn bonus_from_normal_patch_is_separate_from_money_and_cannot_be_reawarded() {
    let mut state = times(0, 1, Seat::Second);
    tiled_fixture(&mut state, Seat::First, "square_missing_domino");
    assert_eq!(state.player(Seat::First).board().occupied_count(), 47);
    state.data.players[0].buttons = 20;
    state.data.supply.neutral = NeutralPosition::OnVacatedSlot { slot: 7 };
    state.validate().unwrap();
    let next = perform(&state, "a", buy(10, 5, 6));
    assert_eq!(next.state.bonus().owner(), Some(Seat::First));
    assert_eq!(next.state.player(Seat::First).buttons(), 18);
    assert_eq!(
        next.events
            .iter()
            .filter(|e| matches!(e, ActionEvent::BonusAwarded { .. }))
            .count(),
        1
    );
    assert!(next.events.contains(&ActionEvent::BonusAwarded {
        owner: Seat::First,
        top_left: cell(0, 0),
        points: 7
    }));
    let restored: GameSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&next.state).unwrap()).unwrap();
    assert!(restored.settle_automatic().unwrap().events.is_empty());
    reject(
        &restored,
        "b",
        buy(10, 0, 0),
        ActionError::Placement(PlacementError::NotAvailable),
    );

    let mut state = restored;
    tiled_fixture(&mut state, Seat::Second, "other_square_missing_special");
    state.data.players[0].time_position = 19;
    state.data.players[1].time_position = 19;
    pending(&mut state, Seat::Second);
    state.validate().unwrap();
    let next = perform(
        &state,
        "b",
        GameAction::PlaceSpecialPatch {
            position: cell(6, 6),
        },
    );
    assert_eq!(
        next.state
            .player(Seat::Second)
            .board()
            .completed_bonus_square(),
        Some(cell(0, 0))
    );
    assert_eq!(next.state.bonus().owner(), Some(Seat::First));
    assert!(
        !next
            .events
            .iter()
            .any(|e| matches!(e, ActionEvent::BonusAwarded { .. }))
    );
    assert_eq!(next.state.input_actor(), Some(Seat::First)); // same time; special did not change last normal actor
}

#[test]
fn last_special_can_award_bonus_discard_remaining_queue_and_then_finish() {
    let mut state = game();
    tiled_fixture(&mut state, Seat::First, "board_missing_center");
    state.data.players[0].time_position = 53;
    state.data.players[1].time_position = 53;
    pending(&mut state, Seat::First);
    state.validate().unwrap();
    assert_eq!(state.player(Seat::First).board().occupied_count(), 80);
    assert_eq!(
        state.player(Seat::First).board().completed_bonus_square(),
        None
    );
    let unchanged = state.settle_automatic().unwrap();
    assert_eq!(unchanged.state, state);
    assert!(unchanged.events.is_empty()); // both at end, but queue still requires a placement
    let next = perform(
        &state,
        "a",
        GameAction::PlaceSpecialPatch {
            position: cell(4, 4),
        },
    );
    assert_eq!(next.state.action().last_normal_actor(), Some(Seat::First));
    assert!(next.state.action().pending_specials().is_empty());
    assert_eq!(next.events.len(), 7); // placement, bonus, four discards, result
    for (index, position) in [25, 31, 43, 49].into_iter().enumerate() {
        assert_eq!(
            next.events[index + 2],
            ActionEvent::SpecialDiscarded {
                owner: Seat::First,
                track_position: position,
                reason: DiscardReason::BoardFull
            }
        );
        assert_eq!(
            next.state.special_patches()[index + 1].status(),
            SpecialPatchStatus::Discarded {
                owner: Seat::First,
                reason: DiscardReason::BoardFull
            }
        );
    }
    let result = next.state.result().unwrap();
    assert_eq!(
        result.outcome,
        Outcome::Won {
            winner: Seat::First
        }
    );
    assert_eq!(result.scores.map(|s| s.total), [12, 5]);
    assert_eq!(next.state.player(Seat::First).buttons(), 5);
    assert!(matches!(
        next.events.last(),
        Some(ActionEvent::GameFinished { .. })
    ));
    let settled = next.state.settle_automatic().unwrap();
    assert_eq!(settled.state, next.state);
    assert!(settled.events.is_empty());
}

#[test]
fn a_full_board_automatically_discards_newly_claimed_specials() {
    let mut state = game();
    tiled_fixture(&mut state, Seat::First, "board_missing_center");
    place_fixture(
        &mut state,
        Seat::First,
        PieceId::Special(19),
        cell(4, 4),
        Orientation::default(),
    );
    state.data.special_patches[0] = SpecialPatchState {
        track_position: 19,
        status: SpecialPatchStatus::Placed {
            owner: Seat::First,
            position: cell(4, 4),
        },
    };
    state.data.bonus.owner = Some(Seat::First);
    state.data.players[0].time_position = 19;
    state.data.players[1].time_position = 24;
    state.data.action.last_normal_actor = Some(Seat::Second);
    state.validate().unwrap();
    let next = perform(&state, "a", GameAction::Advance);
    assert_eq!(
        &next.events[next.events.len() - 2..],
        &[
            ActionEvent::SpecialClaimed {
                owner: Seat::First,
                track_position: 25
            },
            ActionEvent::SpecialDiscarded {
                owner: Seat::First,
                track_position: 25,
                reason: DiscardReason::BoardFull
            },
        ]
    );
    assert!(next.state.action().pending_specials().is_empty());
    assert_eq!(next.state.input_actor(), Some(Seat::Second));
    assert_eq!(
        next.state.player(Seat::First).buttons(),
        11 + state.player(Seat::First).income()
    );
}

#[test]
fn settlement_is_idempotent_and_finished_games_reject_all_actions() {
    let state = times(53, 53, Seat::Second);
    let before = state.clone();
    let settled = state.settle_automatic().unwrap();
    assert_eq!(state, before);
    assert_eq!(settled.events.len(), 1);
    assert_eq!(settled.state.result().unwrap().outcome, Outcome::Draw);
    assert_eq!(settled.state.lifecycle(), Lifecycle::Finished);
    assert!(settled.state.settle_automatic().unwrap().events.is_empty());
    for action in [
        GameAction::Advance,
        buy(10, 0, 0),
        GameAction::PlaceSpecialPatch {
            position: cell(0, 0),
        },
    ] {
        for user in ["a", "b"] {
            reject(&settled.state, user, action, ActionError::GameFinished);
        }
    }
}
