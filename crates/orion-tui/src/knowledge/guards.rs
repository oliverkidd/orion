//! The GUARDS a harvest's entries pass, applied by code and not by the
//! model: an entry credited to the owner must quote him word for word,
//! one-off authorisations are never precedent, and secrets and emails
//! never reach the store.

use super::store::Entry;

/// Rules that are one-off authorisations, never precedent.
const ONE_OFF: &[&str] = &[
    "--no-verify",
    "no-verify",
    "push it",
    "run the e2e",
    "run e2e",
    "just one pr",
    "one pr here",
    "dismiss",
    "force push",
    "force-push",
    "merge it",
    "ship it",
    "skip the hook",
];

/// Words an authorisation is made of: a quote of nothing else only says
/// "go".
const GO_WORDS: &[&str] = &[
    "yes", "yeah", "yep", "y", "ok", "okay", "k", "sure", "go", "ahead", "do", "it", "approved",
    "approve", "lgtm", "ship", "push", "merge", "proceed", "continue", "sounds", "looks", "good",
    "fine", "great", "please", "that", "thats", "that's", "run", "the", "e2e", "now", "and",
    "just", "carry", "on",
];

/// Prefixes of tokens that are credentials.
const SECRET_PREFIXES: &[&str] = &[
    "sk-",
    "sk_live_",
    "sk_test_",
    "rk_live_",
    "ghp_",
    "gho_",
    "ghs_",
    "ghu_",
    "github_pat_",
    "xoxb-",
    "xoxp-",
    "xoxa-",
    "xapp-",
    "lin_api_",
    "AKIA",
    "AIza",
    "glpat-",
    "npm_",
    "eyJ",
];

/// Names whose `name=value` value is a credential.
const SECRET_NAMES: &[&str] = &["key", "secret", "token", "password", "passwd", "pwd"];

/// What the guards did to a batch.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Guarded {
    pub kept: Vec<Entry>,
    /// Why each dropped entry was dropped, one line each.
    pub dropped: Vec<String>,
}

/// Run every guard over `entries`, with `owner_lines` the texts the owner
/// said (the dossier's OWNER lines).
pub(crate) fn apply(entries: Vec<Entry>, owner_lines: &[String]) -> Guarded {
    let mut out = Guarded::default();
    for mut entry in entries {
        scrub_entry(&mut entry);
        if entry.rule.trim().is_empty() {
            out.dropped.push("an entry with no rule".into());
            continue;
        }
        if is_one_off(&entry.rule) {
            out.dropped
                .push(format!("one-off authorisation: {}", entry.rule));
            continue;
        }
        if is_authorisation(&entry.quote) {
            out.dropped
                .push(format!("quote is only an authorisation: {}", entry.rule));
            continue;
        }
        match entry.by.as_str() {
            "owner" if !quoted(&entry.quote, owner_lines) => {
                entry.by = "agent".into();
                entry.confidence = "low".into();
                entry.downgraded = true;
            }
            "agent" => entry.confidence = "low".into(),
            "plan" if entry.confidence == "high" => entry.confidence = "medium".into(),
            _ => {}
        }
        out.kept.push(entry);
    }
    out
}

/// Whether `quote` appears, whitespace-normalised, in one of the owner's
/// lines. A quote cut with `…` must have every piece in the same line.
pub(crate) fn quoted(quote: &str, owner_lines: &[String]) -> bool {
    let quote = normalise(quote);
    let quote = quote.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    let pieces: Vec<&str> = quote
        .split('…')
        .flat_map(|p| p.split("..."))
        .flat_map(|p| p.split(" / "))
        .map(|p| p.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace()))
        .filter(|p| !p.is_empty())
        .collect();
    if pieces.iter().map(|p| p.chars().count()).sum::<usize>() < 4 {
        return false;
    }
    owner_lines.iter().any(|line| {
        let line = normalise(line);
        pieces.iter().all(|p| line.contains(p))
    })
}

