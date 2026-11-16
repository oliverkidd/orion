//! RUNNING THE PICTURES JOB AGAIN: for a PR whose keyframes flow was
//! uploaded after its push.
//!
//! The pictures job (riplo-os's `pr-keyframes.yml`) runs on a PR's
//! `opened` and `synchronize`, and does nothing when it finds no
//! `pr-<n>/flow.keyframes.ts` on `pr-screenshots`. An agent that opens
//! the PR first and uploads its flow after is left with a visible change
//! and no pictures, and nothing runs the job again. So, from the gather
//! (off the loop), orion runs it by hand — `gh workflow run <workflow>
//! --ref <base> -f pr=<n>` — once per head and flow ([`wanted`], then
//! [`look`]) when all of these hold:
//!
//! - the PR is visible and its pictures aren't shown at its head: no
//!   `pictures.json` was taken at that head, or the one taken there shows
//!   no difference in any frame;
//! - the job hasn't run for that head with the flow that is up now
//!   ([`Ran`], pr_body's memory, keyed on the head and the flow file's
//!   blob sha): an agent sent back to stage its pictures again re-uploads
//!   its flow at the same head, and that new flow is run once more;
//! - its flow is on `pr-screenshots`;
//! - no run of the workflow for this PR is queued or in progress.
//!
//! The same look ([`look`]) is made for every visible PR without pictures
//! at its head, to tell whether its pictures are on their way
//! ([`coming`]): its flow is up, and a run for it is queued or in
//! progress or orion ran the job for this head in the last [`WAIT_MS`].
//! While they are, its verdict card waits for them rather than going out
//! as Review with no pictures.
//!
//! The workflow is the contract's `pictures_workflow`, else the one active
//! workflow whose path or name says keyframes or pictures ([`discover`]):
//! none or several and orion does nothing, and says so once a repo.
//!
//! A dispatched run reports its check on the base branch's commit, not the
//! PR's head: orion knows it worked when `pictures.json` names the head.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::pictures::{self, Manifest};
use crate::pull_request::PrDetail;

const TIMEOUT: Duration = Duration::from_secs(20);

/// What the pictures job is known to have run for: a head, and the blob
/// sha of the flow it ran with on the pictures branch. A flow of None is
/// any flow: a run remembered from before flows were told apart, or
/// pictures taken at the head before orion saw which flow was up.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ran {
    pub head: String,
    pub flow: Option<String>,
}

impl Ran {
    /// Whether the job has run for `head` with the flow `flow`.
    pub fn covers(&self, head: &str, flow: &str) -> bool {
        self.head == head && self.flow.as_deref().is_none_or(|f| f == flow)
    }
}

/// What the job has run for at PR `detail`'s head, from pr_body's memory:
/// `ran` when it is for this head; else the head orion last dispatched
/// for, from before flows were remembered; else, when `pictures.json` was
/// taken at this head, the flow that is up now (until orion learns its
/// sha). None when it hasn't run at this head.
pub fn ran_at_head(
    detail: &PrDetail,
    ran: Option<&Ran>,
    dispatched: Option<&str>,
    manifest: &Manifest,
) -> Option<Ran> {
    let head = detail.head_sha.trim();
    if let Some(ran) = ran.filter(|r| r.head == head) {
        return Some(ran.clone());
    }
    let taken_here = !head.is_empty() && manifest.head_sha.trim() == head;
    (dispatched == Some(head) || taken_here).then(|| Ran {
        head: head.to_string(),
        flow: None,
    })
}

/// Whether PR `detail` is one to run the pictures job for, before asking
/// GitHub anything: visible and not shown, with no pictures taken at this
/// head or ones there that show no difference. Whether its flow has run
/// at this head already is [`look`]'s to say, once it knows the flow.
pub fn wanted(detail: &PrDetail, visible_paths: &[String], manifest: &Manifest) -> bool {
    let head = detail.head_sha.trim();
    if head.is_empty() || detail.base.trim().is_empty() {
        return false;
    }
    let signal = pictures::of(detail, visible_paths, manifest);
    signal.visible && !signal.shown && (manifest.head_sha.trim() != head || signal.no_difference)
}

