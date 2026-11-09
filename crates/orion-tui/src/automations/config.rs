//! `automations` in `config.json`: how many automation sessions may run
//! at once, and each automation's own settings, keyed by its id
//! ([`super::Automation::id`]). Global, never per project. Every key has a
//! default, so a config with none of them behaves as the Automations
//! screen describes.

use super::Automation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most automation sessions running at once, out of the box.
pub const DEFAULT_MAX_RUNNING: u8 = 5;
/// The cap `←`/`→` steps between on the Automations screen.
pub const MAX_RUNNING_RANGE: std::ops::RangeInclusive<u8> = 1..=20;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AutomationsSettings {
    /// Never start an automation session while this many are running
    /// ([`super::running`]).
    pub max_running: u8,
    /// By automation id. An automation with no entry reads as
    /// [`PerAutomation::default`], its `on` as the automation's own
    /// default ([`Automation::on_by_default`]).
    pub per: BTreeMap<String, PerAutomation>,
}

impl Default for AutomationsSettings {
    fn default() -> Self {
        Self {
            max_running: DEFAULT_MAX_RUNNING,
            per: BTreeMap::new(),
        }
    }
}

/// One automation's settings. The rows it shows are its runner's
/// (`Runner::settings`); the rest stay at their defaults.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct PerAutomation {
    /// On or off. Unset reads as [`Automation::on_by_default`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    /// Appended to the preset's text as `Extra steer: …` ([`super::compose`]).
    pub extra_steer: String,
    /// The Linear labels a ticket needs for the bug fixer or the feature
    /// builder to pick it up.
    pub labels: Vec<String>,
    /// When week in review runs. None is its own default.
    pub when: Option<Schedule>,
    /// Whose work week in review covers.
    pub whose: Whose,
    /// The preset `⌘U` picked in place of the automation's own SYSTEM
    /// PRESET, by name. None, or a name no preset has any more, is the
    /// system preset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

/// A weekly time: `{"day": "fri", "at": "08:30"}`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Schedule {
    pub day: String,
    pub at: String,
}

/// Whose work an automation reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Whose {
    #[default]
    JustMe,
    WholeTeam,
}

impl Whose {
    pub fn label(self) -> &'static str {
        match self {
            Whose::JustMe => "just me",
            Whose::WholeTeam => "whole team",
        }
    }
}

impl AutomationsSettings {
    /// `which`'s settings as they read: its entry, or the defaults.
    pub fn of(&self, which: Automation) -> PerAutomation {
        self.per.get(which.id()).cloned().unwrap_or_default()
    }

    /// `which`'s entry, made if it has none.
    pub fn of_mut(&mut self, which: Automation) -> &mut PerAutomation {
        self.per.entry(which.id().to_string()).or_default()
    }

    /// Whether `which` is on.
    pub fn is_on(&self, which: Automation) -> bool {
        self.per
            .get(which.id())
            .and_then(|p| p.on)
            .unwrap_or(which.on_by_default())
    }

    /// The cap, never below one: a hand-edited 0 would stop every
    /// automation without saying so.
    pub fn cap(&self) -> usize {
        usize::from(self.max_running.max(*MAX_RUNNING_RANGE.start()))
    }

    /// `←`/`→` on the screen's header: the cap a step down or up, kept in
    /// [`MAX_RUNNING_RANGE`].
    pub fn step_cap(&mut self, step: i8) {
        let (lo, hi) = (*MAX_RUNNING_RANGE.start(), *MAX_RUNNING_RANGE.end());
        self.max_running = self.max_running.saturating_add_signed(step).clamp(lo, hi);
    }
}
