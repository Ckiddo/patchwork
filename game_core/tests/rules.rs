use game_core::{
    Seat,
    rules::{registry::*, *},
};
use serde_json::{Value, json};

#[test]
fn frozen_definition_matches_reviewed_v1_fixture() {
    let frozen: Value =
        serde_json::from_str(include_str!("fixtures/patchwork_custom_v1.json")).unwrap();
    assert_eq!(
        json!({"rules": CUSTOM_V1, "patches": PATCHES.as_slice()}),
        frozen,
        "released data must receive a new version instead of silently changing v1"
    );
    validate_catalog(&PATCHES).unwrap();
    CUSTOM_V1.track.validate().unwrap();
    assert_eq!(PATCHES.iter().map(|p| p.cells.len()).sum::<usize>(), 166);
}

#[test]
fn stable_ids_are_independent_of_catalog_order_and_reject_external_indices() {
    let start = patch(STARTING_PATCH_ID).unwrap();
    assert_eq!(start.cells, [(0, 0), (1, 0)]);
    assert_eq!(
        (start.button_cost, start.time_cost, start.income),
        (2, 1, 0)
    );
    for id in [0, 34, u32::MAX] {
        assert!(patch(PatchId(id)).is_none());
    }
    let mut reordered = PATCHES;
    reordered.reverse();
    validate_catalog(&reordered).unwrap();
    assert_eq!(
        reordered.iter().find(|p| p.id == STARTING_PATCH_ID),
        Some(start)
    );
}

#[test]
fn missing_duplicate_and_invalid_definitions_are_rejected() {
    assert_eq!(
        validate_catalog(&PATCHES[..32]),
        Err(DefinitionError::WrongPatchCount)
    );
    let mut patches = PATCHES;
    patches[1].id = patches[0].id;
    assert_eq!(
        validate_catalog(&patches),
        Err(DefinitionError::InvalidOrDuplicateId(PatchId(1)))
    );
    for id in [0, 34, u32::MAX] {
        let mut patches = PATCHES;
        patches[0].id = PatchId(id);
        assert_eq!(
            validate_catalog(&patches),
            Err(DefinitionError::InvalidOrDuplicateId(PatchId(id)))
        );
    }
    for cells in [
        &[][..],
        &[(0, 0)],
        &[(0, 0), (0, 0)],
        &[(0, 0), (2, 0)],
        &[(0, 0), (9, 0)],
        &[(0, 0), (0, 255)],
        &[(1, 1), (1, 2)],
    ] {
        let mut patches = PATCHES;
        patches[0].cells = cells;
        assert_eq!(
            validate_catalog(&patches),
            Err(DefinitionError::InvalidShape(PatchId(1)))
        );
    }
    for time in [0, 54, 255] {
        let mut patches = PATCHES;
        patches[0].time_cost = time;
        assert_eq!(
            validate_catalog(&patches),
            Err(DefinitionError::InvalidTimeCost(PatchId(1)))
        );
    }
}

#[test]
fn exactly_one_domino_must_have_the_starting_id() {
    let mut patches = PATCHES;
    patches[0].cells = &[(0, 0), (1, 0)];
    assert_eq!(
        validate_catalog(&patches),
        Err(DefinitionError::InvalidStartingPatch)
    );
    let mut patches = PATCHES;
    patches[9].cells = &[(0, 0), (1, 0), (2, 0)];
    assert_eq!(
        validate_catalog(&patches),
        Err(DefinitionError::InvalidStartingPatch)
    );
    let mut patches = PATCHES;
    patches.swap(0, 9);
    patches[0].id = PatchId(1);
    patches[9].id = STARTING_PATCH_ID;
    assert_eq!(
        validate_catalog(&patches),
        Err(DefinitionError::InvalidStartingPatch)
    );
}

#[test]
fn equal_positions_switch_normal_actor_but_initial_first_player_is_preserved() {
    let rule = CUSTOM_V1.same_position_turn;
    assert_eq!(rule.actor(None, Seat::First), Seat::First);
    assert_eq!(rule.actor(None, Seat::Second), Seat::Second);
    assert_eq!(rule.actor(Some(Seat::First), Seat::First), Seat::Second);
    assert_eq!(rule.actor(Some(Seat::Second), Seat::Second), Seat::First);
}

