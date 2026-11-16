//! ONE-SHOT MODEL RUNNER: one ask of a model, text in and text out,
//! through one of the Claude accounts orion runs sessions on. Nothing
//! runs in a checkout, no session appears on the grid and nothing is
//! left to tidy up: the account's CLI in print mode with no tools
//! ([`run`]), or the same asked for JSON and parsed ([`run_json`]). A
//! harvest's check reads code: it runs in a folder, with the read-only
//! tools it names ([`run_in`]).
//!
//! Every model call orion makes outside a session goes through here —
//! the week in review, the automations' briefs, the meeting takeaways —
//! so the command, its account and its failures are said one way.

use std::path::Path;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::agent_presets::AgentPreset;
use crate::config::Config;
use crate::ui::truncate;

/// How much of a failure's first line is worth showing.
const WHY_CUT: usize = 120;

/// What [`run_json`] adds to the system prompt: the reply is parsed.
/// Prose or a fence around the JSON is read past, but costs nothing to
/// ask against.
const JSON_ONLY: &str = "Reply with the JSON value only: no prose, no code fence.";

/// The Claude account an ask goes through: one of the accounts orion runs
/// sessions on — its own program, with the config dir that pins it — run
/// directly, as the sign-in modal runs `claude auth`. A shell alias for
/// the program is not followed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Account {
    /// The account's own CLI: `claude`, or what its `harnesses` row names.
    pub program: String,
    /// Its environment on top of orion's — the `CLAUDE_CONFIG_DIR` that
    /// pins an extra account to its own sign-in.
    pub env: Vec<(String, String)>,
    /// `Work (a@b.co)`, or `Work (not signed in)`: what a screen calls
    /// it. Being signed out is said, never refused on: the record only
    /// knows a browser sign-in, and an account on an API key answers all
    /// the same. One that cannot answer says so itself.
    pub label: String,
}

/// One ask: the prompt, the rules it is answered by, and who answers it.
#[derive(Debug, Clone)]
pub struct Ask {
    /// Sent on stdin, so a long one never meets an argv limit.
    pub prompt: String,
    /// The `--system-prompt`.
    pub system: String,
    pub account: Account,
    pub model: String,
    pub effort: String,
    /// How long the model may take before the ask fails as timed out.
    pub timeout: Duration,
}

/// What [`run`] got back.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    /// What the ask cost, in US dollars, when the command said.
    pub cost: Option<f64>,
}

/// Whether background work that calls out — a model, `gh`, Linear — is
/// off: under test, or while `ORION_AGENT_CMD` stands in for the agents
/// (the e2e tests).
pub(crate) fn offline() -> bool {
    cfg!(test) || orion_core::env::non_empty(orion_core::env::AGENT_CMD).is_some()
}

impl Account {
    /// The account a Claude harness row is: its CLI, its environment and
    /// its label.
    pub fn of(entry: &orion_core::harness::HarnessDescriptor) -> Self {
        Account {
            program: entry.program.trim().to_string(),
            env: entry.launch_env(),
            label: entry.display_label().to_string(),
        }
    }
}

/// The account `cfg` answers an automation `preset`'s asks through: the
/// Claude account the preset pins (`Config::review_account`), or the
/// factory account when it follows the box ([`factory_account`]). None
/// with no Claude account switched on.
pub fn account_for(cfg: &Config, preset: &AgentPreset) -> Option<Account> {
    match preset.follows_box() {
        true => factory_account(cfg),
        false => through(
            cfg,
            orion_core::harness::registry_id(preset.kind, preset.custom_harness.as_deref()),
        ),
    }
}

/// The factory's own account — what the harvest, the backfill and a
/// ticket's intake ask through: **Factory account** in Settings ›
/// Factory, else the default agent's (`automations::account::ask_entry`).
/// None with no Claude account switched on.
pub fn factory_account(cfg: &Config) -> Option<Account> {
    crate::automations::account::ask_entry(cfg).map(|entry| Account::of(&entry))
}

