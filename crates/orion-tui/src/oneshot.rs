//! ONE-SHOT MODEL RUNNER: one ask of a model, text in and text out,
//! through one of the Claude accounts orion runs sessions on. Nothing
//! runs in a checkout, no session appears on the grid and nothing is
//! left to tidy up: the account's CLI in print mode with no tools
//! ([`run`]), or the same asked for JSON and parsed ([`run_json`]).
//!
//! Every model call orion makes outside a session goes through here —
//! the week in review, the automations' briefs, the meeting takeaways —
//! so the command, its account and its failures are said one way.

use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::agent_presets::AgentPreset;
use crate::config::Config;
use crate::ui::truncate;

/// How much of a failure's first line is worth showing.
const WHY_CUT: usize = 120;

/// What [`run_json`] adds to the system prompt: the reply is parsed, so
/// anything around the JSON is a failure.
const JSON_ONLY: &str = "Reply with one JSON value and nothing else: no prose, no code fences.";

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

/// The account `cfg` answers `preset`'s asks through: the Claude account
/// the preset pins (`Config::review_account`), or the default agent's when
/// it follows the box. None with no Claude account switched on.
pub fn account_for(cfg: &Config, preset: &AgentPreset) -> Option<Account> {
    // A preset that follows the box names no account: the default
    // agent's, as `review_account` reads an empty one.
    let pinned = match preset.follows_box() {
        true => "",
        false => orion_core::harness::registry_id(preset.kind, preset.custom_harness.as_deref()),
    };
    through(cfg, pinned)
}

/// The default agent's Claude account — what an ask with no preset of its
/// own goes through (a project's **What it is**). None with no Claude
/// account switched on.
pub fn default_account(cfg: &Config) -> Option<Account> {
    through(cfg, "")
}

/// The Claude account `pinned` names, else the default agent's.
fn through(cfg: &Config, pinned: &str) -> Option<Account> {
    let entry = cfg.review_account(pinned)?;
    Some(Account {
        program: entry.program.trim().to_string(),
        env: entry.launch_env(),
        label: entry.display_label().to_string(),
    })
}

/// Send `ask.prompt` to `ask.model` at `ask.effort` through the account's
/// CLI in print mode, with no tools, under that account's sign-in. The
/// failure is the one line worth showing — the command missing, not
/// signed in, timed out.
pub async fn run(ask: Ask) -> Result<Reply, String> {
    let mut cmd = tokio::process::Command::new(&ask.account.program);
    cmd.envs(ask.account.env.iter().map(|(name, value)| (name, value)));
    cmd.args([
        "-p",
        "--model",
        ask.model.as_str(),
        "--effort",
        ask.effort.as_str(),
        "--tools",
        "",
        "--system-prompt",
        ask.system.as_str(),
        "--no-session-persistence",
        "--output-format",
        "json",
    ]);
    let out = crate::pull_request::run_piped(cmd, &ask.prompt, ask.timeout).await?;
    parse_reply(&out)
}

/// [`run`], told to answer in JSON only, with the answer parsed as `T`. A
/// reply wrapped in a ```json fence is unwrapped first.
pub async fn run_json<T: DeserializeOwned>(mut ask: Ask) -> Result<T, String> {
    ask.system = match ask.system.trim() {
        "" => JSON_ONLY.to_string(),
        system => format!("{system}\n\n{JSON_ONLY}"),
    };
    parse_json(&run(ask).await?.text)
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

/// A model's JSON answer as `T`, out of a ```json (or bare ```) fence
/// when it wrapped one.
fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(unfenced(text))
        .map_err(|e| truncate(&format!("the model's JSON did not read: {e}"), WHY_CUT))
}

/// `text` without the code fence around it, if it has one.
fn unfenced(text: &str) -> &str {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("```") else {
        return text;
    };
    // The info string (`json`) runs to the end of the opening line.
    let body = rest.split_once('\n').map_or("", |(_, body)| body);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
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
        assert!(parse_json::<Shape>("Sure! {\"n\": 4}").is_err());
    }
}
