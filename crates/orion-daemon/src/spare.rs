//! SPARE WORKTREES: the warm next worktree, one per project in use, so a
//! launch into a new worktree starts at once.
//!
//! A CLI's working directory is fixed when it boots, and Claude ties its
//! memory and transcripts to it, so nothing repo-agnostic can stand by.
//! What can is a worktree: for every project used in the last day the
//! DAEMON keeps one cut from the project's **Base branch**, its `.env`
//! links made and its SETUP COMMAND run, with a `claude` booted in it on
//! the `⌘N` account at that account's default model and effort, sitting at
//! its prompt. Its branch and its directory are both `orion-spare-<id>`
//! ([`orion_core::SPARE_PREFIX`]).
//!
//! A spare has no rows. Its setup and its CLI are PTYs in the sessions map
//! like any other, under ids no client knows, so the grid never shows it;
//! `git::list_worktrees` leaves its branch out, so the worktree sync never
//! adopts it, and the TUI's branch lists leave it out too.
//!
//! [`Daemon::tend_spares`], on its own tick, keeps the book: it cuts the
//! spare a project in use lacks — one at a time, and none while another
//! spare is still setting up — and takes away the spare of a project
//! nobody has used for a day, one whose setup failed, and one whose
//! checkout has gone. A CLI that died, or never said it had booted, is
//! booted again a little later; one booted for another account is
//! replaced.

use crate::config::Config;
use crate::git;
use crate::pty::{DEFAULT_COLS, DEFAULT_ROWS};
use crate::registry::Daemon;
use anyhow::{Context, Result};
use orion_core::{
    Agent, AgentId, AgentStatus, LaunchSpec, Project, ProjectId, SessionRef, TerminalId,
    TerminalTab, Worktree, WorktreeId,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a project goes unused before its spare is taken away.
const IDLE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);
/// How long after a spare went wrong — its cut or its setup failed — the
/// project waits for the next one, so a setup that always fails is not
/// run again on every tick.
const RECUT_AFTER: Duration = Duration::from_secs(10 * 60);
/// How long after a spare's CLI died, or never booted, it waits to be
/// booted again.
const REBOOT_AFTER: Duration = Duration::from_secs(2 * 60);
/// A spare's CLI that has not fired its `SessionStart` this long after it
/// was spawned is stuck short of its prompt — a login, a trust question,
/// hooks that never installed — and is not warm.
pub(crate) const BOOT_LIMIT: Duration = Duration::from_secs(60);

/// The DAEMON's SPARE WORKTREES, and what it keeps to tend them.
#[derive(Default)]
pub(crate) struct SpareBook {
    book: Mutex<Book>,
}

#[derive(Default)]
struct Book {
    /// At most one per project.
    spares: HashMap<ProjectId, Spare>,
    /// Projects a spare is being cut, taken, refreshed or removed for: no
    /// second operation starts on one while the first runs.
    busy: HashSet<ProjectId>,
    /// Every spare CLI by its agent id, until its first turn is in: what
    /// its `SessionStart` marks.
    boots: HashMap<AgentId, Arc<Boot>>,
    /// When each project was last used here, epoch ms, beside what the
    /// store's agent rows say ([`Daemon::last_used`]).
    used: HashMap<ProjectId, i64>,
    /// No spare is cut for these projects before then.
    recut_at: HashMap<ProjectId, Instant>,
}

struct Spare {
    /// The project's repo, kept for the removal of a spare whose project
    /// has gone.
    repo: PathBuf,
    /// The row it becomes when taken: its id is the one the setup and the
    /// CLI were spawned under.
    worktree: Worktree,
    /// The commit it was cut from or last reset to, and the ref that named
    /// it (None for the checkout's HEAD).
    base: Base,
    setup: Setup,
    /// The SETUP COMMAND it was set up with.
    setup_command: Option<String>,
    /// What its CLI boots as: the `⌘N` launch when it was cut or last
    /// booted.
    launch: LaunchSpec,
    cli: Option<SpareCli>,
    /// No CLI is booted for it before then.
    reboot_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Base {
    name: Option<String>,
    sha: String,
}

enum Setup {
    None,
    Running(TerminalTab),
    Done,
    Failed,
}

struct SpareCli {
    agent: Agent,
    boot: Arc<Boot>,
}

/// One spare CLI's boot: when it was spawned and when it said it was up.
pub(crate) struct Boot {
    pub(crate) spawned_at: Instant,
    hook_at: Mutex<Option<Instant>>,
}

impl Boot {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            spawned_at: Instant::now(),
            hook_at: Mutex::new(None),
        })
    }

    /// When the CLI fired its `SessionStart`, if it has.
    pub(crate) fn hook_at(&self) -> Option<Instant> {
        *self.hook_at.lock().unwrap()
    }

    /// Never fired its `SessionStart`, and is past waiting for it.
    fn stuck(&self) -> bool {
        self.hook_at().is_none() && self.spawned_at.elapsed() > BOOT_LIMIT
    }
}

