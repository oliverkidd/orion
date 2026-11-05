use super::github::{ReviewThread, Side, ThreadComment};
use super::*;
use crate::git_diff::LineChanges;

const URL: &str = "https://github.com/o/r/pull/7";

/// `a.rs`: one line changed, `two` to `deux`, between two that stay.
const A_DIFF: &str = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n\
    @@ -1,3 +1,3 @@\n one\n-two\n+deux\n three\n";
/// `b.rs`: one line added.
const B_DIFF: &str = "diff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n\
    @@ -1,1 +1,2 @@\n alpha\n+beta\n";

fn file(path: &str, diff: &str) -> ReviewFile {
    ReviewFile {
        path: path.into(),
        status: 'M',
        lines: Some(LineChanges {
            added: 1,
            removed: 1,
        }),
        doc: DiffDoc::build(&[], Some(path), diff),
    }
}

fn said(id: &str, who: &str, body: &str, mine: bool, pending: bool) -> ThreadComment {
    ThreadComment {
        id: id.into(),
        author: who.into(),
        body: body.into(),
        at: "2026-11-05T10:00:00Z".into(),
        mine,
        pending,
    }
}

fn thread(id: &str, path: &str, line: u32, comments: Vec<ThreadComment>) -> ReviewThread {
    ReviewThread {
        id: id.into(),
        path: path.into(),
        line: Some(line),
        start_line: None,
        original_line: Some(line),
        side: Side::Right,
        resolved: false,
        outdated: false,
        comments,
    }
}

/// Two files, a thread on `deux` in the first — ada's, with your pending
/// reply — and a resolved one on `beta` in the second.
fn threads() -> Vec<ReviewThread> {
    let mut resolved = thread(
        "T_b",
        "b.rs",
        2,
        vec![said("C_3", "bo", "why beta?", false, false)],
    );
    resolved.resolved = true;
    vec![
        thread(
            "T_a",
            "a.rs",
            2,
            vec![
                said("C_1", "ada", "rename this", false, false),
                said("C_2", "me", "will do", true, true),
            ],
        ),
        resolved,
    ]
}

/// An app with the PR REVIEW up on `files` and `threads`, read.
fn app_on(files: Vec<ReviewFile>, threads: Vec<ReviewThread>) -> App {
    let mut app = App::new();
    let mut view = ReviewScreen::new(
        URL.into(),
        7,
        "#7 Add things".into(),
        "/nonexistent".into(),
        false,
    );
    view.threads = threads;
    view.reading = false;
    view.pr_id = Some("PR_1".into());
    view.take_files(Ok(files));
    app.overlay = Some(Overlay::Review(Box::new(view)));
    app
}

fn two_files() -> App {
    app_on(vec![file("a.rs", A_DIFF), file("b.rs", B_DIFF)], threads())
}

