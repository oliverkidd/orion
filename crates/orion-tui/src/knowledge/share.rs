//! SHARING: how harvested rules reach the repo — through one pull request
//! per person, never a push to the base branch.
//!
//! - **The working clone** is `<data_dir>/knowledge/<project id>/repo`:
//!   `git clone --shared` of the project's main checkout, its origin set
//!   to the main checkout's origin. Not a linked worktree: the daemon
//!   would adopt one as a grid row.
//! - **The branch** is `orion/knowledge-<gh login>` ([`branch_for`]).
//!   Before each write it is fetched and rebased on `origin/<base>`
//!   ([`Sharer::open`]); a rebase that conflicts is given up, and the
//!   harvested pages rebuilt from `entries.jsonl` on top of the base
//!   instead ([`Sharer::rebuild`]).
//! - **A write** changes only `.orion/decisions/harvested/`, is committed
//!   (`knowledge: harvest #<pr>`), and pushed — that branch alone, with
//!   `--force-with-lease` ([`Sharer::publish`]).
//! - **One pull request** for the branch, kept open: made if missing,
//!   else its body brought up to date ([`Sharer::ensure_pr`]). Merging it
//!   is how the rules become shared; orion may merge it itself after the
//!   daily card's veto window ([`Sharer::merge`]), never forced.
//!
//! `git` and `gh` are a [`Git`] and a [`Gh`]: the real ones run the
//! programs ([`SystemGit`], [`GhCli`]); a test hands in its own, so no
//! test reaches a real remote.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use futures::future::BoxFuture;
use serde_json::Value;

use super::local::{Local, Lock};
use super::store::{self, Entry, Store, ACTIVE, DIR, VETOED};
use crate::pr_actions::MergeMethod;
use crate::pull_request::{str_at, Checks};

/// The knowledge pull request's title.
pub(crate) const TITLE: &str = "Knowledge: rules harvested from merged PRs";
/// What every knowledge branch's name starts with.
pub(crate) const PREFIX: &str = "orion/knowledge-";
/// The most rules a pull request's body lists.
const BODY_RULES: usize = 60;
const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The git a share runs.
pub(crate) trait Git: Send + Sync {
    /// `git <args>` in `dir`: its stdout, or git's complaint.
    fn run<'a>(
        &'a self,
        dir: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// The `gh` a share runs.
pub(crate) trait Gh: Send + Sync {
    /// `gh <args>` in `dir`: its stdout, or what it said went wrong.
    fn run<'a>(
        &'a self,
        dir: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, String>>;
    /// The signed-in GitHub login.
    fn login(&self) -> BoxFuture<'_, Option<String>>;
}

/// git itself: a call that reaches a remote bounded in time, the rest
/// detached from the terminal ([`crate::git_proc`]).
pub(crate) struct SystemGit;

impl Git for SystemGit {
    fn run<'a>(
        &'a self,
        dir: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, String>> {
        let dir = dir.to_path_buf();
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                static NEVER: AtomicBool = AtomicBool::new(false);
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let budget = match args.first() {
                    Some(&"push") => Some(crate::git_proc::PUSH_TIMEOUT),
                    Some(&"fetch") | Some(&"clone") => Some(crate::git_proc::FETCH_TIMEOUT),
                    _ => None,
                };
                match budget {
                    Some(budget) => crate::git_proc::remote_git(&dir, &args, budget, &NEVER)
                        .map(|()| String::new()),
                    None => crate::git_proc::run(&dir, &args),
                }
            })
            .await
            .map_err(|e| e.to_string())?
        })
    }
}

/// The `gh` CLI.
pub(crate) struct GhCli;

impl Gh for GhCli {
    fn run<'a>(
        &'a self,
        dir: &'a Path,
        args: &'a [&'a str],
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            crate::pull_request::run_gh(Some(dir), args, GH_TIMEOUT)
                .await
                .map_err(|why| match why.trim() {
                    "" => format!("gh {} couldn't run", args.first().unwrap_or(&"")),
                    why => why.to_string(),
                })
        })
    }

    fn login(&self) -> BoxFuture<'_, Option<String>> {
        Box::pin(async {
            crate::pull_request::viewer_login()
                .await
                .map(str::to_string)
        })
    }
}

/// `login`'s knowledge branch: `orion/knowledge-<login>`, anything but
/// letters, digits and dashes made a dash.
pub(crate) fn branch_for(login: &str) -> String {
    let safe: String = login
        .trim()
        .chars()
        .map(|c| match c.is_ascii_alphanumeric() || c == '-' {
            true => c,
            false => '-',
        })
        .collect();
    format!("{PREFIX}{}", safe.trim_matches('-'))
}

/// The branch a project's pull requests go to, as orion's PR form picks
/// it: the project's **Base branch** when origin (or the checkout) has
/// it, else origin's default, else `main`. Runs git: off the loop.
pub(crate) fn base_of(main: &Path) -> String {
    let setting = crate::config::base_branch_of(Some(main));
    crate::commit_list::branch_ref(main, &setting)
        .map(|b| b.strip_prefix("origin/").map(str::to_string).unwrap_or(b))
        .unwrap_or_else(|| super::harvest::default_base(main))
}

/// One project's knowledge, as it is written and proposed.
pub(crate) struct Sharer<'a> {
    pub git: &'a dyn Git,
    pub gh: &'a dyn Gh,
    /// The project's main checkout: what the clone shares objects with,
    /// and whose origin it pushes to.
    pub main: PathBuf,
    pub local: Local,
    pub project: String,
    /// The branch the pull request goes to ([`base_of`]).
    pub base: String,
}

/// The knowledge branch, checked out in the working clone and rebased:
/// the clone held for as long as this is.
#[derive(Debug)]
pub(crate) struct Branch {
    pub name: String,
    pub clone: PathBuf,
    _lock: Lock,
}