/// What [`run`] asks GitHub: `gh` for real ([`Gh`]), canned in tests.
pub(crate) trait Ask {
    /// PR `number`'s flow on the pictures branch: its blob sha, None
    /// inside when it isn't there; None when `gh` couldn't ask.
    async fn flow(&self, number: u64) -> Option<Option<String>>;
    /// The repo's workflows, as `actions/workflows` answers.
    async fn workflows(&self) -> Option<String>;
    /// `workflow`'s latest runs, as `gh run list --json` answers.
    async fn runs(&self, workflow: &str) -> Option<String>;
    /// `gh workflow run <workflow> --ref <base> -f pr=<number>`.
    async fn dispatch(&self, workflow: &str, base: &str, number: u64) -> Result<(), String>;
}

/// [`Ask`] through `gh`, for `owner/repo`.
pub(crate) struct Gh<'a> {
    pub owner: &'a str,
    pub repo: &'a str,
}

impl Gh<'_> {
    fn full(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    async fn gh(&self, args: &[&str]) -> Result<String, String> {
        crate::pull_request::run_gh(None, args, TIMEOUT).await
    }
}

impl Ask for Gh<'_> {
    async fn flow(&self, number: u64) -> Option<Option<String>> {
        let endpoint = format!(
            "repos/{}/contents/pr-{number}/flow.keyframes.ts?ref={}",
            self.full(),
            pictures::BRANCH
        );
        match self.gh(&["api", &endpoint, "--jq", ".sha"]).await {
            Ok(sha) => Some(Some(sha.trim().to_string())),
            Err(why) if super::not_found(&why) => Some(None),
            Err(why) => {
                tracing::info!(
                    "pr_body: looking for the flow of {}#{number}: {why}",
                    self.full()
                );
                None
            }
        }
    }

    async fn workflows(&self) -> Option<String> {
        let endpoint = format!("repos/{}/actions/workflows?per_page=100", self.full());
        self.gh(&["api", &endpoint]).await.ok()
    }

    async fn runs(&self, workflow: &str) -> Option<String> {
        let full = self.full();
        let args = [
            "run",
            "list",
            "--repo",
            &full,
            "--workflow",
            workflow,
            "--limit",
            "30",
            "--json",
            "status,event,displayTitle,createdAt,headBranch,headSha",
        ];
        self.gh(&args).await.ok()
    }

    async fn dispatch(&self, workflow: &str, base: &str, number: u64) -> Result<(), String> {
        let (full, pr) = (self.full(), format!("pr={number}"));
        let args = [
            "workflow", "run", workflow, "--repo", &full, "--ref", base, "-f", &pr,
        ];
        self.gh(&args).await.map(|_| ())
    }
}

/// How long after orion runs the pictures job for a head the PR's card
/// waits for the pictures, though no run for it shows: 30 minutes.
pub const WAIT_MS: i64 = 30 * 60 * 1000;

/// What [`look`] found of PR's pictures on their way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Look {
    /// Its flow is on the pictures branch.
    pub flow: bool,
    /// The flow's blob sha there, when it is.
    pub flow_sha: Option<String>,
    /// A run of the pictures workflow for it is queued or in progress.
    pub going: bool,
    /// The workflow was run for it just now.
    pub dispatched: bool,
}

/// Whether PR's pictures are on their way to `head`, from what [`look`]
/// found and the head the job was last run for by hand (`dispatched`, at
/// `dispatched_at_ms`): its flow is up, and a run for it is going or
/// orion's own ran for this head within [`WAIT_MS`] of `now_ms`.
pub fn coming(
    look: &Look,
    head: &str,
    dispatched: Option<&str>,
    dispatched_at_ms: Option<i64>,
    now_ms: i64,
) -> bool {
    let lately = !head.is_empty()
        && dispatched == Some(head)
        && dispatched_at_ms.is_some_and(|at| now_ms.saturating_sub(at) < WAIT_MS);
    look.flow && (look.going || look.dispatched || lately)
}

