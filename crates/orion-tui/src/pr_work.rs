//! The PULL REQUEST PAGE's WORK BLOCK: what is at work on a pull request,
//! between the page's sentence and its tabs (`pr_preview::page`) — up to
//! three lines, each only when it has something to say:
//!
//! * **PR FIXER** — the live `autofix-<n>` on it and what it is doing; or,
//!   with none on it while it is broken, what broke and the key that sends
//!   one: `✗ 1 failing check, 3 files conflict · ⌘G send to the PR fixer`.
//! * **LINEAR** — the issues linked to it, each with its state: those its
//!   branch was linked to (`linear-links.json`, `App::linear_links`) and
//!   those Linear has it attached to, as the project's list (`App::linear`)
//!   has them.
//! * **SESSIONS** — the agents on its head branch, each with the ACCOUNT
//!   TAG its grid row wears.
//!
//! A click on a session lands on it in the grid; on an issue, opens it in
//! the LINEAR VIEW; on `⌘G send to the PR fixer`, is that key ([`click`]).

use crate::app::App;
use crate::theme::Theme;
use orion_core::{Agent, AgentId, AgentStatus, ClientRequest, ProjectId};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// What a click on a part of the block does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkHit {
    /// Land on the session in the grid.
    Session(AgentId),
    /// Open the issue in the LINEAR VIEW.
    Issue { project: ProjectId, id: String },
    /// `⌘G`: the AUTOFIX MODAL on the pull request.
    SendAutofix,
}

/// One thing a line names, and what a click on it does.
#[derive(Debug, Clone)]
struct Item {
    spans: Vec<Span<'static>>,
    hit: Option<WorkHit>,
}

/// One line of the block: its label, and what it names after it.
#[derive(Debug, Clone)]
struct WorkLine {
    label: &'static str,
    items: Vec<Item>,
    /// What stands between two of its items.
    sep: &'static str,
}

/// The block for one pull request, ready to lay out ([`lines`]): no lines
/// at all when nothing is at work on it.
#[derive(Debug, Clone, Default)]
pub struct Work {
    lines: Vec<WorkLine>,
}

/// Where a part of the block that a click acts on landed: its row in the
/// block, its columns `[from, to)`, and what the click does.
pub(crate) type WorkSpot = (u16, u16, u16, WorkHit);

/// The labels' column: the longest label, `SESSIONS`, and two cells of air.
const LABEL_W: usize = 10;
/// Between two issues, or two sessions.
const LIST_SEP: &str = "   ";

/// The block for pull request `number` at `url`, of `project`.
pub fn of(app: &App, project: &ProjectId, url: &str, number: u64) -> Work {
    let head = head_branch(app, project, url);
    let fixer = head
        .as_deref()
        .and_then(|head| crate::automations::pr_fixer::fixer(app, project, head, number));
    let lines = [
        autofix_line(app, url, fixer),
        linear_line(app, project, url, head.as_deref()),
        sessions_line(app, project, head.as_deref(), fixer),
    ];
    Work {
        lines: lines.into_iter().flatten().collect(),
    }
}

/// The branch the pull request comes from: its page's word, else its row
/// in the project's open list.
fn head_branch(app: &App, project: &ProjectId, url: &str) -> Option<String> {
    let row = || crate::automations::pr_fixer::open_pr(app, project, url).map(|pr| pr.head.clone());
    app.pr_detail
        .get(url)
        .map(|d| d.head.clone())
        .filter(|head| !head.is_empty())
        .or_else(row)
        .filter(|head| !head.is_empty())
}