#[test]
fn scoring_has_no_empty_cell_penalty_and_does_not_overflow_player_balance() {
    let rule = CUSTOM_V1.scoring;
    assert_eq!(rule.final_score(5, false), 5);
    assert_eq!(rule.final_score(5, true), 12);
    assert_eq!(rule.final_score(0, false), 0);
    assert_eq!(rule.final_score(u32::MAX, true), u64::from(u32::MAX) + 7);
    assert_eq!(rule.final_score(12, false), rule.final_score(5, true));
    assert_eq!(rule.tied_score, TiedScore::Draw);
    assert_eq!(rule.empty_cell_penalty, 0);
    assert!(!rule.bonus_is_spendable);
    assert_eq!(
        CUSTOM_V1.special_patch.unplaceable,
        UnplaceableSpecialPatch::DiscardAndRecord
    );
}

#[test]
fn marker_crossing_excludes_departure_includes_arrival_and_clamps_overshoot() {
    let track = CUSTOM_V1.track;
    assert!(track.crosses(3, 4, 4));
    assert!(!track.crosses(4, 5, 4));
    assert!(!track.crosses(4, 4, 4));
    assert!(!track.crosses(10, 4, 4));
    assert!(!track.crosses(54, 55, 55));
    let incomes: Vec<_> = track
        .income_positions
        .iter()
        .copied()
        .filter(|&p| track.crosses(3, 23, p))
        .collect();
    assert_eq!(incomes, [4, 10, 16, 22]);
    let specials: Vec<_> = track
        .special_positions
        .iter()
        .copied()
        .filter(|&p| track.crosses(18, 32, p))
        .collect();
    assert_eq!(specials, [19, 25, 31]);
    let incomes: Vec<_> = track
        .income_positions
        .iter()
        .copied()
        .filter(|&p| track.crosses(50, 57, p))
        .collect();
    assert_eq!(incomes, [52]);
    assert!(!track.crosses(50, u16::MAX, 54));
    assert!(!track.income_positions.contains(&53));
}

#[test]
fn malformed_tracks_fail_before_a_game_can_use_them() {
    for positions in [&[4, 4][..], &[10, 4], &[0, 4], &[54]] {
        let mut track = CUSTOM_V1.track;
        track.income_positions = positions;
        assert_eq!(track.validate(), Err(DefinitionError::InvalidTrack));
    }
    let mut track = CUSTOM_V1.track;
    track.special_positions = &[4];
    assert_eq!(track.validate(), Err(DefinitionError::InvalidTrack));
    track = CUSTOM_V1.track;
    track.end = 0;
    assert_eq!(track.validate(), Err(DefinitionError::InvalidTrack));
}

#[test]
fn registered_definitions_only_enable_supported_engines() {
    assert_eq!(resolve(CUSTOM_RULES_VERSION), Ok(RegisteredRules::CustomV1));
    assert_eq!(
        require_startable(CUSTOM_RULES_VERSION),
        Ok(RegisteredRules::CustomV1)
    );
    for version in [LEGACY_PREVIEW_VERSION, PREVIEW_VERSION] {
        assert_eq!(require_startable(version), Ok(RegisteredRules::Preview));
    }
    for version in [
        "",
        "v2",
        "room-preview-v2",
        "patchwork_custom_v2",
        "PATCHWORK_CUSTOM_V1",
        " v1",
        "v1 ",
    ] {
        assert_eq!(resolve(version), Err(RegistryError::UnknownRules));
        assert_eq!(require_startable(version), Err(RegistryError::UnknownRules));
    }
}

#[test]
fn legacy_snapshots_are_readable_but_never_reinterpreted_as_custom_games() {
    assert_eq!(
        snapshot_format("old-arbitrary-label", LEGACY_SNAPSHOT_KIND, None),
        Ok(SnapshotFormat::LegacyPreview)
    );
    assert_eq!(
        snapshot_format(CUSTOM_RULES_VERSION, GAME_SNAPSHOT_KIND, Some(1)),
        Ok(SnapshotFormat::CustomV1)
    );
    for (version, kind, schema) in [
        (CUSTOM_RULES_VERSION, LEGACY_SNAPSHOT_KIND, None),
        (LEGACY_PREVIEW_VERSION, GAME_SNAPSHOT_KIND, Some(1)),
        ("unknown", GAME_SNAPSHOT_KIND, Some(1)),
        (CUSTOM_RULES_VERSION, GAME_SNAPSHOT_KIND, None),
        (CUSTOM_RULES_VERSION, GAME_SNAPSHOT_KIND, Some(0)),
        (CUSTOM_RULES_VERSION, GAME_SNAPSHOT_KIND, Some(2)),
        (CUSTOM_RULES_VERSION, "unknown", Some(1)),
    ] {
        assert_eq!(
            snapshot_format(version, kind, schema),
            Err(RegistryError::IncompatibleSnapshot)
        );
    }
}
