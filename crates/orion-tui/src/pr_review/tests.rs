use super::github::{ReviewThread, Side, ThreadComment};
use super::*;
use crate::app::{DiffFocus, DiffView, Overlay};
use crate::git_diff::{DiffFile, LineChanges};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

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

/// An app with a pull request's DIFF VIEWER up on `files`, its review
/// read: `threads` on them, the keys on the diff.
fn app_on(files: Vec<ReviewFile>, threads: Vec<ReviewThread>) -> App {
    let mut app = App::new();
    let mut review = ReviewScreen::new(
        URL.into(),
        7,
        "#7 Add things".into(),
        "/nonexistent".into(),
        false,
    );
    review.threads = threads;
    review.reading = false;
    review.pr_id = Some("PR_1".into());
    let listed = files
        .iter()
        .map(|f| DiffFile {
            path: f.path.clone(),
            orig_path: None,
            xy: [f.status, ' '],
            lines: f.lines,
        })
        .collect();
    review.take_files(Ok(files));
    let mut diff = DiffView::new("/nonexistent".into(), "#7 Add things".into(), listed, true);
    diff.review = Some(Box::new(review));
    diff.focus = DiffFocus::Diff;
    viewer::sync(&mut diff);
    app.overlay = Some(Overlay::Diff(diff));
    app
}

fn two_files() -> App {
    app_on(vec![file("a.rs", A_DIFF), file("b.rs", B_DIFF)], threads())
}

/// The viewer up.
fn diff(app: &App) -> &DiffView {
    match &app.overlay {
        Some(Overlay::Diff(v)) => v,
        other => panic!("expected the viewer, got {other:?}"),
    }
}

/// Its review.
fn view(app: &App) -> &ReviewScreen {
    diff(app).review.as_deref().expect("the review")
}

fn review(app: &mut App) -> &mut ReviewScreen {
    viewer::review_mut(app).expect("the review")
}

/// The viewer's keys at the bottom edge.
fn hints(app: &App) -> Vec<crate::hints::Hint> {
    crate::ui::diff_hints(diff(app))
}

/// Every key the viewer's bottom edge may name.
fn every_key() -> Vec<crate::hints::Key> {
    let mut all = keys::ALL.to_vec();
    all.extend_from_slice(crate::ui::diff_keys::ALL_KEYS);
    all
}

fn key_in(app: &mut App, key: KeyEvent) {
    crate::event_loop::handle_overlay_key(app, key, &mut Vec::new());
    if let Some(Overlay::Diff(v)) = &mut app.overlay {
        viewer::sync(v);
    }
}

fn press(app: &mut App, code: KeyCode) {
    key_in(app, KeyEvent::new(code, KeyModifiers::NONE));
}

fn screen(app: &mut App, w: u16, h: u16) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| crate::ui::draw(f, app)).unwrap();
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
    assert!(shot.contains("Your review"), "the bar: {shot}");
    // a.rs: an open thread with a pending comment of yours in it; b.rs:
    // one, resolved.
    let a = shot.lines().nth(row_of(&shot, "a.rs ")).unwrap();
    assert!(a.contains("● ✎1"), "{a}");
    let b = shot.lines().nth(row_of(&shot, "b.rs ")).unwrap();
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
    assert_eq!(
        diff(&app).selected_path(),
        Some("b.rs"),
        "the viewer went too"
    );
    // The files: ↑ picks the first, its diff from the top.
    press(&mut app, KeyCode::BackTab);
    assert_eq!(diff(&app).focus, DiffFocus::Files);
    press(&mut app, KeyCode::Up);
    assert_eq!(view(&app).file, 0);
    assert_eq!(view(&app).cursor, None);
    // `←`/`→` never cross a panel's edge: `Tab` / `⇧Tab` do.
    press(&mut app, KeyCode::Right);
    assert_eq!(diff(&app).focus, DiffFocus::Files, "→ stays on the files");
    press(&mut app, KeyCode::Tab);
    assert_eq!(diff(&app).focus, DiffFocus::Diff);
    press(&mut app, KeyCode::Left);
    assert_eq!(diff(&app).focus, DiffFocus::Diff, "← stays in the diff");
}

#[test]
fn a_resolved_thread_opens_while_the_cursor_rests_on_it() {
    let mut app = two_files();
    press(&mut app, KeyCode::BackTab);
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
    assert!(viewer::handle_mouse(&mut app, wheel));
    screen(&mut app, 120, 30);
    assert_eq!(view(&app).scroll, WHEEL_LINES);
}

