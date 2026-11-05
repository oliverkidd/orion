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
//! A launch into a new worktree takes it ([`Daemon::create_launch_worktree`])
//! when it is the launch the spare's CLI booted as — same account, model,
//! effort, edit mode — and its branch name is free: the branch is renamed
//! (`git branch -m`; the directory keeps its name), the row goes out under
//! the id the setup and the CLI were spawned with, a setup still running
//! shows as the worktree's setup row, and the CLI is held for the
//! `CreateAgent` that follows ([`Daemon::take_held_cli`]), which adopts it
//! and types the task in as its first turn once the CLI is at its prompt
//! ([`Daemon::send_first_turn`]). A new spare is cut behind it. Any other
//! launch is cut cold, as before.
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
use orion_core::harness::AgentMode;
use orion_core::{
    Agent, AgentId, AgentStatus, Entity, EntityId, LaunchSpec, Project, ProjectId, ServerEvent,
    SessionRef, TerminalId, TerminalTab, Worktree, WorktreeId,
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
const BOOT_LIMIT: Duration = Duration::from_secs(60);
/// How long a just-booted CLI's screen must sit still after its
/// `SessionStart` before a first turn is typed at it: the prompt is drawn.
const SETTLE: Duration = Duration::from_millis(500);
/// Past this long after its `SessionStart`, a CLI whose screen never sits
/// still is typed at all the same.
const SETTLE_CAP: Duration = Duration::from_secs(5);
/// How often a first turn waiting on its CLI looks again.
const SETTLE_POLL: Duration = Duration::from_millis(50);
/// A taken spare's CLI whose `CreateAgent` has not come this long after the
/// take is killed: the client that took it went away.
const HELD_LIMIT: Duration = Duration::from_secs(2 * 60);
/// How long after a take the next spare is cut, so its fetch and setup do
/// not crowd the launch that just started.
const RECUT_DELAY: Duration = Duration::from_secs(2);

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
    /// The CLIs of spares a launch took, by the worktree they are in, until
    /// the launch's `CreateAgent` adopts them.
    held: HashMap<WorktreeId, Held>,
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

/// A taken spare's CLI, waiting for its launch's `CreateAgent`.
struct Held {
    cli: SpareCli,
    launch: LaunchSpec,
    taken_at: Instant,
}

/// The CLI a `CreateAgent` adopts ([`Daemon::take_held_cli`]): the agent id
/// its row takes — the one in the CLI's own env — and its boot, which its
/// first turn waits on.
pub(crate) struct Adopted {
    pub(crate) agent: AgentId,
    pub(crate) boot: Arc<Boot>,
}

/// One spare CLI's boot: when it was spawned and when it said it was up.
pub(crate) struct Boot {
    spawned_at: Instant,
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
    fn hook_at(&self) -> Option<Instant> {
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
    /// CLI `id` has had its first turn, or is gone: nothing waits on its
    /// boot.
    fn forget_boot(&self, id: &AgentId) {
        self.book.lock().unwrap().boots.remove(id);
    }

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
    /// `CreateWorktree` for a launch: project `project`'s spare when the
    /// launch can take it, else a worktree cut as any other is.
    pub async fn create_launch_worktree(
        self: &Arc<Self>,
        project: &ProjectId,
        branch: &str,
        launch: &LaunchSpec,
    ) -> Result<EntityId> {
        self.create_launch_worktree_with(project, branch, launch, &Config::load())
            .await
    }

    /// [`Daemon::create_launch_worktree`] against a given config.
    pub(crate) async fn create_launch_worktree_with(
        self: &Arc<Self>,
        project: &ProjectId,
        branch: &str,
        launch: &LaunchSpec,
        config: &Config,
    ) -> Result<EntityId> {
        if let Some(worktree) = self.take_spare(project, branch, launch, config).await? {
            return Ok(EntityId::Worktree(worktree));
        }
        self.create_worktree_with(project, branch, None, false, config)
            .await
    }

    /// Hand project `project`'s spare to a launch as worktree `branch`:
    /// None — and the spare left as it was — when there is none ready, the
    /// launch is not the one its CLI booted as, the project's base has
    /// changed since it was cut, or `branch` is taken already. Holds
    /// `worktree_ops` throughout, as every create does.
    pub(crate) async fn take_spare(
        self: &Arc<Self>,
        project: &ProjectId,
        branch: &str,
        launch: &LaunchSpec,
        config: &Config,
    ) -> Result<Option<WorktreeId>> {
        let branch = branch.trim();
        if branch.is_empty() || launch.mode != AgentMode::Edit {
            return Ok(None);
        }
        let _ops = self.worktree_ops.lock().await;
        // Claimed under the book's lock: no tick touches it meanwhile, and
        // its setup's end is still recorded on it.
        let (repo, from, base) = {
            let mut book = self.spares.book.lock().unwrap();
            let Some(spare) = book.spares.get(project) else {
                return Ok(None);
            };
            if book.busy.contains(project)
                || &spare.launch != launch
                || matches!(spare.setup, Setup::Failed)
                || !spare.worktree.path.exists()
            {
                return Ok(None);
            }
            let claim = (
                spare.repo.clone(),
                spare.worktree.branch.clone(),
                spare.base.name.clone(),
            );
            book.busy.insert(project.clone());
            claim
        };
        let renamed = async {
            let now = git::new_worktree_base(&repo, config.base_branch(&repo), false).await;
            if now != base || git::branch_taken(&repo, branch).await {
                return Ok(false);
            }
            git::rename_branch(&repo, &from, branch)
                .await
                .map(|()| true)
        }
        .await;
        let mut book = self.spares.book.lock().unwrap();
        book.busy.remove(project);
        match renamed {
            Ok(true) => {}
            Ok(false) => return Ok(None),
            Err(e) => {
                tracing::warn!(error = %e, "couldn't take the spare worktree; cutting one");
                return Ok(None);
            }
        }
        let spare = book.spares.remove(project).expect("claimed spares stay");
        let worktree = Worktree {
            branch: branch.to_string(),
            ..spare.worktree.clone()
        };
        if let Err(e) = self.store.insert_worktree(&worktree) {
            // Renamed already: the sync adopts the checkout as any other.
            self.kill_spare_sessions(&spare);
            return Err(e);
        }
        self.send_taken_rows(&mut book, &worktree, spare.setup, spare.cli, spare.launch);
        drop(book);
        tracing::info!(worktree = %worktree.path.display(), branch, "a launch took the spare worktree");
        self.spares.note_used(project);
        let daemon = self.clone();
        let project = project.clone();
        let config = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(RECUT_DELAY).await;
            let Ok(Some(project)) = daemon.store.get_project(&project) else {
                return;
            };
            daemon.cut_spare(&project, &config).await;
        });
        Ok(Some(worktree.id))
    }

    /// The rows a take sends, under the book's lock — the order a setup's
    /// exit hears it in ([`SpareBook::setup_ended`]): the worktree, then a
    /// setup still running as its setup row. Its CLI waits for the
    /// `CreateAgent`.
    fn send_taken_rows(
        &self,
        book: &mut Book,
        worktree: &Worktree,
        setup: Setup,
        cli: Option<SpareCli>,
        launch: LaunchSpec,
    ) {
        self.broadcast(ServerEvent::EntityUpserted {
            entity: Entity::Worktree(worktree.clone()),
        });
        if let Setup::Running(mut term) = setup {
            match self.store.insert_terminal(&term) {
                Ok(()) => {
                    term.alive = self.is_alive(&SessionRef::Terminal(term.id.clone()));
                    self.broadcast(ServerEvent::EntityUpserted {
                        entity: Entity::Terminal(term),
                    });
                }
                Err(e) => tracing::warn!(error = %e, "taken spare's setup row not stored"),
            }
        }
        if let Some(cli) = cli {
            book.held.insert(
                worktree.id.clone(),
                Held {
                    cli,
                    launch,
                    taken_at: Instant::now(),
                },
            );
        }
    }

    /// The CLI a taken spare holds in `worktree`, for a `CreateAgent` there
    /// launching as `launch`. One booted as something else, or dead, is
    /// killed, and the create boots cold.
    pub(crate) fn take_held_cli(
        &self,
        worktree: &WorktreeId,
        launch: &LaunchSpec,
    ) -> Option<Adopted> {
        let held = self.spares.book.lock().unwrap().held.remove(worktree)?;
        let sref = SessionRef::Agent(held.cli.agent.id.clone());
        if &held.launch != launch || !self.is_alive(&sref) {
            self.spares.forget_boot(&held.cli.agent.id);
            self.kill_session(&sref);
            return None;
        }
        Some(Adopted {
            agent: held.cli.agent.id,
            boot: held.cli.boot,
        })
    }

    /// Type `task` into adopted CLI `id` as its first turn once it is at its
    /// prompt. One that never gets there is put down, and booted again with
    /// `task` as its first prompt, as a cold launch would be; one that died
    /// is booted that way too ([`Daemon::send_turn`]). No task: the CLI's
    /// own input is the first prompt, and nothing waits on its boot.
    pub(crate) fn send_first_turn(
        self: &Arc<Self>,
        id: AgentId,
        boot: Arc<Boot>,
        task: Option<String>,
    ) {
        let Some(text) = task else {
            self.spares.forget_boot(&id);
            return;
        };
        let daemon = self.clone();
        tokio::spawn(async move {
            if !daemon.settled(&id, &boot).await {
                tracing::info!(agent = %id, "the spare's CLI never reached its prompt; booting it on the task");
                daemon.kill_session(&SessionRef::Agent(id.clone()));
            }
            daemon.spares.forget_boot(&id);
            let sent = tokio::task::spawn_blocking(move || {
                daemon.send_turn(&id, &text, DEFAULT_COLS, DEFAULT_ROWS)
            })
            .await;
            match sent {
                Ok(Err(e)) => tracing::warn!(error = %e, "the first turn didn't go in"),
                Err(e) => tracing::warn!(error = %e, "the first turn panicked"),
                Ok(Ok(())) => {}
            }
        });
    }

    /// Wait for CLI `id` to be at its prompt: its `SessionStart` in and its
    /// screen still for [`SETTLE`]. True once it is — or once it is gone,
    /// which the turn's own boot answers; false when it never says it has
    /// booted inside [`BOOT_LIMIT`].
    async fn settled(&self, id: &AgentId, boot: &Boot) -> bool {
        let sref = SessionRef::Agent(id.clone());
        let mut seen = None;
        let mut still_since = Instant::now();
        loop {
            let Some(session) = self.session(&sref) else {
                return true;
            };
            let end = session.ring.lock().unwrap().end_seq();
            if seen != Some(end) {
                seen = Some(end);
                still_since = Instant::now();
            }
            match boot.hook_at() {
                Some(at) if still_since.elapsed() >= SETTLE || at.elapsed() >= SETTLE_CAP => {
                    return true
                }
                None if boot.spawned_at.elapsed() >= BOOT_LIMIT => return false,
                _ => tokio::time::sleep(SETTLE_POLL).await,
            }
        }
    }

    /// Kill the held CLIs of taken spares in these worktrees (a delete, a
    /// project's removal), and every one whose `CreateAgent` never came.
    pub(crate) fn drop_held_in(&self, worktrees: &[WorktreeId]) {
        let doomed: Vec<AgentId> = {
            let mut book = self.spares.book.lock().unwrap();
            let gone: Vec<WorktreeId> = book
                .held
                .iter()
                .filter(|(w, h)| worktrees.contains(w) || h.taken_at.elapsed() > HELD_LIMIT)
                .map(|(w, _)| w.clone())
                .collect();
            gone.iter()
                .filter_map(|w| book.held.remove(w))
                .map(|h| h.cli.agent.id)
                .collect()
        };
        for id in doomed {
            self.spares.forget_boot(&id);
            self.kill_session(&SessionRef::Agent(id));
        }
    }

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
        self.drop_held_in(&[]);
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
    use crate::registry::tests::{git_in, init_repo, project_at, test_daemon, EnvGuard};
    use crate::registry::CreateAgentSpec;
    use crate::status::HookEvent;
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
        let listed = git::list_worktrees(&repo).await.unwrap();
        assert_eq!(
            listed.len(),
            1,
            "orion's listing is the main checkout alone: {listed:?}"
        );
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

    /// A project in use with its spare cut and `/bin/cat` for its CLI.
    async fn with_spare(
        rows: serde_json::Value,
    ) -> (Arc<Daemon>, tempfile::TempDir, Project, Config, EnvGuard) {
        let daemon = test_daemon();
        let tmp = tempfile::tempdir().unwrap();
        let repo = init_repo(tmp.path());
        let project = project_at(&daemon, &repo);
        let cmd = EnvGuard::set(env::AGENT_CMD, "/bin/cat");
        let cfg = spare_config(&repo, rows);
        daemon.spares.note_used(&project.id);
        daemon.tend_spares_with(&cfg).await;
        assert!(daemon.spare_of(&project.id).is_some(), "a spare to take");
        (daemon, tmp, project, cfg, cmd)
    }

    fn create_spec(
        worktree: &WorktreeId,
        launch: &LaunchSpec,
        task: Option<&str>,
    ) -> CreateAgentSpec {
        CreateAgentSpec {
            worktree: worktree.clone(),
            name: String::new(),
            kind: launch.kind,
            custom_harness: launch.custom_harness.clone(),
            model: launch.model.clone(),
            effort: launch.effort.clone(),
            auto_title: true,
            starting_prompt: task.map(String::from),
            pr_url: None,
            mode: launch.mode,
        }
    }

    fn created_worktree(id: EntityId) -> WorktreeId {
        match id {
            EntityId::Worktree(id) => id,
            other => panic!("a worktree: {other:?}"),
        }
    }

    /// A launch the spare's CLI booted as takes the lot: the branch is
    /// renamed, the row goes out on the spare's directory, the create
    /// that follows adopts the booted CLI, and the task is typed into it
    /// once it says it is up.
    #[tokio::test]
    async fn a_launch_like_the_spares_takes_it_and_its_booted_cli() {
        let (daemon, tmp, project, cfg, _cmd) = with_spare(serde_json::json!({})).await;
        let (spare, cli) = daemon.spare_of(&project.id).unwrap();
        let cli = cli.expect("a booted CLI");
        let launch = cfg.spare_launch().unwrap();
        let mut events = daemon.events.subscribe();

        let id = created_worktree(
            daemon
                .create_launch_worktree_with(&project.id, "feat", &launch, &cfg)
                .await
                .unwrap(),
        );
        assert_eq!(
            id, spare.id,
            "the row takes the id its CLI was spawned under"
        );
        let row = daemon.store.get_worktree(&id).unwrap().expect("its row");
        assert_eq!(
            (row.path.clone(), row.branch.as_str()),
            (spare.path.clone(), "feat")
        );
        assert!(matches!(
            events.try_recv(),
            Ok(ServerEvent::EntityUpserted { entity: Entity::Worktree(w) }) if w.id == id
        ));
        let listed = git::list_worktrees(&tmp.path().join("repo")).await.unwrap();
        assert_eq!(
            listed.iter().map(|w| w.branch.as_str()).collect::<Vec<_>>(),
            ["main", "feat"],
            "renamed in git, and so in the sync's eyes a worktree like any"
        );
        assert!(!branches(&project.repo_path).contains(&spare.branch));
        assert!(daemon.spare_of(&project.id).is_none(), "taken");

        let agent = daemon
            .create_agent(create_spec(&id, &launch, Some("the task")))
            .await
            .unwrap();
        assert_eq!(
            agent,
            EntityId::Agent(cli.clone()),
            "the booted CLI, adopted"
        );
        let row = daemon.store.get_agent(&cli).unwrap().expect("its row");
        assert_eq!(row.status, AgentStatus::Running, "working from the create");
        daemon.apply_hook_event(&cli, HookEvent::SessionStart { source: None }, None);
        let session = daemon
            .session(&SessionRef::Agent(cli))
            .expect("still the one CLI");
        until("the first turn", || {
            String::from_utf8_lossy(&session.snapshot(None).1)
                .matches("the task")
                .count()
                >= 2
        })
        .await;
        daemon.kill_all();
    }

    /// A launch on another model, a branch name already taken, or one in
    /// plan mode is cut cold, and the spare stays for the next.
    #[tokio::test]
    async fn any_other_launch_is_cut_cold_and_the_spare_stays() {
        let (daemon, _tmp, project, cfg, _cmd) = with_spare(serde_json::json!({})).await;
        let (spare, _) = daemon.spare_of(&project.id).unwrap();
        let launch = cfg.spare_launch().unwrap();
        let opus = LaunchSpec {
            model: Some("opus".into()),
            ..launch.clone()
        };
        let plan = LaunchSpec {
            mode: AgentMode::Plan,
            ..launch.clone()
        };
        git_in(&project.repo_path, &["branch", "taken"]);
        for (branch, launch) in [("on-opus", &opus), ("in-plan", &plan), ("taken", &launch)] {
            let id = created_worktree(
                daemon
                    .create_launch_worktree_with(&project.id, branch, launch, &cfg)
                    .await
                    .unwrap(),
            );
            let row = daemon.store.get_worktree(&id).unwrap().unwrap();
            assert_eq!(
                row.path,
                git::worktree_dir(&project.repo_path, branch),
                "{branch}: cold"
            );
            assert_eq!(daemon.spare_of(&project.id).unwrap().0.path, spare.path);
        }
        daemon.kill_all();
    }

    /// A spare taken while its setup still runs: the setup shows as the
    /// worktree's setup row, which goes when it ends cleanly.
    #[tokio::test]
    async fn a_setup_still_running_comes_with_the_take() {
        let (daemon, _tmp, project, cfg, _cmd) =
            with_spare(serde_json::json!({ "setup_command": "sleep 1" })).await;
        let launch = cfg.spare_launch().unwrap();
        let id = created_worktree(
            daemon
                .create_launch_worktree_with(&project.id, "feat", &launch, &cfg)
                .await
                .unwrap(),
        );
        let setups = daemon.store.setup_terminals_in(&id).unwrap();
        assert_eq!(setups.len(), 1, "the setup row is the worktree's now");
        let setup = SessionRef::Terminal(setups[0].id.clone());
        assert!(daemon.is_alive(&setup), "still running");
        until("the setup's row going", || {
            daemon.store.setup_terminals_in(&id).unwrap().is_empty()
        })
        .await;
        daemon.kill_all();
    }

    /// A taken spare's CLI that no create comes for goes with its worktree.
    #[tokio::test]
    async fn a_held_cli_goes_with_its_worktree() {
        let (daemon, _tmp, project, cfg, _cmd) = with_spare(serde_json::json!({})).await;
        let (_, cli) = daemon.spare_of(&project.id).unwrap();
        let cli = SessionRef::Agent(cli.unwrap());
        let launch = cfg.spare_launch().unwrap();
        let id = created_worktree(
            daemon
                .create_launch_worktree_with(&project.id, "feat", &launch, &cfg)
                .await
                .unwrap(),
        );
        assert!(daemon.is_alive(&cli), "held for the create");
        daemon.delete_worktree(&id, true).await.unwrap();
        assert!(!daemon.is_alive(&cli));
        daemon.kill_all();
    }
}
