//! The BLUE SKY BUILDER: an idea from Todos › Blue sky, briefed, approved
//! by you, then built as a prototype to try.
//!
//! * **Where.** Each idea has a target (`Item::target`, `w` on its row):
//!   a project, built in a new worktree on `proto/<slug>` and never sent
//!   as a pull request; or `blue sky`, built in the folder `<slug>/` of
//!   the Blue sky folder ([`folder`], `~/Code/orion-blue-sky`), which is
//!   made a git repository and added as a project the first time an idea
//!   goes there.
//! * **Enter** on an idea queues it (`Item::queued`, this run only) and
//!   the runner's tick starts what is queued as soon as it can: once the
//!   Blue sky folder is a project, and under the automations' cap. Its
//!   session is tagged `blue_sky`, starts in plan mode and is told the
//!   **Blue sky builder** preset, so its brief arrives as a plan to
//!   approve — on Needs you as `brief to approve`, and on the phone.
//! * **State.** An idea's row says how its build stands ([`Idea::of`]),
//!   read off its session: `brief to approve`, `building 18m`,
//!   `built · proto/onboard`, or `not started`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use orion_core::{Agent, AgentId, AgentMode, AgentStatus, ClientRequest, ProjectId, WaitReason};

use super::{Automation, Dot, Panel, PanelList, PanelRow, Runner, Tone};
use crate::app::App;
use crate::flash::Flash;
use crate::quick_prompt::{QuickLaunch, QuickTarget};
use crate::todos::{store, Item, TodoFile, TodoRef};

/// What the **Blue sky builder** preset says while it holds none of the
/// user's.
pub const PRESET_TEXT: &str = "You are orion's blue sky builder. Build a working prototype of the idea below.

First write a short brief: who it is for, what it is, what you will build it from, what they will get, and how big it is. Present the brief as your plan and wait for approval before you build anything.

Once it is approved, build a working prototype. If it is a web tool and you can publish it as a claude.ai artifact, do, and note the link. Finish by saying how to try it.

{task}";

/// The target an idea tied to no project has, as its row spells it.
pub const BLUE_SKY: &str = "blue sky";

/// How long an add of the Blue sky folder is left to land before it is
/// asked again.
const ASK_AGAIN: Duration = Duration::from_secs(30);

/// Where ideas tied to no project are built: `ORION_BLUE_SKY_DIR`, else
/// `~/Code/orion-blue-sky`. None in the unit tests unless one names a
/// folder ([`with_folder`]): no test may make the real one.
pub fn folder() -> Option<PathBuf> {
    #[cfg(test)]
    let folder = FOLDER.with(|f| f.borrow().clone());
    #[cfg(not(test))]
    let folder = orion_core::env::non_empty(orion_core::env::BLUE_SKY_DIR)
        .map(PathBuf::from)
        .or_else(|| Some(orion_core::env::home_dir()?.join("Code/orion-blue-sky")));
    folder
}