/// A thread on a file the pull request no longer changes: the review
/// keeps it, but the viewer lists no such file, so `]` passes it by.
#[test]
fn a_thread_on_a_file_gone_from_the_diff_is_passed_by() {
    let mut threads = threads();
    threads.push(thread(
        "T_c",
        "gone.rs",
        4,
        vec![said("C_9", "ada", "still?", false, false)],
    ));
    let mut app = app_on(vec![file("a.rs", A_DIFF), file("b.rs", B_DIFF)], threads);
    let files: Vec<&str> = view(&app)
        .file_list()
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(files, ["a.rs", "b.rs", "gone.rs"]);
    for want in ["T_a", "T_b", "T_a"] {
        press(&mut app, KeyCode::Char(']'));
        assert_eq!(view(&app).cursor, Some(Stop::Thread(want.into())));
    }
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
    let flash = format!("{:?}", app.flash);
    assert!(
        flash.contains(
            "couldn't read #7's diff for its review threads: no history in common with main"
        ),
        "{flash}"
    );
}

/// The review goes into a pull request's viewer once its page is in —
/// its head and base are what the diff is read between — the page's
/// threads at once, and GitHub asked again.
#[test]
fn attaching_waits_for_the_page_and_seeds_its_threads() {
    let mut app = App::new();
    let mut diff = DiffView::new("/nonexistent".into(), "#7".into(), Vec::new(), true);
    attach(
        &app,
        &mut diff,
        7,
        URL,
        "#7",
        std::path::Path::new("/nonexistent"),
    );
    assert!(diff.review.is_none());
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
    attach(
        &app,
        &mut diff,
        7,
        URL,
        "#7",
        std::path::Path::new("/nonexistent"),
    );
    let review = diff.review.as_deref().expect("the review");
    assert_eq!(review.threads.len(), 2, "the page's threads, at once");
    assert!(review.reading, "and asked for again");
    assert_eq!(
        diff.next_focus(false),
        DiffFocus::Bar,
        "⇧Tab from the files: the bar"
    );
}

/// A repo with `a.txt` on `main` and a branch that changes it and adds
/// `b.txt`: the branch's head.
pub(crate) fn repo() -> (tempfile::TempDir, String) {
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
    let all = every_key();
    // The diff, the bar, the files, and round to the diff.
    for _ in 0..3 {
        crate::hints::assert_hints_from(&hints(&app), &all);
        press(&mut app, KeyCode::Tab);
    }
    press(&mut app, KeyCode::Char(']'));
    crate::hints::assert_hints_from(&hints(&app), &all);
    press(&mut app, KeyCode::Char('r'));
    crate::hints::assert_hints_from(&hints(&app), &all);
}

// ---- writing ----

fn chars(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn shift(app: &mut App, code: KeyCode) {
    key_in(app, KeyEvent::new(code, KeyModifiers::SHIFT));
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
    let a = shot.lines().nth(row_of(&shot, "a.rs ")).unwrap();
    assert!(a.contains("● ✎2"), "{a}");
}

#[test]
fn shift_arrows_mark_a_run_of_lines_for_one_comment() {
    let mut app = two_files();
    sent();
    shift(&mut app, KeyCode::Down);
    shift(&mut app, KeyCode::Down);
    let marked = hints(&app);
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
    assert!(hints(&app).iter().any(|h| h.does == "reopen"));

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
    assert!(viewer::paste(&mut app, "first\nsecond"));
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
    review(&mut app).pr_id = None;
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

// ---- the verdict and the submit ----

fn submit_key() -> KeyEvent {
    KeyEvent::new(
        KeyCode::Char('r'),
        KeyModifiers::SUPER | KeyModifiers::SHIFT,
    )
}

fn submitted() -> Vec<(github::Verdict, Option<String>, String)> {
    bar::tests::take_submitted()
}

/// The review landing as GitHub answered it.
fn sent_review(app: &mut App, verdict: github::Verdict, result: Result<(), String>) {
    let ticket = view(app).ticket;
    land(
        app,
        Answer::Submitted {
            ticket,
            number: 7,
            url: URL.into(),
            verdict,
            result,
        },
    );
}

#[test]
fn the_bar_picks_a_verdict_and_submit_sends_it_with_everything_pending() {
    let mut app = two_files();
    submitted();
    review(&mut app).pending = Some("PRR_9".into());
    press(&mut app, KeyCode::Tab);
    assert_eq!(diff(&app).focus, DiffFocus::Bar);
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Your review"), "{shot}");
    assert!(
        shot.contains("● Comment   ○ Approve   ○ Request changes"),
        "{shot}"
    );
    assert!(shot.contains("1 pending comment"), "{shot}");
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right);
    assert_eq!(view(&app).verdict, github::Verdict::RequestChanges);
    // A letter on the verdict starts the summary.
    chars(&mut app, "Looks close");
    assert_eq!(view(&app).bar_row, bar::BarRow::Summary);
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Looks close"), "{shot}");
    assert!(shot.contains("Enter submit review"), "{shot}");
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        submitted(),
        [(
            github::Verdict::RequestChanges,
            Some("PRR_9".into()),
            "Looks close".into()
        )]
    );
    // Refused: the bar says why, and nothing is lost.
    sent_review(
        &mut app,
        github::Verdict::RequestChanges,
        Err("GraphQL: Can not request changes on your own pull request".into()),
    );
    let v = view(&app);
    assert!(v.saving.is_none());
    assert_eq!(v.summary.as_str(), "Looks close");
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("Can not request changes"), "{shot}");
    // ⌘⇧R from anywhere sends it again; once GitHub takes it the screen
    // gives way to the pull request's Reviews tab, its page read again.
    press(&mut app, KeyCode::Tab);
    assert_eq!(diff(&app).focus, DiffFocus::Files);
    key_in(&mut app, submit_key());
    assert_eq!(submitted().len(), 1);
    sent_review(&mut app, github::Verdict::RequestChanges, Ok(()));
    assert!(app.overlay.is_none());
    assert!(format!("{:?}", app.flash).contains("requested changes on #7"));
    assert!(app.pr_detail_stale.contains(URL));
    assert_eq!(app.pr_tabs.tab, crate::pr_preview::PrTab::Reviews);
}