/// PR FIXER: the `autofix-<n>` at work on it, as a session chip and what it
/// is doing — or, with none, what broke while it is open and broken, and
/// the key that sends one.
fn autofix_line(app: &App, url: &str, fixer: Option<&Agent>) -> Option<WorkLine> {
    let th = app.theme;
    let dim = Style::default().fg(th.dim);
    if let Some(agent) = fixer {
        let mut spans = crate::ui::session_chip(app, agent);
        spans.push(Span::styled(format!("  {}", doing(agent)), dim));
        return Some(WorkLine {
            label: "PR FIXER",
            items: vec![Item {
                spans,
                hit: Some(WorkHit::Session(agent.id.clone())),
            }],
            sep: "",
        });
    }
    let status = app.prs.status_or_open(url);
    if !status.is_open() {
        return None;
    }
    let failed = app.pr_detail.get(url).map_or(0, |d| {
        crate::pull_request::CheckCounts::of(&d.checks).failed
    });
    let mut broke = Vec::new();
    if failed > 0 {
        broke.push(crate::flash::plural(failed, "failing check"));
    }
    if status.health.conflicts {
        broke.push(
            match crate::conflicts::of_page(app, url).map(<[String]>::len) {
                Some(1) => "1 file conflicts".to_string(),
                Some(n) => format!("{n} files conflict"),
                None => "merge conflicts".to_string(),
            },
        );
    }
    if broke.is_empty() {
        return None;
    }
    let key = Style::default().fg(th.text).add_modifier(Modifier::BOLD);
    Some(WorkLine {
        label: "PR FIXER",
        items: vec![
            Item {
                spans: vec![Span::styled(
                    format!("✗ {}", broke.join(", ")),
                    Style::default().fg(th.err),
                )],
                hit: None,
            },
            Item {
                spans: vec![
                    Span::styled(crate::pr_modal::keys::AUTOFIX.label(), key),
                    Span::styled(" send to the PR fixer", dim),
                ],
                hit: Some(WorkHit::SendAutofix),
            },
        ],
        sep: " · ",
    })
}

/// What an `autofix-<n>` is doing: `starting`, `working · 4m`, `waiting
/// on you · 2m` — how long since it last moved, once that is a minute.
fn doing(agent: &Agent) -> String {
    let word = match agent.status {
        AgentStatus::Fresh => "starting",
        AgentStatus::NeedsFeedback => "waiting on you",
        _ => "working",
    };
    if agent.status_changed_at <= 0 {
        return word.to_string();
    }
    let secs = (crate::app::now_ms() - agent.status_changed_at) / 1000;
    match crate::hosts::ago_short(secs).as_str() {
        "now" => word.to_string(),
        since => format!("{word} · {since}"),
    }
}

/// LINEAR: the issues linked to the pull request — on its branch's link,
/// then any other Linear has it attached to — each with its state as the
/// project's list has it; one the list does not hold, by identifier alone.
fn linear_line(app: &App, project: &ProjectId, url: &str, head: Option<&str>) -> Option<WorkLine> {
    let th = app.theme;
    let list = app
        .linear
        .get(project)
        .map(|l| l.list.as_slice())
        .unwrap_or_default();
    let mut issues: Vec<(String, String)> = head
        .map(|head| app.linear_links.issues_on(project, head))
        .unwrap_or_default();
    for issue in list {
        let attached = issue.prs.iter().any(|pr| pr.url == url);
        if attached && !issues.iter().any(|(id, _)| *id == issue.id) {
            issues.push((issue.id.clone(), issue.identifier.clone()));
        }
    }
    let items: Vec<Item> = issues
        .into_iter()
        .map(|(id, identifier)| {
            let row = list.iter().find(|issue| issue.id == id);
            let (kind, color) = row.map_or(("", ""), |i| {
                (i.status_type.as_str(), i.state_color.as_str())
            });
            let (glyph, tint) = crate::linear::state_glyph(kind, color, th);
            let mut spans = vec![
                Span::styled(format!("{glyph} "), Style::default().fg(tint)),
                Span::styled(identifier, Style::default().fg(th.text)),
            ];
            if let Some(issue) = row.filter(|i| !i.status.is_empty()) {
                spans.push(Span::styled(
                    format!(" {}", issue.status),
                    Style::default().fg(th.dim),
                ));
            }
            Item {
                spans,
                hit: Some(WorkHit::Issue {
                    project: project.clone(),
                    id,
                }),
            }
        })
        .collect();
    (!items.is_empty()).then_some(WorkLine {
        label: "LINEAR",
        items,
        sep: LIST_SEP,
    })
}