#[cfg(test)]
thread_local! {
    static FOLDER: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

/// Run `f` with the Blue sky folder at `dir`.
#[cfg(test)]
pub fn with_folder<T>(dir: PathBuf, f: impl FnOnce() -> T) -> T {
    FOLDER.with(|slot| {
        let prev = slot.replace(Some(dir));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// `path` as the filesystem names it, so `/tmp` and `/private/tmp` are
/// one place.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The project the Blue sky folder is, once it was added.
pub fn folder_project(app: &App) -> Option<&orion_core::Project> {
    let folder = canonical(&folder()?);
    app.tree
        .projects
        .iter()
        .find(|p| canonical(&p.repo_path) == folder)
}

/// The Blue sky list.
fn ideas(app: &App) -> Option<&TodoFile> {
    app.todos.get(&store::blue_sky_key())
}

/// The projects an idea can be built in, in the tabs' order: every one
/// but the Blue sky folder's own, which is `blue sky`.
fn projects(app: &App) -> Vec<(ProjectId, String)> {
    let own = folder_project(app).map(|p| p.id.clone());
    crate::todos::view::tabs(app)
        .into_iter()
        .filter_map(|(tab, name, _)| Some((tab.project()?.clone(), name)))
        .filter(|(id, _)| Some(id) != own.as_ref())
        .collect()
}

/// `w`: the target after `current` — `blue sky`, then each project — round
/// the end. One whose project has gone counts as `blue sky`.
pub fn next_target(app: &App, current: Option<&str>) -> Option<String> {
    let mut cycle: Vec<Option<String>> = vec![None];
    cycle.extend(projects(app).into_iter().map(|(id, _)| Some(id.0)));
    let at = cycle
        .iter()
        .position(|t| t.as_deref() == current)
        .unwrap_or(0);
    cycle[(at + 1) % cycle.len()].clone()
}

/// A target as its row spells it: the project's name, or `blue sky`.
pub fn target_label(app: &App, target: Option<&str>) -> String {
    target
        .and_then(|id| app.tree.projects.iter().find(|p| p.id.0 == id))
        .map_or_else(|| BLUE_SKY.to_string(), |p| p.name.clone())
}

/// The project an idea is built in, when it is one that is still there.
fn target_project<'a>(app: &'a App, item: &Item) -> Option<&'a orion_core::Project> {
    let id = item.target.as_deref()?;
    app.tree.projects.iter().find(|p| p.id.0 == id)
}

/// The session building `item`, while it is there.
fn session<'a>(app: &'a App, item: &Item) -> Option<&'a Agent> {
    let id = item.agent.as_deref()?;
    app.tree.agents.iter().find(|a| a.id.0 == id)
}

/// How an idea's build stands, in a word or two and its colour.
fn stage(app: &App, item: &Item) -> (String, Tone) {
    if item.queued {
        return ("starting".into(), Tone::Working);
    }
    let Some(built_in) = &item.built_in else {
        return ("not started".into(), Tone::Quiet);
    };
    let built = (format!("built · {built_in}"), Tone::Done);
    let Some(agent) = session(app, item) else {
        return built;
    };
    match agent.status {
        AgentStatus::NeedsFeedback => match app.needs_you.reasons.get(&agent.id) {
            Some(WaitReason::Plan { .. }) | None => ("brief to approve".into(), Tone::Waiting),
            Some(_) => ("waiting for you".into(), Tone::Waiting),
        },
        AgentStatus::Terminated => ("crashed".into(), Tone::Waiting),
        AgentStatus::Running => {
            let mins = (crate::app::now_ms() - agent.status_changed_at).max(0) / 60_000;
            (format!("building {mins}m"), Tone::Working)
        }
        AgentStatus::Fresh if agent.alive => ("starting".into(), Tone::Working),
        _ => built,
    }
}

/// An idea's row on the Blue sky tab, past its text: where it goes and
/// how its build stands.
pub struct Idea {
    pub target_label: String,
    /// `● brief to approve`, `● building 18m`, `● built · proto/onboard`,
    /// `not started`.
    pub state: String,
    pub tone: Tone,
}

impl Idea {
    pub fn of(app: &App, item: &Item) -> Self {
        let (state, tone) = stage(app, item);
        let state = match tone {
            Tone::Quiet => state,
            _ => format!("● {state}"),
        };
        Self {
            target_label: target_label(app, item.target.as_deref()),
            state,
            tone,
        }
    }
}

/// `Enter` on ideas in Todos › Blue sky: each open one with no session at
/// work on it is queued, and started at once where it can be.
pub fn start(app: &mut App, ids: &[u64], out: &mut Vec<ClientRequest>) {
    if !app.automations.is_on(Automation::BlueSky) {
        app.flash = Some(Flash::note(
            "the Blue sky builder is off — turn it on in Automations (⌘J)",
        ));
        return;
    }
    let live: Vec<u64> = ideas(app)
        .map(|file| {
            ids.iter()
                .filter_map(|id| file.item(*id))
                .filter(|i| i.done.is_none() && session(app, i).is_none_or(|a| !super::occupies(a)))
                .map(|i| i.id)
                .collect()
        })
        .unwrap_or_default();
    let Some(file) = app.todos.get_mut(&store::blue_sky_key()) else {
        return;
    };
    for id in &live {
        if let Some(item) = file.item_mut(*id) {
            item.queued = true;
        }
    }
    if live.is_empty() {
        return;
    }
    app.dirty = true;
    super::with_runner(app, Automation::BlueSky, |runner, app| {
        runner.tick(app, out)
    });
    // What started said so as it went ([`send`]); what still waits says why.
    if !ideas(app).is_some_and(|f| f.items.iter().any(|i| i.queued)) {
        return;
    }
    app.flash = Some(if super::may_start(app) {
        Flash::working(format!(
            "making {} a project, then the Blue sky builder starts",
            folder().map_or_else(|| BLUE_SKY.to_string(), |f| f.display().to_string())
        ))
    } else {
        Flash::note(format!(
            "{} automation agents running, at most {}: it starts when one finishes",
            super::running(app),
            app.automations.settings.cap()
        ))
    });
}

