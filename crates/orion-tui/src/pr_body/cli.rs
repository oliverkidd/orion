//! `orion pr facts init|set`: how the agent that built a pull request
//! hands orion its facts (`super::facts`). `init` prints the skeleton;
//! `set <file>` checks the filled-in document and leaves it under
//! `<data_dir>/pr-facts/`, where the TUI's poller picks it up and writes
//! the PR's top.

use anyhow::{bail, Context as _, Result};
use std::io::Read as _;

use super::facts::{self, Facts};

/// `orion pr facts <op>`, as the CLI parsed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FactsOp {
    Init,
    /// `file` is a path, or `-` for stdin; `pr` a URL or number, else the
    /// current branch's PR.
    Set {
        file: String,
        pr: Option<String>,
    },
}

pub fn run(op: FactsOp) -> Result<()> {
    match op {
        FactsOp::Init => {
            println!("{}", facts::skeleton());
            Ok(())
        }
        FactsOp::Set { file, pr } => {
            let text = match file.as_str() {
                "-" => {
                    let mut text = String::new();
                    std::io::stdin()
                        .read_to_string(&mut text)
                        .context("reading the facts from stdin")?;
                    text
                }
                path => std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?,
            };
            let facts = parse(&text)?;
            let url = pr_url(pr.as_deref())?;
            let dir = super::facts_dir().context("no data dir to save the facts in")?;
            facts::write_for(&dir, &url, &facts).context("saving the facts")?;
            println!("facts saved for {url}; orion writes the top of its body on its next look");
            Ok(())
        }
    }
}

/// The document, checked.
pub fn parse(text: &str) -> Result<Facts> {
    let facts: Facts = serde_json::from_str(text).context("the facts aren't valid JSON")?;
    let problems = facts.problems();
    if !problems.is_empty() {
        bail!(
            "the facts aren't filled in:\n  - {}",
            problems.join("\n  - ")
        );
    }
    Ok(facts)
}

/// The PR the facts are for: a URL as given, else `gh pr view [<pr>]`
/// from here.
pub(crate) fn pr_url(pr: Option<&str>) -> Result<String> {
    if let Some(url) = pr.filter(|p| p.starts_with("https://")) {
        if super::parse_url(url).is_none() {
            bail!("{url} isn't a GitHub pull request URL");
        }
        return Ok(url.trim().to_string());
    }
    let mut args = vec!["pr", "view"];
    args.extend(pr);
    args.extend(["--json", "url", "-q", ".url"]);
    let out = std::process::Command::new("gh")
        .args(&args)
        .output()
        .context("running gh")?;
    let url = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || url.is_empty() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!(
            "no pull request found for this branch ({}): open it first, then run this again",
            why.lines().next().unwrap_or("gh said nothing").trim()
        );
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_refuses_the_unfilled_skeleton_and_bad_json() {
        assert!(parse(&facts::skeleton()).is_err());
        assert!(parse("{").is_err());
        assert!(parse(r#"{"what_users_get":"It works."}"#).is_ok());
    }
}
