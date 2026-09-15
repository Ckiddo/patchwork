use super::*;
use crate::geometry::{Orientation, PlacementError, PlacementRequest};
use crate::rules::PATCHES;
use serde_json::{Value, json};

fn new_game() -> GameSnapshot {
    GameSnapshot::new(
        "game-1".into(),
        ["player-a".into(), "player-b".into()],
        Seat::Second,
        PATCHES.iter().map(|p| p.id).collect(),
    )
    .unwrap()
}
fn cell(x: i32, y: i32) -> BoardPosition {
    BoardPosition::new(x, y).unwrap()
}
fn round_trip(state: &GameSnapshot) -> GameSnapshot {
    let restored: GameSnapshot =
        serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap();
    assert_eq!(&restored, state);
    assert_eq!(restored.action_phase(), state.action_phase());
    assert_eq!(restored.supply().candidates(), state.supply().candidates());
    restored
}
// Fixtures test state invariants. Public action histories are exercised by tests/actions.rs.
fn normal_fixture(state: &mut GameSnapshot, actor: Seat, id: PatchId) {
    state.data.supply.take_candidate(id).unwrap();
    let definition = patch(id).unwrap();
    let cells: Vec<_> = definition
        .cells
        .iter()
        .map(|&(x, y)| cell(i32::from(x), i32::from(y)))
        .collect();
    let player = &mut state.data.players[actor.index()];
    for &position in &cells {
        let (x, y) = position.coordinates();
        player.board.cells[usize::from(y)][usize::from(x)] = Some(PieceId::Normal(id));
    }
    player.placed_pieces.push(PlacedPiece {
        piece: PieceId::Normal(id),
        cells,
        quarter_turns: 0,
        flipped: false,
    });
    player.income += u32::from(definition.income);
    player.time_position = definition.time_cost;
    state.data.action.last_normal_actor = Some(actor);
}
fn pending_fixture(a: u8, b: u8) -> GameSnapshot {
    let mut state = new_game();
    state.data.players[0].time_position = a;
    state.data.players[1].time_position = b;
    state.data.action.last_normal_actor = Some(Seat::First);
    for special in &mut state.data.special_patches {
        if special.track_position <= a {
            special.status = SpecialPatchStatus::Pending { owner: Seat::First };
            state
                .data
                .action
                .pending_specials
                .push_back(PendingSpecialPatch {
                    owner: Seat::First,
                    track_position: special.track_position,
                });
        }
    }
    state.validate().unwrap();
    state
}
fn resolve_one_fixture(state: &mut GameSnapshot) {
    let pending = state.data.action.pending_specials.pop_front().unwrap();
    let index = CUSTOM_V1
        .track
        .special_positions
        .iter()
        .position(|&p| p == pending.track_position)
        .unwrap();
    let position = cell(index as i32, 0);
    let piece = PieceId::Special(pending.track_position);
    let player = &mut state.data.players[pending.owner.index()];
    player.board.cells[0][index] = Some(piece);
    player.placed_pieces.push(PlacedPiece {
        piece,
        cells: vec![position],
        quarter_turns: 0,
        flipped: false,
    });
    state.data.special_patches[index].status = SpecialPatchStatus::Placed {
        owner: pending.owner,
        position,
    };
}

#[test]
fn every_normal_shape_orientation_survives_snapshot_restore() {
    for definition in PATCHES {
        for flipped in [false, true] {
            for turn in 0..4 {
                // Put the tested patch among the first three, without changing the frozen catalog.
                let mut order = vec![PatchId(10)];
                if definition.id != PatchId(10) {
                    order.push(definition.id);
                }
                order.extend(
                    PATCHES
                        .iter()
                        .map(|p| p.id)
                        .filter(|id| *id != PatchId(10) && *id != definition.id),
                );
                let mut state = GameSnapshot::new(
                    "geometry-restore".into(),
                    ["player-a".into(), "player-b".into()],
                    Seat::Second,
                    order,
                )
                .unwrap();
                let preview = state
                    .preview_placement(
                        "player-b",
                        PlacementRequest {
                            target_seat: Seat::Second,
                            piece: PieceId::Normal(definition.id),
                            anchor: cell(1, 1),
                            orientation: Orientation::new(turn, flipped).unwrap(),
                        },
                    )
                    .unwrap();
                state.data.supply.take_candidate(definition.id).unwrap();
                let player = &mut state.data.players[1];
                player.board = preview.board;
                player.placed_pieces.push(preview.placed_piece);
                player.income = u32::from(definition.income);
                player.time_position = definition.time_cost;
                state.data.action.last_normal_actor = Some(Seat::Second);
                state.validate().unwrap();
                round_trip(&state);
                // Occupancy records are a set: different serialization order is still legitimate.
                state.data.players[1].placed_pieces[0].cells.reverse();
                round_trip(&state);
            }
        }
    }
}