/// `o` on an idea: what it was built in — its folder in the Blue sky
/// folder, or its `proto/` checkout — opened as macOS opens a folder.
pub fn open_built(app: &mut App, item: &Item) {
    let Some(built_in) = &item.built_in else {
        app.flash = Some(Flash::note("not built yet — Enter builds it"));
        return;
    };
    let place = match target_project(app, item) {
        Some(project) => app
            .tree
            .worktrees
            .iter()
            .find(|w| w.project_id == project.id && w.branch == *built_in)
            .map(|w| w.path.clone()),
        None if item.target.is_some() => None,
        None => folder().map(|f| f.join(built_in.trim_end_matches('/'))),
    };
    match place.filter(|p| p.exists()) {
        Some(path) => {
            crate::open_url::open_url(&path.to_string_lossy(), None);
        }
        None => app.flash = Some(Flash::failed(format!("{built_in} isn't there any more"))),
    }
}

/// The Automations screen's IDEAS row for `agent`: the Todos modal on
/// Blue sky's tab, on the idea it builds.
pub fn open_idea(app: &mut App, agent: &AgentId) {
    // An idea no longer found opens the tab at its top.
    let item = ideas(app)
        .and_then(|f| {
            f.items
                .iter()
                .find(|i| i.agent.as_deref() == Some(agent.0.as_str()))
        })
        .map_or(0, |i| i.id);
    crate::todos::view::open_at(app, &crate::todos::view::Tab::BlueSky, item);
}

/// The runner: what was queued, started.
#[derive(Default)]
pub struct BlueSkyBuilder {
    /// When the Blue sky folder was last asked to be added as a project.
    asked: Option<Instant>,
}

impl Runner for BlueSkyBuilder {
    fn automation(&self) -> Automation {
        Automation::BlueSky
    }

    fn tick(&mut self, app: &mut App, out: &mut Vec<ClientRequest>) {
        let queued: Vec<Item> = ideas(app)
            .map(|f| f.items.iter().filter(|i| i.queued).cloned().collect())
            .unwrap_or_default();
        for item in queued {
            if !super::may_start(app) {
                return;
            }
            if item.target.is_some() {
                match target_project(app, &item).map(|p| p.id.clone()) {
                    Some(project) => start_in_project(app, &item, project, out),
                    None => {
                        unqueue(app, item.id, None);
                        app.flash = Some(Flash::failed(format!(
                            "{}: its project is gone — w picks another",
                            item.text
                        )));
                    }
                }
                continue;
            }
            let Some(folder) = folder() else {
                unqueue(app, item.id, None);
                app.flash = Some(Flash::failed("no home folder to build blue sky ideas in"));
                continue;
            };
            let root = folder_project(app).and_then(|p| app.root_worktree(&p.id));
            match root {
                Some(root) => start_in_folder(app, &item, &folder, root, out),
                None => self.add_folder(app, &folder, out),
            }
        }
    }

