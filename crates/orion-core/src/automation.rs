//! The AUTOMATIONS' ids and names: the tag a session an automation
//! started carries (`Agent::automation`) and what it is called wherever a
//! session's kind is shown — the TUI's grid and Needs you, the DAEMON's
//! phone messages. The one table both read; the TUI's
//! `automations::Automation` is the registry built on it.

/// `(id, name)` for every automation, in the order the Automations screen
/// lists them.
pub const ALL: [(&str, &str); 6] = [
    ("pr_fixer", "PR fixer"),
    ("bug_fixer", "Bug fixer"),
    ("feature_builder", "Feature builder"),
    ("week_review", "Week in review"),
    ("meeting_takeaways", "Meeting takeaways"),
    ("blue_sky", "Blue sky builder"),
];

/// The name of the automation tagged `id`, if one is.
pub fn name(id: &str) -> Option<&'static str> {
    ALL.iter()
        .find(|(known, _)| *known == id.trim())
        .map(|(_, name)| *name)
}

/// What a session is called by kind: its automation's name, else `agent`.
pub fn kind_label(tag: Option<&str>) -> &'static str {
    tag.and_then(name).unwrap_or("agent")
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_tag_reads_as_its_name_and_none_as_agent() {
        assert_eq!(super::kind_label(Some("pr_fixer")), "PR fixer");
        assert_eq!(super::kind_label(Some("nope")), "agent");
        assert_eq!(super::kind_label(None), "agent");
    }
}
