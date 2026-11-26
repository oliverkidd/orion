//! THE PICTURES FLOW, FROM ITS CHECKOUT: getting the flow a PR's agent
//! wrote to where the repo's pictures job reads it, by the command the
//! repo names in its contract — `pictures_upload`, run with `sh` from the
//! checkout's root, `{pr}` and `{flow}` filled in; `pictures_flow` says
//! where the flow sits ([`super::contract::Contract::upload_command`]).
//! orion knows nothing of the flow's language or the repo's tooling.
//!
//! Two ways it goes:
//!
//! - **`orion pr pictures`** ([`cli`]), which the agent runs once its
//!   flow is written: one orion command its permissions already allow,
//!   whatever the repo's upload runs underneath. The contract is read
//!   from the PR's base branch, as orion reads it everywhere, so a branch
//!   can't change the command it runs.
//! - **The net** ([`net`], from the gather): a visible factory PR without
//!   pictures at its head, whose checkout holds a flow that isn't the one
//!   up — none is, or another is — has it sent by orion, once per head and
//!   flow ([`Sent`]). A session that wrote its flow and couldn't send it
//!   (a permission refused, a step skipped) no longer leaves its PR
//!   without pictures; the job is then run for it as for any flow sent
//!   after the push (`super::dispatch`).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context as _, Result};
use serde::{Deserialize, Serialize};

use super::contract::{self, Contract};

/// How long an upload may take: the repo's command may start a toolchain.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);

/// A flow orion sent from a checkout itself: the PR's head then, and the
/// flow's git blob sha.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sent {
    pub head: String,
    pub flow: String,
}

/// The git blob sha of `flow` (from `root`): what the pictures branch
/// reports for the same bytes.
pub async fn blob_sha(root: &Path, flow: &str) -> Option<String> {
    let out = tokio::process::Command::new("git")
        .args(["hash-object", "--no-filters", "--", flow])
        .current_dir(root)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !sha.is_empty()).then_some(sha)
}

/// Whether orion sends the flow whose blob is `local` for a PR at `head`:
/// the one up (`up`) isn't it, and orion hasn't sent it for this head.
pub fn should_send(local: &str, up: Option<&str>, head: &str, sent: Option<&Sent>) -> bool {
    !head.trim().is_empty()
        && up != Some(local)
        && !sent.is_some_and(|s| s.head == head.trim() && s.flow == local)
}

/// `command` through `sh -c` from `root`, under `timeout`: Ok on success,
/// Err with the end of what it said.
pub async fn run(root: &Path, command: &str, timeout: Duration) -> Result<(), String> {
    let out = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(timeout, out).await {
        Err(_) => Err(format!("it took over {}s", timeout.as_secs())),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let said = String::from_utf8_lossy(&out.stderr);
            let said = said
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("it said nothing");
            Err(format!("{}: {}", out.status, said.trim()))
        }
    }
}

/// The net: send the flow in `checkout` for PR `number` on `branch` at
/// `head`, when the contract says where it is and how, it is there, and
/// [`should_send`] (`up` is the blob on the pictures branch, `sent` what
/// orion sent last). Some when it tried: what it sent, and whether that
/// worked.
pub async fn net(
    rules: &Contract,
    checkout: &Path,
    branch: &str,
    number: u64,
    head: &str,
    up: Option<&str>,
    sent: Option<&Sent>,
) -> Option<(Sent, bool)> {
    let flow = rules.flow_path(branch)?;
    if !checkout.join(&flow).is_file() {
        return None;
    }
    let command = rules.upload_command(number, Some(&flow)).ok()?;
    let local = blob_sha(checkout, &flow).await?;
    if !should_send(&local, up, head, sent) {
        return None;
    }
    let worked = match run(checkout, &command, UPLOAD_TIMEOUT).await {
        Ok(()) => {
            tracing::info!(
                "pr_body: sent the pictures flow {flow} for #{number} from its checkout"
            );
            true
        }
        Err(why) => {
            tracing::warn!("pr_body: sending the pictures flow {flow} for #{number}: {why}");
            false
        }
    };
    let sent = Sent {
        head: head.trim().to_string(),
        flow: local,
    };
    Some((sent, worked))
}

// ---- `orion pr pictures` ----

/// The PR `orion pr pictures` sends for.
struct Pr {
    url: String,
    number: u64,
    base: String,
    head: String,
}