fn view(app: &App) -> &ReviewScreen {
    match &app.overlay {
        Some(Overlay::Review(v)) => v,
        other => panic!("expected the review, got {other:?}"),
    }
}

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| {
        let Some(Overlay::Review(v)) = app.overlay.clone() else {
            panic!("no review");
        };
        draw(f, app, &v, app.theme);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    (0..h)
        .map(|y| {
            (0..w)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The row of `shot` holding `text`.
fn row_of(shot: &str, text: &str) -> usize {
    shot.lines()
        .position(|l| l.contains(text))
        .unwrap_or_else(|| panic!("no {text:?} in\n{shot}"))
}

#[test]
fn the_files_count_their_threads_and_each_thread_sits_under_its_line() {
    let mut app = two_files();
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Review #7 Add things · 2 files"), "{shot}");
    // a.rs: an open thread with a pending comment of yours in it; b.rs:
    // one, resolved.
    let a = shot.lines().nth(row_of(&shot, "M a.rs")).unwrap();
    assert!(a.contains("● 1"), "{a}");
    let b = shot.lines().nth(row_of(&shot, "M b.rs")).unwrap();
    assert!(b.contains("✓1"), "{b}");
    // The thread under the line it is on, its comments in order.
    let deux = row_of(&shot, "deux");
    let first = row_of(&shot, "rename this");
    assert!(deux < row_of(&shot, "ada · "), "{shot}");
    assert!(deux < first && first < row_of(&shot, "will do"), "{shot}");
    assert!(shot.contains("me · pending"), "{shot}");
    assert!(row_of(&shot, "will do") < row_of(&shot, "three"), "{shot}");
}

#[test]
fn the_cursor_walks_lines_and_threads_and_brackets_go_file_to_file() {
    let mut app = two_files();
    screen(&mut app, 140, 30);
    // From the first line, down to the thread under `deux`.
    let first = view(&app).cursor.clone();
    assert!(matches!(first, Some(Stop::Line(_))), "{first:?}");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(view(&app).cursor, Some(Stop::Thread("T_a".into())));
    assert_eq!(
        view(&app).thread_at_cursor().map(|t| t.id.as_str()),
        Some("T_a")
    );
    // `]`: on to the next file's thread, and round to the first again.
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(view(&app).file, 1);
    assert_eq!(view(&app).cursor, Some(Stop::Thread("T_b".into())));
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(view(&app).file, 0);
    assert_eq!(view(&app).cursor, Some(Stop::Thread("T_a".into())));
    // `[` back round the other way.
    press(&mut app, KeyCode::Char('['));
    assert_eq!(view(&app).file, 1);
    // The files: ↑ picks the first, its diff from the top.
    press(&mut app, KeyCode::Tab);
    assert_eq!(view(&app).focus, Focus::Files);
    press(&mut app, KeyCode::Up);
    assert_eq!(view(&app).file, 0);
    assert_eq!(view(&app).cursor, None);
    press(&mut app, KeyCode::Enter);
    assert_eq!(view(&app).focus, Focus::Diff);
}

#[test]
fn a_resolved_thread_opens_while_the_cursor_rests_on_it() {
    let mut app = two_files();
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Tab);
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("✓ resolved · bo: why beta?"), "{shot}");
    assert!(!shot.contains("bo · "), "{shot}");
    press(&mut app, KeyCode::Char(']'));
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("bo · "), "{shot}");
    assert!(shot.contains("resolved"), "{shot}");
}

#[test]
fn the_page_keys_and_the_wheel_move_through_a_long_diff() {
    let body: String = (1..=80).map(|n| format!("+line {n}\n")).collect();
    let diff =
        format!("diff --git a/c.rs b/c.rs\n--- a/c.rs\n+++ b/c.rs\n@@ -0,0 +1,80 @@\n{body}");
    let mut app = app_on(vec![file("c.rs", &diff)], Vec::new());
    screen(&mut app, 120, 30);
    press(&mut app, KeyCode::PageDown);
    let shot = screen(&mut app, 120, 30);
    assert!(view(&app).scroll > 0, "the cursor took the pane with it");
    assert!(!shot.contains("line 1 "), "{shot}");
    press(&mut app, KeyCode::End);
    let shot = screen(&mut app, 120, 30);
    assert!(shot.contains("line 80"), "{shot}");
    press(&mut app, KeyCode::Home);
    screen(&mut app, 120, 30);
    assert_eq!(view(&app).scroll, 0);
    let area = view(&app).diff_area;
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: area.x + 2,
        row: area.y + 2,
        modifiers: KeyModifiers::NONE,
    };
    handle_mouse(&mut app, wheel);
    screen(&mut app, 120, 30);
    assert_eq!(view(&app).scroll, WHEEL_LINES);
}

#[test]
fn a_thread_on_a_file_gone_from_the_diff_keeps_a_row() {
    let mut threads = threads();
    threads.push(thread(
        "T_c",
        "gone.rs",
        4,
        vec![said("C_9", "ada", "still?", false, false)],
    ));
    let mut app = app_on(vec![file("a.rs", A_DIFF)], threads);
    let files: Vec<&str> = view(&app)
        .file_list()
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(files, ["a.rs", "b.rs", "gone.rs"]);
    for _ in 0..3 {
        press(&mut app, KeyCode::Char(']'));
    }
    assert_eq!(view(&app).file, 2);
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains(GONE), "{shot}");
    assert!(shot.contains("still?"), "{shot}");
}