/// SESSIONS: the live agents in the project's checkouts of the head
/// branch, the most recently active first — the one the AUTOFIX line
/// already shows aside.
fn sessions_line(
    app: &App,
    project: &ProjectId,
    head: Option<&str>,
    fixer: Option<&Agent>,
) -> Option<WorkLine> {
    let head = head?;
    let mut agents: Vec<&Agent> = app
        .tree
        .agents
        .iter()
        .filter(|a| !a.archived && Some(&a.id) != fixer.map(|f| &f.id))
        .filter(|a| crate::automations::pr_fixer::on_branch(app, a, project, head))
        .collect();
    agents.sort_by_key(|a| crate::app::recency_key(a));
    let items: Vec<Item> = agents
        .into_iter()
        .map(|a| Item {
            spans: crate::ui::session_chip(app, a),
            hit: Some(WorkHit::Session(a.id.clone())),
        })
        .collect();
    (!items.is_empty()).then_some(WorkLine {
        label: "SESSIONS",
        items,
        sep: LIST_SEP,
    })
}

/// The block laid out for `width` columns: a line each, its label in the
/// dim column, its items after — whatever runs past the edge cut — and
/// where each item a click acts on landed, clipped to the width.
pub(crate) fn lines(work: &Work, width: usize, th: Theme) -> (Vec<Line<'static>>, Vec<WorkSpot>) {
    let indent = crate::pr_preview::INDENT;
    let label = Style::default().fg(th.dim).add_modifier(Modifier::BOLD);
    let mut out = Vec::new();
    let mut spots = Vec::new();
    for (row, line) in work.lines.iter().enumerate() {
        let mut spans = vec![
            Span::raw(indent),
            Span::styled(format!("{:<LABEL_W$}", line.label), label),
        ];
        let mut x = indent.width() + LABEL_W;
        for (i, item) in line.items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(line.sep, Style::default().fg(th.faint)));
                x += line.sep.width();
            }
            let w: usize = item.spans.iter().map(Span::width).sum();
            if let (Some(hit), true) = (&item.hit, x < width) {
                spots.push((row as u16, x as u16, (x + w).min(width) as u16, hit.clone()));
            }
            spans.extend(item.spans.iter().cloned());
            x += w;
        }
        out.push(crate::pr_preview::fit(spans, width));
    }
    (out, spots)
}

