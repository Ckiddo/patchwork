use game_core::{
    BoardPosition, Seat,
    geometry::{Orientation, PlacementError, PlacementRequest, distinct_orientations, patch_shape},
    rules::{PATCHES, PatchId},
    state::{GameSnapshot, PieceId, QuiltBoard},
};
use serde_json::json;
use std::collections::BTreeSet;

fn cell(x: i32, y: i32) -> BoardPosition {
    BoardPosition::new(x, y).unwrap()
}
fn normal(id: u32) -> PieceId {
    PieceId::Normal(PatchId(id))
}
fn new_game() -> GameSnapshot {
    GameSnapshot::new(
        "geometry-test".into(),
        ["a".into(), "b".into()],
        Seat::Second,
        PATCHES.iter().map(|p| p.id).collect(),
    )
    .unwrap()
}
fn request() -> PlacementRequest {
    PlacementRequest {
        target_seat: Seat::Second,
        piece: normal(10),
        anchor: cell(0, 0),
        orientation: Orientation::default(),
    }
}
// Occupancy-only fixtures exercise board geometry independently from match history/partition.
fn occupied_board(cells: &[(i32, i32)]) -> QuiltBoard {
    let mut value = serde_json::to_value(QuiltBoard::default()).unwrap();
    for &(x, y) in cells {
        value["cells"][y as usize][x as usize] = serde_json::to_value(normal(1)).unwrap();
    }
    serde_json::from_value(value).unwrap()
}

#[test]
fn asymmetric_shape_has_all_eight_expected_poses() {
    let expected: [&[(u8, u8)]; 8] = [
        &[(0, 0), (0, 1), (1, 1), (0, 2), (0, 3)],
        &[(0, 0), (1, 0), (2, 0), (3, 0), (2, 1)],
        &[(1, 0), (1, 1), (0, 2), (1, 2), (1, 3)],
        &[(1, 0), (0, 1), (1, 1), (2, 1), (3, 1)],
        &[(1, 0), (0, 1), (1, 1), (1, 2), (1, 3)],
        &[(2, 0), (0, 1), (1, 1), (2, 1), (3, 1)],
        &[(0, 0), (0, 1), (0, 2), (1, 2), (0, 3)],
        &[(0, 0), (1, 0), (2, 0), (3, 0), (1, 1)],
    ];
    let poses = distinct_orientations(PatchId(4)).unwrap();
    assert_eq!(poses.len(), 8);
    for ((orientation, shape), expected) in poses.iter().zip(expected) {
        assert_eq!(shape.cells(), expected, "{orientation:?}");
    }
}

#[test]
fn rotations_and_base_reflections_restore_every_catalog_shape() {
    for patch in PATCHES {
        let poses = distinct_orientations(patch.id).unwrap();
        assert!((1..=8).contains(&poses.len()));
        for flipped in [false, true] {
            for turn in 0..4 {
                let orientation = Orientation::new(turn, flipped).unwrap();
                let mut rotated = orientation;
                for _ in 0..4 {
                    rotated = rotated.rotate_clockwise();
                }
                assert_eq!(rotated, orientation);
                assert_eq!(orientation.flip().flip(), orientation);
                assert_eq!(
                    orientation.rotate_clockwise().rotate_counterclockwise(),
                    orientation
                );
                let shape = patch_shape(patch.id, orientation).unwrap();
                assert_eq!(patch_shape(patch.id, rotated).unwrap(), shape);
                assert_eq!(shape.cells().len(), patch.cells.len());
                assert_eq!(
                    shape.cells().iter().collect::<BTreeSet<_>>().len(),
                    patch.cells.len()
                );
                assert_eq!(shape.cells().iter().map(|c| c.0).min(), Some(0));
                assert_eq!(shape.cells().iter().map(|c| c.1).min(), Some(0));
            }
        }
    }
    assert_eq!(distinct_orientations(PatchId(10)).unwrap().len(), 2); // domino symmetry
}