/// The default agent's Claude account — what an ask with no preset of its
/// own goes through (a project's **What it is**). None with no Claude
/// account switched on.
pub fn default_account(cfg: &Config) -> Option<Account> {
    through(cfg, "")
}

/// The Claude account `pinned` names, else the default agent's.
fn through(cfg: &Config, pinned: &str) -> Option<Account> {
    cfg.review_account(pinned).map(|entry| Account::of(&entry))
}

/// Send `ask.prompt` to `ask.model` at `ask.effort` through the account's
/// CLI in print mode, with no tools, under that account's sign-in. The
/// failure is the one line worth showing — the command missing, not
/// signed in, timed out.
pub async fn run(ask: Ask) -> Result<Reply, String> {
    run_in(ask, None, None).await
}

/// [`run`], in `cwd` when given, with `tools` (`Read,Grep,Glob`) allowed
/// and nothing else; None allows none. While the factory is paused on the
/// ask's account the ask is refused at once with the pause's line; an ask
/// on the factory's account refused for a usage limit pauses it
/// (`automations::limit`).
pub(crate) async fn run_in(
    ask: Ask,
    cwd: Option<&Path>,
    tools: Option<&str>,
) -> Result<Reply, String> {
    if let Some(why) = crate::automations::limit::refusal(&ask.account) {
        return Err(why);
    }
    let account = ask.account.clone();
    let reply = run_cli(ask, cwd, tools).await;
    if let Err(why) = &reply {
        crate::automations::limit::note_refusal(&account, why);
    }
    reply
}

/// The account's CLI, asked once.
async fn run_cli(ask: Ask, cwd: Option<&Path>, tools: Option<&str>) -> Result<Reply, String> {
    let mut cmd = tokio::process::Command::new(&ask.account.program);
    cmd.envs(ask.account.env.iter().map(|(name, value)| (name, value)));
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let tools = tools.unwrap_or("");
    cmd.args([
        "-p",
        "--model",
        ask.model.as_str(),
        "--effort",
        ask.effort.as_str(),
        "--tools",
        tools,
    ]);
    if !tools.is_empty() {
        cmd.args(["--allowedTools", tools]);
    }
    cmd.args([
        "--system-prompt",
        ask.system.as_str(),
        "--no-session-persistence",
        "--output-format",
        "json",
    ]);
    // A refused ask (a usage limit, a sign-out) exits non-zero with its
    // reason in the JSON on stdout: that reason is the failure.
    let out = crate::pull_request::run_piped(cmd, &ask.prompt, ask.timeout)
        .await
        .map_err(|why| match why.trim_start().starts_with('{') {
            true => parse_reply(&why).err().unwrap_or(why),
            false => why,
        })?;
    parse_reply(&out)
}

/// [`run`], told to answer in JSON only, with the answer parsed as `T`
/// ([`parse_json`]). A reply that doesn't read is asked for once more,
/// told so ([`asked_again`]).
pub async fn run_json<T: DeserializeOwned>(mut ask: Ask) -> Result<T, String> {
    ask.system = match ask.system.trim() {
        "" => JSON_ONLY.to_string(),
        system => format!("{system}\n\n{JSON_ONLY}"),
    };
    let why = match parse_json(&run(ask.clone()).await?.text) {
        Ok(answer) => return Ok(answer),
        Err(why) => why,
    };
    tracing::info!("oneshot: asking again, {why}");
    ask.prompt = asked_again(&ask.prompt, NOT_JSON_VALUE);
    parse_json(&run(ask).await?.text)
}

/// What a reply that didn't read as JSON gets added to its prompt when it
/// is asked again: an ask after a JSON object, and after any JSON value.
pub(crate) const NOT_JSON_OBJECT: &str =
    "Your last reply wasn't valid JSON; reply with the JSON object only.";
const NOT_JSON_VALUE: &str = "Your last reply wasn't valid JSON; reply with the JSON value only.";

/// `prompt` with `nudge` after it, for the one ask again.
pub(crate) fn asked_again(prompt: &str, nudge: &str) -> String {
    format!("{}\n\n{nudge}", prompt.trim_end())
}

