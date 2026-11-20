//! STACK COMMANDS: whether a shell line an agent is about to run starts,
//! stops or restarts its checkout's stack — by the commands the checkout's
//! own files name ([`Verbs`]), or a foreground `docker compose up` /
//! `docker compose stop`. Such a line belongs in orion's run line, where
//! the stack's status is kept (`stack`), so the agent's hook turns it away
//! with the `orion run …` that does the same ([`redirect`]).
//!
//! Pure, and deliberately narrow: a line it can't read for certain is
//! left alone. A detached `compose up -d`, `compose down` (it removes what
//! it stops) and a stop of named services are the repo's own business.

use crate::run_targets::Verbs;

/// What a shell line does to the checkout's stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackIntent {
    /// Starts it, by this command (its arguments kept).
    Start(String),
    Stop,
    Restart,
}

/// What `command` does to the stack [`Verbs`] describes, if anything.
/// Its parts (split on `&&`, `||`, `;` and newlines) are read one by one:
/// a stop followed by a start is a restart.
pub fn classify(command: &str, verbs: &Verbs) -> Option<StackIntent> {
    let intents: Vec<StackIntent> = parts(command)
        .into_iter()
        .filter_map(|part| classify_part(&part, verbs))
        .collect();
    match intents.as_slice() {
        [] => None,
        [one] => Some(one.clone()),
        [StackIntent::Stop, StackIntent::Start(_), ..] => Some(StackIntent::Restart),
        [first, ..] => Some(first.clone()),
    }
}

/// The line an agent runs instead of one [`classify`] read as `intent`,
/// and why — the reason its hook gives for turning the line away.
pub fn redirect(intent: &StackIntent) -> String {
    let instead = match intent {
        StackIntent::Start(command) => format!("orion run {command}"),
        StackIntent::Stop => "orion run --stop".to_string(),
        StackIntent::Restart => "orion run --restart".to_string(),
    };
    format!(
        "This checkout's stack is started and stopped through orion's run line, which keeps its status. Run `{instead}` instead (it returns at once; `orion run --status` says whether the stack is up and on which ports)."
    )
}