#[test]
fn unknown_ids_and_malformed_wire_values_are_rejected() {
    for id in [0, 34, u32::MAX] {
        assert_eq!(
            patch_shape(PatchId(id), Orientation::default()),
            Err(PlacementError::UnknownPiece)
        );
        assert_eq!(
            distinct_orientations(PatchId(id)),
            Err(PlacementError::UnknownPiece)
        );
    }
    for turn in [4, 255] {
        assert_eq!(
            Orientation::new(turn, false),
            Err(PlacementError::InvalidOrientation)
        );
    }
    for turn in [
        json!(-1),
        json!(4),
        json!(256),
        json!(1.5),
        json!("1"),
        json!(null),
    ] {
        let mut value = serde_json::to_value(request()).unwrap();
        value["orientation"]["quarter_turns"] = turn;
        assert!(serde_json::from_value::<PlacementRequest>(value).is_err());
    }
    for (x, y) in [(-1, 0), (9, 0), (0, -1), (0, 9), (i32::MAX, i32::MIN)] {
        let mut value = serde_json::to_value(request()).unwrap();
        value["anchor"] = json!({"x":x,"y":y});
        assert!(serde_json::from_value::<PlacementRequest>(value).is_err());
    }
    let mut value = serde_json::to_value(request()).unwrap();
    value["target_seat"] = json!(2);
    assert!(serde_json::from_value::<PlacementRequest>(value).is_err());
    assert_eq!(
        serde_json::from_value::<PlacementRequest>(serde_json::to_value(request()).unwrap())
            .unwrap(),
        request()
    );
}

#[test]
fn all_poses_at_all_board_anchors_obey_exact_bounds_without_mutation() {
    let board = QuiltBoard::default();
    for patch in PATCHES {
        for flipped in [false, true] {
            for turn in 0..4 {
                let orientation = Orientation::new(turn, flipped).unwrap();
                let shape = patch_shape(patch.id, orientation).unwrap();
                let (width, height) = shape.dimensions();
                for y in 0..9 {
                    for x in 0..9 {
                        let result =
                            board.preview_placement(normal(patch.id.0), cell(x, y), orientation);
                        if x + i32::from(width) > 9 || y + i32::from(height) > 9 {
                            assert_eq!(result, Err(PlacementError::OutOfBounds));
                        } else {
                            let preview = result.unwrap();
                            assert_eq!(preview.board().occupied_count(), patch.cells.len());
                            assert_eq!(preview.placed_piece().cells().len(), patch.cells.len());
                            assert!(preview.placed_piece().cells().iter().all(|c| {
                                let (cx, cy) = c.coordinates();
                                cx >= x as u8 && cy >= y as u8
                            }));
                        }
                    }
                }
            }
        }
    }
    assert_eq!(board, QuiltBoard::default());
}

#[test]
fn normalized_anchor_can_be_an_empty_cell_and_does_not_drift_on_rotation() {
    let anchor = cell(3, 4);
    let shape = patch_shape(PatchId(4), Orientation::new(2, false).unwrap()).unwrap();
    let cells = shape.cells_at(anchor).unwrap();
    assert!(!cells.contains(&anchor)); // concave corner: bounding box origin is not a patch cell
    assert_eq!(cells.iter().map(|c| c.coordinates().0).min(), Some(3));
    assert_eq!(cells.iter().map(|c| c.coordinates().1).min(), Some(4));
}

#[test]
fn overlap_and_reusing_a_piece_leave_the_board_unchanged() {
    let board = QuiltBoard::default()
        .preview_placement(normal(10), cell(0, 0), Orientation::default())
        .unwrap()
        .board()
        .clone();
    let before = board.clone();
    assert_eq!(
        board.preview_placement(normal(5), cell(1, 0), Orientation::default()),
        Err(PlacementError::Overlap)
    );
    assert_eq!(
        board.preview_placement(normal(10), cell(5, 5), Orientation::default()),
        Err(PlacementError::AlreadyPlaced)
    );
    assert_eq!(
        board.preview_placement(PieceId::Special(19), cell(0, 0), Orientation::default()),
        Err(PlacementError::Overlap)
    );
    assert_eq!(board, before);
}

