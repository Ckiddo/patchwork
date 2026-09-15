//! Runtime policy. Durations are charged only between healthy room observations.
use serde::Deserialize;
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RecoveryConfig {
    pub waiting_grace_secs: u64,
    pub game_budget_secs: u64,
    pub both_offline_retention_secs: u64,
    pub restart_grace_secs: u64,
}
impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            waiting_grace_secs: 60,
            game_budget_secs: 120,
            both_offline_retention_secs: 600,
            restart_grace_secs: 120,
        }
    }
}
impl RecoveryConfig {
    pub fn valid(&self) -> bool {
        [
            self.waiting_grace_secs,
            self.game_budget_secs,
            self.both_offline_retention_secs,
            self.restart_grace_secs,
        ]
        .into_iter()
        .all(|s| (1..=86400).contains(&s))
    }
}
