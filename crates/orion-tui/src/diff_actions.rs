//! What the DIFF VIEWER (`⌘E`) knows beyond the diff.
//!
//! **The top line** names the pull request the viewer is about, over its
//! panels: `#1342 Fix PDF header · ✗ 44/45 checks`. A checkout's viewer reads
//! its branch's pull request (`App::pull_requests`, what `gh pr view` found);
//! a pull request's viewer reads that one. Where it stands comes from the PR
//! STORE (`App::prs`) — the page's tally of its checks once the page has been
//! read (`App::pr_detail`), else the store's one word for them, and `merged`
//! or `closed` once it is past taking work. Last, the files that conflict
//! with its base: `3 files conflict with origin/dev`. A branch with no pull
//! request and no conflict has no line.
//!
//! **Conflicts are worked out locally** (`conflicts::local`): `git
//! merge-tree` of the base and HEAD — a pull request's viewer, its tip —
//! off the loop as the viewer opens, so it never waits on it, and kept by
//! the two commits' SHAs. No network: base sync keeps origin's copy of the
//! base fresh, so it works before a pull request exists. The base is the
//! pull request's once its page is read, else the one the COMMIT LIST
//! measures against. The files it names wear git's red `U` in the file
//! list, and `Enter` on one opens the CONFLICT RESOLVER on the checkout
//! (`conflicts::resolve_in`; a pull request's viewer goes through
//! `conflicts::open`) — or, with an `autofix-<n>` already on the pull
//! request, says so and leaves it to that agent. Esc in the resolver comes
//! back to the viewer, its conflicts asked again.

use crossterm::event::KeyEvent;
use orion_core::ClientRequest;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DiffFocus, DiffView, Overlay};
use crate::flash::Flash;
use crate::pull_request::{CheckCounts, Checks, Standing};
use crate::theme::Theme;

/// What a DIFF VIEWER knows of its branch's conflicts with its base.
#[derive(Debug, Clone, Default)]
pub struct Conflicts {
    /// The ask in flight, by ticket: an answer to any other is dropped.
    pub asking: Option<u64>,
    /// The last answer — kept while the next is asked, so the marks don't
    /// blink.
    pub known: Option<crate::conflicts::Local>,
}

/// The files `view`'s branch conflicts in, as last asked; none while
/// nothing is known.
pub(crate) fn conflicted(view: &DiffView) -> &[String] {
    view.conflicts
        .known
        .as_ref()
        .map_or(&[], |local| local.files.as_slice())
}

/// The conflicted file under the cursor, by path: what `Enter` resolves.
pub(crate) fn conflicted_file(view: &DiffView) -> Option<String> {
    let path = &view.selected_file()?.path;
    conflicted(view).contains(path).then(|| path.clone())
}

/// The pull request a DIFF VIEWER is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ViewedPr {
    pub number: u64,
    pub url: String,
    pub title: String,
}

impl ViewedPr {
    /// `#42 title`.
    fn label(&self) -> String {
        crate::pull_request::numbered_label(self.number, &self.title)
    }
}

/// The checkout `view` reads, when it reads one: None for a pull request's
/// viewer, which reads the project's own git (`event_loop::open_pr_review`).
pub(crate) fn checkout<'a>(app: &'a App, view: &DiffView) -> Option<&'a orion_core::Worktree> {
    if view.pr_review.is_some() {
        return None;
    }
    app.tree.worktrees.iter().find(|w| w.path == view.root)
}

/// The pull request `view` is about: the one it reads, or its checkout's
/// branch's — titled by the newest answer that named it.
pub(crate) fn pull_request(app: &App, view: &DiffView) -> Option<ViewedPr> {
    let (number, url, copy) = match &view.pr_review {
        Some(review) => (review.number, review.url.clone(), String::new()),
        None => {
            let pr = app.pull_requests.get(&checkout(app, view)?.id)?.as_ref()?;
            (pr.number, pr.url.clone(), pr.title.clone())
        }
    };
    let title = [
        app.prs.status(&url).map(|status| status.title),
        app.pr_detail.get(&url).map(|d| d.title.clone()),
    ]
    .into_iter()
    .flatten()
    .find(|title| !title.is_empty())
    .unwrap_or(copy);
    Some(ViewedPr { number, url, title })
}