/// `claude -p --output-format json`'s answer: the text under `result`,
/// unless it says it is an error.
pub fn parse_reply(json: &str) -> Result<Reply, String> {
    let answer: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "claude answered something that was not JSON")?;
    let text = answer
        .get("result")
        .and_then(|r| r.as_str())
        .unwrap_or_default()
        .trim();
    if answer.get("is_error").and_then(|e| e.as_bool()) == Some(true) || text.is_empty() {
        let why = text.lines().next().unwrap_or("claude wrote nothing");
        return Err(truncate(why, WHY_CUT));
    }
    Ok(Reply {
        text: text.to_string(),
        cost: answer.get("total_cost_usd").and_then(|c| c.as_f64()),
    })
}

/// How many JSON values one reply is searched for the one asked for.
const VALUES_TRIED: usize = 3;
/// How much of a reply that didn't read its error quotes, in chars.
const REPLY_QUOTED: usize = 200;

/// A model's JSON answer as `T` ([`parse_json_where`], any `T` fits).
pub(crate) fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    parse_json_where(text, |_: &T| true)
}

/// A model's JSON answer as a `T` that `fits`, however it was wrapped: out
/// of a ```json (or bare ```) fence when it has one, else from its first
/// `{` or `[` — prose before it and anything after it are skipped. When a
/// value isn't the shape asked for, the next one in the reply is tried,
/// up to [`VALUES_TRIED`]. The error says what went wrong and quotes the
/// start of the reply.
pub(crate) fn parse_json_where<T: DeserializeOwned>(
    text: &str,
    fits: impl Fn(&T) -> bool,
) -> Result<T, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("the model replied with nothing".into());
    }
    let mut misfit = None;
    let mut unread = None;
    for part in [fenced(text), Some(text)].into_iter().flatten() {
        for value in json_values(part, &mut unread) {
            match serde_json::from_value::<T>(value) {
                Ok(answer) if fits(&answer) => return Ok(answer),
                Ok(_) => {
                    misfit.get_or_insert_with(String::new);
                }
                Err(e) => {
                    misfit.get_or_insert_with(|| format!(": {e}"));
                }
            }
        }
    }
    let why = match (misfit, unread) {
        (Some(why), _) => format!("the model's JSON was not the shape asked for{why}"),
        (None, Some(why)) => format!("the model's JSON did not read: {why}"),
        (None, None) => "the model's reply had no JSON in it".to_string(),
    };
    let said = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Err(format!(
        "{why}; it replied: {}",
        truncate(&said, REPLY_QUOTED)
    ))
}

/// The body of the first code fence in `text`, when it has one: from the
/// line after the opening ``` (and its info string, `json`) to the
/// closing one, or the end.
fn fenced(text: &str) -> Option<&str> {
    let (_, rest) = text.split_once("```")?;
    let body = rest.split_once('\n').map_or("", |(_, body)| body);
    Some(body.split_once("```").map_or(body, |(body, _)| body))
}