/// `command`'s parts, each trimmed: split on `&&`, `||`, `;` and
/// newlines, outside quotes.
fn parts(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (Some(_), c) => cur.push(c),
            (None, '\'' | '"') => {
                quote = Some(c);
                cur.push(c);
            }
            (None, ';' | '\n') => out.push(std::mem::take(&mut cur)),
            (None, '&' | '|') if chars.peek() == Some(&c) => {
                chars.next();
                out.push(std::mem::take(&mut cur));
            }
            (None, c) => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// One part's intent: its words past any `VAR=x`, `nohup`, `exec`, `time`
/// or `command`, before any pipe, without redirections or a trailing `&`.
fn classify_part(part: &str, verbs: &Verbs) -> Option<StackIntent> {
    let words = words(part);
    let line = words.join(" ");
    if line.is_empty() {
        return None;
    }
    let names = |list: &[String]| {
        list.iter()
            .any(|v| line == *v || line.starts_with(&format!("{v} ")))
    };
    if names(&verbs.restart) {
        return Some(StackIntent::Restart);
    }
    if names(&verbs.stop) {
        return Some(StackIntent::Stop);
    }
    if names(&verbs.start) {
        return Some(StackIntent::Start(line));
    }
    compose(&words, &line)
}

/// A `docker compose` / `docker-compose` line: a foreground `up` starts
/// the stack, a `stop` of every service stops it.
fn compose(words: &[String], line: &str) -> Option<StackIntent> {
    let rest = match words {
        [docker, compose, rest @ ..] if docker == "docker" && compose == "compose" => rest,
        [compose, rest @ ..] if compose == "docker-compose" => rest,
        _ => return None,
    };
    // Global flags before the subcommand, the ones that take a value
    // skipping it too.
    const VALUED: [&str; 8] = [
        "-f",
        "--file",
        "-p",
        "--project-name",
        "--project-directory",
        "--env-file",
        "--profile",
        "--ansi",
    ];
    let mut i = 0;
    while let Some(w) = rest.get(i).filter(|w| w.starts_with('-')) {
        i += if VALUED.contains(&w.as_str()) { 2 } else { 1 };
    }
    let (sub, args) = (rest.get(i)?, &rest[(i + 1).min(rest.len())..]);
    match sub.as_str() {
        "up" if !args
            .iter()
            .any(|a| a == "-d" || a == "--detach" || a == "--wait" || a.starts_with("--wait=")) =>
        {
            Some(StackIntent::Start(line.to_string()))
        }
        "stop" if names_no_service(args) => Some(StackIntent::Stop),
        _ => None,
    }
}

/// `compose stop`'s arguments name no service: flags only, `-t 5` too.
fn names_no_service(args: &[String]) -> bool {
    let mut args = args.iter();
    while let Some(a) = args.next() {
        if a == "-t" || a == "--timeout" {
            args.next();
        } else if !a.starts_with('-') {
            return false;
        }
    }
    true
}

/// `part`'s words as [`classify_part`] reads them.
fn words(part: &str) -> Vec<String> {
    let before_pipe = top_level_pipe(part).map_or(part, |at| &part[..at]);
    let mut words: Vec<String> = Vec::new();
    let mut tokens = before_pipe.split_whitespace().peekable();
    while let Some(t) = tokens.next() {
        // A redirection, and the file it names when that is a word of
        // its own: `> log`, `2>&1`, `&>/dev/null`.
        if let Some(at) = t.find(['>', '<']) {
            let target = &t[at + 1..].trim_start_matches(['>', '&']);
            if target.is_empty() && !t.ends_with('&') {
                tokens.next();
            }
            if at > 0 && !t[..at].chars().all(|c| c.is_ascii_digit() || c == '&') {
                words.push(t[..at].to_string());
            }
            continue;
        }
        words.push(t.to_string());
    }
    while words.last().is_some_and(|w| w == "&") {
        words.pop();
    }
    if let Some(last) = words.last_mut() {
        if let Some(stripped) = last.strip_suffix('&') {
            *last = stripped.to_string();
        }
    }
    let lead = words
        .iter()
        .take_while(|w| {
            is_assignment(w) || ["nohup", "exec", "time", "command"].contains(&w.as_str())
        })
        .count();
    words.drain(..lead);
    words.retain(|w| !w.is_empty());
    words
}

/// `FOO=bar`: an environment assignment ahead of a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Where `part`'s first pipe outside quotes is.
fn top_level_pipe(part: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in part.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c),
            (None, '|') => return Some(i),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verbs() -> Verbs {
        Verbs {
            start: vec!["just start".into(), "just os start".into()],
            stop: vec!["just stop".into()],
            restart: vec!["just os restart".into()],
        }
    }

    fn start(command: &str) -> Option<StackIntent> {
        Some(StackIntent::Start(command.into()))
    }

    #[test]
    fn the_checkouts_own_verbs_are_read_with_their_arguments() {
        let v = verbs();
        assert_eq!(classify("just start", &v), start("just start"));
        assert_eq!(
            classify("just start --no-open --no-ai", &v),
            start("just start --no-open --no-ai")
        );
        assert_eq!(classify("just os start", &v), start("just os start"));
        assert_eq!(classify("just stop", &v), Some(StackIntent::Stop));
        assert_eq!(classify("just os restart", &v), Some(StackIntent::Restart));
        assert_eq!(classify("just startup", &v), None, "a word, not a prefix");
        assert_eq!(classify("just test", &v), None);
        assert_eq!(classify("orion run just start", &v), None);
    }

    #[test]
    fn wrappers_around_a_verb_are_seen_through() {
        let v = verbs();
        assert_eq!(
            classify("cd /src/wt && just start", &v),
            start("just start")
        );
        assert_eq!(
            classify("PORT=3000 nohup just start > /tmp/log 2>&1 &", &v),
            start("just start")
        );
        assert_eq!(
            classify("just start 2>&1 | tee log", &v),
            start("just start")
        );
        assert_eq!(classify("just start&", &v), start("just start"));
        assert_eq!(classify("just start &>/dev/null", &v), start("just start"));
    }

    #[test]
    fn a_stop_then_a_start_is_a_restart() {
        let v = verbs();
        assert_eq!(
            classify("just stop && just start", &v),
            Some(StackIntent::Restart)
        );
        assert_eq!(
            classify("just stop; sleep 2; just start", &v),
            Some(StackIntent::Restart)
        );
        assert_eq!(classify("npm ci && just start", &v), start("just start"));
    }

    #[test]
    fn quotes_keep_their_operators() {
        let v = verbs();
        assert_eq!(classify("echo 'just stop && just start'", &v), None);
        assert_eq!(classify("git commit -m \"just start; ok\"", &v), None);
    }

    #[test]
    fn a_foreground_compose_up_starts_and_a_full_stop_stops() {
        let v = Verbs::default();
        assert_eq!(
            classify("docker compose up", &v),
            start("docker compose up")
        );
        assert_eq!(
            classify("docker compose -f a.yml -p x up --build", &v),
            start("docker compose -f a.yml -p x up --build")
        );
        assert_eq!(
            classify("docker-compose up", &v),
            start("docker-compose up")
        );
        assert_eq!(classify("docker compose stop", &v), Some(StackIntent::Stop));
        assert_eq!(
            classify("docker compose -p x stop -t 5", &v),
            Some(StackIntent::Stop)
        );
    }

    #[test]
    fn what_compose_does_on_its_own_is_left_alone() {
        let v = Verbs::default();
        assert_eq!(classify("docker compose up -d postgres", &v), None);
        assert_eq!(classify("docker compose up --wait", &v), None);
        assert_eq!(classify("docker compose down", &v), None);
        assert_eq!(classify("docker compose stop postgres", &v), None);
        assert_eq!(classify("docker compose ps", &v), None);
        assert_eq!(classify("docker ps", &v), None);
    }

    #[test]
    fn the_redirect_names_the_exact_line() {
        assert!(redirect(&StackIntent::Start("just start --no-open".into()))
            .contains("`orion run just start --no-open`"));
        assert!(redirect(&StackIntent::Stop).contains("`orion run --stop`"));
        assert!(redirect(&StackIntent::Restart).contains("`orion run --restart`"));
    }
}