#[test]
fn answers_land_on_the_screen_that_asked_and_say_what_failed() {
    let mut app = app_on(Vec::new(), Vec::new());
    let ticket = view(&app).ticket;
    // An answer for a screen since closed is dropped.
    land(
        &mut app,
        Answer::Diff {
            ticket: ticket + 1000,
            result: Ok(vec![file("a.rs", A_DIFF)]),
        },
    );
    assert!(view(&app).file_list().is_empty());
    land(
        &mut app,
        Answer::Threads {
            ticket,
            url: URL.into(),
            result: Ok(github::Threads {
                pr_id: "PR_1".into(),
                mine: true,
                pending: Some(github::PendingReview {
                    id: "PRR_9".into(),
                    body: String::new(),
                }),
                threads: threads(),
            }),
        },
    );
    let v = view(&app);
    assert_eq!(v.pr_id.as_deref(), Some("PR_1"));
    assert_eq!(v.pending.as_deref(), Some("PRR_9"));
    assert!(v.own, "GitHub says whose it is");
    assert_eq!(v.threads.len(), 2);
    land(
        &mut app,
        Answer::Threads {
            ticket,
            url: URL.into(),
            result: Err("GraphQL: no".into()),
        },
    );
    let flash = format!("{:?}", app.flash);
    assert!(
        flash.contains("couldn't read #7's review threads: GraphQL: no"),
        "{flash}"
    );
    land(
        &mut app,
        Answer::Diff {
            ticket,
            result: Err("no history in common with main".into()),
        },
    );
    let shot = screen(&mut app, 120, 30);
    assert!(
        shot.contains("couldn't read the diff: no history in common with main"),
        "{shot}"
    );
}

#[test]
fn opening_waits_for_the_page_and_seeds_its_threads() {
    let mut app = App::new();
    let view = ReviewScreen::new(URL.into(), 7, "#7".into(), "/nonexistent".into(), false);
    open(&mut app, view.clone());
    assert!(app.overlay.is_none());
    assert!(format!("{:?}", app.flash).contains("#7 is still loading"));
    app.pr_detail.insert(
        URL.into(),
        crate::pull_request::PrDetail {
            url: URL.into(),
            number: 7,
            base: "main".into(),
            head_sha: "abc123".into(),
            threads: threads(),
            ..Default::default()
        },
    );
    open(&mut app, view);
    assert_eq!(
        self::view(&app).threads.len(),
        2,
        "the page's threads, at once"
    );
    assert!(self::view(&app).reading, "and asked for again");
    // Opened from the pane, Esc goes back to the panels.
    press(&mut app, KeyCode::Esc);
    assert!(app.overlay.is_none());
}

/// A repo with `a.txt` on `main` and a branch that changes it and adds
/// `b.txt`: the branch's head.
fn repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(args)
            .output()
            .unwrap();
        assert!(ok.status.success(), "git {args:?}: {ok:?}");
        String::from_utf8_lossy(&ok.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "t@orion.dev"]);
    git(&["config", "user.name", "orion-test"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "base"]);
    git(&["switch", "-q", "-c", "feat"]);
    std::fs::write(dir.path().join("a.txt"), "one\ndeux\nthree\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "feature"]);
    let head = git(&["rev-parse", "HEAD"]);
    git(&["switch", "-q", "main"]);
    (dir, head)
}

#[test]
fn the_diff_is_read_from_git_from_where_the_branch_left_its_base() {
    let (dir, head) = repo();
    let quit = std::sync::atomic::AtomicBool::new(false);
    let files = read_files(dir.path(), 7, "main", &head, &quit).unwrap();
    let paths: Vec<(&str, char)> = files.iter().map(|f| (f.path.as_str(), f.status)).collect();
    assert_eq!(paths, [("a.txt", 'M'), ("b.txt", 'A')]);
    let a = &files[0];
    assert_eq!(
        a.lines,
        Some(LineChanges {
            added: 1,
            removed: 1
        })
    );
    let deux = (0..a.doc.lines.len())
        .find(|i| layout::side_line(&a.doc, *i) == Some((Side::Right, 2)))
        .unwrap();
    assert!(layout::is_code(&a.doc, deux));
}