    fn panel(&self, app: &App) -> Panel {
        let rows: Vec<PanelRow> = ideas(app)
            .map(|f| {
                f.items
                    .iter()
                    .filter(|i| i.done.is_none() && (i.queued || i.built_in.is_some()))
                    .map(|i| {
                        let (state, tone) = stage(app, i);
                        PanelRow {
                            text: i.text.clone(),
                            state,
                            tone,
                            session: session(app, i).map(|a| a.id.clone()),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let count = |tone: Tone| rows.iter().filter(|r| r.tone == tone).count();
        let (dot, state) = match (count(Tone::Waiting), count(Tone::Working)) {
            (1, _) => (Dot::Waiting, "1 brief waiting".to_string()),
            (n @ 2.., _) => (Dot::Waiting, format!("{n} briefs waiting")),
            (_, n @ 1..) => (Dot::Working, format!("building {n}")),
            _ => (Dot::Idle, "idle".to_string()),
        };
        Panel {
            dot,
            state,
            lists: vec![PanelList {
                title: "IDEAS",
                empty: "Add ideas in Todos › Blue sky (⌘I).",
                rows,
            }],
        }
    }
}

impl BlueSkyBuilder {
    /// The Blue sky folder made a git repository and added as a project
    /// — asked once, then again only if it has not landed in a while.
    fn add_folder(&mut self, app: &mut App, folder: &Path, out: &mut Vec<ClientRequest>) {
        if self.asked.is_some_and(|at| at.elapsed() < ASK_AGAIN) {
            return;
        }
        self.asked = Some(Instant::now());
        if let Err(err) = std::fs::create_dir_all(folder) {
            app.flash = Some(Flash::failed(format!(
                "couldn't make {}: {err}",
                folder.display()
            )));
            return;
        }
        let path = canonical(folder);
        crate::event_loop::send(app, out, |req_id| ClientRequest::AddProject {
            req_id,
            path,
            name: None,
            create_missing: true,
        });
    }
}

/// `id` no longer queued — started, with where it is built, or given up.
fn unqueue(app: &mut App, id: u64, built_in: Option<String>) {
    let Some(file) = app.todos.get_mut(&store::blue_sky_key()) else {
        return;
    };
    if let Some(item) = file.item_mut(id) {
        item.queued = false;
        if built_in.is_some() {
            item.built_in = built_in;
            item.agent = None;
            item.worktree = None;
        }
        store::save(file);
    }
    app.dirty = true;
}

/// An idea for a project: a new worktree on `proto/<slug>`.
fn start_in_project(app: &mut App, item: &Item, project: ProjectId, out: &mut Vec<ClientRequest>) {
    let slug = slug(&item.text);
    let branch =
        crate::branch_name::unused(&format!("proto/{slug}"), &app.project_branches(&project));
    let place = format!(
        "Build it in this checkout, on its branch {branch}. It is a prototype to try: never open a pull request for it."
    );
    let target = QuickTarget::NewWorktree {
        project,
        branch: branch.clone(),
        existing: false,
    };
    send(app, item, target, &place, branch, out);
}

/// An idea tied to no project: the folder `<slug>/` in the Blue sky
/// folder, its session in the folder's own checkout.
fn start_in_folder(
    app: &mut App,
    item: &Item,
    folder: &Path,
    root: orion_core::WorktreeId,
    out: &mut Vec<ClientRequest>,
) {
    let taken: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let name = crate::branch_name::unused(&slug(&item.text), &taken);
    let _ = std::fs::create_dir_all(folder.join(&name));
    let place = format!(
        "It isn't tied to any project. Build it in the folder {name}/ of this repository ({}), and keep everything you make inside that folder.",
        folder.display()
    );
    send(
        app,
        item,
        QuickTarget::Worktree(root),
        &place,
        format!("{name}/"),
        out,
    );
}

/// `text` as a branch or folder name: its words, lowercased and hyphened.
fn slug(text: &str) -> String {
    match crate::branch_name::title_slug(text) {
        slug if slug.is_empty() => "idea".into(),
        slug => slug,
    }
}

/// The session for `item`: in plan mode, on the Blue sky builder's
/// preset — what it runs on, and its text with the idea and `place` (where
/// to build it) at `{task}` and the extra steer after — tagged `blue_sky`
/// and written onto the idea.
fn send(
    app: &mut App,
    item: &Item,
    target: QuickTarget,
    place: &str,
    built_in: String,
    out: &mut Vec<ClientRequest>,
) {
    let cfg = crate::config::Config::load();
    let preset = super::preset_of(&cfg.automations, Automation::BlueSky);
    let context = format!("The idea: {}\n\n{place}", item.text.trim());
    let prompt = super::compose(
        &preset,
        &context,
        &cfg.automations.of(Automation::BlueSky).extra_steer,
    );
    let mut launch = QuickLaunch::of_preset(target, preset, None, &cfg)
        .with_mode(AgentMode::Plan, &cfg)
        .with_todo(Some(TodoRef {
            repo_path: store::blue_sky_key(),
            item: item.id,
            also: Vec::new(),
        }));
    // The prompt is composed already.
    launch.preset = None;
    // At the cap it stays queued, for a later tick.
    if !super::launch_in_new_worktree(app, Automation::BlueSky, launch, prompt, out) {
        return;
    }
    unqueue(app, item.id, Some(built_in));
    app.flash = Some(Flash::working(format!(
        "Blue sky builder started on {}: its brief comes to Needs you",
        item.text.trim()
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use orion_core::{AgentKind, Project, Worktree, WorktreeId};

    fn project(id: &str, name: &str, path: &str) -> Project {
        Project {
            id: ProjectId(id.into()),
            name: name.into(),
            repo_path: path.into(),
            sort_order: 0,
        }
    }

    fn idea(app: &mut App, text: &str) -> u64 {
        let key = store::blue_sky_key();
        let file = app
            .todos
            .entry(key.clone())
            .or_insert_with(|| TodoFile::new(&key));
        let group = file.inbox();
        file.add_item(group, text, crate::todos::today())
    }

    fn item(app: &App, id: u64) -> Item {
        ideas(app).unwrap().item(id).unwrap().clone()
    }

    /// `w` goes round `blue sky` and every project in the tabs' order —
    /// never the Blue sky folder's own project — and a target whose
    /// project has gone starts again from `blue sky`.
    #[test]
    fn the_target_cycles_through_blue_sky_and_the_projects() {
        let dir = tempfile::tempdir().unwrap();
        with_folder(dir.path().to_path_buf(), || {
            let mut app = App::new();
            app.tree.projects = vec![
                project("p1", "web", "/tmp/web"),
                project("p2", "api", "/tmp/api"),
                project("p3", "orion-blue-sky", &dir.path().to_string_lossy()),
            ];
            let next = |t: Option<&str>| next_target(&app, t);
            assert_eq!(next(None).as_deref(), Some("p1"));
            assert_eq!(next(Some("p1")).as_deref(), Some("p2"));
            assert_eq!(next(Some("p2")), None, "round to blue sky");
            assert_eq!(next(Some("gone")).as_deref(), Some("p1"));
            assert_eq!(target_label(&app, None), "blue sky");
            assert_eq!(target_label(&app, Some("p2")), "api");
            assert_eq!(target_label(&app, Some("gone")), "blue sky");
        });
    }

    /// An idea's state reads off its session: not started, a brief to
    /// approve, building, built.
    #[test]
    fn an_ideas_state_follows_its_session() {
        let mut app = App::new();
        let id = idea(&mut app, "onboarding tour");
        assert_eq!(Idea::of(&app, &item(&app, id)).state, "not started");

        let file = app.todos.get_mut(&store::blue_sky_key()).unwrap();
        let it = file.item_mut(id).unwrap();
        it.built_in = Some("proto/onboarding-tour".into());
        it.agent = Some("a1".into());
        app.tree.agents.push(Agent {
            id: AgentId("a1".into()),
            worktree_id: WorktreeId("w".into()),
            name: "a1".into(),
            status: AgentStatus::NeedsFeedback,
            archived: false,
            archived_at: 0,
            unseen: false,
            status_changed_at: crate::app::now_ms() - 18 * 60_000,
            kind: AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: Some("blue_sky".into()),
        });
        let state = |app: &App| Idea::of(app, &item(app, id));
        assert_eq!(state(&app).state, "● brief to approve");
        assert_eq!(state(&app).tone, Tone::Waiting);
        app.tree.agents[0].status = AgentStatus::Running;
        assert_eq!(state(&app).state, "● building 18m");
        app.tree.agents[0].status = AgentStatus::Finished;
        assert_eq!(state(&app).state, "● built · proto/onboarding-tour");
        assert_eq!(state(&app).tone, Tone::Done);
        app.tree.agents.clear();
        assert_eq!(
            state(&app).state,
            "● built · proto/onboarding-tour",
            "session gone"
        );
    }

    /// Enter on an idea for `blue sky` the first time: the folder is made
    /// and asked to be added as a project — once — and once it is one,
    /// the session starts in its checkout, in plan mode, tagged, told the
    /// preset, in a folder of its own.
    #[test]
    fn a_blue_sky_idea_adds_the_folder_then_starts_there() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("orion-blue-sky");
        with_folder(folder.clone(), || {
            let mut app = App::new();
            app.automations.settings.of_mut(Automation::BlueSky).on = Some(true);
            let id = idea(&mut app, "A tiny habit tracker");
            let mut out = Vec::new();
            start(&mut app, &[id], &mut out);
            assert!(folder.is_dir(), "the folder is made");
            let added: Vec<_> = out
                .iter()
                .filter_map(|r| match r {
                    ClientRequest::AddProject {
                        path,
                        create_missing,
                        ..
                    } => Some((path.clone(), *create_missing)),
                    _ => None,
                })
                .collect();
            assert_eq!(added, [(canonical(&folder), true)]);
            assert!(item(&app, id).queued);
            assert_eq!(Idea::of(&app, &item(&app, id)).state, "● starting");

            // A tick before the project lands asks nothing more.
            out.clear();
            super::super::tick(&mut app, &mut out);
            assert!(out.is_empty(), "{out:?}");

            // The project lands, with its checkout.
            app.tree
                .projects
                .push(project("pb", "orion-blue-sky", &folder.to_string_lossy()));
            app.tree.worktrees.push(Worktree {
                id: WorktreeId("wb".into()),
                project_id: ProjectId("pb".into()),
                path: folder.clone(),
                branch: "main".into(),
                is_main: true,
                sort_order: 0,
            });
            super::super::tick(&mut app, &mut out);
            let create = out
                .iter()
                .find_map(|r| match r {
                    ClientRequest::CreateAgent {
                        worktree,
                        starting_prompt,
                        mode,
                        automation,
                        ..
                    } => Some((
                        worktree.clone(),
                        starting_prompt.clone(),
                        *mode,
                        automation.clone(),
                    )),
                    _ => None,
                })
                .expect("a create");
            assert_eq!(create.0, WorktreeId("wb".into()));
            let prompt = create.1.unwrap();
            assert!(
                prompt.starts_with("You are orion's blue sky builder"),
                "{prompt}"
            );
            assert!(prompt.contains("folder a-tiny-habit-tracker/"), "{prompt}");
            assert_eq!(create.2, AgentMode::Plan);
            assert_eq!(create.3.as_deref(), Some("blue_sky"));
            assert!(folder.join("a-tiny-habit-tracker").is_dir());
            let it = item(&app, id);
            assert!(!it.queued);
            assert_eq!(it.built_in.as_deref(), Some("a-tiny-habit-tracker/"));
        });
    }

    /// An idea for a project is a new worktree on `proto/<slug>`, its
    /// prompt saying never to open a pull request.
    #[test]
    fn a_project_idea_cuts_a_proto_branch() {
        let mut app = App::new();
        app.automations.settings.of_mut(Automation::BlueSky).on = Some(true);
        app.tree.projects.push(project("p1", "web", "/tmp/web"));
        let id = idea(&mut app, "Onboard tour");
        app.todos
            .get_mut(&store::blue_sky_key())
            .unwrap()
            .item_mut(id)
            .unwrap()
            .target = Some("p1".into());
        let mut out = Vec::new();
        start(&mut app, &[id], &mut out);
        let branch = out
            .iter()
            .find_map(|r| match r {
                ClientRequest::CreateWorktree { branch, .. } => Some(branch.clone()),
                _ => None,
            })
            .expect("a worktree cut first");
        assert_eq!(branch, "proto/onboard-tour");
        assert_eq!(
            item(&app, id).built_in.as_deref(),
            Some("proto/onboard-tour")
        );
    }

    /// Off, Enter starts nothing and says how to turn it on.
    #[test]
    fn off_it_starts_nothing() {
        let mut app = App::new();
        let id = idea(&mut app, "x");
        let mut out = Vec::new();
        start(&mut app, &[id], &mut out);
        assert!(out.is_empty());
        assert!(!item(&app, id).queued);
        assert!(app.flash.unwrap().text.contains("Automations"));
    }
}