#[test]
fn on_your_own_pull_request_only_a_comment_goes() {
    let mut app = two_files();
    let ticket = view(&app).ticket;
    review(&mut app).verdict = github::Verdict::Approve;
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
                    body: "begun on github.com".into(),
                }),
                threads: threads(),
            }),
        },
    );
    assert_eq!(view(&app).verdict, github::Verdict::Comment);
    // What was written into the pending review is carried on from.
    assert_eq!(view(&app).summary.as_str(), "begun on github.com");
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Right);
    assert_eq!(view(&app).verdict, github::Verdict::Comment);
    let shot = screen(&mut app, 140, 30);
    assert!(shot.contains("yours, so GitHub takes a comment"), "{shot}");
    assert_eq!(
        view(&app).verdict_rects.len(),
        1,
        "only Comment is a target"
    );
}

#[test]
fn a_review_with_nothing_said_is_refused_before_github_is_asked() {
    let mut app = app_on(vec![file("a.rs", A_DIFF)], Vec::new());
    submitted();
    key_in(&mut app, submit_key());
    assert!(submitted().is_empty());
    assert_eq!(diff(&app).focus, DiffFocus::Bar);
    let v = view(&app);
    assert_eq!((v.focus, v.bar_row), (Focus::Bar, bar::BarRow::Summary));
    assert!(v
        .notice
        .as_deref()
        .is_some_and(|n| n.contains("Comment needs something said")));
    // An approval needs no words.
    press(&mut app, KeyCode::Up);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    assert_eq!(
        submitted(),
        [(github::Verdict::Approve, None, String::new())]
    );
}

#[test]
fn a_comment_still_being_written_is_finished_first() {
    let mut app = two_files();
    submitted();
    press(&mut app, KeyCode::Char('c'));
    key_in(&mut app, submit_key());
    assert!(submitted().is_empty());
    assert!(format!("{:?}", app.flash).contains("finish the comment first"));
    assert!(view(&app).composer.is_some());
}

#[test]
fn a_review_sent_from_the_modal_lands_on_its_reviews_tab() {
    let mut app = two_files();
    if let Some(Overlay::Diff(v)) = &mut app.overlay {
        v.back = Some(Box::new(crate::pr_modal::PullRequestsView::new(
            orion_core::ProjectId("p1".into()),
            "demo".into(),
            "/nonexistent".into(),
        )));
    }
    sent_review(&mut app, github::Verdict::Approve, Ok(()));
    let Some(Overlay::PullRequests(modal)) = &app.overlay else {
        panic!("not the modal: {:?}", app.overlay);
    };
    assert_eq!(modal.tabs.tab, crate::pr_preview::PrTab::Reviews);
    assert!(format!("{:?}", app.flash).contains("approved #7"));
}
