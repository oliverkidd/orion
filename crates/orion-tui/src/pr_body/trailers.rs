//! HOOK TRAILERS: the `Hook:` lines the repo's commit skill writes into a
//! commit message when a hook didn't run as it should —
//! `Hook: e2e skipped (no browser on this machine)`. One that says
//! `skipped` sends the PR to Review (`super::verdict`).

/// One `Hook:` trailer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hook {
    /// What follows `Hook:`.
    pub text: String,
    pub skipped: bool,
}

/// The `Hook:` trailers across `messages` (each commit's full message),
/// without repeats, in commit order.
pub fn hooks<'a>(messages: impl IntoIterator<Item = &'a str>) -> Vec<Hook> {
    let mut out: Vec<Hook> = Vec::new();
    for message in messages {
        for line in message.lines() {
            let Some((key, text)) = line.trim().split_once(':') else {
                continue;
            };
            if !key.trim().eq_ignore_ascii_case("hook") {
                continue;
            }
            let text = text.trim().to_string();
            if text.is_empty() || out.iter().any(|h| h.text == text) {
                continue;
            }
            let skipped = text
                .split(|c: char| !c.is_ascii_alphanumeric())
                .any(|w| w.eq_ignore_ascii_case("skipped"));
            out.push(Hook { text, skipped });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_trailers_are_found_and_skips_flagged() {
        let messages = [
            "fix: login loop\n\nBody text that mentions a hook: not a trailer line? It is not at a line start.\n\nHook: e2e skipped (no browser)\nCo-Authored-By: x",
            "chore: more\n\nhook: lint ran\nHook: e2e skipped (no browser)",
            "Hook:",
        ];
        let found = hooks(messages);
        assert_eq!(
            found,
            [
                Hook {
                    text: "e2e skipped (no browser)".into(),
                    skipped: true
                },
                Hook {
                    text: "lint ran".into(),
                    skipped: false
                },
            ]
        );
    }

    #[test]
    fn skipped_must_be_a_word() {
        assert!(!hooks(["Hook: unskippedness"])[0].skipped);
        assert!(hooks(["Hook: pre-push SKIPPED"])[0].skipped);
    }
}