/// A click on a part of the block: a session landed on in the grid, an
/// issue opened in the LINEAR VIEW, the AUTOFIX MODAL as `⌘G` opens it.
pub(crate) fn click(app: &mut App, hit: WorkHit, out: &mut Vec<ClientRequest>) {
    match hit {
        WorkHit::Session(id) => crate::event_loop::jump_to_session(app, id, out),
        WorkHit::Issue { project, id } => crate::linear::open_at(app, &project, &id),
        WorkHit::SendAutofix => crate::automations::pr_fixer::open_asked(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Overlay;
    use crate::pull_request::{Answered, CheckState, OpenPr, PrCheck, PrDetail};
    use orion_core::WorktreeId;

    const URL: &str = "https://github.com/o/r/pull/7";

    fn project() -> ProjectId {
        ProjectId("p".into())
    }

    fn failed(name: &str) -> PrCheck {
        PrCheck {
            name: name.into(),
            workflow: "CI".into(),
            state: CheckState::Failed,
            word: "FAILURE".into(),
            started: String::new(),
            completed: String::new(),
            url: format!("https://ci/{name}"),
        }
    }

    /// Observe `detail` into `app.prs`, as a page landing does.
    fn observe(app: &mut App, detail: &PrDetail) {
        app.prs.observe(
            &detail.url,
            crate::pr_store::PrObservation::of_detail(detail),
            crate::fetch::Asked::At(crate::fetch::now()),
        );
    }

    /// Project `p` at `repo`, its root checkout on `main` and one of
    /// `feat`, with pull request #7 from `feat` on its open list and its
    /// page read: `checks` on it, conflicting with `main` when `conflicts`.
    fn app_with_pr(
        repo: &std::path::Path,
        head_sha: &str,
        conflicts: bool,
        checks: Vec<PrCheck>,
    ) -> App {
        let mut app = App::new();
        app.tree.projects.push(orion_core::Project {
            id: project(),
            name: "repo".into(),
            repo_path: repo.to_path_buf(),
            sort_order: 0,
        });
        for (id, branch, is_main) in [("root", "main", true), ("feat-wt", "feat", false)] {
            app.tree.worktrees.push(orion_core::Worktree {
                id: WorktreeId(id.into()),
                project_id: project(),
                path: repo.join(id),
                branch: branch.into(),
                is_main,
                sort_order: 0,
            });
        }
        let answered = Answered {
            conflicts: Some(conflicts),
            checks: None,
        };
        let detail = PrDetail {
            number: 7,
            url: URL.into(),
            title: "Add things".into(),
            answered_state: "OPEN".into(),
            answered,
            author: "kate".into(),
            base: "main".into(),
            head: "feat".into(),
            head_sha: head_sha.into(),
            checks,
            ..PrDetail::default()
        };
        observe(&mut app, &detail);
        app.pr_detail.insert(URL.into(), detail);
        let now = std::time::Instant::now();
        app.open_prs.insert(
            project(),
            crate::app::OpenPrs {
                merged: Vec::new(),
                list: vec![OpenPr {
                    number: 7,
                    title: "Add things".into(),
                    url: URL.into(),
                    answered_draft: false,
                    answered,
                    head: "feat".into(),
                    mine: true,
                    head_sha: head_sha.into(),
                    meta: Default::default(),
                }],
                at: now,
                due: now + std::time::Duration::from_secs(60),
                step: std::time::Duration::from_secs(60),
            },
        );
        app
    }

    const NOWHERE: &str = "/nonexistent/orion-pr-work";

    /// [`app_with_pr`] on a repo that isn't there, its PR clean.
    fn quiet_app() -> App {
        app_with_pr(std::path::Path::new(NOWHERE), "abc", false, Vec::new())
    }

    /// Session `name` in checkout `worktree`, at `status`, last moved
    /// `ago_ms` ago.
    fn session(
        app: &mut App,
        name: &str,
        worktree: &str,
        status: AgentStatus,
        ago_ms: i64,
    ) -> AgentId {
        let id = AgentId(format!("id-{name}"));
        app.tree.agents.push(Agent {
            id: id.clone(),
            worktree_id: WorktreeId(worktree.into()),
            name: name.into(),
            status,
            archived: false,
            archived_at: 0,
            unseen: false,
            kind: orion_core::AgentKind::Claude,
            custom_harness: None,
            model: None,
            effort: None,
            session_id: None,
            sort_order: 0,
            status_changed_at: crate::app::now_ms() - ago_ms,
            alive: true,
            recent_prompts: Vec::new(),
            usage_limit: None,
            automation: None,
        });
        id
    }

    /// The block as text, a line each, laid out `width` wide.
    fn shown(app: &App, width: usize) -> Vec<String> {
        let work = of(app, &project(), URL, 7);
        let (lines, _) = lines(&work, width, app.theme);
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    /// Where the block's clickable parts landed, `width` wide.
    fn spots(app: &App, width: usize) -> Vec<WorkSpot> {
        lines(&of(app, &project(), URL, 7), width, app.theme).1
    }

    /// A broken pull request nothing is on says what broke — its failing
    /// checks, and how many files conflict as the repo merged them — and
    /// the key that sends an agent; one that isn't broken, or isn't open,
    /// has no AUTOFIX line at all.
    #[test]
    fn a_broken_pr_with_nothing_on_it_offers_autofix() {
        let dir = tempfile::tempdir().unwrap();
        let (_, clone) = crate::conflicts::tests::conflicted_clone(&dir);
        let head = crate::conflicts::tests::git(&clone, &["rev-parse", "HEAD"]);
        let mut app = app_with_pr(&clone, &head, true, vec![failed("unit")]);
        crate::conflicts::want(&mut app, URL, &clone);
        assert_eq!(
            shown(&app, 100),
            [" PR FIXER  ✗ 1 failing check, 2 files conflict · ⌘G send to the PR fixer"]
        );

        let app = app_with_pr(&clone, &head, false, vec![failed("unit"), failed("lint")]);
        assert_eq!(
            shown(&app, 100),
            [" PR FIXER  ✗ 2 failing checks · ⌘G send to the PR fixer"]
        );
        assert!(
            shown(&quiet_app(), 100).is_empty(),
            "nothing at work, nothing broken"
        );

        let mut merged = app_with_pr(&clone, &head, true, vec![failed("unit")]);
        let mut page = merged.pr_detail[URL].clone();
        page.answered_state = "MERGED".into();
        observe(&mut merged, &page);
        assert!(shown(&merged, 100).is_empty(), "past fixing");
    }

    /// The live `autofix-<n>` on the branch is the AUTOFIX line — its
    /// chip, what it is doing and for how long — and a click on it lands
    /// on it; SESSIONS lists the branch's other live agents, most recently
    /// active first, each as the grid draws it, and none of another
    /// branch's or archived.
    #[test]
    fn the_branch_sessions_and_its_autofix_at_work() {
        let mut app = quiet_app();
        let fixer = session(
            &mut app,
            "autofix-7",
            "feat-wt",
            AgentStatus::Running,
            4 * 60_000,
        );
        let older = session(
            &mut app,
            "billing",
            "feat-wt",
            AgentStatus::Finished,
            3_600_000,
        );
        let newer = session(
            &mut app,
            "docs",
            "feat-wt",
            AgentStatus::NeedsFeedback,
            60_000,
        );
        session(&mut app, "elsewhere", "root", AgentStatus::Running, 0);
        let gone = session(&mut app, "old", "feat-wt", AgentStatus::Finished, 0);
        if let Some(a) = app.tree.agents.iter_mut().find(|a| a.id == gone) {
            a.archived = true;
        }

        let lines = shown(&app, 100);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with(" PR FIXER  "), "{lines:?}");
        assert!(lines[0].ends_with("autofix-7  working · 4m"), "{lines:?}");
        let chip = |id: &AgentId| -> String {
            let a = app.tree.agents.iter().find(|a| &a.id == id).unwrap();
            crate::ui::session_chip(&app, a)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(
            lines[1],
            format!(" SESSIONS  {}   {}", chip(&newer), chip(&older)),
            "the grid's own chips, the newest first"
        );

        let spots = spots(&app, 100);
        let hits: Vec<&WorkHit> = spots.iter().map(|(.., hit)| hit).collect();
        assert_eq!(
            hits,
            [
                &WorkHit::Session(fixer),
                &WorkHit::Session(newer.clone()),
                &WorkHit::Session(older)
            ]
        );
        let (row, from, to, _) = spots[1].clone();
        assert_eq!(
            (row, usize::from(from), usize::from(to)),
            (1, 11, 11 + chip(&newer).width()),
            "where `docs` was drawn"
        );
        for width in 1..=100 {
            for line in shown(&app, width) {
                assert!(line.width() <= width, "{width}: {line:?}");
            }
        }
    }

    /// LINEAR names the issues on the branch's link, then those Linear
    /// has the pull request attached to, each with its state as the
    /// project's list has it — and one the list doesn't hold by its
    /// identifier alone.
    #[test]
    fn linear_names_the_linked_issues_with_their_state() {
        use crate::linear::tests::issue;
        let mut app = quiet_app();
        let linked = issue("1", "ENG-1", "Login");
        let unlisted = issue("9", "ENG-9", "Old");
        let mut attached = issue("2", "ENG-2", "Logout");
        attached.status = "In Review".into();
        attached.prs.push(crate::linear::IssuePr {
            number: 7,
            url: URL.into(),
            ..Default::default()
        });
        app.linear_links
            .remember(&project(), "feat", &[linked.clone(), unlisted]);
        app.linear.insert(
            project(),
            crate::linear::LinearList {
                list: vec![linked, attached, issue("3", "ENG-3", "Nobody's")],
                ..Default::default()
            },
        );
        assert_eq!(
            shown(&app, 100),
            [" LINEAR    ◑ ENG-1 In Progress   · ENG-9   ◑ ENG-2 In Review"]
        );
        assert_eq!(
            spots(&app, 100).last().map(|(.., hit)| hit.clone()),
            Some(WorkHit::Issue {
                project: project(),
                id: "2".into()
            })
        );
    }

    /// A click on a session closes what is up and lands the grid's cursor
    /// on it; on an issue, the LINEAR VIEW opens on it.
    #[test]
    fn a_click_lands_on_the_session_or_opens_the_issue() {
        use crate::linear::tests::issue;
        let mut app = quiet_app();
        let id = session(&mut app, "docs", "feat-wt", AgentStatus::Running, 0);
        crate::pr_modal::open(&mut app);
        assert!(matches!(app.overlay, Some(Overlay::PullRequests(_))));
        let mut out = Vec::new();
        click(&mut app, WorkHit::Session(id.clone()), &mut out);
        assert!(app.overlay.is_none(), "{:?}", app.overlay);
        assert_eq!(app.selected_session().map(|a| a.id), Some(id));

        app.linear.insert(
            project(),
            crate::linear::LinearList {
                list: vec![issue("1", "ENG-1", "Login"), issue("2", "ENG-2", "Logout")],
                ..Default::default()
            },
        );
        let issue = WorkHit::Issue {
            project: project(),
            id: "2".into(),
        };
        click(&mut app, issue, &mut out);
        match &app.overlay {
            Some(Overlay::Linear(view)) => assert_eq!(view.selected, 1),
            other => panic!("the LINEAR VIEW, not {other:?}"),
        }
    }

    /// The modal's page draws the block between its sentence and its
    /// tabs, and a click on a session there lands on it.
    #[test]
    fn the_modal_page_draws_the_block_and_a_click_on_it_acts() {
        let repo = std::path::Path::new(NOWHERE);
        let mut app = app_with_pr(repo, "abc", false, vec![failed("unit")]);
        let id = session(&mut app, "docs", "feat-wt", AgentStatus::Running, 0);
        crate::pr_modal::open(&mut app);
        let (w, h) = (140u16, 34u16);
        let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(w, h)).unwrap();
        term.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
        let buf = term.backend().buffer().clone();
        let screen: Vec<String> = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        let at = |needle: &str| {
            screen
                .iter()
                .position(|l| l.contains(needle))
                .unwrap_or_else(|| panic!("{needle}:\n{}", screen.join("\n")))
        };
        assert!(at("kate · feat → main") < at("PR FIXER  ✗ 1 failing check"));
        assert_eq!(at("PR FIXER") + 1, at("SESSIONS"));
        assert!(at("SESSIONS") < at("Description"));
        let Some(Overlay::PullRequests(view)) = &app.overlay else {
            panic!("the modal");
        };
        let hits: Vec<&WorkHit> = view.tabs.work_hits.iter().map(|(_, hit)| hit).collect();
        assert_eq!(hits, [&WorkHit::SendAutofix, &WorkHit::Session(id.clone())]);
        let (rect, _) = view.tabs.work_hits[1];
        assert!(screen[usize::from(rect.y)].contains("docs"));
        let mouse = crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: rect.x + 2,
            row: rect.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        };
        let pos = ratatui::layout::Position::new(mouse.column, mouse.row);
        crate::pr_modal::handle_mouse(&mut app, mouse, pos, &mut Vec::new());
        assert!(app.overlay.is_none());
        assert_eq!(app.selected_session().map(|a| a.id), Some(id));
    }
}