/// Lowercase, typographic quotes made plain, every run of whitespace one
/// space, and a leading `>` dropped from each line.
fn normalise(text: &str) -> String {
    let plain: String = text
        .lines()
        .map(|l| l.trim_start().trim_start_matches('>'))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .map(|c| match c {
            '‘' | '’' => '\'',
            '“' | '”' => '"',
            c => c,
        })
        .collect();
    plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Whether `rule` is one of the one-off authorisations.
pub(crate) fn is_one_off(rule: &str) -> bool {
    let rule = rule.to_lowercase();
    ONE_OFF.iter().any(|o| rule.contains(o))
}

/// Whether `quote` only says "go": a few words, all of them assent.
pub(crate) fn is_authorisation(quote: &str) -> bool {
    let words: Vec<String> = quote
        .split(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == '!')
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect();
    !words.is_empty() && words.len() <= 6 && words.iter().all(|w| GO_WORDS.contains(&w.as_str()))
}

/// Every text field of `entry` with secrets and emails taken out.
fn scrub_entry(entry: &mut Entry) {
    for field in [
        &mut entry.rule,
        &mut entry.why,
        &mut entry.quote,
        &mut entry.confirm_by,
        &mut entry.source_at,
    ] {
        *field = scrub(field);
    }
    for list in [&mut entry.applies_to, &mut entry.options_rejected] {
        for item in list.iter_mut() {
            *item = scrub(item);
        }
    }
}

/// `text` with emails as `[email]`, credentials as `[secret]` and
/// private key blocks gone.
pub(crate) fn scrub(text: &str) -> String {
    let text = strip_key_blocks(text);
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for c in text.chars() {
        if c.is_whitespace() {
            out.push_str(&scrub_word(&word));
            word.clear();
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&scrub_word(&word));
    out
}

/// One whitespace-free word, scrubbed, its surrounding punctuation kept.
fn scrub_word(word: &str) -> String {
    const EDGE: &[char] = &[
        '"', '\'', '`', '(', ')', '<', '>', '[', ']', ',', ';', '{', '}',
    ];
    let core = word.trim_matches(EDGE);
    if core.is_empty() {
        return word.to_string();
    }
    let start = word.find(core).unwrap_or(0);
    let (head, tail) = (&word[..start], &word[start + core.len()..]);
    let core = core.trim_end_matches(['.', ':', '!', '?']);
    let rest = &word[start + core.len()..word.len() - tail.len()];
    let scrubbed = if is_email(core) {
        "[email]".to_string()
    } else if is_secret(core) {
        "[secret]".to_string()
    } else if let Some(name) = named_secret(core) {
        format!("{name}[secret]")
    } else {
        core.to_string()
    };
    format!("{head}{scrubbed}{rest}{tail}")
}

/// `local@domain.tld`.
fn is_email(word: &str) -> bool {
    let Some((local, domain)) = word.split_once('@') else {
        return false;
    };
    let mailbox = |c: char| c.is_alphanumeric() || "._%+-".contains(c);
    !local.is_empty()
        && local.chars().all(mailbox)
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && domain
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
}

/// A token that starts like a credential and is long enough to be one.
fn is_secret(word: &str) -> bool {
    word.len() >= 16
        && SECRET_PREFIXES.iter().any(|p| word.starts_with(p))
        && word
            .chars()
            .all(|c| c.is_alphanumeric() || "-_.".contains(c))
}

/// `NAME=value` or `name:value` where the name says credential: the name
/// with its separator, the value being what goes.
fn named_secret(word: &str) -> Option<&str> {
    let split = word.find(['=', ':'])?;
    let (name, value) = (&word[..=split], &word[split + 1..]);
    let bare = name[..name.len() - 1].to_lowercase();
    let says = SECRET_NAMES.iter().any(|n| bare.ends_with(n));
    (says && value.len() >= 6 && !value.starts_with("//")).then_some(name)
}

/// `text` without its `-----BEGIN … PRIVATE KEY----- … -----END …-----`
/// blocks.
fn strip_key_blocks(text: &str) -> String {
    let mut out = text.to_string();
    while let Some(start) = out.find("-----BEGIN") {
        let Some(head_end) = out[start + 10..].find("-----").map(|i| start + 10 + i + 5) else {
            break;
        };
        if !out[start..head_end].contains("PRIVATE KEY") {
            break;
        }
        let end = out[head_end..]
            .find("-----END")
            .and_then(|i| {
                let from = head_end + i + 8;
                out[from..].find("-----").map(|j| from + j + 5)
            })
            .unwrap_or(out.len());
        out.replace_range(start..end, "[secret]");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(by: &str, rule: &str, quote: &str) -> Entry {
        Entry {
            by: by.into(),
            rule: rule.into(),
            quote: quote.into(),
            confidence: "high".into(),
            ..Entry::default()
        }
    }

    fn owner() -> Vec<String> {
        vec![
            "Fix the login redirect. Never send users to /home after login,\n  always back where they were.".into(),
            "yes".into(),
        ]
    }

    #[test]
    fn an_owner_quote_must_be_his_words() {
        let lines = owner();
        assert!(quoted(
            "never send users to /home after login, always back",
            &lines
        ));
        assert!(quoted(
            "“Never send users to /home after login,   always back where they were.”",
            &lines
        ));
        assert!(quoted(
            "Fix the login redirect … always back where they were",
            &lines
        ));
        assert!(!quoted("Never send users to /dashboard", &lines));
        assert!(!quoted("Fix the login … never mind", &lines));
        assert!(!quoted("", &lines));
    }

    #[test]
    fn an_unquoted_owner_entry_is_downgraded_to_the_agent() {
        let out = apply(
            vec![
                entry(
                    "owner",
                    "Login returns to the page the user came from",
                    "always back where they were",
                ),
                entry(
                    "owner",
                    "Login goes to the dashboard",
                    "send them to the dashboard",
                ),
                entry("plan", "Store the return path in the session", ""),
            ],
            &owner(),
        );
        assert!(out.dropped.is_empty());
        assert_eq!(out.kept[0].by, "owner");
        assert!(!out.kept[0].downgraded);
        assert_eq!(
            (out.kept[1].by.as_str(), out.kept[1].confidence.as_str()),
            ("agent", "low")
        );
        assert!(out.kept[1].downgraded);
        assert_eq!(out.kept[2].confidence, "medium");
    }

    #[test]
    fn one_off_authorisations_are_never_kept() {
        let out = apply(
            vec![
                entry(
                    "owner",
                    "Use --no-verify when the hook is slow",
                    "use --no-verify",
                ),
                entry("owner", "Just one PR for this change", "just one pr here"),
                entry(
                    "owner",
                    "Dismiss the Cubic P1 on generated files",
                    "dismiss it",
                ),
                entry("owner", "The plan is approved as written", "yes"),
                entry("owner", "Ship the branch now", "go ahead, push it"),
                entry(
                    "owner",
                    "Login keeps the query string",
                    "always back where they were",
                ),
            ],
            &owner(),
        );
        assert_eq!(out.dropped.len(), 5, "{:?}", out.dropped);
        assert_eq!(out.kept.len(), 1);
        assert!(is_authorisation("OK, do it!"));
        assert!(!is_authorisation("do it with the shared date picker"));
        assert!(!is_authorisation(""));
    }

    #[test]
    fn secrets_and_emails_are_stripped() {
        assert_eq!(
            scrub("ask (jane.doe@client.com) for ghp_abcdefghijklmnopqrstuvwx, token=s3cr3tvalue."),
            "ask ([email]) for [secret], token=[secret]."
        );
        assert_eq!(
            scrub("API_KEY: sk-live-1234567890abcdef stays out"),
            "API_KEY: [secret] stays out"
        );
        assert_eq!(
            scrub("a\n-----BEGIN RSA PRIVATE KEY-----\nMIIabc\n-----END RSA PRIVATE KEY-----\nb"),
            "a\n[secret]\nb"
        );
        let plain = "see docs/auth.md at https://example.com/a?b=c and fix #123";
        assert_eq!(scrub(plain), plain);
        let mut e = entry("owner", "mail ops@riplo.ai first", "");
        e.applies_to = vec!["bob@x.io".into()];
        scrub_entry(&mut e);
        assert_eq!(e.rule, "mail [email] first");
        assert_eq!(e.applies_to, vec!["[email]"]);
    }
}