/// The viewer's top line, `width` cells wide: the pull request's number and
/// title, the title cut to leave room for where it stands and what
/// conflicts. None with neither a pull request nor a conflict.
pub(crate) fn top_line(
    app: &App,
    view: &DiffView,
    width: usize,
    th: Theme,
) -> Option<Line<'static>> {
    let pr = pull_request(app, view);
    let facts: Vec<(String, Color)> = pr
        .iter()
        .filter_map(|pr| standing(app, &pr.url, th))
        .chain(conflict_count(view, th))
        .collect();
    if pr.is_none() && facts.is_empty() {
        return None;
    }
    let mut tail = Vec::new();
    for (text, color) in facts {
        if pr.is_some() || !tail.is_empty() {
            tail.push(Span::styled(" · ", Style::default().fg(th.faint)));
        }
        tail.push(Span::styled(text, Style::default().fg(color)));
    }
    let mut spans = vec![Span::raw(" ")];
    if let Some(pr) = pr {
        let number = format!("#{} ", pr.number);
        let tail_w: usize = tail.iter().map(|s| s.content.width()).sum();
        let room = width.saturating_sub(number.width() + tail_w + 2);
        spans.push(Span::styled(number, Style::default().fg(th.muted)));
        spans.push(Span::styled(
            crate::ui::truncate_cells(&pr.title, room),
            Style::default().fg(th.text).add_modifier(Modifier::BOLD),
        ));
    }
    spans.extend(tail);
    Some(Line::from(spans))
}

/// `3 files conflict with origin/dev`, while any does.
fn conflict_count(view: &DiffView, th: Theme) -> Option<(String, Color)> {
    let local = view.conflicts.known.as_ref()?;
    let n = local.files.len();
    let verb = match n {
        0 => return None,
        1 => "conflicts",
        _ => "conflict",
    };
    let files = crate::flash::plural(n, "file");
    Some((format!("{files} {verb} with {}", local.base), th.err))
}

/// Where the pull request at `url` stands, after its title: `merged` or
/// `closed` once it is past taking work, in the colours its page's border
/// gives them; while it is open, its checks — the page's tally
/// (`✗ 44/45 checks`), else the store's one word for them. None while
/// nothing is known of them, or there are none.
fn standing(app: &App, url: &str, th: Theme) -> Option<(String, Color)> {
    let status = app.prs.status_or_open(url);
    match status.standing {
        Standing::Merged => return Some((status.word().to_string(), th.merged)),
        Standing::Closed => return Some((status.word().to_string(), th.faint)),
        Standing::Open | Standing::Draft => {}
    }
    let counted = app.pr_detail.get(url).map(|d| CheckCounts::of(&d.checks));
    if let Some((tally, color)) = counted.and_then(|n| crate::pr_preview::check_tally(n, th)) {
        return Some((format!("{tally} checks"), color));
    }
    if !status.checks_known {
        return None;
    }
    match status.health.checks {
        Checks::Failing => Some(("✗ checks failing".into(), th.err)),
        Checks::Pending => Some(("◐ checks running".into(), th.warn)),
        Checks::Passing => Some(("✓ checks passed".into(), th.ok)),
        Checks::Absent => None,
    }
}

// ---- the conflicts ----