impl Branch {
    /// The harvested pages as the branch has them.
    pub(crate) fn store(&self) -> Store {
        Store::of(&self.clone)
    }
}

/// The knowledge pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Pr {
    pub number: u64,
    pub url: String,
}

/// What a publish did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Published {
    /// The open pull request; None while the branch has nothing the base
    /// hasn't.
    pub pr: Option<Pr>,
    /// The branch's head, as pushed.
    pub head: String,
    pub pushed: bool,
}

/// How the pull request stands for a merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ready {
    Go,
    /// Not yet: checks running, or GitHub still working out mergeability.
    Wait(String),
    /// It conflicts with the base: rebuild, push, and try again.
    Conflicting,
    /// It can't be merged as it is.
    Refuse(String),
    /// Merged already (by hand).
    Merged,
    /// Closed without merging.
    Closed,
}

/// What a merge attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Merge {
    /// Merged — by orion or by hand — and the branch reset to the base.
    Merged,
    Closed,
    Waiting(String),
    Conflicting,
}

impl Sharer<'_> {
    async fn git(&self, args: &[&str]) -> Result<String, String> {
        self.git.run(&self.local.clone_dir(), args).await
    }

    /// The commit `rev` names in the clone, if any.
    async fn rev(&self, rev: &str) -> Option<String> {
        let spec = format!("{rev}^{{commit}}");
        self.git(&["rev-parse", "--verify", "-q", &spec])
            .await
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    fn base_ref(&self) -> String {
        format!("refs/remotes/origin/{}", self.base)
    }

    /// The working clone, made on first use, its origin the main
    /// checkout's.
    async fn ensure_clone(&self) -> Result<(), String> {
        let url = self
            .git
            .run(&self.main, &["remote", "get-url", "origin"])
            .await
            .map_err(|_| format!("{} has no origin to propose rules to", self.project))?;
        let url = url.trim().to_string();
        let clone = self.local.clone_dir();
        if !clone.join(".git").exists() {
            if clone.exists() {
                let _ = std::fs::remove_dir_all(&clone);
            }
            std::fs::create_dir_all(&self.local.dir).map_err(|e| e.to_string())?;
            let main = self.main.to_string_lossy().into_owned();
            self.git
                .run(&self.local.dir, &["clone", "-q", "--shared", &main, "repo"])
                .await
                .map_err(|why| format!("couldn't make the working clone: {why}"))?;
        }
        self.git(&["remote", "set-url", "origin", &url]).await?;
        let email = self
            .git(&["config", "user.email"])
            .await
            .unwrap_or_default();
        if email.trim().is_empty() {
            self.git(&["config", "user.name", "orion"]).await?;
            self.git(&["config", "user.email", "orion@localhost"])
                .await?;
        }
        // Commits made off the loop: no pinentry, no repo hooks.
        self.git(&["config", "commit.gpgsign", "false"]).await?;
        self.git(&["config", "core.hooksPath", "/dev/null"]).await?;
        Ok(())
    }

    /// The knowledge branch, ready to write: the clone made or cleaned,
    /// the base and the branch fetched, the branch checked out and
    /// rebased on `origin/<base>` — rebuilt there when the rebase
    /// conflicts. Holds the project's lock until the [`Branch`] drops.
    pub(crate) async fn open(&self) -> Result<Branch, String> {
        let lock = self.local.lock().await?;
        let login = self
            .gh
            .login()
            .await
            .ok_or("gh isn't signed in (gh auth login)")?;
        let name = branch_for(&login);
        self.ensure_clone().await?;
        let _ = self.git(&["rebase", "--abort"]).await;
        let _ = self.git(&["reset", "-q", "--hard"]).await;
        self.git(&["clean", "-qfd"]).await?;
        let base = &self.base;
        let base_ref = self.base_ref();
        self.git(&[
            "fetch",
            "-q",
            "origin",
            &format!("+refs/heads/{base}:{base_ref}"),
        ])
        .await
        .map_err(|why| format!("couldn't fetch origin/{base}: {why}"))?;
        let tracking = format!("refs/remotes/origin/{name}");
        match self
            .git(&[
                "fetch",
                "-q",
                "origin",
                &format!("+refs/heads/{name}:{tracking}"),
            ])
            .await
        {
            Ok(_) => {}
            Err(why) if why.contains("couldn't find remote ref") => {
                let _ = self.git(&["update-ref", "-d", &tracking]).await;
            }
            Err(why) => return Err(format!("couldn't fetch {name}: {why}")),
        }
        let has_local = self.rev(&format!("refs/heads/{name}")).await.is_some();
        let remote = self.rev(&tracking).await;
        match (has_local, &remote) {
            (true, _) => self.git(&["checkout", "-q", &name]).await?,
            (false, Some(_)) => {
                self.git(&["checkout", "-q", "-B", &name, &tracking])
                    .await?
            }
            (false, None) => {
                self.git(&["checkout", "-q", "-B", &name, &base_ref])
                    .await?
            }
        };
        // Behind its own copy on origin (pushed from another of this
        // person's machines): catch up first.
        if has_local
            && remote.is_some()
            && self
                .git(&["merge-base", "--is-ancestor", "HEAD", &tracking])
                .await
                .is_ok()
        {
            self.git(&["reset", "-q", "--hard", &tracking]).await?;
        }
        let branch = Branch {
            name,
            clone: self.local.clone_dir(),
            _lock: lock,
        };
        if self.git(&["rebase", "-q", &base_ref]).await.is_err() {
            let _ = self.git(&["rebase", "--abort"]).await;
            self.rebuild(&branch).await?;
        }
        Ok(branch)
    }

    /// The text of `path` at `rev`, if it has one.
    async fn show(&self, rev: &str, path: &str) -> Option<String> {
        self.git(&["show", &format!("{rev}:{path}")]).await.ok()
    }

    /// The harvested files at `rev`, repo-relative.
    async fn harvested_files(&self, rev: &str) -> Vec<String> {
        self.git(&["ls-tree", "-r", "--name-only", rev, "--", DIR])
            .await
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// After a rebase that conflicted: the branch made again on
    /// `origin/<base>`, its entries merged with the base's, and each page
    /// it changed taken as it was where the base left the page alone —
    /// else made afresh from the merged entries ([`store::render_page`]).
    /// Committed; the next publish pushes it with a lease.
    pub(crate) async fn rebuild(&self, branch: &Branch) -> Result<(), String> {
        let base_ref = self.base_ref();
        let tip = self.rev("HEAD").await.ok_or("the branch has no commit")?;
        let fork = self
            .git(&["merge-base", "HEAD", &base_ref])
            .await
            .ok()
            .map(|s| s.trim().to_string());
        let entries_file = format!("{DIR}/entries.jsonl");
        let read = |text: Option<String>| store::read_entries(&text.unwrap_or_default());
        let ours = read(self.show(&tip, &entries_file).await);
        let theirs = read(self.show(&base_ref, &entries_file).await);
        let old = match &fork {
            Some(fork) => read(self.show(fork, &entries_file).await),
            None => Vec::new(),
        };
        let merged = merge_entries(&theirs, &ours, &old);
        let mut pages = Vec::new();
        for path in self.harvested_files(&tip).await {
            let Some(topic) = path
                .strip_prefix(&format!("{DIR}/"))
                .and_then(|p| p.strip_suffix(".md"))
                .filter(|t| *t != "index")
            else {
                continue;
            };
            let mine = self.show(&tip, &path).await;
            let at_fork = match &fork {
                Some(fork) => self.show(fork, &path).await,
                None => None,
            };
            if mine == at_fork {
                continue;
            }
            let base_page = self.show(&base_ref, &path).await;
            pages.push((path.clone(), topic.to_string(), mine, base_page == at_fork));
        }
        self.git(&["checkout", "-q", "-B", &branch.name, &base_ref])
            .await?;
        let store = branch.store();
        store.save_entries(&merged)?;
        for (path, topic, mine, base_untouched) in pages {
            let text = match (base_untouched, mine) {
                (true, Some(mine)) => store::unmarked(&mine).to_string(),
                _ => {
                    let of_topic: Vec<&Entry> =
                        merged.iter().filter(|e| e.topic == topic).collect();
                    store::render_page(&topic, &of_topic)
                }
            };
            store.write(&store.repo.join(&path), &text)?;
        }
        store.write_index(&self.project)?;
        self.commit(&format!("knowledge: rebuilt on {}", self.base))
            .await?;
        Ok(())
    }

    /// Commit what changed under [`DIR`] as `message` — and nothing
    /// else. False when nothing had.
    pub(crate) async fn commit(&self, message: &str) -> Result<bool, String> {
        match self.git(&["add", "-A", "--", DIR]).await {
            Err(why) if why.contains("did not match any files") => return Ok(false),
            other => other?,
        };
        let staged = self.git(&["diff", "--cached", "--name-only"]).await?;
        let staged: Vec<&str> = staged.lines().filter(|l| !l.is_empty()).collect();
        if staged.is_empty() {
            return Ok(false);
        }
        let prefix = format!("{DIR}/");
        if let Some(outside) = staged.iter().find(|p| !p.starts_with(&prefix)) {
            let _ = self.git(&["reset", "-q"]).await;
            return Err(format!(
                "refused to commit {outside}: orion writes only {DIR}/"
            ));
        }
        self.git(&["commit", "-q", "-m", message]).await?;
        Ok(true)
    }

    /// Commit the branch's changes as `message`, push it — that branch
    /// alone, with a lease on what origin had — and keep its one pull
    /// request open and up to date. Nothing is pushed or proposed while
    /// the branch has nothing the base hasn't.
    pub(crate) async fn publish(
        &self,
        branch: &Branch,
        message: &str,
    ) -> Result<Published, String> {
        self.commit(message).await?;
        let base_ref = self.base_ref();
        let head = self.rev("HEAD").await.ok_or("the branch has no commit")?;
        let ahead = self
            .git(&["rev-list", "--count", &format!("{base_ref}..HEAD")])
            .await?
            .trim()
            .parse::<usize>()
            .unwrap_or(0);
        if ahead == 0 {
            return Ok(Published {
                pr: None,
                head,
                pushed: false,
            });
        }
        let name = &branch.name;
        let tracking = format!("refs/remotes/origin/{name}");
        let remote = self.rev(&tracking).await;
        let mut pushed = false;
        if remote.as_deref() != Some(head.as_str()) {
            let lease = format!(
                "--force-with-lease=refs/heads/{name}:{}",
                remote.unwrap_or_default()
            );
            let refspec = format!("refs/heads/{name}:refs/heads/{name}");
            self.git(&["push", "-q", &lease, "origin", &refspec])
                .await
                .map_err(|why| format!("couldn't push {name}: {why}"))?;
            let _ = self.git(&["update-ref", &tracking, &head]).await;
            pushed = true;
        }
        let theirs = store::read_entries(
            &self
                .show(&base_ref, &format!("{DIR}/entries.jsonl"))
                .await
                .unwrap_or_default(),
        );
        let body = pr_body(&self.project, &theirs, &branch.store().entries());
        let pr = self.ensure_pr(branch, &body).await?;
        Ok(Published {
            pr: Some(pr),
            head,
            pushed,
        })
    }

    /// The branch's one open pull request: its body brought up to date,
    /// or — with none open — made.
    pub(crate) async fn ensure_pr(&self, branch: &Branch, body: &str) -> Result<Pr, String> {
        let dir = &branch.clone;
        let name = branch.name.as_str();
        let list = self
            .gh
            .run(
                dir,
                &[
                    "pr",
                    "list",
                    "--head",
                    name,
                    "--state",
                    "open",
                    "--json",
                    "number,url",
                    "--limit",
                    "10",
                ],
            )
            .await?;
        let open = serde_json::from_str::<Value>(&list)
            .ok()
            .and_then(|v| v.as_array().and_then(|a| a.first()).cloned())
            .map(|v| Pr {
                number: crate::pull_request::u64_at(&v, "number"),
                url: str_at(&v, "url"),
            })
            .filter(|pr| pr.number > 0);
        if let Some(pr) = open {
            let n = pr.number.to_string();
            self.gh
                .run(dir, &["pr", "edit", &n, "--body", body])
                .await?;
            return Ok(pr);
        }
        let out = self
            .gh
            .run(
                dir,
                &[
                    "pr", "create", "--base", &self.base, "--head", name, "--title", TITLE,
                    "--body", body,
                ],
            )
            .await?;
        let url = out
            .lines()
            .map(str::trim)
            .rfind(|l| l.starts_with("http"))
            .unwrap_or_default()
            .to_string();
        let number = url
            .rsplit('/')
            .next()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        Ok(Pr { number, url })
    }

    /// Merge `pr` when it stands ([`readiness`]) — its head the commit
    /// orion last pushed (`pushed`) — by `wanted` when the repo allows
    /// it, else the first method it does; then reset the branch to the
    /// new base. Never forced. `Err` is why it can't merge.
    pub(crate) async fn merge(
        &self,
        pr: &Pr,
        pushed: &str,
        wanted: MergeMethod,
    ) -> Result<Merge, String> {
        let dir = self.local.clone_dir();
        let n = pr.number.to_string();
        let view = self
            .gh
            .run(
                &dir,
                &[
                    "pr",
                    "view",
                    &n,
                    "--json",
                    "state,mergeable,headRefOid,statusCheckRollup",
                ],
            )
            .await?;
        let view: Value = serde_json::from_str(&view).map_err(|e| e.to_string())?;
        match readiness(&view, pushed) {
            Ready::Go => {}
            Ready::Merged => {
                self.reset_to_base().await?;
                return Ok(Merge::Merged);
            }
            Ready::Closed => return Ok(Merge::Closed),
            Ready::Wait(why) => return Ok(Merge::Waiting(why)),
            Ready::Conflicting => return Ok(Merge::Conflicting),
            Ready::Refuse(why) => return Err(why),
        }
        let options = self
            .gh
            .run(
                &dir,
                &[
                    "repo",
                    "view",
                    "--json",
                    "squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed,deleteBranchOnMerge",
                ],
            )
            .await
            .ok();
        let allowed = options
            .as_deref()
            .and_then(crate::pr_actions::parse_merge_options)
            .map(|(methods, _)| methods)
            .unwrap_or_else(|| MergeMethod::ALL.to_vec());
        let method = match allowed.contains(&wanted) {
            true => wanted,
            false => allowed.first().copied().unwrap_or(wanted),
        };
        self.gh
            .run(
                &dir,
                &[
                    "pr",
                    "merge",
                    &n,
                    method.flag(),
                    "--match-head-commit",
                    pushed,
                ],
            )
            .await?;
        self.reset_to_base().await?;
        Ok(Merge::Merged)
    }

    /// After a merge: the branch put back on the new `origin/<base>`,
    /// ready for the next day's harvests.
    pub(crate) async fn reset_to_base(&self) -> Result<(), String> {
        let _lock = self.local.lock().await?;
        let login = self.gh.login().await.ok_or("gh isn't signed in")?;
        let name = branch_for(&login);
        let base = &self.base;
        let base_ref = self.base_ref();
        self.git(&[
            "fetch",
            "-q",
            "origin",
            &format!("+refs/heads/{base}:{base_ref}"),
        ])
        .await?;
        let _ = self.git(&["reset", "-q", "--hard"]).await;
        self.git(&["checkout", "-q", "-B", &name, &base_ref])
            .await?;
        Ok(())
    }
}

/// `theirs` (the base's entries) with `ours` (the branch's) laid over
/// them: an entry the branch changed since `old` (where it forked) wins,
/// one it didn't leaves the base's standing.
pub(crate) fn merge_entries(theirs: &[Entry], ours: &[Entry], old: &[Entry]) -> Vec<Entry> {
    let mut out = theirs.to_vec();
    for e in ours.iter().filter(|e| !old.contains(e)) {
        match out.iter_mut().find(|t| t.id == e.id) {
            Some(t) => *t = e.clone(),
            None => out.push(e.clone()),
        }
    }
    out
}

/// Whether `view` (`gh pr view --json
/// state,mergeable,headRefOid,statusCheckRollup`) may be merged: open,
/// its head the commit orion last pushed (`pushed`), no conflict, and no
/// check failing or still running.
pub(crate) fn readiness(view: &Value, pushed: &str) -> Ready {
    match str_at(view, "state").as_str() {
        "MERGED" => return Ready::Merged,
        "CLOSED" => return Ready::Closed,
        _ => {}
    }
    let head = str_at(view, "headRefOid");
    if pushed.is_empty() || head != pushed {
        let short: String = head.chars().take(8).collect();
        return Ready::Refuse(format!("its head {short} isn't the commit orion pushed"));
    }
    match str_at(view, "mergeable").as_str() {
        "CONFLICTING" => return Ready::Conflicting,
        "MERGEABLE" => {}
        _ => return Ready::Wait("GitHub is still working out whether it can merge".into()),
    }
    let rollup = view
        .get("statusCheckRollup")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    match crate::pull_request::checks(rollup) {
        Checks::Failing => Ready::Refuse("a check is failing".into()),
        Checks::Pending => Ready::Wait("checks are still running".into()),
        _ => Ready::Go,
    }
}

/// The pull request's body: the pull requests harvested and the rules the
/// branch adds or changes (`ours` against the base's `theirs`), each
/// credited to who decided it.
pub(crate) fn pr_body(project: &str, theirs: &[Entry], ours: &[Entry]) -> String {
    let changed: Vec<&Entry> = ours.iter().filter(|e| !theirs.contains(e)).collect();
    let mut prs: Vec<u64> = changed.iter().map(|e| e.pr).filter(|&n| n > 0).collect();
    prs.sort_unstable();
    prs.dedup();
    let mut out = vec![
        format!("Rules orion harvested from {project}'s merged pull requests. Merging this is how they become shared: agents read `{DIR}/` as the base branch has it."),
        format!("orion writes only `{DIR}/`, and regenerates it: change a rule through a pull request. Accepted records elsewhere in `.orion/decisions/` win over these pages."),
        String::new(),
        "### Harvested".into(),
        match prs.is_empty() {
            true => "Nothing new.".into(),
            false => prs.iter().map(|n| format!("#{n}")).collect::<Vec<_>>().join(", "),
        },
    ];
    let rule = |e: &Entry| {
        format!(
            "`{}` — {} (#{}, {})",
            e.topic,
            e.rule,
            e.pr,
            if e.by.is_empty() {
                "agent"
            } else {
                e.by.as_str()
            }
        )
    };
    let active: Vec<&&Entry> = changed.iter().filter(|e| e.status == ACTIVE).collect();
    if !active.is_empty() {
        out.push(String::new());
        out.push("### New or changed rules".into());
        for e in active.iter().take(BODY_RULES) {
            out.push(format!("- {}", rule(e)));
        }
        if active.len() > BODY_RULES {
            out.push(format!("- … and {} more", active.len() - BODY_RULES));
        }
    }
    let vetoed: Vec<&&Entry> = changed.iter().filter(|e| e.status == VETOED).collect();
    if !vetoed.is_empty() {
        out.push(String::new());
        out.push("### Vetoed".into());
        for e in vetoed.iter().take(BODY_RULES) {
            out.push(format!("- ~{}~", rule(e)));
        }
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Real git, run against folders in a temp dir (origin among them),
    /// every call written down.
    #[derive(Default)]
    pub(crate) struct LocalGit {
        pub calls: Mutex<Vec<Vec<String>>>,
    }

    impl Git for LocalGit {
        fn run<'a>(
            &'a self,
            dir: &'a Path,
            args: &'a [&'a str],
        ) -> BoxFuture<'a, Result<String, String>> {
            self.calls
                .lock()
                .unwrap()
                .push(args.iter().map(|a| a.to_string()).collect());
            Box::pin(async move { crate::git_proc::run(dir, args) })
        }
    }

    impl LocalGit {
        pub(crate) fn pushes(&self) -> Vec<Vec<String>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|c| c.first().is_some_and(|a| a == "push"))
                .cloned()
                .collect()
        }
    }

    /// A `gh` that answers from a script by its first two words, and
    /// writes every call down. It never reaches GitHub.
    pub(crate) struct FakeGh {
        pub login: Option<String>,
        pub replies: Mutex<Vec<(String, Result<String, String>)>>,
        pub calls: Mutex<Vec<Vec<String>>>,
    }

    impl FakeGh {
        pub(crate) fn new(login: &str) -> Self {
            FakeGh {
                login: Some(login.into()),
                replies: Mutex::new(Vec::new()),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// Answer the next `gh <verb>` (`pr list`, `pr create` …) with `reply`.
        pub(crate) fn on(&self, verb: &str, reply: Result<&str, &str>) {
            let reply = reply.map(str::to_string).map_err(str::to_string);
            self.replies.lock().unwrap().push((verb.into(), reply));
        }

        pub(crate) fn verbs(&self) -> Vec<String> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|c| c[..2.min(c.len())].join(" "))
                .collect()
        }

        pub(crate) fn call(&self, verb: &str) -> Option<Vec<String>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .find(|c| c[..2.min(c.len())].join(" ") == verb)
                .cloned()
        }
    }

    impl Gh for FakeGh {
        fn run<'a>(
            &'a self,
            _dir: &'a Path,
            args: &'a [&'a str],
        ) -> BoxFuture<'a, Result<String, String>> {
            let call: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let verb = call[..2.min(call.len())].join(" ");
            self.calls.lock().unwrap().push(call);
            let mut replies = self.replies.lock().unwrap();
            let reply = match replies.iter().position(|(v, _)| *v == verb) {
                Some(at) => replies.remove(at).1,
                None => Ok(String::new()),
            };
            Box::pin(async move { reply })
        }

        fn login(&self) -> BoxFuture<'_, Option<String>> {
            let login = self.login.clone();
            Box::pin(async move { login })
        }
    }

    pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
        let mut all = vec![
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ];
        all.extend_from_slice(args);
        crate::git_proc::run(dir, &all).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    /// A bare origin on `dev` holding a hand-written record, and a main
    /// checkout cloned from it.
    pub(crate) struct World {
        pub tmp: tempfile::TempDir,
        pub origin: PathBuf,
        pub main: PathBuf,
        pub local: Local,
    }

    pub(crate) const RECORD: &str = ".orion/decisions/0001-sessions.md";
    pub(crate) const RECORD_TEXT: &str = "# Sessions last 30 days\n\nDecided with legal.\n";

    impl World {
        pub(crate) fn new() -> World {
            let tmp = tempfile::tempdir().unwrap();
            let origin = tmp.path().join("origin.git");
            let seed = tmp.path().join("seed");
            std::fs::create_dir_all(&seed).unwrap();
            git(&seed, &["init", "-q", "-b", "dev"]);
            let record = seed.join(RECORD);
            std::fs::create_dir_all(record.parent().unwrap()).unwrap();
            std::fs::write(&record, RECORD_TEXT).unwrap();
            std::fs::write(seed.join("app.txt"), "app").unwrap();
            git(&seed, &["add", "."]);
            git(&seed, &["commit", "-q", "-m", "seed"]);
            git(tmp.path(), &["clone", "-q", "--bare", "seed", "origin.git"]);
            git(tmp.path(), &["clone", "-q", "origin.git", "main"]);
            let main = tmp.path().join("main");
            let local = Local::at("01P", &tmp.path().join("data/knowledge/01P"));
            World {
                tmp,
                origin,
                main,
                local,
            }
        }

        pub(crate) fn sharer<'a>(&self, git: &'a dyn Git, gh: &'a dyn Gh) -> Sharer<'a> {
            Sharer {
                git,
                gh,
                main: self.main.clone(),
                local: self.local.clone(),
                project: "app".into(),
                base: "dev".into(),
            }
        }

        /// The origin's `ref`, if it has one.
        pub(crate) fn origin_rev(&self, r: &str) -> Option<String> {
            crate::git_proc::read(&self.origin, &["rev-parse", "--verify", "-q", r])
                .ok()
                .map(|s| s.trim().to_string())
        }

        /// A change landed on origin's dev from elsewhere: `path` set to
        /// `text`.
        pub(crate) fn land_on_dev(&self, path: &str, text: &str) {
            let other = self.tmp.path().join("other");
            if !other.exists() {
                git(self.tmp.path(), &["clone", "-q", "origin.git", "other"]);
            }
            git(&other, &["pull", "-q", "origin", "dev"]);
            let file = other.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
            git(&other, &["add", "."]);
            git(&other, &["commit", "-q", "-m", "elsewhere"]);
            git(&other, &["push", "-q", "origin", "dev"]);
        }
    }

    pub(crate) fn entry(id: &str, pr: u64, topic: &str, by: &str) -> Entry {
        Entry {
            id: id.into(),
            pr,
            topic: topic.into(),
            rule: format!("rule {id}"),
            by: by.into(),
            status: ACTIVE.into(),
            ..Entry::default()
        }
    }

    /// Write one harvest's page and entry into `store`.
    pub(crate) fn harvest_into(store: &Store, e: &Entry) {
        store.append(std::slice::from_ref(e)).unwrap();
        let mine: Vec<Entry> = store
            .entries()
            .into_iter()
            .filter(|x| x.topic == e.topic)
            .collect();
        let refs: Vec<&Entry> = mine.iter().collect();
        store
            .write_page(&e.topic, &store::render_page(&e.topic, &refs))
            .unwrap();
        store.write_index("app").unwrap();
    }

    #[test]
    fn a_branch_is_named_for_its_person() {
        assert_eq!(branch_for("ann-b"), "orion/knowledge-ann-b");
        assert_eq!(branch_for("OliverKidd"), "orion/knowledge-OliverKidd");
        assert_eq!(branch_for(" bot[x] "), "orion/knowledge-bot-x");
    }

    #[tokio::test]
    async fn a_harvest_is_pushed_to_its_branch_and_proposed_in_one_pull_request() {
        let w = World::new();
        let git = LocalGit::default();
        let gh = FakeGh::new("ann");
        let share = w.sharer(&git, &gh);

        let branch = share.open().await.unwrap();
        assert_eq!(branch.name, "orion/knowledge-ann");
        let clone = w.local.clone_dir();
        let origin_url = crate::git_proc::read(&clone, &["remote", "get-url", "origin"]).unwrap();
        let main_url = crate::git_proc::read(&w.main, &["remote", "get-url", "origin"]).unwrap();
        assert_eq!(origin_url, main_url, "the main checkout's origin");
        assert!(main_url.trim().ends_with("origin.git"));
        assert!(
            std::fs::read_to_string(clone.join(".git/objects/info/alternates")).is_ok(),
            "a --shared clone"
        );
        assert!(
            !crate::git_proc::read(&w.main, &["worktree", "list"])
                .unwrap()
                .contains("knowledge"),
            "not a linked worktree of the main checkout"
        );
        harvest_into(&branch.store(), &entry("k12-1", 12, "auth/login", "owner"));
        gh.on("pr list", Ok("[]"));
        gh.on("pr create", Ok("https://github.com/o/app/pull/77\n"));
        let out = share
            .publish(&branch, "knowledge: harvest #12")
            .await
            .unwrap();
        drop(branch);
        assert_eq!(
            out.pr,
            Some(Pr {
                number: 77,
                url: "https://github.com/o/app/pull/77".into()
            })
        );
        assert!(out.pushed);
        assert_eq!(
            w.origin_rev("refs/heads/orion/knowledge-ann"),
            Some(out.head.clone())
        );
        let create = gh.call("pr create").unwrap();
        let at = |flag: &str| create[create.iter().position(|a| a == flag).unwrap() + 1].clone();
        assert_eq!(at("--base"), "dev");
        assert_eq!(at("--head"), "orion/knowledge-ann");
        assert_eq!(at("--title"), TITLE);
        assert!(
            at("--body").contains("### Harvested\n#12"),
            "{}",
            at("--body")
        );
        assert!(at("--body").contains("- `auth/login` — rule k12-1 (#12, owner)"));
        let log = crate::git_proc::read(&clone, &["log", "-1", "--format=%s"]).unwrap();
        assert_eq!(log.trim(), "knowledge: harvest #12");

        // The next harvest updates the same pull request.
        let branch = share.open().await.unwrap();
        harvest_into(&branch.store(), &entry("k14-1", 14, "ui/forms", "agent"));
        gh.on(
            "pr list",
            Ok(r#"[{"number": 77, "url": "https://github.com/o/app/pull/77"}]"#),
        );
        let out = share
            .publish(&branch, "knowledge: harvest #14")
            .await
            .unwrap();
        assert_eq!(out.pr.unwrap().number, 77);
        assert_eq!(
            gh.verbs(),
            vec!["pr list", "pr create", "pr list", "pr edit"]
        );
        let edit = gh.call("pr edit").unwrap();
        assert_eq!(edit[2], "77");
        assert!(edit[4].contains("#12, #14") && edit[4].contains("(#14, agent)"));

        // Only the knowledge branch was ever pushed, always with a lease.
        let pushes = git.pushes();
        assert_eq!(pushes.len(), 2);
        for push in &pushes {
            assert!(push[2].starts_with("--force-with-lease=refs/heads/orion/knowledge-ann:"));
            assert_eq!(
                push[3..],
                [
                    "origin",
                    "refs/heads/orion/knowledge-ann:refs/heads/orion/knowledge-ann"
                ]
            );
        }
        assert_eq!(
            w.origin_rev("refs/heads/dev").unwrap(),
            crate::git_proc::read(&w.main, &["rev-parse", "origin/dev"])
                .unwrap()
                .trim(),
            "dev never moved"
        );
    }

    #[tokio::test]
    async fn nothing_new_is_neither_pushed_nor_proposed() {
        let w = World::new();
        let (git, gh) = (LocalGit::default(), FakeGh::new("ann"));
        let share = w.sharer(&git, &gh);
        let branch = share.open().await.unwrap();
        let out = share.publish(&branch, "knowledge: nothing").await.unwrap();
        assert_eq!((out.pr, out.pushed), (None, false));
        assert!(gh.verbs().is_empty() && git.pushes().is_empty());
    }

    #[tokio::test]
    async fn hand_written_records_are_never_changed_or_committed() {
        let w = World::new();
        let (git, gh) = (LocalGit::default(), FakeGh::new("ann"));
        let share = w.sharer(&git, &gh);
        let branch = share.open().await.unwrap();
        let store = branch.store();
        harvest_into(&store, &entry("k12-1", 12, "auth/login", "owner"));
        // Something else changed in the clone: not orion's to commit.
        std::fs::write(branch.clone.join(RECORD), "tampered").unwrap();
        assert!(store.write(&branch.clone.join(RECORD), "over").is_err());
        gh.on("pr list", Ok("[]"));
        share
            .publish(&branch, "knowledge: harvest #12")
            .await
            .unwrap();
        let files =
            crate::git_proc::read(&branch.clone, &["show", "--name-only", "--format=", "HEAD"])
                .unwrap();
        assert!(
            files
                .lines()
                .all(|f| f.starts_with(".orion/decisions/harvested/")),
            "{files}"
        );
        let pushed = crate::git_proc::read(
            &w.origin,
            &["show", &format!("refs/heads/orion/knowledge-ann:{RECORD}")],
        )
        .unwrap();
        assert_eq!(pushed, RECORD_TEXT, "the record is as the base has it");
    }

    #[tokio::test]
    async fn a_conflicting_rebase_rebuilds_from_the_entries_on_the_base() {
        let w = World::new();
        let (git, gh) = (LocalGit::default(), FakeGh::new("ann"));
        let share = w.sharer(&git, &gh);
        let branch = share.open().await.unwrap();
        harvest_into(&branch.store(), &entry("k12-1", 12, "auth/login", "owner"));
        gh.on("pr list", Ok("[]"));
        share
            .publish(&branch, "knowledge: harvest #12")
            .await
            .unwrap();
        drop(branch);
        // Someone else's knowledge PR merged into dev meanwhile: the same
        // files, different lines.
        let theirs = Store::of(w.tmp.path().join("scratch").as_path());
        harvest_into(&theirs, &entry("k20-1", 20, "auth/login", "owner"));
        for file in ["entries.jsonl", "auth/login.md", "index.md"] {
            let text = std::fs::read_to_string(theirs.dir.join(file)).unwrap();
            w.land_on_dev(&format!("{DIR}/{file}"), &text);
        }
        let pushed_before = w.origin_rev("refs/heads/orion/knowledge-ann").unwrap();

        let branch = share.open().await.unwrap();
        let ids: Vec<String> = branch.store().entries().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, vec!["k20-1", "k12-1"]);
        let page = branch.store().page("auth/login").unwrap();
        assert!(
            page.contains("<!-- k20-1 -->") && page.contains("<!-- k12-1 -->"),
            "{page}"
        );
        let log = crate::git_proc::read(&branch.clone, &["log", "-2", "--format=%s"]).unwrap();
        assert_eq!(log, "knowledge: rebuilt on dev\nelsewhere\n");
        gh.on("pr list", Ok(r#"[{"number": 77, "url": "u"}]"#));
        let out = share
            .publish(&branch, "knowledge: nothing more")
            .await
            .unwrap();
        assert!(out.pushed);
        let lease = &git.pushes()[1][2];
        assert_eq!(
            lease,
            &format!("--force-with-lease=refs/heads/orion/knowledge-ann:{pushed_before}")
        );
        assert_eq!(
            w.origin_rev("refs/heads/orion/knowledge-ann"),
            Some(out.head)
        );
    }

    #[test]
    fn entries_merge_with_the_branchs_changes_winning() {
        let a = entry("k1-1", 1, "t/a", "owner");
        let b = entry("k2-1", 2, "t/a", "owner");
        let mut b_vetoed = b.clone();
        b_vetoed.status = VETOED.into();
        let c = entry("k3-1", 3, "t/a", "agent");
        // Base vetoed b; the branch left b alone and added c.
        let merged = merge_entries(
            &[a.clone(), b_vetoed.clone()],
            &[a.clone(), b.clone(), c.clone()],
            &[a.clone(), b.clone()],
        );
        assert_eq!(merged, vec![a.clone(), b_vetoed.clone(), c.clone()]);
        // The branch vetoed b itself.
        let merged = merge_entries(
            &[a.clone(), b.clone()],
            &[a.clone(), b_vetoed.clone()],
            &[a.clone(), b],
        );
        assert_eq!(merged, vec![a, b_vetoed]);
    }

    fn view(state: &str, mergeable: &str, head: &str, checks: Value) -> Value {
        serde_json::json!({"state": state, "mergeable": mergeable, "headRefOid": head, "statusCheckRollup": checks})
    }

    fn run(conclusion: &str) -> Value {
        serde_json::json!({"__typename": "CheckRun", "name": "ci", "status": "COMPLETED", "conclusion": conclusion})
    }

    #[test]
    fn a_merge_waits_for_checks_and_refuses_what_orion_didnt_push() {
        let ok = serde_json::json!([run("SUCCESS")]);
        assert_eq!(
            readiness(&view("OPEN", "MERGEABLE", "abc", ok.clone()), "abc"),
            Ready::Go
        );
        assert_eq!(
            readiness(
                &view("OPEN", "MERGEABLE", "abc", serde_json::json!([])),
                "abc"
            ),
            Ready::Go,
            "no checks yet on the repo"
        );
        assert_eq!(
            readiness(
                &view(
                    "OPEN",
                    "MERGEABLE",
                    "abc",
                    serde_json::json!([run("FAILURE")])
                ),
                "abc"
            ),
            Ready::Refuse("a check is failing".into())
        );
        let pending =
            serde_json::json!([{"__typename": "CheckRun", "name": "ci", "status": "IN_PROGRESS"}]);
        assert_eq!(
            readiness(&view("OPEN", "MERGEABLE", "abc", pending), "abc"),
            Ready::Wait("checks are still running".into())
        );
        assert_eq!(
            readiness(&view("OPEN", "MERGEABLE", "def12345678", ok.clone()), "abc"),
            Ready::Refuse("its head def12345 isn't the commit orion pushed".into())
        );
        assert!(matches!(
            readiness(&view("OPEN", "MERGEABLE", "abc", ok.clone()), ""),
            Ready::Refuse(_)
        ));
        assert_eq!(
            readiness(&view("OPEN", "CONFLICTING", "abc", ok.clone()), "abc"),
            Ready::Conflicting
        );
        assert!(matches!(
            readiness(&view("OPEN", "UNKNOWN", "abc", ok.clone()), "abc"),
            Ready::Wait(_)
        ));
        assert_eq!(
            readiness(&view("MERGED", "UNKNOWN", "x", ok.clone()), "abc"),
            Ready::Merged
        );
        assert_eq!(
            readiness(&view("CLOSED", "MERGEABLE", "abc", ok), "abc"),
            Ready::Closed
        );
    }

    #[tokio::test]
    async fn orion_merges_its_pull_request_by_the_repos_method_then_starts_afresh() {
        let w = World::new();
        let (git, gh) = (LocalGit::default(), FakeGh::new("ann"));
        let share = w.sharer(&git, &gh);
        let branch = share.open().await.unwrap();
        harvest_into(&branch.store(), &entry("k12-1", 12, "auth/login", "owner"));
        gh.on("pr list", Ok("[]"));
        gh.on("pr create", Ok("https://github.com/o/app/pull/77"));
        let out = share
            .publish(&branch, "knowledge: harvest #12")
            .await
            .unwrap();
        drop(branch);
        let pr = out.pr.unwrap();
        let ok = view(
            "OPEN",
            "MERGEABLE",
            &out.head,
            serde_json::json!([run("SUCCESS")]),
        );
        gh.on("pr view", Ok(&ok.to_string()));
        gh.on("repo view", Ok(r#"{"squashMergeAllowed": false, "mergeCommitAllowed": true, "rebaseMergeAllowed": true}"#));
        // GitHub merges: dev moves on to the branch's head.
        w.land_on_dev("app.txt", "moved on");
        assert_eq!(
            share.merge(&pr, &out.head, MergeMethod::Squash).await,
            Ok(Merge::Merged)
        );
        let merge = gh.call("pr merge").unwrap();
        assert_eq!(
            merge,
            [
                "pr",
                "merge",
                "77",
                "--merge",
                "--match-head-commit",
                out.head.as_str()
            ]
        );
        assert!(!merge.iter().any(|a| a == "--admin" || a == "--auto"));
        let clone = w.local.clone_dir();
        let head = crate::git_proc::read(&clone, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(
            Some(head.trim().to_string()),
            w.origin_rev("refs/heads/dev"),
            "back on the new base"
        );
        assert_eq!(
            crate::git_proc::read(&clone, &["branch", "--show-current"])
                .unwrap()
                .trim(),
            "orion/knowledge-ann"
        );

        // A head orion didn't push is refused, and nothing merges.
        let foreign = view("OPEN", "MERGEABLE", "beef", serde_json::json!([]));
        gh.on("pr view", Ok(&foreign.to_string()));
        assert!(share
            .merge(&pr, &out.head, MergeMethod::Merge)
            .await
            .is_err());
        assert_eq!(gh.verbs().iter().filter(|v| *v == "pr merge").count(), 1);
    }
}