#[test]
fn snapshot_preview_checks_identity_turn_board_and_candidates_without_mutation() {
    let state = new_game();
    let before = serde_json::to_vec(&state).unwrap();
    assert_eq!(
        state.preview_placement("spectator", request()),
        Err(PlacementError::UnknownPlayer)
    );
    assert_eq!(
        state.preview_placement("a", request()),
        Err(PlacementError::WrongBoard)
    );
    assert_eq!(
        state.preview_placement(
            "a",
            PlacementRequest {
                target_seat: Seat::First,
                ..request()
            }
        ),
        Err(PlacementError::NotYourTurn)
    );
    assert_eq!(
        state.preview_placement(
            "b",
            PlacementRequest {
                target_seat: Seat::First,
                ..request()
            }
        ),
        Err(PlacementError::WrongBoard)
    );
    assert_eq!(
        state.preview_placement(
            "b",
            PlacementRequest {
                piece: normal(1),
                ..request()
            }
        ),
        Err(PlacementError::NotCandidate)
    );
    assert_eq!(
        state.preview_placement(
            "b",
            PlacementRequest {
                piece: normal(34),
                ..request()
            }
        ),
        Err(PlacementError::UnknownPiece)
    );
    assert_eq!(
        state.preview_placement(
            "b",
            PlacementRequest {
                piece: PieceId::Special(19),
                ..request()
            }
        ),
        Err(PlacementError::WrongPhase)
    );
    assert_eq!(
        state.preview_placement(
            "b",
            PlacementRequest {
                anchor: cell(8, 8),
                ..request()
            }
        ),
        Err(PlacementError::OutOfBounds)
    );
    let preview = state.preview_placement("b", request()).unwrap();
    assert_eq!(preview.board().occupied_count(), 2);
    // Geometry preview does not purchase even an unaffordable candidate (T37 handles cost).
    assert!(
        state
            .preview_placement(
                "b",
                PlacementRequest {
                    piece: normal(11),
                    ..request()
                }
            )
            .is_ok()
    );
    assert_eq!(serde_json::to_vec(&state).unwrap(), before);
    let mut value = serde_json::to_value(&state).unwrap();
    value["lifecycle"] = json!("paused");
    let paused: GameSnapshot = serde_json::from_value(value).unwrap();
    assert_eq!(
        paused.preview_placement("b", request()),
        Err(PlacementError::NotRunning)
    );
}

#[test]
fn special_geometry_requires_known_id_and_canonical_orientation() {
    let board = QuiltBoard::default();
    assert_eq!(
        board.preview_placement(PieceId::Special(37), cell(0, 0), Orientation::default()),
        Err(PlacementError::UnknownPiece)
    );
    for orientation in [
        Orientation::new(1, false).unwrap(),
        Orientation::new(0, true).unwrap(),
    ] {
        assert_eq!(
            board.preview_placement(PieceId::Special(19), cell(0, 0), orientation),
            Err(PlacementError::InvalidOrientation)
        );
    }
    let preview = board
        .preview_placement(PieceId::Special(19), cell(8, 8), Orientation::default())
        .unwrap();
    assert_eq!(preview.placed_piece().cells(), &[cell(8, 8)]);
}

#[test]
fn every_bonus_anchor_is_detected_after_normal_or_special_placement() {
    for y in 0..=2 {
        for x in 0..=2 {
            let full: Vec<_> = (y..y + 7)
                .flat_map(|cy| (x..x + 7).map(move |cx| (cx, cy)))
                .collect();
            let special_hole = (x + 6, y + 6);
            let normal_holes = [(x + 5, y + 6), special_hole];
            let board = occupied_board(
                &full
                    .iter()
                    .copied()
                    .filter(|c| !normal_holes.contains(c))
                    .collect::<Vec<_>>(),
            );
            assert_eq!(board.completed_bonus_square(), None);
            let preview = board
                .preview_placement(normal(10), cell(x + 5, y + 6), Orientation::default())
                .unwrap();
            assert_eq!(preview.completed_square(), Some(cell(x, y)));
            assert_eq!(board.occupied_count(), 47);
            let board = occupied_board(
                &full
                    .iter()
                    .copied()
                    .filter(|&c| c != special_hole)
                    .collect::<Vec<_>>(),
            );
            assert_eq!(board.completed_bonus_square(), None);
            let preview = board
                .preview_placement(
                    PieceId::Special(19),
                    cell(x + 6, y + 6),
                    Orientation::default(),
                )
                .unwrap();
            assert_eq!(preview.completed_square(), Some(cell(x, y)));
            assert_eq!(board.occupied_count(), 48);
        }
    }
}

#[test]
fn forty_nine_noncontiguous_cells_and_large_regions_with_holes_do_not_qualify() {
    let mut cells: Vec<_> = (0..5).flat_map(|y| (0..9).map(move |x| (x, y))).collect();
    cells.extend([(0, 6), (2, 6), (4, 6), (6, 6)]);
    let board = occupied_board(&cells);
    assert_eq!(board.occupied_count(), 49);
    assert_eq!(board.completed_bonus_square(), None);
    let cells: Vec<_> = (0..9)
        .flat_map(|y| (0..9).map(move |x| (x, y)))
        .filter(|&p| p != (4, 4))
        .collect();
    let board = occupied_board(&cells);
    assert_eq!(board.occupied_count(), 80);
    assert_eq!(board.completed_bonus_square(), None);
    assert_eq!(
        board
            .preview_placement(PieceId::Special(19), cell(4, 4), Orientation::default())
            .unwrap()
            .completed_square(),
        Some(cell(0, 0))
    );
}