#[test]
fn restore_rejects_same_area_wrong_shape_and_mismatched_orientation() {
    let mut state = new_game();
    normal_fixture(&mut state, Seat::Second, PatchId(10));
    // Keep area, piece partition, income and board/list agreement: only geometry is corrupted.
    state.data.players[1].board.cells[0][1] = None;
    state.data.players[1].board.cells[0][2] = Some(PieceId::Normal(PatchId(10)));
    state.data.players[1].placed_pieces[0].cells[1] = cell(2, 0);
    assert_eq!(state.validate(), Err(StateError::InvalidBoard));
    assert!(serde_json::from_value::<GameSnapshot>(serde_json::to_value(state).unwrap()).is_err());

    for (turn, flipped) in [(1, false), (2, false), (0, true)] {
        let mut state = new_game();
        normal_fixture(&mut state, Seat::Second, PatchId(11));
        state.data.players[1].placed_pieces[0].quarter_turns = turn;
        state.data.players[1].placed_pieces[0].flipped = flipped;
        assert_eq!(state.validate(), Err(StateError::InvalidBoard));
        assert!(
            serde_json::from_value::<GameSnapshot>(serde_json::to_value(state).unwrap()).is_err()
        );
    }
}

#[test]
fn preview_rejects_taken_patches_and_checks_special_queue_ownership_and_order() {
    let mut state = new_game();
    normal_fixture(&mut state, Seat::Second, PatchId(10));
    let request = PlacementRequest {
        target_seat: Seat::First,
        piece: PieceId::Normal(PatchId(10)),
        anchor: cell(5, 5),
        orientation: Orientation::default(),
    };
    let before = state.clone();
    assert_eq!(
        state.preview_placement("player-a", request),
        Err(PlacementError::NotAvailable)
    );
    assert_eq!(state, before);

    let state = pending_fixture(31, 32);
    let before = state.clone();
    let request = PlacementRequest {
        piece: PieceId::Special(19),
        ..request
    };
    assert!(state.preview_placement("player-a", request).is_ok());
    assert_eq!(
        state.preview_placement(
            "player-b",
            PlacementRequest {
                target_seat: Seat::Second,
                ..request
            }
        ),
        Err(PlacementError::NotYourTurn)
    );
    assert_eq!(
        state.preview_placement(
            "player-a",
            PlacementRequest {
                piece: PieceId::Special(25),
                ..request
            }
        ),
        Err(PlacementError::WrongPhase)
    );
    assert_eq!(
        state.preview_placement(
            "player-a",
            PlacementRequest {
                piece: PieceId::Normal(PatchId(10)),
                ..request
            }
        ),
        Err(PlacementError::WrongPhase)
    );
    assert_eq!(state, before);
    let mut state = state;
    resolve_one_fixture(&mut state);
    state.validate().unwrap();
    assert_eq!(
        state.preview_placement("player-a", request),
        Err(PlacementError::WrongPhase)
    );
    assert!(
        state
            .preview_placement(
                "player-a",
                PlacementRequest {
                    piece: PieceId::Special(25),
                    ..request
                }
            )
            .is_ok()
    );
}

#[test]
fn a_bonus_claim_without_a_completed_square_cannot_be_restored() {
    for owner in [Seat::First, Seat::Second] {
        let mut state = new_game();
        state.data.bonus.owner = Some(owner);
        assert_eq!(state.validate(), Err(StateError::InvalidBonus));
        assert!(
            serde_json::from_value::<GameSnapshot>(serde_json::to_value(state).unwrap()).is_err()
        );
    }
}

