//! Definition registration and runtime availability are separate during staged development.

use super::CUSTOM_RULES_VERSION;

pub const LEGACY_PREVIEW_VERSION: &str = "v1";
pub const PREVIEW_VERSION: &str = "friend_room_empty_v1";
pub const LEGACY_SNAPSHOT_KIND: &str = "friend_room_empty_v1";
pub const GAME_SNAPSHOT_KIND: &str = "patchwork_game_v1";
pub const GAME_SNAPSHOT_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisteredRules {
    Preview,
    CustomV1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegistryError {
    UnknownRules,
    EngineNotReady,
    IncompatibleSnapshot,
}

pub fn resolve(version: &str) -> Result<RegisteredRules, RegistryError> {
    match version {
        LEGACY_PREVIEW_VERSION | PREVIEW_VERSION => Ok(RegisteredRules::Preview),
        CUSTOM_RULES_VERSION => Ok(RegisteredRules::CustomV1),
        _ => Err(RegistryError::UnknownRules),
    }
}

/// Startable versions supported by the T38 authority; unknown versions remain closed.
pub fn require_startable(version: &str) -> Result<RegisteredRules, RegistryError> {
    match resolve(version)? {
        RegisteredRules::Preview => Ok(RegisteredRules::Preview),
        RegisteredRules::CustomV1 => Ok(RegisteredRules::CustomV1),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotFormat {
    LegacyPreview,
    CustomV1,
}

/// Classify a stored envelope before decoding its body, not a validator for its game state.
/// Historical previews may carry old arbitrary labels. They never become a real game by
/// relabelling: custom rules require both the dedicated kind and an explicit schema version.
pub fn snapshot_format(
    version: &str,
    kind: &str,
    schema: Option<u32>,
) -> Result<SnapshotFormat, RegistryError> {
    match (version, kind, schema) {
        (CUSTOM_RULES_VERSION, GAME_SNAPSHOT_KIND, Some(GAME_SNAPSHOT_SCHEMA)) => {
            Ok(SnapshotFormat::CustomV1)
        }
        (version, LEGACY_SNAPSHOT_KIND, None)
            if !version.is_empty() && version != CUSTOM_RULES_VERSION =>
        {
            Ok(SnapshotFormat::LegacyPreview)
        }
        _ => Err(RegistryError::IncompatibleSnapshot),
    }
}