/// Look at PR `detail`'s pictures on their way — its flow, and a run of
/// `workflow` (the contract's) or the one [`discover`] finds going for it
/// — and, when `may_run` (one [`wanted`] passed), run the job for it if
/// the flow is there, the job hasn't run at this head with that flow
/// (`ran`, from [`ran_at_head`]) and no run is going.
pub(crate) async fn look<A: Ask>(
    ask: &A,
    repo: &str,
    detail: &PrDetail,
    workflow: Option<&str>,
    may_run: bool,
    ran: Option<&Ran>,
) -> Look {
    let number = detail.number;
    let Some(Some(sha)) = ask.flow(number).await else {
        return Look::default();
    };
    let may_run = may_run && !ran.is_some_and(|r| r.covers(detail.head_sha.trim(), &sha));
    let mut found = Look {
        flow: true,
        flow_sha: Some(sha),
        ..Look::default()
    };
    let workflow = match workflow.map(str::trim).filter(|w| !w.is_empty()) {
        Some(named) => named.to_string(),
        None => {
            let Some(json) = ask.workflows().await else {
                return found;
            };
            match discover(&json) {
                Ok(named) => named,
                Err(why) => {
                    say_once(repo, &why);
                    return found;
                }
            }
        }
    };
    let Some(runs) = ask.runs(&workflow).await else {
        return found;
    };
    found.going = going(&runs, detail);
    if found.going || !may_run {
        return found;
    }
    match ask.dispatch(&workflow, detail.base.trim(), number).await {
        Ok(()) => {
            tracing::info!(
                "pr_body: ran {workflow} for {repo}#{number} at {}: its flow came after the push",
                detail.head_sha
            );
            found.dispatched = true;
        }
        Err(why) => {
            tracing::warn!("pr_body: running {workflow} for {repo}#{number}: {why}");
        }
    }
    found
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Workflows {
    workflows: Vec<Workflow>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Workflow {
    name: String,
    path: String,
    state: String,
}

/// The pictures workflow's file name from `actions/workflows`: the one
/// active workflow whose path or name says keyframes or pictures. Err
/// says why there is none to run.
pub fn discover(json: &str) -> Result<String, String> {
    let list: Workflows =
        serde_json::from_str(json).map_err(|e| format!("its workflows don't parse: {e}"))?;
    let says = |w: &Workflow| {
        [&w.path, &w.name].iter().any(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("keyframe") || s.contains("picture")
        })
    };
    let found: Vec<&Workflow> = list
        .workflows
        .iter()
        .filter(|w| w.state == "active" && says(w))
        .collect();
    match found.as_slice() {
        [one] => {
            let file = one.path.rsplit('/').next().unwrap_or_default().trim();
            match file.is_empty() {
                true => Err(format!("workflow {:?} has no file", one.name)),
                false => Ok(file.to_string()),
            }
        }
        [] => Err("no active workflow is named for keyframes or pictures".into()),
        several => Err(format!(
            "{} active workflows are named for keyframes or pictures ({}); name one as \
             `pictures_workflow` in {}",
            several.len(),
            several
                .iter()
                .map(|w| w.path.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            super::contract::PATH
        )),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Run {
    status: String,
    event: String,
    display_title: String,
    head_branch: String,
    head_sha: String,
}

/// Whether `runs` (`gh run list --json`) has one for PR `detail` queued
/// or in progress. A pull_request run is its when it ran on its branch or
/// head. A dispatched run says which PR only in its title, if at all: one
/// naming no PR may be this one's, so it counts too. Runs that can't be
/// read count as going: better a later dispatch than a second.
fn going(runs: &str, detail: &PrDetail) -> bool {
    let Ok(runs) = serde_json::from_str::<Vec<Run>>(runs) else {
        return true;
    };
    let number = format!("#{}", detail.number);
    let names = |title: &str, pr: &str| {
        title
            .match_indices(pr)
            .any(|(at, _)| !title[at + pr.len()..].starts_with(|c: char| c.is_ascii_digit()))
    };
    runs.iter()
        .filter(|r| r.status != "completed")
        .any(|r| match r.event.as_str() {
            "pull_request" | "pull_request_target" => {
                (!r.head_sha.is_empty() && r.head_sha == detail.head_sha)
                    || (!r.head_branch.is_empty() && r.head_branch == detail.head)
                    || names(&r.display_title, &number)
            }
            "workflow_dispatch" => {
                names(&r.display_title, &number) || !names_a_pr(&r.display_title)
            }
            _ => false,
        })
}

/// Whether `title` has a `#<digits>` in it.
fn names_a_pr(title: &str) -> bool {
    title
        .match_indices('#')
        .any(|(at, _)| title[at + 1..].starts_with(|c: char| c.is_ascii_digit()))
}

/// Log `why` for `repo` the first time only.
fn say_once(repo: &str, why: &str) {
    static SAID: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if said
        .get_or_insert_with(HashSet::new)
        .insert(repo.to_string())
    {
        tracing::info!("pr_body: not running the pictures job for {repo}: {why}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::PrFile;
    use std::cell::RefCell;

    /// Canned answers, and every call made.
    #[derive(Default)]
    struct Fake {
        flow: Option<Option<String>>,
        workflows: Option<String>,
        runs: String,
        calls: RefCell<Vec<String>>,
    }

    impl Ask for Fake {
        async fn flow(&self, number: u64) -> Option<Option<String>> {
            self.calls.borrow_mut().push(format!("flow {number}"));
            self.flow.clone()
        }
        async fn workflows(&self) -> Option<String> {
            self.calls.borrow_mut().push("workflows".into());
            self.workflows.clone()
        }
        async fn runs(&self, workflow: &str) -> Option<String> {
            self.calls.borrow_mut().push(format!("runs {workflow}"));
            Some(self.runs.clone())
        }
        async fn dispatch(&self, workflow: &str, base: &str, number: u64) -> Result<(), String> {
            self.calls
                .borrow_mut()
                .push(format!("dispatch {workflow} --ref {base} -f pr={number}"));
            Ok(())
        }
    }

    impl Fake {
        fn with_flow() -> Fake {
            Fake {
                flow: Some(Some("f1".into())),
                runs: "[]".into(),
                ..Default::default()
            }
        }
        fn dispatched(&self) -> Vec<String> {
            self.calls
                .borrow()
                .iter()
                .filter(|c| c.starts_with("dispatch"))
                .cloned()
                .collect()
        }
    }

    fn pr() -> PrDetail {
        PrDetail {
            number: 2130,
            base: "dev".into(),
            head: "factory/riplo-12".into(),
            head_sha: "abc123".into(),
            files: vec![PrFile {
                path: "apps/web/src/components/a.tsx".into(),
                additions: 1,
                deletions: 0,
                change: String::new(),
            }],
            ..Default::default()
        }
    }

    fn visible() -> Vec<String> {
        vec!["apps/web/src/components/**".into()]
    }

    /// A look that may run the job: true when it did.
    async fn run(fake: &Fake, repo: &str, detail: &PrDetail, workflow: Option<&str>) -> bool {
        look(fake, repo, detail, workflow, true, None)
            .await
            .dispatched
    }

    #[tokio::test]
    async fn a_look_says_what_is_on_its_way_and_runs_the_job_only_when_it_may() {
        let fake = Fake::with_flow();
        let found = look(&fake, "o/r", &pr(), Some("k.yml"), false, None).await;
        assert_eq!(
            found,
            Look {
                flow: true,
                flow_sha: Some("f1".into()),
                ..Look::default()
            }
        );
        assert!(fake.dispatched().is_empty(), "not wanted: only looked");
        let fake = Fake {
            runs: r#"[{"status": "queued", "event": "pull_request", "headSha": "abc123"}]"#.into(),
            ..Fake::with_flow()
        };
        let found = look(&fake, "o/r", &pr(), Some("k.yml"), true, None).await;
        assert!(found.flow && found.going && !found.dispatched);
        let fake = Fake::default();
        assert_eq!(
            look(&fake, "o/r", &pr(), Some("k.yml"), true, None).await,
            Look::default(),
            "no flow"
        );
    }

    #[test]
    fn pictures_are_coming_while_a_run_goes_or_for_half_an_hour_after_orion_s() {
        let flow = Look {
            flow: true,
            ..Look::default()
        };
        let going = Look {
            going: true,
            ..flow.clone()
        };
        let now = 10 * WAIT_MS;
        assert!(coming(&going, "abc", None, None, now));
        assert!(coming(
            &Look {
                dispatched: true,
                ..flow.clone()
            },
            "abc",
            None,
            None,
            now
        ));
        assert!(coming(
            &flow,
            "abc",
            Some("abc"),
            Some(now - WAIT_MS + 1),
            now
        ));
        assert!(
            !coming(&flow, "abc", Some("abc"), Some(now - WAIT_MS), now),
            "half an hour on"
        );
        assert!(
            !coming(&flow, "abc", Some("old"), Some(now), now),
            "run for an older head"
        );
        assert!(
            !coming(&flow, "abc", Some("abc"), None, now),
            "when unknown"
        );
        assert!(
            !coming(&flow, "abc", None, None, now),
            "flow up, nothing run"
        );
        assert!(
            !coming(
                &Look {
                    flow: false,
                    ..going.clone()
                },
                "abc",
                Some("abc"),
                Some(now),
                now
            ),
            "no flow"
        );
    }

    const WORKFLOWS: &str = r#"{"total_count": 3, "workflows": [
        {"id": 1, "name": "CI", "path": ".github/workflows/ci.yml", "state": "active"},
        {"id": 2, "name": "PR keyframes", "path": ".github/workflows/pr-keyframes.yml", "state": "active"},
        {"id": 3, "name": "Old pictures", "path": ".github/workflows/old-pictures.yml", "state": "disabled_manually"}
    ]}"#;

    #[tokio::test]
    async fn the_contract_s_workflow_is_run_without_looking() {
        let fake = Fake::with_flow();
        assert!(run(&fake, "o/r", &pr(), Some("pictures.yml")).await);
        assert_eq!(
            *fake.calls.borrow(),
            [
                "flow 2130",
                "runs pictures.yml",
                "dispatch pictures.yml --ref dev -f pr=2130"
            ]
        );
    }

    #[tokio::test]
    async fn without_the_key_the_workflow_is_found_by_its_name() {
        let fake = Fake {
            workflows: Some(WORKFLOWS.into()),
            ..Fake::with_flow()
        };
        assert!(run(&fake, "o/r", &pr(), None).await);
        assert_eq!(
            fake.dispatched(),
            ["dispatch pr-keyframes.yml --ref dev -f pr=2130"]
        );
        // A blank key is no key.
        let fake = Fake {
            workflows: Some(WORKFLOWS.into()),
            ..Fake::with_flow()
        };
        assert!(run(&fake, "o/r", &pr(), Some(" ")).await);
        assert!(fake.calls.borrow().contains(&"workflows".to_string()));
    }

    #[tokio::test]
    async fn none_or_several_found_runs_nothing() {
        for json in [
            r#"{"workflows": [{"name": "CI", "path": ".github/workflows/ci.yml", "state": "active"}]}"#,
            r#"{"workflows": [
                {"name": "PR keyframes", "path": ".github/workflows/pr-keyframes.yml", "state": "active"},
                {"name": "Pictures", "path": ".github/workflows/pictures.yml", "state": "active"}]}"#,
        ] {
            let fake = Fake {
                workflows: Some(json.into()),
                ..Fake::with_flow()
            };
            assert!(!run(&fake, "o/r", &pr(), None).await);
            assert!(fake.dispatched().is_empty());
        }
        assert!(discover("{}").is_err());
        // Only a disabled one says pictures.
        let renamed = WORKFLOWS
            .replace("pr-keyframes", "x")
            .replace("PR keyframes", "X");
        assert!(discover(&renamed)
            .unwrap_err()
            .contains("no active workflow"));
    }

    #[tokio::test]
    async fn no_flow_no_dispatch() {
        for flow in [Some(None), None] {
            let fake = Fake {
                flow,
                ..Fake::with_flow()
            };
            assert!(!run(&fake, "o/r", &pr(), Some("pr-keyframes.yml")).await);
            assert_eq!(*fake.calls.borrow(), ["flow 2130"], "nothing else asked");
        }
    }

    #[tokio::test]
    async fn no_dispatch_while_a_run_for_the_pr_is_going() {
        let run_of = |status: &str, event: &str, title: &str, branch: &str, sha: &str| {
            serde_json::json!([{"status": status, "event": event, "displayTitle": title,
                "createdAt": "2026-11-15T10:00:00Z", "headBranch": branch, "headSha": sha}])
            .to_string()
        };
        let busy = [
            run_of(
                "in_progress",
                "pull_request",
                "Fix it",
                "factory/riplo-12",
                "old",
            ),
            run_of("queued", "pull_request", "Fix it", "x", "abc123"),
            run_of("queued", "workflow_dispatch", "PR keyframes", "dev", "d0"),
            run_of(
                "in_progress",
                "workflow_dispatch",
                "Pictures for #2130",
                "dev",
                "d0",
            ),
        ];
        for runs in busy {
            let fake = Fake {
                runs: runs.clone(),
                ..Fake::with_flow()
            };
            assert!(!run(&fake, "o/r", &pr(), Some("k.yml")).await, "{runs}");
            assert!(fake.dispatched().is_empty());
        }
        let free = [
            run_of(
                "completed",
                "pull_request",
                "Fix it",
                "factory/riplo-12",
                "abc123",
            ),
            run_of("in_progress", "pull_request", "Other", "other", "fff"),
            run_of(
                "in_progress",
                "workflow_dispatch",
                "Pictures for #21301",
                "dev",
                "d0",
            ),
            run_of("queued", "schedule", "PR keyframes", "dev", "d0"),
        ];
        for runs in free {
            let fake = Fake {
                runs: runs.clone(),
                ..Fake::with_flow()
            };
            assert!(run(&fake, "o/r", &pr(), Some("k.yml")).await, "{runs}");
        }
        assert!(going("not json", &pr()), "can't tell: wait");
    }

    #[test]
    fn wanted_for_a_visible_pr_without_pictures_that_show_a_change() {
        let none = Manifest::default();
        assert!(wanted(&pr(), &visible(), &none));
        assert!(!wanted(&pr(), &[], &none), "not visible");
        let ran_here = Manifest {
            head_sha: "abc123".into(),
            ..Default::default()
        };
        assert!(
            !wanted(&pr(), &visible(), &ran_here),
            "the job ran at this head"
        );
        let grid = pictures::Grid {
            file: "g.png".into(),
            url: "https://x/g.png".into(),
        };
        let stale = Manifest {
            head_sha: "older".into(),
            grids: vec![grid.clone()],
            ..Default::default()
        };
        assert!(
            wanted(&pr(), &visible(), &stale),
            "pictures of an older push"
        );
        let nothing_shown = Manifest {
            head_sha: "abc123".into(),
            grids: vec![grid],
            frames: vec![pictures::Frame {
                verdict: "no visible difference".into(),
                difference: Some(0.0),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            wanted(&pr(), &visible(), &nothing_shown),
            "pictures here that show no difference"
        );
        let no_base = PrDetail {
            base: String::new(),
            ..pr()
        };
        assert!(!wanted(&no_base, &visible(), &none));
    }

    #[test]
    fn what_ran_at_the_head_comes_from_memory_or_the_pictures_taken_there() {
        let none = Manifest::default();
        let ran = |head: &str, flow: Option<&str>| Ran {
            head: head.into(),
            flow: flow.map(String::from),
        };
        let remembered = ran("abc123", Some("f1"));
        assert_eq!(
            ran_at_head(&pr(), Some(&remembered), None, &none),
            Some(remembered)
        );
        let older = ran("older", Some("f1"));
        assert_eq!(ran_at_head(&pr(), Some(&older), None, &none), None);
        // Dispatched here before flows were remembered: any flow.
        assert_eq!(
            ran_at_head(&pr(), None, Some("abc123"), &none),
            Some(ran("abc123", None))
        );
        // Taken here by the job on the push: whatever flow is up.
        let taken = Manifest {
            head_sha: "abc123".into(),
            ..Default::default()
        };
        assert_eq!(
            ran_at_head(&pr(), Some(&older), None, &taken),
            Some(ran("abc123", None))
        );
        assert!(ran("abc123", None).covers("abc123", "f9"));
        assert!(!ran("abc123", Some("f1")).covers("abc123", "f2"));
        assert!(!ran("older", Some("f1")).covers("abc123", "f1"));
    }

    #[tokio::test]
    async fn a_new_flow_at_the_same_head_runs_the_job_once_more() {
        let ran = Ran {
            head: "abc123".into(),
            flow: Some("f1".into()),
        };
        // The same head, the same flow: not again.
        let fake = Fake::with_flow();
        let found = look(&fake, "o/r", &pr(), Some("k.yml"), true, Some(&ran)).await;
        assert!(found.flow && !found.dispatched, "{found:?}");
        assert!(fake.dispatched().is_empty());
        // The same head, the flow uploaded again: once more.
        let fake = Fake {
            flow: Some(Some("f2".into())),
            ..Fake::with_flow()
        };
        let found = look(&fake, "o/r", &pr(), Some("k.yml"), true, Some(&ran)).await;
        assert!(found.dispatched, "{found:?}");
        assert_eq!(found.flow_sha.as_deref(), Some("f2"));
        assert_eq!(fake.dispatched(), ["dispatch k.yml --ref dev -f pr=2130"]);
        // Still guarded by a run in progress.
        let fake = Fake {
            flow: Some(Some("f2".into())),
            runs: r#"[{"status": "in_progress", "event": "pull_request", "headSha": "abc123"}]"#
                .into(),
            ..Fake::with_flow()
        };
        let found = look(&fake, "o/r", &pr(), Some("k.yml"), true, Some(&ran)).await;
        assert!(found.going && !found.dispatched);
    }
}