/// What a tick does about one project's spare.
enum Tend {
    Cut,
    Remove { recut: bool },
    BootCli,
}

impl SpareBook {
    /// Project `project` was just used: a create in it, an attach.
    pub(crate) fn note_used(&self, project: &ProjectId) {
        self.note_used_at(project, orion_core::clock::now_ms());
    }

    fn note_used_at(&self, project: &ProjectId, at: i64) {
        let mut book = self.book.lock().unwrap();
        let stamp = book.used.entry(project.clone()).or_insert(at);
        *stamp = (*stamp).max(at);
    }

    /// A `SessionStart` from agent `id`: a spare CLI's says it is up.
    pub(crate) fn note_boot(&self, id: &AgentId) {
        if let Some(boot) = self.book.lock().unwrap().boots.get(id) {
            boot.hook_at
                .lock()
                .unwrap()
                .get_or_insert_with(Instant::now);
        }
    }

    /// SETUP TERMINAL `id` exited with `exit_code`: when it is a spare's,
    /// the spare records how its setup went and the answer is true.
    pub(crate) fn setup_ended(&self, id: &TerminalId, exit_code: Option<i32>) -> bool {
        let mut book = self.book.lock().unwrap();
        let Some(spare) = book
            .spares
            .values_mut()
            .find(|s| matches!(&s.setup, Setup::Running(term) if &term.id == id))
        else {
            return false;
        };
        tracing::info!(worktree = %spare.worktree.path.display(), exit_code, "spare worktree's setup ended");
        spare.setup = if exit_code == Some(0) {
            Setup::Done
        } else {
            Setup::Failed
        };
        true
    }
}

impl Daemon {
    /// One tick of the SPARE WORKTREES, under the settings as they are now.
    pub async fn tend_spares(self: &Arc<Self>) {
        self.tend_spares_with(&Config::load()).await;
    }

    /// [`Daemon::tend_spares`] against a given config, for the tests that
    /// can't pin the settings file.
    pub(crate) async fn tend_spares_with(self: &Arc<Self>, config: &Config) {
        let Ok((projects, worktrees, agents, _)) = self.store.load_tree() else {
            return;
        };
        // A project's spare goes with the project.
        let gone: Vec<ProjectId> = {
            let book = self.spares.book.lock().unwrap();
            book.spares
                .keys()
                .filter(|id| !projects.iter().any(|p| &p.id == *id))
                .cloned()
                .collect()
        };
        for project in gone {
            self.remove_spare(&project, false).await;
        }
        let launch = config.spare_launch();
        let mut cut_one = false;
        for project in &projects {
            let used = self.last_used(project, &worktrees, &agents);
            let wanted = launch.is_some() && used_lately(used);
            match self.what_to_tend(project, wanted, launch.as_ref(), cut_one) {
                Some(Tend::Cut) => {
                    cut_one = true;
                    self.cut_spare(project, config).await;
                }
                Some(Tend::Remove { recut }) => self.remove_spare(&project.id, recut).await,
                Some(Tend::BootCli) => {
                    if let Some(launch) = &launch {
                        self.boot_spare_cli(&project.id, launch).await;
                    }
                }
                None => {}
            }
        }
    }

    /// When project `project` was last used, epoch ms: a create or an
    /// attach here, or a status change on one of its sessions — the
    /// store's half, which outlives a DAEMON restart.
    fn last_used(&self, project: &Project, worktrees: &[Worktree], agents: &[Agent]) -> i64 {
        let ours: HashSet<&WorktreeId> = worktrees
            .iter()
            .filter(|w| w.project_id == project.id)
            .map(|w| &w.id)
            .collect();
        let stored = agents
            .iter()
            .filter(|a| ours.contains(&a.worktree_id))
            .map(|a| a.status_changed_at)
            .max()
            .unwrap_or(0);
        let noted = self
            .spares
            .book
            .lock()
            .unwrap()
            .used
            .get(&project.id)
            .copied()
            .unwrap_or(0);
        stored.max(noted)
    }