#[test]
fn the_hints_come_from_the_keys_table() {
    let mut app = two_files();
    for _ in 0..2 {
        crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Char(']'));
    crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);
    press(&mut app, KeyCode::Char('r'));
    crate::hints::assert_hints_from(&hints(view(&app)), keys::ALL);
}

// ---- writing ----

fn chars(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn shift(app: &mut App, code: KeyCode) {
    handle_key(app, KeyEvent::new(code, KeyModifiers::SHIFT));
}

/// A write landing on the screen as GitHub answered it.
fn wrote(app: &mut App, result: Result<write::Wrote, String>) {
    let ticket = view(app).ticket;
    land(
        app,
        Answer::Wrote {
            ticket,
            number: 7,
            what: "do it",
            result,
        },
    );
}

fn sent() -> Vec<write::Write> {
    write::tests::take_sent()
}

#[test]
fn c_writes_a_comment_in_place_under_its_line_and_github_holds_it() {
    let mut app = two_files();
    sent();
    // `one`, `two`, then `deux` — the line ada's thread is on.
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Char('c'));
    chars(&mut app, "nit: deux");
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Comment on line 2"), "{shot}");
    // Under the line, after the thread already on it.
    assert!(
        row_of(&shot, "will do") < row_of(&shot, "Comment on line 2"),
        "{shot}"
    );
    assert!(
        row_of(&shot, "Comment on line 2") < row_of(&shot, "nit: deux"),
        "{shot}"
    );
    assert!(shot.contains("Enter add to review"), "{shot}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        sent(),
        [write::Write::Comment(github::NewThread {
            path: "a.rs".into(),
            side: Side::Right,
            line: 2,
            start: None,
            body: "nit: deux".into(),
        })]
    );
    assert!(view(&app).saving.is_some());
    // One write at a time.
    press(&mut app, KeyCode::Enter);
    assert!(sent().is_empty());
    assert!(format!("{:?}", app.flash).contains("one moment"));

    let made = thread(
        "T_new",
        "a.rs",
        2,
        vec![said("C_new", "me", "nit: deux", true, true)],
    );
    wrote(
        &mut app,
        Ok(write::Wrote::Comment {
            review: "PRR_new".into(),
            thread: made,
        }),
    );
    let v = view(&app);
    assert!(v.composer.is_none() && v.saving.is_none());
    assert_eq!(v.pending.as_deref(), Some("PRR_new"));
    assert_eq!(v.cursor, Some(Stop::Thread("T_new".into())));
    assert!(format!("{:?}", app.flash).contains("held in your pending review"));
    let shot = screen(&mut app, 140, 30);
    let a = shot.lines().nth(row_of(&shot, "M a.rs")).unwrap();
    assert!(a.contains("● 2"), "{a}");
}

#[test]
fn shift_arrows_mark_a_run_of_lines_for_one_comment() {
    let mut app = two_files();
    sent();
    shift(&mut app, KeyCode::Down);
    shift(&mut app, KeyCode::Down);
    let marked = hints(view(&app));
    assert!(
        marked.iter().any(|h| h.does == "comment on 3 lines"),
        "{marked:?}"
    );
    assert!(
        marked.iter().any(|h| h.does == "clear the lines"),
        "{marked:?}"
    );
    press(&mut app, KeyCode::Char('c'));
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Comment on lines 1–2"), "{shot}");
    chars(&mut app, "all of it");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        sent(),
        [write::Write::Comment(github::NewThread {
            path: "a.rs".into(),
            side: Side::Right,
            line: 2,
            start: Some((Side::Right, 1)),
            body: "all of it".into(),
        })]
    );
    // Esc puts the comment away, then lets the marked lines go, before it
    // leaves.
    wrote(&mut app, Err("GraphQL: no".into()));
    press(&mut app, KeyCode::Esc);
    assert!(view(&app).composer.is_none());
    assert!(view(&app).anchor.is_some());
    press(&mut app, KeyCode::Esc);
    assert!(view(&app).anchor.is_none());
    assert!(app.overlay.is_some());
}