#[test]
fn initialization_is_deterministic_and_identity_relative() {
    let state = new_game();
    assert_eq!(state, new_game());
    assert_eq!(
        state.action_phase(),
        ActionPhase::Normal {
            actor: Seat::Second
        }
    );
    assert_eq!(
        state.supply().candidates(),
        [PatchId(10), PatchId(11), PatchId(12)]
    );
    for seat in [Seat::First, Seat::Second] {
        let p = state.player(seat);
        assert_eq!((p.buttons(), p.income(), p.time_position()), (5, 0, 0));
        assert_eq!(p.board().occupied_count(), 0);
        let (own, opponent) = state.perspective(p.user_id()).unwrap();
        assert_eq!(own.seat(), seat);
        assert_eq!(opponent.seat(), seat.other());
    }
    assert!(state.perspective("spectator").is_none());
    let value = serde_json::to_value(&state).unwrap();
    assert_eq!(value["first_player_seat"], 1); // compatible with the existing room DB projection
    assert!(value.get("bank_money").is_none());
    round_trip(&state);
}

#[test]
fn invalid_identifiers_and_permutations_do_not_create_games() {
    for users in [["same", "same"], ["", "b"], ["a", "with space"]] {
        assert!(
            GameSnapshot::new(
                "game".into(),
                users.map(String::from),
                Seat::First,
                PATCHES.iter().map(|p| p.id).collect()
            )
            .is_err()
        );
    }
    for order in [
        vec![],
        vec![PatchId(10); PATCH_COUNT],
        (1..=32).map(PatchId).collect(),
        (2..=34).map(PatchId).collect(),
    ] {
        assert!(SupplyState::new(order).is_err());
    }
}

#[test]
fn supply_wraps_skips_empty_slots_and_keeps_original_positions() {
    let mut order: Vec<_> = PATCHES
        .iter()
        .map(|p| p.id)
        .filter(|&id| id != PatchId(10))
        .collect();
    order.push(PatchId(10));
    let mut supply = SupplyState::new(order.clone()).unwrap();
    assert_eq!(supply.neutral(), NeutralPosition::BeforeSlot { slot: 32 });
    assert_eq!(supply.candidates(), [PatchId(10), PatchId(1), PatchId(2)]);
    assert_eq!(supply.take_candidate(PatchId(10)), Ok(32));
    assert_eq!(supply.candidates(), [PatchId(1), PatchId(2), PatchId(3)]);
    assert_eq!(supply.take_candidate(PatchId(2)), Ok(1));
    assert_eq!(supply.candidates(), [PatchId(3), PatchId(4), PatchId(5)]);
    assert_eq!(supply.slots()[0], Some(PatchId(1)));
    assert_eq!(supply.slots()[1], None);
    assert_eq!(supply.initial_order(), order);
    let saved = supply.clone();
    assert_eq!(
        supply.take_candidate(PatchId(2)),
        Err(StateError::NotCandidate)
    );
    assert_eq!(
        supply.take_candidate(PatchId(1)),
        Err(StateError::NotCandidate)
    );
    assert_eq!(
        supply.take_candidate(PatchId(u32::MAX)),
        Err(StateError::NotCandidate)
    );
    assert_eq!(supply, saved);
    assert_eq!(
        serde_json::from_value::<SupplyState>(serde_json::to_value(&supply).unwrap()).unwrap(),
        supply
    );
}

#[test]
fn every_starting_slot_can_be_exhausted_without_duplicate_candidates() {
    for rotation in 0..PATCH_COUNT {
        let mut order: Vec<_> = PATCHES.iter().map(|p| p.id).collect();
        order.rotate_left(rotation);
        let mut supply = SupplyState::new(order).unwrap();
        for left in (1..=PATCH_COUNT).rev() {
            let candidates = supply.candidates();
            assert_eq!(candidates.len(), left.min(3));
            assert_eq!(
                candidates.iter().collect::<BTreeSet<_>>().len(),
                candidates.len()
            );
            supply.take_candidate(*candidates.last().unwrap()).unwrap();
            supply.validate().unwrap();
            assert_eq!(supply.remaining_count(), left - 1);
        }
        assert!(supply.candidates().is_empty());
        assert_eq!(
            supply.take_candidate(PatchId(10)),
            Err(StateError::NotCandidate)
        );
    }
}