/// `gh pr view [<pr>]` from here.
fn pr_here(pr: Option<&str>) -> Result<Pr> {
    let mut args = vec!["pr", "view"];
    args.extend(pr);
    args.extend(["--json", "url,number,baseRefName,headRefName"]);
    let out = std::process::Command::new("gh")
        .args(&args)
        .output()
        .context("running gh")?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        bail!(
            "no pull request found ({}): open it first, then run this again",
            why.lines().next().unwrap_or("gh said nothing").trim()
        );
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).context("gh's answer isn't JSON")?;
    let s = |k| crate::pull_request::str_at(&v, k);
    Ok(Pr {
        url: s("url"),
        number: crate::pull_request::u64_at(&v, "number"),
        base: s("baseRefName"),
        head: s("headRefName"),
    })
}

/// The root of the checkout this runs in.
fn checkout_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("no current directory")?;
    match crate::git_proc::read(&cwd, &["rev-parse", "--show-toplevel"]) {
        Ok(root) if !root.trim().is_empty() => Ok(PathBuf::from(root.trim())),
        _ => bail!("not in a git checkout: run this from the pull request's checkout"),
    }
}

/// `orion pr pictures [--pr <PR>]`: send this checkout's flow for the PR
/// (default: the current branch's) by the contract's `pictures_upload`,
/// its output passed through.
pub async fn cli(pr: Option<String>) -> Result<()> {
    let pr = pr_here(pr.as_deref())?;
    let (owner, repo, _) = super::parse_url(&pr.url)
        .ok_or_else(|| anyhow!("{} isn't a GitHub pull request URL", pr.url))?;
    let root = checkout_root()?;
    let rules = match contract::fetch(&owner, &repo, &pr.base).await {
        Some(Ok(rules)) => rules,
        Some(Err(why)) => bail!("{why}"),
        None => bail!(
            "couldn't read {} from {}: try again",
            contract::PATH,
            pr.base
        ),
    };
    let flow = rules.flow_path(&pr.head);
    if let Some(flow) = &flow {
        if !root.join(flow).is_file() {
            bail!("no flow at {flow}: write it first (the repo's docs say how)");
        }
    }
    let command = rules
        .upload_command(pr.number, flow.as_deref())
        .map_err(|why| {
            anyhow!("{why}: this repo hasn't told orion how to send its pictures flow")
        })?;
    eprintln!("orion: {command}");
    let status = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(&root)
        .status()
        .await
        .context("running sh")?;
    if !status.success() {
        bail!("the upload failed ({status})");
    }
    println!(
        "sent the flow for #{}: the pictures job takes it on the next push, \
         or orion runs the job for it",
        pr.number
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flow_is_sent_once_per_head_and_never_when_it_is_the_one_up() {
        let sent = Sent {
            head: "h1".into(),
            flow: "f1".into(),
        };
        assert!(should_send("f1", None, "h1", None), "none up");
        assert!(should_send("f2", Some("f1"), "h1", None), "another is up");
        assert!(
            !should_send("f1", Some("f1"), "h1", None),
            "it is the one up"
        );
        assert!(
            !should_send("f1", None, "h1", Some(&sent)),
            "sent at this head"
        );
        assert!(should_send("f1", None, "h2", Some(&sent)), "a new push");
        assert!(should_send("f2", None, "h1", Some(&sent)), "a new flow");
        assert!(!should_send("f1", None, " ", None), "no head");
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn the_net_sends_a_flow_left_in_the_checkout_once() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::create_dir_all(root.join(".flows")).unwrap();
        std::fs::write(root.join(".flows/feat-x.ts"), "flow").unwrap();
        let rules = contract::parse(
            r#"
pictures_flow = ".flows/{branch}.ts"
pictures_upload = "printf '%s %s' {pr} {flow} > sent.txt"
"#,
        )
        .unwrap();
        let (sent, worked) = net(&rules, root, "feat/x", 7, "h1", None, None)
            .await
            .expect("sent");
        assert!(worked);
        assert_eq!(
            std::fs::read_to_string(root.join("sent.txt")).unwrap(),
            "7 .flows/feat-x.ts"
        );
        // The blob sha is git's: what the pictures branch reports.
        assert_eq!(sent.flow, blob_sha(root, ".flows/feat-x.ts").await.unwrap());
        assert_eq!(sent.flow.len(), 40);
        assert!(net(&rules, root, "feat/x", 7, "h1", None, Some(&sent))
            .await
            .is_none());
        assert!(
            net(&rules, root, "feat/x", 7, "h1", Some(&sent.flow), None)
                .await
                .is_none(),
            "the one up"
        );
        assert!(
            net(&rules, root, "other", 7, "h1", None, None)
                .await
                .is_none(),
            "no flow for that branch"
        );
    }

    #[tokio::test]
    async fn a_failed_upload_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(dir.path(), "echo nope >&2; exit 3", UPLOAD_TIMEOUT)
            .await
            .unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }
}