#[test]
fn r_replies_x_resolves_and_backspace_drops_your_pending_comment() {
    let mut app = two_files();
    sent();
    press(&mut app, KeyCode::Char(']'));
    assert_eq!(view(&app).cursor, Some(Stop::Thread("T_a".into())));
    press(&mut app, KeyCode::Char('r'));
    chars(&mut app, "ok");
    let shot = screen(&mut app, 140, 30);
    assert!(row_of(&shot, "will do") < row_of(&shot, "Reply"), "{shot}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        sent(),
        [write::Write::Reply {
            thread: "T_a".into(),
            body: "ok".into()
        }]
    );
    wrote(
        &mut app,
        Ok(write::Wrote::Reply {
            review: "PRR_9".into(),
            thread: "T_a".into(),
            comment: said("C_new", "me", "ok", true, true),
        }),
    );
    assert_eq!(view(&app).thread_at_cursor().unwrap().comments.len(), 3);

    press(&mut app, KeyCode::Char('x'));
    assert_eq!(
        sent(),
        [write::Write::Resolve {
            thread: "T_a".into(),
            resolve: true
        }]
    );
    wrote(
        &mut app,
        Ok(write::Wrote::Resolved {
            thread: "T_a".into(),
            resolved: true,
        }),
    );
    assert!(view(&app).thread_at_cursor().unwrap().resolved);
    assert!(hints(view(&app)).iter().any(|h| h.does == "reopen"));

    press(&mut app, KeyCode::Backspace);
    assert_eq!(
        sent(),
        [write::Write::Drop {
            thread: "T_a".into(),
            comment: "C_new".into()
        }]
    );
    wrote(
        &mut app,
        Ok(write::Wrote::Dropped {
            thread: "T_a".into(),
            comment: "C_new".into(),
        }),
    );
    assert_eq!(view(&app).thread_at_cursor().unwrap().comments.len(), 2);

    // bo's thread holds nothing of yours to drop.
    press(&mut app, KeyCode::Char(']'));
    press(&mut app, KeyCode::Backspace);
    assert!(sent().is_empty());
    assert!(format!("{:?}", app.flash).contains("this thread has none"));
}

#[test]
fn a_refused_write_keeps_what_was_typed_and_says_why() {
    let mut app = two_files();
    sent();
    press(&mut app, KeyCode::Char('c'));
    assert!(paste(&mut app, "first\nsecond"));
    press(&mut app, KeyCode::Enter);
    assert_eq!(sent().len(), 1);
    wrote(&mut app, Err("GraphQL: Line could not be resolved".into()));
    let v = view(&app);
    assert_eq!(v.composer.as_ref().unwrap().input.as_str(), "first\nsecond");
    assert!(v.saving.is_none());
    let flash = format!("{:?}", app.flash);
    assert!(
        flash.contains("couldn't do it: GraphQL: Line could not be resolved"),
        "{flash}"
    );

    // Nothing is sent until GitHub has said which pull request this is.
    if let Some(Overlay::Review(v)) = &mut app.overlay {
        v.pr_id = None;
    }
    press(&mut app, KeyCode::Enter);
    assert!(sent().is_empty());
    assert!(format!("{:?}", app.flash).contains("still reading the pull request"));

    // An empty comment goes nowhere.
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('c'));
    press(&mut app, KeyCode::Enter);
    assert!(sent().is_empty());
    assert!(format!("{:?}", app.flash).contains("write something first"));

    // A write landing once the screen is gone still says what it did.
    let ticket = view(&app).ticket;
    app.overlay = None;
    land(
        &mut app,
        Answer::Wrote {
            ticket,
            number: 7,
            what: "resolve the thread",
            result: Ok(write::Wrote::Resolved {
                thread: "T_a".into(),
                resolved: true,
            }),
        },
    );
    assert!(format!("{:?}", app.flash).contains("#7: thread resolved"));
}