/// Ask which files the DIFF VIEWER up conflicts in
/// (`conflicts::local`): a checkout's HEAD against its pull request's
/// base — the base the COMMIT LIST measures against while there is no pull
/// request, or its page is not read — and a pull request's tip against the
/// base it merges into. Off the loop in a viewer with BACKGROUND READS,
/// inline in one without.
pub(crate) fn ask(app: &mut App) {
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return;
    };
    let page = pull_request(app, view).and_then(|pr| app.pr_detail.get(&pr.url));
    let (base, head) = match (view.pr_review.is_some(), page) {
        (true, Some(page)) if !page.head_sha.is_empty() => {
            (page.base.clone(), page.head_sha.clone())
        }
        (true, _) => return,
        (false, Some(page)) if !page.base.is_empty() => (page.base.clone(), "HEAD".to_string()),
        (false, _) => (app.tree.base_branch_at(&view.root), "HEAD".to_string()),
    };
    let (root, jobs) = (view.root.clone(), view.jobs.clone());
    let ticket = crate::view_jobs::ticket();
    if let Some(Overlay::Diff(view)) = &mut app.overlay {
        view.conflicts.asking = Some(ticket);
    }
    let work = move || crate::conflicts::local(&root, &base, &head);
    match jobs {
        Some(jobs) => jobs.run(move || {
            let answer = crate::conflicts::Answer::Local {
                ticket,
                result: work(),
            };
            Some(crate::view_jobs::Answer::Conflicts(answer))
        }),
        None => land(app, ticket, work()),
    }
}

/// [`ask`]'s answer, for the viewer still waiting on it. One that failed —
/// no base to compare with, a commit the repo lacks — keeps what was known.
pub(crate) fn land(app: &mut App, ticket: u64, result: Result<crate::conflicts::Local, String>) {
    let Some(Overlay::Diff(view)) = &mut app.overlay else {
        return;
    };
    if view.conflicts.asking != Some(ticket) {
        return;
    }
    view.conflicts.asking = None;
    match result {
        Ok(local) => view.conflicts.known = Some(local),
        Err(why) => tracing::debug!(root = %view.root.display(), "conflicts: {why}"),
    }
}

/// The viewer back up after the CONFLICT RESOLVER that stood over it, read
/// afresh: the merge may have changed its files and settled its conflicts.
pub(crate) fn reopen(app: &mut App, view: DiffView) {
    app.overlay = Some(Overlay::Diff(view));
    reread(app);
}

/// The viewer up read again where it stands — its diff, the reader's
/// place kept (`git_diff::reload`), and its conflicts: the BUILT-IN EDITOR
/// or the RESOLVER that stood over it may have changed both.
pub(crate) fn reread(app: &mut App) {
    if let Some(Overlay::Diff(view)) = &mut app.overlay {
        crate::git_diff::reload(view);
    }
    ask(app);
    app.dirty = true;
}

// ---- the keys ----

/// The DIFF VIEWER's keys that reach past the diff: `⌘U`, `Enter` on a
/// conflicted file, and `Enter` on the diff. True when `key` was one of
/// them, and is spent.
pub(crate) fn handle_key(app: &mut App, key: &KeyEvent, out: &mut Vec<ClientRequest>) -> bool {
    use crate::ui::diff_keys::{EDIT, OPEN};
    let chord = crate::keymap::KeyChord::from_event(key);
    let action = app.keymap.lookup(crate::keymap::Scope::Global, &chord);
    if action == Some(crate::keymap::Action::PullRequests) {
        open_pull_requests(app);
        return true;
    }
    if crate::hints::IN_TREE.matches(key) {
        reveal_in_tree(app);
        return true;
    }
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return false;
    };
    if EDIT.matches(key) && view.focus == DiffFocus::Diff {
        edit_at_line(app);
        return true;
    }
    if !OPEN.matches(key) || view.focus != DiffFocus::Files {
        return false;
    }
    let Some(path) = conflicted_file(view) else {
        return false;
    };
    resolve(app, &path, out);
    true
}

/// `Enter` on the diff: its file in the BUILT-IN EDITOR at the line at the
/// top of the diff (`DiffView::file_at_line`), in the checkout the viewer
/// reads — for a pull request's viewer, the project's checkout of its
/// branch, when there is one. The viewer is read again when the editor
/// closes ([`reread`]).
fn edit_at_line(app: &mut App) {
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return;
    };
    let Some((path, line)) = view.file_at_line() else {
        return;
    };
    edit_at(app, path, line);
}

/// The viewer's file `path` in the BUILT-IN EDITOR at `line`, in the
/// checkout it reads ([`viewer_checkout`]) — [`edit_at_line`]'s, and a pull
/// request's review's at the line under its cursor (`pr_review::viewer`).
pub(crate) fn edit_at(app: &mut App, path: String, line: u64) {
    let Some(root) = viewer_checkout(app) else {
        return;
    };
    let editor = crate::config::Config::load().editor_command();
    let size = crate::event_loop::editor_size_guess(app);
    crate::event_loop::spawn_editor_modal(app, &editor, &root, &path, line, size);
}