    /// What this tick does for `project`: nothing while something is
    /// already under way for it. `cut_one`: a spare was cut this tick.
    fn what_to_tend(
        &self,
        project: &Project,
        wanted: bool,
        launch: Option<&LaunchSpec>,
        cut_one: bool,
    ) -> Option<Tend> {
        let mut guard = self.spares.book.lock().unwrap();
        let book = &mut *guard;
        if book.busy.contains(&project.id) {
            return None;
        }
        let Some(spare) = book.spares.get_mut(&project.id) else {
            // One cut a tick, and none while another spare's setup runs:
            // a setup is an install, and several at once would drag the
            // machine down for a launch nobody has asked for yet.
            let setting_up = book
                .spares
                .values()
                .any(|s| matches!(s.setup, Setup::Running(_)));
            let due = book
                .recut_at
                .get(&project.id)
                .is_none_or(|at| Instant::now() >= *at);
            return (wanted && due && !cut_one && !setting_up).then_some(Tend::Cut);
        };
        if !wanted {
            return Some(Tend::Remove { recut: false });
        }
        if matches!(spare.setup, Setup::Failed) || !spare.worktree.path.exists() {
            return Some(Tend::Remove { recut: true });
        }
        // A CLI that died, never came up, or was booted for an account
        // `⌘N` no longer starts on goes; another is booted in its place
        // once its wait is over.
        let relaunch = Some(&spare.launch) != launch;
        let stale = spare.cli.take_if(|cli| {
            relaunch || cli.boot.stuck() || !self.is_alive(&SessionRef::Agent(cli.agent.id.clone()))
        });
        if stale.is_some() && !relaunch {
            spare.reboot_at = Some(Instant::now() + REBOOT_AFTER);
        }
        let due = spare.reboot_at.is_none_or(|at| Instant::now() >= at);
        let tend = (spare.cli.is_none() && due).then_some(Tend::BootCli);
        if let Some(cli) = stale {
            book.boots.remove(&cli.agent.id);
            drop(guard);
            self.kill_session(&SessionRef::Agent(cli.agent.id));
        }
        tend
    }

    /// Cut project `project` a spare: its base fetched, the worktree
    /// added, its `.env` links made, its setup and its CLI started. A cut
    /// that fails is logged, and the project waits [`RECUT_AFTER`].
    pub(crate) async fn cut_spare(self: &Arc<Self>, project: &Project, config: &Config) {
        let Some(launch) = config.spare_launch() else {
            return;
        };
        {
            let mut book = self.spares.book.lock().unwrap();
            if book.spares.contains_key(&project.id) || !book.busy.insert(project.id.clone()) {
                return;
            }
        }
        let cut = self.cut_spare_inner(project, config, launch).await;
        let mut book = self.spares.book.lock().unwrap();
        book.busy.remove(&project.id);
        match cut {
            Ok(spare) => {
                if let Some(cli) = &spare.cli {
                    book.boots.insert(cli.agent.id.clone(), cli.boot.clone());
                }
                book.spares.insert(project.id.clone(), spare);
            }
            Err(e) => {
                tracing::warn!(project = %project.name, error = %e, "couldn't cut a spare worktree");
                book.recut_at
                    .insert(project.id.clone(), Instant::now() + RECUT_AFTER);
            }
        }
    }

