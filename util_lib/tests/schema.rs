mod support;

#[test]
fn schema_matches_reviewed_v1_baseline() {
    assert_eq!(
        support::schema_manifest(),
        include_str!("../proto/v1.schema.txt").replace("\r\n", "\n"),
        "protocol changed: review compatibility and explicitly regenerate the descriptor baseline"
    );
}