/// The checkout the viewer's files are opened in: the one it reads — for
/// a pull request's viewer, the project's checkout of its branch, the
/// footer saying how to make one when there is none.
fn viewer_checkout(app: &mut App) -> Option<std::path::PathBuf> {
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return None;
    };
    let Some(review) = &view.pr_review else {
        return Some(view.root.clone());
    };
    let (number, url) = (review.number, review.url.clone());
    let root = checkout_of_pr(app, &url);
    if root.is_none() {
        app.flash = Some(Flash::note(format!(
            "#{number} isn't checked out here — {} in the pull requests modal checks it out",
            crate::pr_modal::keys::CHECKOUT.label()
        )));
    }
    root
}

/// The project's checkout of pull request `url`'s branch, when there is
/// one on disk (`pr_checkout::worktree_of`).
fn checkout_of_pr(app: &App, url: &str) -> Option<std::path::PathBuf> {
    let (project, head) = match crate::pr_checkout::of_url(app, url) {
        Some((project, launch)) => (project, launch.head),
        None => (
            app.selected_project()?.id.clone(),
            app.pr_detail.get(url)?.head.clone(),
        ),
    };
    crate::pr_checkout::worktree_of(app, &project, &head).map(|w| w.path.clone())
}

/// `⌘U`, the pull-requests key everywhere: the PULL REQUESTS MODAL over the
/// viewer, on the page of the pull request it is about, and Esc there
/// comes back (`pr_modal::open_from_diff`). A pull request's viewer that
/// the modal opened is a level inside it already: `⌘U` goes back up, as
/// Esc does.
fn open_pull_requests(app: &mut App) {
    let Some(Overlay::Diff(mut view)) = app.overlay.take() else {
        return;
    };
    if let Some(back) = view.back.take() {
        return crate::pr_modal::reopen(app, *back);
    }
    let url = pull_request(app, &view).map(|pr| pr.url);
    crate::pr_modal::open_from_diff(app, view, url);
}

/// `⌘⇧B`: the TREE BROWSER on the viewer's checkout ([`viewer_checkout`])
/// with its cursor on the file the viewer is on — none, for a folder's row
/// or a file the change deletes — and Esc there comes back to the viewer
/// as it was.
fn reveal_in_tree(app: &mut App) {
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return;
    };
    let (branch, file) = (
        view.branch.clone(),
        view.file_at_line().map(|(path, _)| path),
    );
    let Some(root) = viewer_checkout(app) else {
        return;
    };
    let back = app.overlay.take();
    crate::event_loop::tree_revealing(app, root, branch, file.as_deref(), back);
}

/// `Enter` on conflicted `path`: the CONFLICT RESOLVER over the viewer, on
/// the checkout it reads — a pull request's viewer on the project's
/// checkout of it, cut first if there is none — unless an `autofix-<n>` is
/// already on the pull request, when the footer says so.
fn resolve(app: &mut App, path: &str, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Diff(view)) = &app.overlay else {
        return;
    };
    let worktree = checkout(app, view).cloned();
    let pr = pull_request(app, view);
    let base = view.conflicts.known.as_ref().map(|l| l.base.clone());
    if let Some(pr) = &pr {
        if autofix_on_it(app, worktree.as_ref(), pr) {
            let fixer = crate::autofix::agent_name(pr.number);
            app.flash = Some(Flash::note(format!("{fixer} is on it")));
            return;
        }
    }
    match (worktree, pr) {
        (Some(worktree), pr) => {
            let base = crate::commit_list::base_name(base.as_deref().unwrap_or_default());
            let base = base.to_string();
            let (url, label, head_sha) = match &pr {
                Some(pr) => (
                    pr.url.as_str(),
                    pr.label(),
                    app.prs.status_or_open(&pr.url).head_sha,
                ),
                None => ("", worktree.branch.clone(), String::new()),
            };
            let pr = crate::conflicts::Pr {
                url,
                label: &label,
                head_sha: &head_sha,
            };
            crate::conflicts::resolve_in(app, worktree, base, pr, Some(path));
        }
        (None, Some(pr)) => crate::conflicts::open(app, &pr.url, &pr.label(), Some(path), out),
        (None, None) => {}
    }
}