    async fn cut_spare_inner(
        self: &Arc<Self>,
        project: &Project,
        config: &Config,
        launch: LaunchSpec,
    ) -> Result<Spare> {
        let repo = &project.repo_path;
        let base = spare_base(repo, config, true)
            .await
            .context("no commit to cut it from")?;
        let branch = format!("{}{}", orion_core::SPARE_PREFIX, spare_suffix());
        let path = git::add_spare_worktree(repo, &branch, &base.sha).await?;
        let worktree = Worktree {
            id: WorktreeId::generate(),
            project_id: project.id.clone(),
            path,
            branch,
            is_main: false,
            sort_order: 0,
        };
        let mut spare = Spare {
            repo: repo.clone(),
            worktree,
            base,
            setup: Setup::None,
            setup_command: config.setup_command(repo).map(str::to_string),
            launch,
            cli: None,
            reboot_at: None,
        };
        tracing::info!(
            project = %project.name,
            worktree = %spare.worktree.path.display(),
            base = spare.base.name.as_deref().unwrap_or("HEAD"),
            "cut a spare worktree"
        );
        // Gone while the cut ran: nothing of it is wanted.
        if self.shutdown.is_cancelled() || self.store.get_project(&project.id)?.is_none() {
            git::remove_spare_worktree(repo, &spare.worktree.path, &spare.worktree.branch).await?;
            anyhow::bail!("its project went while it was cut");
        }
        self.link_env_files(repo, &spare.worktree.path).await;
        if let Some(command) = &spare.setup_command {
            match self.start_spare_setup(&spare.worktree, command) {
                Ok(term) => spare.setup = Setup::Running(term),
                Err(e) => {
                    tracing::warn!(error = %e, "spare worktree's setup didn't start");
                    spare.setup = Setup::Failed;
                }
            }
        }
        spare.cli = self.spawn_spare_cli(&spare.worktree, &spare.launch).await;
        spare.reboot_at = spare.cli.is_none().then(|| Instant::now() + REBOOT_AFTER);
        Ok(spare)
    }

    /// The spare's SETUP TERMINAL: a command terminal with no row, which
    /// [`SpareBook::setup_ended`] hears the end of.
    fn start_spare_setup(
        self: &Arc<Self>,
        worktree: &Worktree,
        command: &str,
    ) -> Result<TerminalTab> {
        let term = TerminalTab {
            id: TerminalId::generate(),
            worktree_id: worktree.id.clone(),
            name: crate::registry::SETUP_TERMINAL_NAME.into(),
            sort_order: 0,
            alive: false,
            run_command: Some(command.to_string()),
            setup: true,
            run: Default::default(),
        };
        self.spawn_terminal_session(&term, worktree, DEFAULT_COLS, DEFAULT_ROWS)?;
        Ok(term)
    }

