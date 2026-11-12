//! `automations` in `config.json`: how many automation sessions may run
//! at once, all of them together and each on its own, and each
//! automation's own settings, keyed by its id ([`super::Automation::id`]).
//! Global, never per project. Every key has a default, so a config with
//! none of them behaves as Settings › Automations describes.

use super::Automation;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

/// The most automation sessions running at once, out of the box. A saved
/// `max_running` is kept as it is, whatever this says.
pub const DEFAULT_MAX_RUNNING: u8 = 3;
/// The overall cap `←`/`→` step through in Settings › Automations.
pub const MAX_RUNNING_RANGE: RangeInclusive<u8> = 1..=20;
/// One automation's own cap: `←`/`→` step through these, and a step up
/// from the top is "any".
pub const OWN_CAP_RANGE: RangeInclusive<u8> = 1..=10;
/// A saved [`PerAutomation::max_running`] of this is "any": only the
/// overall cap applies.
pub const ANY: u8 = 0;

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
    /// Whether automation agents run end-to-end tests on this machine
    /// ([`LocalE2e::rule`], added to every automation agent's prompt).
    pub local_e2e: LocalE2e,
}

/// End-to-end tests on this machine, for every automation agent: never
/// (CI runs them on the pull request), or only the specs that cover the
/// change. Several agents each running a full e2e suite at once is what
/// grinds a laptop to a halt, so never is the default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalE2e {
    #[default]
    Never,
    Covering,
}

impl LocalE2e {
    pub fn label(self) -> &'static str {
        match self {
            LocalE2e::Never => "never, CI runs them",
            LocalE2e::Covering => "only the specs that cover the change",
        }
    }

    /// The other choice: there are two.
    pub fn next(self) -> Self {
        match self {
            LocalE2e::Never => LocalE2e::Covering,
            LocalE2e::Covering => LocalE2e::Never,
        }
    }

    /// What every automation agent that writes code is told about
    /// testing, after its preset's text.
    pub fn rule(self) -> &'static str {
        match self {
            LocalE2e::Never => "Testing on this machine: never run end-to-end (e2e) tests or the full test suite here; CI runs them on the pull request. Run only the unit tests for the code you changed. When an e2e check fails in CI, work from its logs, push the fix and let CI run it again.",
            LocalE2e::Covering => "Testing on this machine: run the unit tests for the code you changed, and only the e2e specs that cover the change, never the full e2e suite. When an e2e check fails in CI, re-run only its failing specs locally.",
        }
    }
}

impl Default for AutomationsSettings {
    fn default() -> Self {
        Self {
            max_running: DEFAULT_MAX_RUNNING,
            per: BTreeMap::new(),
            local_e2e: LocalE2e::default(),
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
    /// The most of this automation's sessions running at once, [`ANY`]
    /// for no cap of its own. None is its own default
    /// ([`Automation::default_cap`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_running: Option<u8>,
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

    /// `which`'s own cap: its saved one, else its default. None is "any",
    /// only the overall cap applying.
    pub fn cap_of(&self, which: Automation) -> Option<usize> {
        match self.per.get(which.id()).and_then(|p| p.max_running) {
            Some(ANY) => None,
            Some(n) => Some(usize::from(n)),
            None => which.default_cap().map(usize::from),
        }
    }

    /// `←`/`→` on **Sessions at once**: the overall cap a step down or up,
    /// kept in [`MAX_RUNNING_RANGE`].
    pub fn step_cap(&mut self, step: i8) {
        self.max_running = stepped(self.max_running, step, &MAX_RUNNING_RANGE);
    }

    /// `←`/`→` on `which`'s **At most at once**: its own cap a step down
    /// or up through [`OWN_CAP_RANGE`], "any" above the top.
    pub fn step_cap_of(&mut self, which: Automation, step: i8) {
        let top = *OWN_CAP_RANGE.end();
        let now = self
            .cap_of(which)
            .map_or(ANY, |n| u8::try_from(n).unwrap_or(top).min(top));
        let next = match (now, step.signum()) {
            (ANY, -1) => top,
            (ANY, _) => ANY,
            (n, 1) if n == top => ANY,
            (n, _) => stepped(n, step, &OWN_CAP_RANGE),
        };
        self.of_mut(which).max_running = Some(next);
    }
}

/// `value` a `step` down or up, kept in `range`: the one stepper both caps
/// go through.
pub fn stepped(value: u8, step: i8, range: &RangeInclusive<u8>) -> u8 {
    value
        .saturating_add_signed(step)
        .clamp(*range.start(), *range.end())
}

/// A cap as its row reads: the number, or "any".
pub fn cap_label(cap: Option<usize>) -> String {
    cap.map_or_else(|| "any".into(), |n| n.to_string())
}