/// Whether an `autofix-<n>` is on `pr`, in one of its project's checkouts
/// of its branch: the viewer's own checkout's, or the branch a pull
/// request's page names.
fn autofix_on_it(app: &App, worktree: Option<&orion_core::Worktree>, pr: &ViewedPr) -> bool {
    let on = match worktree {
        Some(w) => Some((w.project_id.clone(), w.branch.clone())),
        None => app
            .selected_project()
            .map(|p| p.id.clone())
            .zip(app.pr_detail.get(&pr.url).map(|d| d.head.clone())),
    };
    on.is_some_and(|(project, branch)| crate::autofix::fixing(app, &project, &branch, pr.number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflicts::tests::{conflicted_clone, git};
    use crossterm::event::KeyCode;
    use orion_core::{Entity, ProjectId, ServerEvent, WorktreeId};
    use std::path::Path;

    const URL: &str = "https://github.com/o/r/pull/7";

    /// A project whose one checkout, worktree `w` on `feature`, is `clone`.
    fn app_on(clone: &Path) -> App {
        let mut app = App::new();
        let mut out = Vec::new();
        let entities = [
            Entity::Project(orion_core::Project {
                id: ProjectId("p".into()),
                name: "demo".into(),
                repo_path: clone.to_path_buf(),
                sort_order: 0,
            }),
            Entity::Worktree(orion_core::Worktree {
                id: WorktreeId("w".into()),
                project_id: ProjectId("p".into()),
                path: clone.to_path_buf(),
                branch: "feature".into(),
                is_main: false,
                sort_order: 0,
            }),
        ];
        for entity in entities {
            let event = ServerEvent::EntityUpserted { entity };
            crate::event_loop::handle_server_event(&mut app, event, &mut out);
        }
        app
    }

    /// `w`'s branch has pull request #7.
    fn with_pull_request(app: &mut App) {
        let pr = crate::pull_request::PullRequest {
            number: 7,
            url: URL.into(),
            title: "Fix login".into(),
            answered_state: crate::pull_request::STATE_OPEN.into(),
            answered_draft: false,
            answered: Default::default(),
            activity: Vec::new(),
        };
        app.pull_requests.insert(WorktreeId("w".into()), Some(pr));
    }

    fn the_view(app: &App) -> &DiffView {
        match &app.overlay {
            Some(Overlay::Diff(view)) => view,
            other => panic!("no viewer: {other:?} {:?}", app.flash),
        }
    }

    fn screen(app: &mut App) -> String {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(150, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40)
            .map(|y| {
                (0..150)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(app: &mut App, code: KeyCode) -> Vec<ClientRequest> {
        let mut out = Vec::new();
        crate::event_loop::handle_overlay_key(app, KeyEvent::from(code), &mut out);
        out
    }

    /// `main` and `feature` edit the same line of a.txt, and feature deletes
    /// gone.txt that main edits: `⌘E` on feature asks git, with no pull
    /// request and no network, and both files wear `U`, the top line
    /// counting them. With a pull request the count follows its checks.
    #[test]
    fn a_real_conflict_marks_its_files_and_heads_the_viewer() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let mut app = app_on(&clone);
        crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
        assert_eq!(conflicted(the_view(&app)), ["a.txt", "gone.txt"]);
        let shot = screen(&mut app);
        assert!(
            shot.contains(" 2 files conflict with origin/main"),
            "{shot}"
        );
        assert!(shot.contains("U    a.txt"), "{shot}");
        assert!(shot.contains("U    gone.txt"), "{shot}");
        assert!(!git(&clone, &["status", "--porcelain"]).contains("UU"));

        with_pull_request(&mut app);
        let shot = screen(&mut app);
        assert!(
            shot.contains(" #7 Fix login · 2 files conflict with origin/main"),
            "{shot}"
        );

        // A branch that merges cleanly: nothing marked, and no line.
        let clean = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&clean);
        git(&clone, &["reset", "-q", "--hard", "origin/main"]);
        std::fs::write(clone.join("new.txt"), "new\n").unwrap();
        git(&clone, &["add", "new.txt"]);
        git(&clone, &["commit", "-q", "-m", "new"]);
        let mut app = app_on(&clone);
        crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
        assert!(conflicted(the_view(&app)).is_empty());
        assert!(!screen(&mut app).contains("conflict with"));
    }

    /// The merge-tree runs on the blocking pool: the viewer is up, asking,
    /// before it answers, and the marks land with the answer.
    #[test]
    fn the_conflicts_are_asked_off_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let mut app = app_on(&clone);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        app.view_jobs = Some(crate::view_jobs::Jobs::new(tx));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
            let view = the_view(&app);
            assert!(view.conflicts.asking.is_some() && view.conflicts.known.is_none());
            loop {
                let wait = std::time::Duration::from_secs(20);
                let answer = tokio::time::timeout(wait, rx.recv())
                    .await
                    .unwrap()
                    .unwrap();
                if let crate::view_jobs::Answer::Conflicts(answer) = answer {
                    crate::conflicts::land(&mut app, answer);
                    break;
                }
            }
        });
        assert_eq!(conflicted(the_view(&app)), ["a.txt", "gone.txt"]);
        assert_eq!(the_view(&app).conflicts.asking, None);
    }

    /// `Enter` on a `U` file opens the CONFLICT RESOLVER over the viewer on
    /// its checkout, mid-merge at that file, and Esc comes back to the
    /// viewer; with an `autofix-7` on the pull request it only says so.
    #[test]
    fn enter_on_a_conflicted_file_resolves_it_unless_autofix_is_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = conflicted_clone(&dir);
        let mut app = app_on(&clone);
        with_pull_request(&mut app);
        crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
        let Some(Overlay::Diff(view)) = &mut app.overlay else {
            panic!("no viewer");
        };
        view.focus = DiffFocus::Files;
        assert!(view.select_path("gone.txt"));
        let shot = screen(&mut app);
        assert!(shot.contains("╰ Enter resolve · "), "{shot}");

        app.tree.agents.push(orion_core::Agent {
            id: orion_core::AgentId("fixer".into()),
            worktree_id: WorktreeId("w".into()),
            name: crate::autofix::agent_name(7),
            status: orion_core::AgentStatus::Running,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: orion_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            status_changed_at: 1,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
        });
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.flash.as_deref(), Some("autofix-7 is on it"));
        the_view(&app);
        assert!(
            git(&clone, &["status", "--porcelain"]).is_empty(),
            "nothing merged"
        );

        app.tree.agents.clear();
        press(&mut app, KeyCode::Enter);
        let Some(Overlay::Conflicts(resolver)) = &app.overlay else {
            panic!("no resolver: {:?}", app.flash);
        };
        assert!(matches!(
            resolver.back,
            Some(crate::conflicts::Under::Diff(_))
        ));
        assert_eq!(resolver.url, URL);
        assert_eq!(resolver.label, "#7 Fix login");
        let open = resolver
            .files
            .get(resolver.selected)
            .map(|f| f.path.as_str());
        assert_eq!(open, Some("gone.txt"), "at the file Enter was on");
        assert!(git(&clone, &["status", "--porcelain"]).contains("a.txt"));

        crate::conflicts::handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(the_view(&app).conflicts.known.is_some());
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.contains("mid-merge")),
            "{:?}",
            app.flash
        );
        git(&clone, &["merge", "--abort"]);
    }

    /// `⌘U` in the viewer opens the PULL REQUESTS MODAL on this branch's
    /// pull request — another listed first — and its Esc puts the viewer
    /// back as it was; with no pull request it opens as the hotkey does,
    /// and still comes back.
    #[test]
    fn cmd_u_opens_the_branchs_pull_request_and_esc_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_on(dir.path());
        let row = |number: u64, url: &str| crate::pull_request::OpenPr {
            number,
            title: "t".into(),
            url: url.into(),
            answered_draft: false,
            answered: Default::default(),
            head: "other".into(),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        };
        let now = std::time::Instant::now();
        app.open_prs.insert(
            ProjectId("p".into()),
            crate::app::OpenPrs {
                merged: Vec::new(),
                list: vec![row(3, "https://github.com/o/r/pull/3"), row(7, URL)],
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        let cmd_u = || KeyEvent::new(KeyCode::Char('u'), crossterm::event::KeyModifiers::SUPER);
        let viewer = DiffView::new(dir.path().to_path_buf(), "feature".into(), Vec::new(), true);
        let id = viewer.id;

        app.overlay = Some(Overlay::Diff(viewer));
        crate::event_loop::handle_overlay_key(&mut app, cmd_u(), &mut Vec::new());
        let Some(Overlay::PullRequests(modal)) = &app.overlay else {
            panic!("no modal: {:?}", app.overlay);
        };
        assert!(modal.from_diff.is_some());
        let first = modal.selected_url.clone();
        press(&mut app, KeyCode::Esc);
        assert_eq!(the_view(&app).id, id, "the same viewer, back");

        with_pull_request(&mut app);
        crate::event_loop::handle_overlay_key(&mut app, cmd_u(), &mut Vec::new());
        let Some(Overlay::PullRequests(modal)) = &app.overlay else {
            panic!("no modal: {:?}", app.overlay);
        };
        assert_eq!(modal.selected_url.as_deref(), Some(URL), "on #7's page");
        assert_ne!(first.as_deref(), Some(URL), "not where the hotkey lands");
        let hints = crate::hints::text(&crate::pr_modal::hints(modal), 400);
        assert!(hints.ends_with("Esc back"), "{hints}");
        press(&mut app, KeyCode::Esc);
        assert_eq!(the_view(&app).id, id);
    }

    /// `f` with `/bin/sh` standing in for fresh, on an empty config.
    fn with_sh_editor<T>(f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.json");
        std::fs::write(&config, "{}").unwrap();
        crate::config::with_config_path(config, || crate::editor::with_command("/bin/sh", f))
    }

    /// A repo whose notes.txt, 300 lines, has every fourth line changed
    /// and not committed: a diff far taller than the pane.
    fn repo_with_long_diff(dir: &Path) {
        git(dir, &["init", "-q", "-b", "main"]);
        crate::conflicts::tests::identify(dir);
        let text = |changed: &dyn Fn(usize) -> bool| -> String {
            (1..=300)
                .map(|n| match changed(n) {
                    true => format!("changed {n}\n"),
                    false => format!("line {n}\n"),
                })
                .collect()
        };
        std::fs::write(dir.join("notes.txt"), text(&|_| false)).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "-q", "-m", "init"]);
        std::fs::write(dir.join("notes.txt"), text(&|n| n % 4 == 0)).unwrap();
    }

    /// `Enter` on the diff opens fresh on its file at the line at the top
    /// of the pane, in the checkout; when the editor closes the diff is
    /// read again — what was saved shows — with the reader's place kept.
    #[test]
    fn enter_on_the_diff_edits_at_the_line_and_the_diff_reloads() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_long_diff(dir.path());
        let mut app = app_on(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.editor_tx = Some(tx);
        crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
        screen(&mut app);
        let Some(Overlay::Diff(view)) = &mut app.overlay else {
            panic!("no viewer");
        };
        view.focus = DiffFocus::Diff;
        view.scroll_by(40);
        let (scroll, line) = (view.scroll, view.line_on_screen());
        assert!(scroll == 40 && line > 20, "scrolled: {scroll} {line}");

        with_sh_editor(|| press(&mut app, KeyCode::Enter));
        let editor = app.editor.as_ref().expect("Enter edits");
        assert_eq!((editor.file.as_str(), editor.line), ("notes.txt", line));
        assert_eq!(editor.cwd, dir.path());
        assert!(!editor.embedded, "over the viewer");

        let mut saved = std::fs::read_to_string(dir.path().join("notes.txt")).unwrap();
        saved.push_str("saved in the editor\n");
        std::fs::write(dir.path().join("notes.txt"), saved).unwrap();
        crate::event_loop::close_editor(&mut app);
        let view = the_view(&app);
        assert!(view.diff.contains("saved in the editor"), "read again");
        assert_eq!(
            view.selected_file().map(|f| f.path.as_str()),
            Some("notes.txt")
        );
        assert_eq!(view.scroll, scroll, "the reader's place kept");
    }

    /// `⌘⇧B` in the viewer opens the TREE BROWSER on its checkout with the
    /// cursor on the viewer's file, its folders open; Esc there puts the
    /// viewer back on that file, read afresh.
    #[test]
    fn cmd_shift_b_shows_the_viewers_file_in_the_tree_and_esc_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_long_diff(dir.path());
        std::fs::create_dir_all(dir.path().join("docs/deep")).unwrap();
        std::fs::write(dir.path().join("docs/deep/n.txt"), "new\n").unwrap();
        let mut app = app_on(dir.path());
        crate::event_loop::run_action(&mut app, crate::keymap::Action::GitDiff);
        let Some(Overlay::Diff(view)) = &mut app.overlay else {
            panic!("no viewer");
        };
        let at = view
            .files
            .iter()
            .position(|f| f.path == "docs/deep/n.txt")
            .expect("the new file listed");
        view.select(at as i64);
        screen(&mut app);
        let cmd_shift_b = KeyEvent::new(
            KeyCode::Char('B'),
            crossterm::event::KeyModifiers::SUPER | crossterm::event::KeyModifiers::SHIFT,
        );
        crate::event_loop::handle_overlay_key(&mut app, cmd_shift_b, &mut Vec::new());
        let Some(Overlay::Tree(tree)) = &app.overlay else {
            panic!("no tree: {:?}", app.overlay);
        };
        assert_eq!(
            tree.selected_node().map(|n| n.path.as_str()),
            Some("docs/deep/n.txt")
        );
        assert!(tree.back.is_some());
        // What the tree's Esc runs (`closes_on_esc`).
        crate::overlay_close::click_outside(&mut app, &mut Vec::new());
        assert_eq!(
            the_view(&app).selected_file().map(|f| f.path.as_str()),
            Some("docs/deep/n.txt"),
            "the viewer back, read afresh, on its file"
        );
    }

    /// A pull request's viewer edits in the project's checkout of its
    /// branch, and says so when there is none.
    #[test]
    fn a_pull_requests_diff_edits_in_its_checkout() {
        let dir = tempfile::tempdir().unwrap();
        repo_with_long_diff(dir.path());
        let mut app = app_on(dir.path());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        app.editor_tx = Some(tx);
        let detail = crate::pull_request::PrDetail {
            number: 7,
            url: URL.into(),
            head: "feature".into(),
            ..Default::default()
        };
        app.pr_detail.insert(URL.into(), detail);
        let file = crate::git_diff::DiffFile {
            path: "notes.txt".into(),
            orig_path: None,
            xy: ['M', ' '],
            lines: None,
        };
        let repo = std::path::PathBuf::from("/nonexistent-project-repo");
        let mut viewer = DiffView::new(repo, "#7 Fix login".into(), vec![file], true);
        viewer.pr_review = Some(crate::event_loop::PrReview {
            number: 7,
            url: URL.into(),
            title: "#7 Fix login".into(),
            at: crate::event_loop::PrReviewAt::Top,
        });
        viewer.focus = DiffFocus::Diff;
        app.overlay = Some(Overlay::Diff(viewer.clone()));
        with_sh_editor(|| press(&mut app, KeyCode::Enter));
        let editor = app.editor.as_ref().expect("Enter edits");
        assert_eq!(editor.cwd, dir.path(), "in the checkout of `feature`");
        assert_eq!(editor.file, "notes.txt");

        app.editor = None;
        app.tree.worktrees.clear();
        press(&mut app, KeyCode::Enter);
        assert!(app.editor.is_none());
        assert!(
            app.flash
                .as_deref()
                .is_some_and(|f| f.contains("#7 isn't checked out")),
            "{:?}",
            app.flash
        );
    }
}