    /// Boot `launch`'s CLI in spare `worktree`, bare, under an agent id no
    /// row has yet. None when its CLI is not installed or would not spawn.
    async fn spawn_spare_cli(
        self: &Arc<Self>,
        worktree: &Worktree,
        launch: &LaunchSpec,
    ) -> Option<SpareCli> {
        let harness =
            crate::registry::resolve_harness(launch.kind, launch.custom_harness.as_deref()).ok()?;
        if self.shutdown.is_cancelled() || !self.cli_available(harness.program.trim()).await {
            return None;
        }
        let agent = Agent {
            id: AgentId::generate(),
            worktree_id: worktree.id.clone(),
            name: "spare".into(),
            status: AgentStatus::Fresh,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: launch.kind,
            custom_harness: launch.custom_harness.clone(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
            session_id: None,
            sort_order: 0,
            status_changed_at: 0,
            alive: false,
            recent_prompts: Vec::new(),
            usage_limit: None,
        };
        match self.spawn_agent_session(&agent, worktree, DEFAULT_COLS, DEFAULT_ROWS) {
            Ok(_) => Some(SpareCli {
                agent,
                boot: Boot::new(),
            }),
            Err(e) => {
                tracing::warn!(error = %e, "spare worktree's CLI didn't spawn");
                None
            }
        }
    }

    /// Boot project `project`'s spare a CLI as `launch`, its last one gone.
    async fn boot_spare_cli(self: &Arc<Self>, project: &ProjectId, launch: &LaunchSpec) {
        let worktree = {
            let mut book = self.spares.book.lock().unwrap();
            let Some(spare) = book.spares.get(project) else {
                return;
            };
            if spare.cli.is_some() || !book.busy.insert(project.clone()) {
                return;
            }
            book.spares[project].worktree.clone()
        };
        let cli = self.spawn_spare_cli(&worktree, launch).await;
        let mut book = self.spares.book.lock().unwrap();
        book.busy.remove(project);
        let Some(spare) = book.spares.get_mut(project) else {
            if let Some(cli) = cli {
                self.kill_session(&SessionRef::Agent(cli.agent.id));
            }
            return;
        };
        spare.launch = launch.clone();
        spare.reboot_at = cli.is_none().then(|| Instant::now() + REBOOT_AFTER);
        if let Some(cli) = cli {
            book.boots.insert(cli.agent.id.clone(), cli.boot.clone());
            book.spares.get_mut(project).expect("still there").cli = Some(cli);
        }
    }

    /// Take project `project`'s spare off the disk — its PTYs killed, its
    /// checkout and branch removed. `recut`: it went wrong, and the next
    /// one waits [`RECUT_AFTER`].
    pub(crate) async fn remove_spare(self: &Arc<Self>, project: &ProjectId, recut: bool) {
        let spare = {
            let mut book = self.spares.book.lock().unwrap();
            if !book.busy.insert(project.clone()) {
                return;
            }
            let spare = book.spares.remove(project);
            if let Some(cli) = spare.as_ref().and_then(|s| s.cli.as_ref()) {
                book.boots.remove(&cli.agent.id);
            }
            if recut {
                book.recut_at
                    .insert(project.clone(), Instant::now() + RECUT_AFTER);
            }
            spare
        };
        if let Some(spare) = spare {
            self.kill_spare_sessions(&spare);
            tracing::info!(worktree = %spare.worktree.path.display(), "removing a spare worktree");
            if let Err(e) = git::remove_spare_worktree(
                &spare.repo,
                &spare.worktree.path,
                &spare.worktree.branch,
            )
            .await
            {
                tracing::warn!(worktree = %spare.worktree.path.display(), error = %e, "spare worktree not fully removed");
            }
        }
        self.spares.book.lock().unwrap().busy.remove(project);
    }

    /// Kill a spare's setup and CLI.
    fn kill_spare_sessions(&self, spare: &Spare) {
        if let Some(cli) = &spare.cli {
            self.kill_session(&SessionRef::Agent(cli.agent.id.clone()));
        }
        if let Setup::Running(term) = &spare.setup {
            self.kill_session(&SessionRef::Terminal(term.id.clone()));
        }
    }

    /// Project `project`'s spare, as tests look at it: its checkout and
    /// branch, and its CLI's agent id.
    #[cfg(test)]
    pub(crate) fn spare_of(&self, project: &ProjectId) -> Option<(Worktree, Option<AgentId>)> {
        let book = self.spares.book.lock().unwrap();
        let spare = book.spares.get(project)?;
        Some((
            spare.worktree.clone(),
            spare.cli.as_ref().map(|cli| cli.agent.id.clone()),
        ))
    }

    /// Stamp project `project` last used at `at` (epoch ms), for tests.
    #[cfg(test)]
    pub(crate) fn note_spare_use_at(&self, project: &ProjectId, at: i64) {
        self.spares
            .book
            .lock()
            .unwrap()
            .used
            .insert(project.clone(), at);
    }
}

/// Whether a project last used at `used` (epoch ms) still has a spare.
fn used_lately(used: i64) -> bool {
    orion_core::clock::now_ms() - used < IDLE_AFTER.as_millis() as i64
}

/// The base a spare of the project at `repo` starts from: the ref a cold
/// new worktree would ([`git::new_worktree_base`]), and its commit.
async fn spare_base(repo: &Path, config: &Config, fetch: bool) -> Option<Base> {
    let name = git::new_worktree_base(repo, config.base_branch(repo), fetch).await;
    let sha = git::rev_parse(repo, name.as_deref().unwrap_or("HEAD")).await?;
    Some(Base { name, sha })
}

/// Six characters off a fresh ULID's random half: what tells one spare's
/// name from the next.
fn spare_suffix() -> String {
    let id = orion_core::ids::ProjectId::generate()
        .0
        .to_ascii_lowercase();
    id[id.len() - 6..].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::tests::{init_repo, project_at, test_daemon, EnvGuard};
    use orion_core::env;

    /// A config with Claude on for `⌘N` and `rows` as the project at
    /// `repo`'s Projects-tab rows.
    fn spare_config(repo: &Path, rows: serde_json::Value) -> Config {
        serde_json::from_value(serde_json::json!({
            "claude_enabled": true,
            "projects": { repo.display().to_string(): rows },
        }))
        .unwrap()
    }

    /// `git worktree list --porcelain` as git says it, spares and all.
    fn git_worktree_paths(repo: &Path) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn branches(repo: &Path) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["branch", "--list", "--format=%(refname:short)"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "{what} never happened");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// A project in use gets one spare: cut from its base under an
    /// orion-spare name, set up, its CLI booted — and nothing a client or
    /// the worktree sync can see, though git lists it.
    #[tokio::test]
    async fn a_project_in_use_gets_one_spare_hidden_from_its_rows() {
        let daemon = test_daemon();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path());
        let project = project_at(&daemon, &repo);
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let cfg = spare_config(
            &repo,
            serde_json::json!({ "setup_command": "touch set-up" }),
        );

        daemon.tend_spares_with(&cfg).await;
        assert!(daemon.spare_of(&project.id).is_none(), "unused: no spare");

        daemon.spares.note_used(&project.id);
        daemon.tend_spares_with(&cfg).await;
        let (spare, cli) = daemon.spare_of(&project.id).expect("a spare");
        assert!(
            orion_core::is_spare_branch(&spare.branch),
            "{}",
            spare.branch
        );
        assert_eq!(
            spare.path.file_name().unwrap().to_string_lossy(),
            spare.branch,
            "its directory goes by its branch's name"
        );
        assert_eq!(
            git::rev_parse(&spare.path, "HEAD").await,
            git::rev_parse(&repo, "main").await,
            "cut from the base"
        );
        let cli = cli.expect("its CLI booted");
        assert!(daemon.is_alive(&SessionRef::Agent(cli)));
        let marker = spare.path.join("set-up");
        until("the spare's setup", || marker.exists()).await;

        assert!(git_worktree_paths(&repo).contains(&*spare.path.to_string_lossy()));
        assert!(git::list_worktrees(&repo)
            .await
            .unwrap()
            .iter()
            .all(|w| w.path != spare.path));
        daemon.sync_project_worktrees(&project).await.unwrap();
        let (_, worktrees, agents, terminals) = daemon.store.load_tree().unwrap();
        assert_eq!(
            worktrees.len(),
            1,
            "the sync adopts no spare: {worktrees:?}"
        );
        assert!(
            agents.is_empty() && terminals.is_empty(),
            "no rows of its own"
        );

        daemon.tend_spares_with(&cfg).await;
        assert_eq!(
            daemon.spare_of(&project.id).unwrap().0.path,
            spare.path,
            "one spare a project"
        );
        daemon.kill_all();
    }