/// Up to [`VALUES_TRIED`] JSON objects or arrays in `text`, each read from
/// a `{` or `[` — the first, then the next after what was read — and
/// nothing after it; the first that didn't read says why in `unread`.
fn json_values(text: &str, unread: &mut Option<String>) -> Vec<serde_json::Value> {
    let mut values = Vec::new();
    let mut from = 0;
    for _ in 0..VALUES_TRIED {
        let Some(start) = text[from..].find(['{', '[']).map(|at| from + at) else {
            break;
        };
        let mut read =
            serde_json::Deserializer::from_str(&text[start..]).into_iter::<serde_json::Value>();
        match read.next() {
            Some(Ok(value)) => {
                values.push(value);
                from = start + read.byte_offset();
            }
            Some(Err(e)) => {
                unread.get_or_insert_with(|| e.to_string());
                from = start + 1;
            }
            None => break,
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct Shape {
        n: u32,
    }

    #[test]
    fn a_reply_is_its_result_and_cost() {
        let reply = parse_reply(r#"{"result": " hi \n", "total_cost_usd": 0.25}"#).unwrap();
        assert_eq!(
            reply,
            Reply {
                text: "hi".into(),
                cost: Some(0.25)
            }
        );
    }

    #[test]
    fn an_error_or_empty_reply_says_why_in_one_line() {
        assert_eq!(
            parse_reply(r#"{"result": "Not logged in\nmore", "is_error": true}"#),
            Err("Not logged in".into())
        );
        assert_eq!(
            parse_reply(r#"{"result": ""}"#),
            Err("claude wrote nothing".into())
        );
        assert_eq!(
            parse_reply("not json"),
            Err("claude answered something that was not JSON".into())
        );
    }

    #[test]
    fn json_is_read_bare_or_out_of_a_fence() {
        assert_eq!(parse_json::<Shape>(r#"{"n": 1}"#), Ok(Shape { n: 1 }));
        assert_eq!(
            parse_json::<Shape>("```json\n{\"n\": 2}\n```\n"),
            Ok(Shape { n: 2 })
        );
        assert_eq!(
            parse_json::<Shape>("```\n{\"n\": 3}\n```"),
            Ok(Shape { n: 3 })
        );
        assert_eq!(
            parse_json::<Shape>("Here it is [checked]:\n```json\n{\"n\": 4}\n```\nDone."),
            Ok(Shape { n: 4 }),
            "prose around a fence, a bracket in it"
        );
    }

    #[test]
    fn prose_before_the_json_and_text_after_it_are_read_past() {
        assert_eq!(
            parse_json::<Shape>("I read the code. Here's the page:\n{\"n\": 5}"),
            Ok(Shape { n: 5 })
        );
        // What failed the backfill: "trailing characters at line 2 column 1".
        assert_eq!(
            parse_json::<Shape>("{\"n\": 6}\nAll entries hold."),
            Ok(Shape { n: 6 })
        );
        assert_eq!(
            parse_json::<Shape>("{\"n\": 7}\n{\"n\": 8}"),
            Ok(Shape { n: 7 }),
            "the first of two"
        );
    }

    #[test]
    fn a_value_not_in_shape_gives_way_to_the_next() {
        assert_eq!(
            parse_json::<Shape>("{\"m\": 1}\n{\"n\": 9}"),
            Ok(Shape { n: 9 })
        );
        let page = |v: &serde_json::Value| v.get("page").is_some_and(|p| p.is_string());
        let v = parse_json_where("[1] then {\"note\": 1} then {\"page\": \"p\"}", page).unwrap();
        assert_eq!(v["page"], "p");
        assert!(
            parse_json_where("[1] [2] [3] {\"page\": \"p\"}", page).is_err(),
            "three tried, and no more"
        );
    }

    #[test]
    fn a_reply_with_no_json_says_why_and_how_it_began() {
        assert_eq!(
            parse_json::<Shape>("  \n"),
            Err("the model replied with nothing".into())
        );
        let none = parse_json::<Shape>("I couldn't check this one.\nSorry.").unwrap_err();
        assert_eq!(
            none,
            "the model's reply had no JSON in it; it replied: I couldn't check this one. Sorry."
        );
        let broken = parse_json::<Shape>("{\"n\": }").unwrap_err();
        assert!(
            broken.starts_with("the model's JSON did not read: expected value"),
            "{broken}"
        );
        let wrong =
            parse_json::<Shape>(&format!("{{\"m\": \"{}\"}}", "x".repeat(400))).unwrap_err();
        assert!(
            wrong.starts_with("the model's JSON was not the shape asked for: missing field `n`"),
            "{wrong}"
        );
        let quoted = wrong.split_once("; it replied: ").unwrap().1;
        assert_eq!(quoted.chars().count(), REPLY_QUOTED);
        assert!(quoted.starts_with("{\"m\": \"xxx") && quoted.ends_with('…'));
    }

    #[test]
    fn an_ask_again_ends_with_the_nudge() {
        assert_eq!(
            asked_again("Read this.\n", NOT_JSON_OBJECT),
            "Read this.\n\nYour last reply wasn't valid JSON; reply with the JSON object only."
        );
    }
}
