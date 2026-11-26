//! THE REPO'S FLOW: how a repo's pull requests move, as its PR contract's
//! `[flow]` table says (`pr_body::contract::Flow`), read for the PR fixer
//! from the project's base branch as this Mac last fetched it — `git show
//! origin/<base>:.orion/pr-contract.toml`, no GitHub asked — and kept for
//! [`FLOW_TTL`].
//!
//! `ready = "when-clean"`: the repo's agents open drafts, and a draft is
//! where the cheap checks and the review bot run. The fixer then treats
//! your drafts there as it treats ready pull requests — their failing
//! checks, conflicts and Cubic findings sent at once — and marks one ready
//! itself ([`mark_ready`]) once it is clean: every check passed on its
//! head, it merges, Cubic left nothing open at P0 or P1, and no session is
//! at work on its branch. Marking it ready is what starts the expensive
//! suite, which then runs once.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use orion_core::ProjectId;

use crate::app::App;
use crate::flash::Flash;
use crate::pr_body::contract::{self, Flow, Ready};
use crate::pull_request::{Checks, OpenPr, PrDetail};

/// How long a project's flow is trusted before it is read again.
pub const FLOW_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a read may be out before another is asked.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `gh pr ready` may take.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// A project's flow as last read.
#[derive(Debug, Clone)]
pub struct Known {
    pub at: Instant,
    pub flow: Flow,
}

/// The project's flow as last read; the default (`ready = "manual"`)
/// before the first read lands.
pub fn of(app: &App, project: &ProjectId) -> Flow {
    app.autofix
        .flows
        .get(project)
        .map(|k| k.flow.clone())
        .unwrap_or_default()
}

/// Whether the fixer works on your drafts in `project` as on ready pull
/// requests: its flow has orion mark them ready when clean.
pub fn fixes_drafts(app: &App, project: &ProjectId) -> bool {
    of(app, project).ready == Ready::WhenClean
}

/// Read `project`'s flow again when it is older than [`FLOW_TTL`] — on
/// the blocking pool, inline where there is none — unless a read is out.
pub fn refresh(app: &mut App, project: &ProjectId) {
    let fresh = app
        .autofix
        .flows
        .get(project)
        .is_some_and(|k| k.at.elapsed() < FLOW_TTL);
    let out = app
        .autofix
        .flow_reads
        .get(project)
        .is_some_and(|at| at.elapsed() < READ_TIMEOUT);
    if fresh || out {
        return;
    }
    let Some(repo) = app
        .tree
        .projects
        .iter()
        .find(|p| &p.id == project)
        .map(|p| p.repo_path.clone())
    else {
        return;
    };
    app.autofix
        .flow_reads
        .insert(project.clone(), Instant::now());
    let project = project.clone();
    match app.view_jobs.clone() {
        Some(jobs) => jobs.run(move || {
            let flow = read(&repo);
            Some(crate::view_jobs::Answer::RepoFlow { project, flow })
        }),
        None => {
            let flow = read(&repo);
            land(app, project, flow);
        }
    }
}

/// A read landed.
pub fn land(app: &mut App, project: ProjectId, flow: Flow) {
    app.autofix.flow_reads.remove(&project);
    app.autofix.flows.insert(
        project,
        Known {
            at: Instant::now(),
            flow,
        },
    );
}

/// The flow in the contract on the project's base branch, as fetched —
/// the default when there is no contract, or it doesn't parse. Blocking.
fn read(repo: &Path) -> Flow {
    let setting = crate::config::base_branch_of(Some(repo));
    let Some(base) = crate::commit_list::resolve_base(repo, &setting) else {
        return Flow::default();
    };
    let spec = format!("{base}:{}", contract::PATH);
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["show", &spec])
        .output();
    match out {
        Ok(out) if out.status.success() => contract::parse(&String::from_utf8_lossy(&out.stdout))
            .map(|c| c.flow)
            .unwrap_or_default(),
        _ => Flow::default(),
    }
}

/// Why `pr`, one of your drafts in a `when-clean` project, is not clean
/// yet — or None when it is: every check passed on the head `detail` was
/// read at, GitHub says it merges, Cubic left nothing open at P0 or P1,
/// and nobody is at work on it (a session busy on its branch, work queued
/// or a fixer on its way, a breakage watched).
pub fn not_clean(
    app: &App,
    project: &ProjectId,
    pr: &OpenPr,
    detail: &PrDetail,
) -> Option<&'static str> {
    let status = app.prs.status_or_open(&pr.url);
    if !status.is_draft() {
        return Some("not a draft");
    }
    if !status.checks_known || status.health.checks != Checks::Passing {
        return Some("checks not all passed");
    }
    if !status.conflicts_known || status.health.conflicts {
        return Some("not known to merge");
    }
    if detail.head_sha.is_empty() || detail.head_sha != status.head_sha {
        return Some("body not read at its head");
    }
    let serious = crate::pr_body::cubic::unanswered(&detail.threads).any(|t| {
        t.comments
            .first()
            .and_then(|c| crate::pr_body::cubic::severity(&c.body))
            .is_some_and(|p| p <= 1)
    });
    if serious {
        return Some("Cubic has a P0 or P1 open");
    }
    let busy = app.tree.agents.iter().any(|a| {
        !a.archived && super::router::busy(a.status) && super::works_on(app, a, project, &pr.head)
    });
    let pending = app.autofix.watching.contains_key(&pr.url)
        || app.autofix.launches.contains_key(&pr.url)
        || app.autofix.queued.iter().any(|q| q.work.pr().url == pr.url);
    if busy || pending {
        return Some("someone is at work on it");
    }
    None
}

/// Mark `pr` ready (`gh pr ready`) when its project's flow says to and it
/// is clean as `detail` shows it ([`not_clean`]) — once per head commit
/// (`Record::readied`), so a refusal is not asked again until a push.
pub fn mark_ready(app: &mut App, project: &ProjectId, pr: &OpenPr, detail: &PrDetail) {
    if !fixes_drafts(app, project) {
        return;
    }
    if not_clean(app, project, pr, detail).is_some() {
        return;
    }
    let head = detail.head_sha.clone();
    if app
        .autofix
        .ledger
        .get(&pr.url)
        .and_then(|r| r.readied.as_deref())
        == Some(head.as_str())
    {
        return;
    }
    let Some(dir) = super::repo_dir(app, project) else {
        return;
    };
    app.autofix.record(&pr.url).readied = Some(head);
    spawn_ready(dir, pr.number);
    app.flash = Some(Flash::working(format!(
        "PR fixer marked #{} ready: its checks passed and Cubic has nothing open",
        pr.number
    )));
    app.dirty = true;
}

fn spawn_ready(dir: PathBuf, number: u64) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    rt.spawn(async move {
        let n = number.to_string();
        crate::pull_request::gh(Some(&dir), &["pr", "ready", &n], READY_TIMEOUT).await;
    });
}