    /// A project nobody has used for a day loses its spare — checkout and
    /// branch — and so does one whose `⌘N` no longer starts on Claude.
    #[tokio::test]
    async fn a_spare_goes_once_its_project_is_idle_or_cmd_n_moves_off_claude() {
        let daemon = test_daemon();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path());
        let project = project_at(&daemon, &repo);
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let cfg = spare_config(&repo, serde_json::json!({}));

        daemon.spares.note_used(&project.id);
        daemon.tend_spares_with(&cfg).await;
        let (spare, _) = daemon.spare_of(&project.id).expect("a spare");
        let day_ago = orion_core::clock::now_ms() - IDLE_AFTER.as_millis() as i64 - 1_000;
        daemon.note_spare_use_at(&project.id, day_ago);
        daemon.tend_spares_with(&cfg).await;
        assert!(daemon.spare_of(&project.id).is_none());
        assert!(!spare.path.exists(), "its checkout went");
        assert!(!branches(&repo).contains(&spare.branch), "its branch went");

        daemon.spares.note_used(&project.id);
        daemon.tend_spares_with(&cfg).await;
        let (spare, _) = daemon.spare_of(&project.id).expect("in use again");
        let mut codex = cfg.clone();
        codex.quick_prompt_kind = "codex".into();
        daemon.tend_spares_with(&codex).await;
        assert!(daemon.spare_of(&project.id).is_none());
        assert!(!spare.path.exists() && !branches(&repo).contains(&spare.branch));
        daemon.kill_all();
    }

    /// A spare whose setup failed is taken away, and the project waits
    /// before the next one is cut rather than failing on every tick.
    #[tokio::test]
    async fn a_failed_setup_takes_the_spare_away_and_waits() {
        let daemon = test_daemon();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path());
        let project = project_at(&daemon, &repo);
        let _cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let cfg = spare_config(&repo, serde_json::json!({ "setup_command": "exit 3" }));

        daemon.spares.note_used(&project.id);
        daemon.tend_spares_with(&cfg).await;
        let (spare, _) = daemon.spare_of(&project.id).expect("a spare");
        until("the failed setup", || {
            let book = daemon.spares.book.lock().unwrap();
            matches!(book.spares[&project.id].setup, Setup::Failed)
        })
        .await;
        daemon.tend_spares_with(&cfg).await;
        assert!(daemon.spare_of(&project.id).is_none());
        assert!(!spare.path.exists());
        daemon.tend_spares_with(&cfg).await;
        assert!(daemon.spare_of(&project.id).is_none(), "no recut yet");
        daemon.kill_all();
    }
}