#[test]
fn malformed_supply_restore_is_rejected_before_candidate_scanning() {
    let original = serde_json::to_value(new_game().supply()).unwrap();
    for (pointer, replacement) in [
        ("/initial_order/0", json!(10)),
        ("/remaining/0", json!(34)),
        ("/neutral/slot", json!(255)),
        ("/remaining", json!([])),
        ("/neutral", json!({"kind":"on_vacated_slot","slot":9})),
        ("/neutral", json!({"kind":"before_slot","slot":8})),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            serde_json::from_value::<SupplyState>(value).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn boards_resources_and_income_are_separate_and_survive_restore() {
    let mut state = new_game();
    normal_fixture(&mut state, Seat::Second, PatchId(11));
    state.data.players[1].buttons = 7;
    state.validate().unwrap();
    assert_eq!(state.player(Seat::First).board().occupied_count(), 0);
    assert_eq!(state.player(Seat::First).buttons(), 5);
    assert_eq!(state.player(Seat::Second).board().occupied_count(), 5);
    assert_eq!(state.player(Seat::Second).income(), 2); // once per patch, not per occupied cell
    let copy = round_trip(&state);
    assert_eq!(
        copy.player(Seat::Second).board().at(cell(0, 0)),
        Some(PieceId::Normal(PatchId(11)))
    );
    state.data.players[1].buttons = 1;
    assert_eq!(copy.player(Seat::Second).buttons(), 7);
}

#[test]
fn behind_player_can_repeat_and_equal_positions_switch_normal_actor() {
    let mut state = new_game();
    state.data.action.last_normal_actor = Some(Seat::First);
    for (times, actor) in [
        ([2, 5], Seat::First),
        ([5, 2], Seat::Second),
        ([5, 5], Seat::Second),
        ([53, 18], Seat::Second),
    ] {
        // Test the time selector separately; crossed-special invariants are covered below.
        state.data.players[0].time_position = times[0];
        state.data.players[1].time_position = times[1];
        assert_eq!(state.action_phase(), ActionPhase::Normal { actor });
    }
    state.data.players[0].time_position = 5;
    state.data.players[1].time_position = 5;
    state.data.action.last_normal_actor = Some(Seat::Second);
    assert_eq!(
        state.action_phase(),
        ActionPhase::Normal { actor: Seat::First }
    );
    round_trip(&state);
}

#[test]
fn pending_specials_keep_owner_order_and_turn_across_pause_and_restore() {
    let mut state = pending_fixture(31, 31);
    state.data.lifecycle = Lifecycle::Paused;
    let mut state = round_trip(&state);
    assert_eq!(state.input_actor(), None);
    state.data.lifecycle = Lifecycle::Running;
    for position in [19, 25, 31] {
        assert_eq!(
            state.action_phase(),
            ActionPhase::Special {
                actor: Seat::First,
                track_position: position
            }
        );
        assert_eq!(state.input_actor(), Some(Seat::First));
        resolve_one_fixture(&mut state);
        state = round_trip(&state);
        assert_eq!(state.action().last_normal_actor(), Some(Seat::First));
    }
    assert_eq!(
        state.action_phase(),
        ActionPhase::Normal {
            actor: Seat::Second
        }
    );
    assert_eq!(state.player(Seat::First).income(), 0);
    assert_eq!(state.player(Seat::Second).board().occupied_count(), 0);
}

#[test]
fn final_specials_precede_scoring_and_finished_state_preserves_result() {
    let mut state = pending_fixture(53, 53);
    assert!(matches!(state.action_phase(), ActionPhase::Special { .. }));
    for _ in 0..5 {
        resolve_one_fixture(&mut state);
    }
    state = round_trip(&state);
    assert_eq!(state.action_phase(), ActionPhase::AwaitingScoring);
    assert_eq!(state.input_actor(), None);
    state.data.result = Some(GameResult {
        scores: [ScoreBreakdown {
            buttons: 5,
            bonus_points: 0,
            total: 5,
        }; 2],
        reason: ResultReason::Scored,
        outcome: Outcome::Draw,
    });
    state.data.lifecycle = Lifecycle::Finished;
    state = round_trip(&state);
    assert_eq!(state.action_phase(), ActionPhase::Finished);
    assert_eq!(state.result().unwrap().outcome, Outcome::Draw);
    assert_eq!(state.input_actor(), None);
    state.data.result.as_mut().unwrap().outcome = Outcome::Won {
        winner: Seat::First,
    };
    assert_eq!(state.validate(), Err(StateError::InvalidResult));
}

#[test]
fn malformed_snapshot_metadata_identity_board_and_ownership_are_rejected() {
    let mut state = new_game();
    normal_fixture(&mut state, Seat::First, PatchId(10));
    let original = serde_json::to_value(state).unwrap();
    for (pointer, replacement) in [
        ("/kind", json!(LEGACY_SNAPSHOT_KIND)),
        ("/schema_version", json!(2)),
        ("/rules_version", json!("v1")),
        ("/game_id", json!("")),
        ("/first_player_seat", json!(2)),
        ("/players/0/seat", json!(1)),
        ("/players/1/user_id", json!("player-a")),
        ("/players/0/time_position", json!(54)),
        ("/players/0/placed_pieces/0/cells/0/x", json!(9)),
        ("/players/0/placed_pieces/0/cells/0/y", json!(-1)),
        ("/players/0/placed_pieces/0/quarter_turns", json!(4)),
        ("/players/0/board/cells/0/0", Value::Null),
        ("/players/0/income", json!(99)),
        ("/players/0/placed_pieces", json!([])),
        ("/action/last_normal_actor", Value::Null),
        ("/lifecycle", json!("finished")),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            serde_json::from_value::<GameSnapshot>(value).is_err(),
            "{pointer}"
        );
    }
    assert!(serde_json::from_value::<BoardPosition>(json!({"x":0,"y":0,"extra":1})).is_err());
}

#[test]
fn special_claim_queue_cannot_be_lost_reordered_or_reassigned_by_restore() {
    let original = serde_json::to_value(pending_fixture(31, 18)).unwrap();
    for (pointer, replacement) in [
        ("/action/pending_specials", json!([])),
        ("/action/pending_specials/0/track_position", json!(25)),
        ("/action/pending_specials/0/owner", json!(1)),
        ("/special_patches/0/status", json!({"kind":"available"})),
        (
            "/special_patches/0/status",
            json!({"kind":"pending","owner":1}),
        ),
        (
            "/special_patches/0/status",
            json!({"kind":"discarded","owner":0,"reason":"board_full"}),
        ),
        ("/special_patches/0/track_position", json!(20)),
        ("/action/last_normal_actor", json!(1)),
    ] {
        let mut value = original.clone();
        *value.pointer_mut(pointer).unwrap() = replacement;
        assert!(
            serde_json::from_value::<GameSnapshot>(value).is_err(),
            "{pointer}"
        );
    }
}

#[test]
fn forfeits_and_abandoned_results_are_distinct_from_natural_scoring() {
    let mut state = new_game();
    state.data.result = Some(GameResult {
        scores: [ScoreBreakdown {
            buttons: 5,
            bonus_points: 0,
            total: 5,
        }; 2],
        reason: ResultReason::Forfeit {
            loser: Seat::Second,
        },
        outcome: Outcome::Won {
            winner: Seat::First,
        },
    });
    state.data.lifecycle = Lifecycle::Finished;
    round_trip(&state);
    state.data.result.as_mut().unwrap().reason = ResultReason::Abandoned;
    state.data.result.as_mut().unwrap().outcome = Outcome::Abandoned;
    round_trip(&state);
    state.data.result.as_mut().unwrap().scores[0].total = 12;
    assert_eq!(state.validate(), Err(StateError::InvalidResult));
}
