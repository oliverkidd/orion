//! The PR REVIEW's GitHub half: a pull request's review threads, read
//! with the rest of its page, and the review written into GitHub's own
//! *pending* review — the one github.com keeps while you comment and
//! sends when you submit.
//!
//! * **Read** ([`read`]): one GraphQL query through `gh api graphql`, its
//!   `$owner` and `$repo` filled in by `gh` from the checkout, as every
//!   other query orion asks (`pull_request::repo_graphql`). It answers the
//!   pull request's node id, whether it is yours, your pending review (and
//!   its summary so far) and every review thread with its comments — each
//!   comment yours or not, pending or sent.
//! * **Write**: one mutation each, its request body — the query and its
//!   variables, as JSON — on `gh`'s stdin (`--input -`), never in argv: a
//!   comment is markdown from a person's keyboard. A comment or a reply
//!   joins the pending review, which the first one starts
//!   (`addPullRequestReview` with no verdict); resolving a thread is
//!   immediate; a pending comment of yours can be deleted; and the review
//!   is submitted with a verdict and a summary — the pending one, or, with
//!   nothing pending, a new review sent at once.
//!
//! Every call runs off the loop and answers `Result<_, String>`: the
//! error is `gh`'s own first line (`GraphQL: …`), for the footer.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::pull_request::{bool_at, login, run_piped, str_at};

/// How long one call may take before it is given up on.
pub(crate) const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The diff side a line is on: the old file's (`LEFT`, a removed line) or
/// the new file's (`RIGHT`, an added or unchanged one).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    #[default]
    Right,
}

impl Side {
    /// GraphQL's `DiffSide`.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        }
    }

    fn parse(word: &str) -> Self {
        if word == "LEFT" {
            Side::Left
        } else {
            Side::Right
        }
    }
}

/// One comment in a review thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadComment {
    pub id: String,
    pub author: String,
    pub body: String,
    /// RFC 3339, as GitHub gives it.
    pub at: String,
    /// Written by whoever `gh` is signed in as.
    pub mine: bool,
    /// In your pending review: nobody else sees it until it is submitted.
    pub pending: bool,
}

/// One review thread: comments on a line, or a run of lines, of a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewThread {
    pub id: String,
    pub path: String,
    /// The line it is on in the diff as it is now, on [`ReviewThread::side`];
    /// None once a push has moved the code out from under it (outdated)
    /// or for a comment on the whole file.
    pub line: Option<u32>,
    /// The first line of a comment on a run of lines.
    pub start_line: Option<u32>,
    /// The line it was written on, in the diff of its day.
    pub original_line: Option<u32>,
    pub side: Side,
    pub resolved: bool,
    pub outdated: bool,
    pub comments: Vec<ThreadComment>,
}

impl ReviewThread {
    /// The line to name it by: where it is, else where it was written.
    pub fn shown_line(&self) -> Option<u32> {
        self.line.or(self.original_line)
    }

    /// `line 12`, `lines 12–14`, `the file`.
    pub fn place(&self) -> String {
        match (self.start_line, self.shown_line()) {
            (Some(start), Some(end)) if start < end => format!("lines {start}–{end}"),
            (_, Some(line)) => format!("line {line}"),
            (_, None) => "the file".to_string(),
        }
    }

    /// Whoever started it is the one signed in.
    pub fn mine(&self) -> bool {
        self.comments.first().is_some_and(|c| c.mine)
    }

    /// The last of your pending comments in it: what `⌫` drops.
    pub fn my_pending(&self) -> Option<&ThreadComment> {
        self.comments.iter().rev().find(|c| c.mine && c.pending)
    }

    /// How many of its comments are your pending ones.
    pub fn pending_count(&self) -> usize {
        self.comments.iter().filter(|c| c.mine && c.pending).count()
    }
}

/// Your pending review on a pull request: what the next comment joins and
/// a submit sends.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingReview {
    pub id: String,
    /// Its summary so far — typed on github.com, perhaps.
    pub body: String,
}

/// What [`read`] answers about a pull request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Threads {
    /// Its node id, for the mutations.
    pub pr_id: String,
    /// You opened it: GitHub takes only a comment from its author.
    pub mine: bool,
    pub pending: Option<PendingReview>,
    pub threads: Vec<ReviewThread>,
}

/// What a submit says about the pull request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Verdict {
    #[default]
    Comment,
    Approve,
    RequestChanges,
}

impl Verdict {
    pub const ALL: [Verdict; 3] = [Verdict::Comment, Verdict::Approve, Verdict::RequestChanges];

    /// GraphQL's `PullRequestReviewEvent`.
    pub fn event(self) -> &'static str {
        match self {
            Verdict::Comment => "COMMENT",
            Verdict::Approve => "APPROVE",
            Verdict::RequestChanges => "REQUEST_CHANGES",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Comment => "Comment",
            Verdict::Approve => "Approve",
            Verdict::RequestChanges => "Request changes",
        }
    }

    /// What the footer says once it is sent: `approved #42`.
    pub fn done(self, number: u64) -> String {
        match self {
            Verdict::Comment => format!("reviewed #{number}"),
            Verdict::Approve => format!("approved #{number}"),
            Verdict::RequestChanges => format!("requested changes on #{number}"),
        }
    }

    /// Whether GitHub takes it on a pull request you opened yourself: only
    /// a comment.
    pub fn allowed(self, own: bool) -> bool {
        !own || self == Verdict::Comment
    }
}

/// A new comment, on `line` of `path` (on `side`), or on the run of lines
/// from `start` to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewThread {
    pub path: String,
    pub side: Side,
    pub line: u32,
    pub start: Option<(Side, u32)>,
    pub body: String,
}

// ---- the requests ----

/// The fields of a comment, wherever one is asked for.
const COMMENT: &str = "id author { login } body createdAt viewerDidAuthor state";

/// The fields of a thread, wherever one is asked for.
fn thread_fields() -> String {
    format!(
        "id isResolved isOutdated path line startLine originalLine diffSide \
         comments(first: 100) {{ nodes {{ {COMMENT} }} }}"
    )
}

/// The query [`read`] asks.
pub(crate) fn threads_query() -> String {
    format!(
        "query($owner: String!, $repo: String!, $number: Int!) {{ \
         repository(owner: $owner, name: $repo) {{ pullRequest(number: $number) {{ \
         id viewerDidAuthor \
         reviews(states: [PENDING], first: 10) {{ nodes {{ id body viewerDidAuthor }} }} \
         reviewThreads(first: 100) {{ nodes {{ {} }} }} }} }} }}",
        thread_fields()
    )
}

/// `gh api graphql`'s arguments for [`threads_query`] on pull request
/// `number`: `$owner` and `$repo` from the checkout `gh` runs in.
pub(crate) fn threads_args(number: u64) -> Vec<String> {
    [
        "api",
        "graphql",
        "-F",
        "owner={owner}",
        "-F",
        "repo={repo}",
        "-F",
        &format!("number={number}"),
        "-f",
        &format!("query={}", threads_query()),
    ]
    .map(str::to_string)
    .to_vec()
}

/// One mutation's request body.
fn request(query: &str, variables: Value) -> Value {
    json!({ "query": query, "variables": variables })
}

/// Start your pending review on the pull request: a review with no verdict.
pub(crate) fn start_review_request(pr_id: &str) -> Value {
    request(
        "mutation($pr: ID!) { addPullRequestReview(input: {pullRequestId: $pr}) \
         { pullRequestReview { id } } }",
        json!({ "pr": pr_id }),
    )
}

/// A new thread in pending review `review`.
pub(crate) fn add_thread_request(review: &str, new: &NewThread) -> Value {
    let query = format!(
        "mutation($review: ID!, $path: String!, $body: String!, $line: Int!, \
         $side: DiffSide!, $startLine: Int, $startSide: DiffSide) {{ \
         addPullRequestReviewThread(input: {{pullRequestReviewId: $review, path: $path, \
         body: $body, line: $line, side: $side, startLine: $startLine, startSide: $startSide}}) \
         {{ thread {{ {} }} }} }}",
        thread_fields()
    );
    request(
        &query,
        json!({
            "review": review,
            "path": new.path,
            "body": new.body,
            "line": new.line,
            "side": new.side.as_str(),
            "startLine": new.start.map(|(_, line)| line),
            "startSide": new.start.map(|(side, _)| side.as_str()),
        }),
    )
}

/// A reply to `thread`, in pending review `review`.
pub(crate) fn reply_request(review: &str, thread: &str, body: &str) -> Value {
    let query = format!(
        "mutation($review: ID!, $thread: ID!, $body: String!) {{ \
         addPullRequestReviewThreadReply(input: {{pullRequestReviewId: $review, \
         pullRequestReviewThreadId: $thread, body: $body}}) {{ comment {{ {COMMENT} }} }} }}"
    );
    request(
        &query,
        json!({ "review": review, "thread": thread, "body": body }),
    )
}

/// The mutation that resolves a thread, or unresolves it.
fn resolve_verb(resolve: bool) -> &'static str {
    if resolve {
        "resolveReviewThread"
    } else {
        "unresolveReviewThread"
    }
}

/// Resolve `thread`, or unresolve it.
pub(crate) fn resolve_request(thread: &str, resolve: bool) -> Value {
    let verb = resolve_verb(resolve);
    request(
        &format!(
            "mutation($thread: ID!) {{ {verb}(input: {{threadId: $thread}}) \
             {{ thread {{ id isResolved }} }} }}"
        ),
        json!({ "thread": thread }),
    )
}

/// Delete pending comment `comment`.
pub(crate) fn delete_request(comment: &str) -> Value {
    request(
        "mutation($comment: ID!) { deletePullRequestReviewComment(input: {id: $comment}) \
         { clientMutationId } }",
        json!({ "comment": comment }),
    )
}

/// Send the review: pending review `pending` with `verdict` and `body`,
/// or — nothing pending — a new review on pull request `pr_id`, sent at
/// once. An empty summary is no summary.
pub(crate) fn submit_request(
    pr_id: &str,
    pending: Option<&str>,
    verdict: Verdict,
    body: &str,
) -> Value {
    let body = (!body.trim().is_empty()).then_some(body);
    match pending {
        Some(review) => request(
            "mutation($review: ID!, $event: PullRequestReviewEvent!, $body: String) { \
             submitPullRequestReview(input: {pullRequestReviewId: $review, event: $event, \
             body: $body}) { pullRequestReview { id state } } }",
            json!({ "review": review, "event": verdict.event(), "body": body }),
        ),
        None => request(
            "mutation($pr: ID!, $event: PullRequestReviewEvent!, $body: String) { \
             addPullRequestReview(input: {pullRequestId: $pr, event: $event, body: $body}) \
             { pullRequestReview { id state } } }",
            json!({ "pr": pr_id, "event": verdict.event(), "body": body }),
        ),
    }
}

// ---- the answers ----

fn line_at(v: &Value, key: &str) -> Option<u32> {
    v.get(key)?.as_u64().and_then(|n| u32::try_from(n).ok())
}

fn nodes<'a>(v: &'a Value, pointer: &str) -> &'a [Value] {
    v.pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// One comment node; one with no id is dropped.
pub(crate) fn parse_comment(v: &Value) -> Option<ThreadComment> {
    let id = str_at(v, "id");
    (!id.is_empty()).then(|| ThreadComment {
        id,
        author: login(v.get("author")),
        body: str_at(v, "body"),
        at: str_at(v, "createdAt"),
        mine: bool_at(v, "viewerDidAuthor"),
        pending: str_at(v, "state") == "PENDING",
    })
}

/// One thread node; one with no id or path is dropped.
pub(crate) fn parse_thread(v: &Value) -> Option<ReviewThread> {
    let id = str_at(v, "id");
    let path = str_at(v, "path");
    if id.is_empty() || path.is_empty() {
        return None;
    }
    Some(ReviewThread {
        id,
        path,
        line: line_at(v, "line"),
        start_line: line_at(v, "startLine"),
        original_line: line_at(v, "originalLine"),
        side: Side::parse(&str_at(v, "diffSide")),
        resolved: bool_at(v, "isResolved"),
        outdated: bool_at(v, "isOutdated"),
        comments: nodes(v, "/comments/nodes")
            .iter()
            .filter_map(parse_comment)
            .collect(),
    })
}

/// [`threads_query`]'s answer. None when it names no pull request.
pub(crate) fn parse_threads(v: &Value) -> Option<Threads> {
    let pr = v.pointer("/data/repository/pullRequest")?;
    let pr_id = str_at(pr, "id");
    if pr_id.is_empty() {
        return None;
    }
    let pending = nodes(pr, "/reviews/nodes")
        .iter()
        .find(|r| bool_at(r, "viewerDidAuthor"))
        .map(|r| PendingReview {
            id: str_at(r, "id"),
            body: str_at(r, "body"),
        })
        .filter(|r| !r.id.is_empty());
    Some(Threads {
        pr_id,
        mine: bool_at(pr, "viewerDidAuthor"),
        pending,
        threads: nodes(pr, "/reviewThreads/nodes")
            .iter()
            .filter_map(parse_thread)
            .collect(),
    })
}

/// What GitHub said went wrong, from a GraphQL answer's `errors`.
fn graphql_error(v: &Value) -> Option<String> {
    let first = v.get("errors")?.as_array()?.first()?;
    Some(match first.get("message").and_then(Value::as_str) {
        Some(message) => format!("GraphQL: {message}"),
        None => "GitHub refused it".to_string(),
    })
}

/// `gh`'s stdout read as a GraphQL answer — or what GitHub said went wrong.
fn answer(out: &str) -> Result<Value, String> {
    let v: Value =
        serde_json::from_str(out).map_err(|_| "GitHub's answer didn't read".to_string())?;
    match graphql_error(&v) {
        Some(err) => Err(err),
        None => Ok(v),
    }
}

// ---- the calls ----

/// `gh <args>` in `dir` — a stub under test, so nothing reaches GitHub.
fn gh(dir: &Path, args: &[&str]) -> tokio::process::Command {
    #[cfg(not(test))]
    let program = std::path::PathBuf::from("gh");
    #[cfg(test)]
    let program = tests::GH.with_borrow(|stub| stub.clone());
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).current_dir(dir);
    cmd
}

/// Pull request `number`'s threads, its pending review and its node id,
/// from `dir`'s repo, within `timeout`.
pub async fn read(
    dir: &Path,
    number: u64,
    timeout: std::time::Duration,
) -> Result<Threads, String> {
    let args = threads_args(number);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = run_piped(gh(dir, &args), "", timeout).await?;
    parse_threads(&answer(&out)?).ok_or_else(|| "GitHub named no such pull request".to_string())
}

/// Send one mutation, its `body` on stdin: the answer's `data`.
async fn mutate(dir: &Path, body: &Value) -> Result<Value, String> {
    let cmd = gh(dir, &["api", "graphql", "-X", "POST", "--input", "-"]);
    let out = run_piped(cmd, &body.to_string(), TIMEOUT).await?;
    answer(&out)?
        .get("data")
        .cloned()
        .ok_or_else(|| "GitHub said nothing back".to_string())
}

/// Your pending review's id: `pending` when there is one, else a new one
/// started on pull request `pr_id`.
async fn pending_review(
    dir: &Path,
    pr_id: &str,
    pending: Option<String>,
) -> Result<String, String> {
    if let Some(id) = pending {
        return Ok(id);
    }
    let data = mutate(dir, &start_review_request(pr_id)).await?;
    let id = data
        .pointer("/addPullRequestReview/pullRequestReview/id")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if id.is_empty() {
        return Err("GitHub started no review".into());
    }
    Ok(id.to_string())
}

/// A new comment in your pending review (started first when there is
/// none): the review's id, and the thread it made.
pub async fn comment(
    dir: &Path,
    pr_id: &str,
    pending: Option<String>,
    new: &NewThread,
) -> Result<(String, ReviewThread), String> {
    let review = pending_review(dir, pr_id, pending).await?;
    let data = mutate(dir, &add_thread_request(&review, new)).await?;
    let thread = data
        .pointer("/addPullRequestReviewThread/thread")
        .and_then(parse_thread)
        .ok_or("GitHub kept no comment")?;
    Ok((review, thread))
}

/// A reply to `thread` in your pending review (started first when there is
/// none): the review's id, and the comment it made.
pub async fn reply(
    dir: &Path,
    pr_id: &str,
    pending: Option<String>,
    thread: &str,
    body: &str,
) -> Result<(String, ThreadComment), String> {
    let review = pending_review(dir, pr_id, pending).await?;
    let data = mutate(dir, &reply_request(&review, thread, body)).await?;
    let comment = data
        .pointer("/addPullRequestReviewThreadReply/comment")
        .and_then(parse_comment)
        .ok_or("GitHub kept no reply")?;
    Ok((review, comment))
}

/// Resolve `thread`, or unresolve it: whether it is resolved now.
pub async fn resolve(dir: &Path, thread: &str, resolve: bool) -> Result<bool, String> {
    let data = mutate(dir, &resolve_request(thread, resolve)).await?;
    data.pointer(&format!("/{}/thread/isResolved", resolve_verb(resolve)))
        .and_then(Value::as_bool)
        .ok_or_else(|| "GitHub didn't say".to_string())
}

/// Delete your pending comment `comment`.
pub async fn delete(dir: &Path, comment: &str) -> Result<(), String> {
    mutate(dir, &delete_request(comment)).await.map(|_| ())
}

/// Send the review ([`submit_request`]).
pub async fn submit(
    dir: &Path,
    pr_id: &str,
    pending: Option<&str>,
    verdict: Verdict,
    body: &str,
) -> Result<(), String> {
    mutate(dir, &submit_request(pr_id, pending, verdict, body))
        .await
        .map(|_| ())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    thread_local! {
        /// What [`gh`] runs here: a stub, so nothing reaches GitHub.
        pub(crate) static GH: RefCell<PathBuf> = RefCell::new(PathBuf::from("/nonexistent/gh"));
    }

    /// One thread node as GitHub answers it.
    pub(crate) fn thread_node(id: &str, path: &str, line: u32, comments: &[Value]) -> Value {
        json!({
            "id": id, "isResolved": false, "isOutdated": false, "path": path,
            "line": line, "startLine": null, "originalLine": line, "diffSide": "RIGHT",
            "comments": { "nodes": comments },
        })
    }

    /// One comment node as GitHub answers it.
    pub(crate) fn comment_node(
        id: &str,
        who: &str,
        body: &str,
        mine: bool,
        pending: bool,
    ) -> Value {
        json!({
            "id": id, "author": { "login": who }, "body": body,
            "createdAt": "2026-11-05T10:00:00Z", "viewerDidAuthor": mine,
            "state": if pending { "PENDING" } else { "SUBMITTED" },
        })
    }

    /// [`threads_query`]'s answer around `threads`.
    pub(crate) fn threads_answer(mine: bool, pending: Option<&str>, threads: &[Value]) -> String {
        let reviews: Vec<Value> = pending
            .map(|id| json!({ "id": id, "body": "so far", "viewerDidAuthor": true }))
            .into_iter()
            .collect();
        json!({ "data": { "repository": { "pullRequest": {
            "id": "PR_1", "viewerDidAuthor": mine,
            "reviews": { "nodes": reviews },
            "reviewThreads": { "nodes": threads },
        } } } })
        .to_string()
    }

    /// Run `f` with `gh` a stub that logs each call's arguments and stdin
    /// and answers each from `answers` — `(what the call's arguments or
    /// body hold, its stdout)`, the first that matches — or fails. What it
    /// returned, and every call: its arguments, then its stdin.
    pub(crate) fn with_stub<T>(
        answers: &[(&str, String)],
        f: impl AsyncFnOnce(&Path) -> T,
    ) -> (T, Vec<(String, String)>) {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("gh");
        let log = dir.path().join("log");
        let mut script = format!(
            "#!/bin/sh\nn=$(find '{log}' -name '*.args' 2>/dev/null | wc -l | tr -d ' ')\nmkdir -p '{log}'\n\
             printf '%s' \"$*\" > '{log}'/$n.args\ncat > '{log}'/$n.stdin\n",
            log = log.display()
        );
        for (i, (needle, out)) in answers.iter().enumerate() {
            let file = dir.path().join(format!("answer{i}"));
            std::fs::write(&file, out).unwrap();
            script.push_str(&format!(
                "if grep -qF -- '{needle}' '{log}'/$n.args '{log}'/$n.stdin; then cat '{file}'; exit 0; fi\n",
                log = log.display(),
                file = file.display(),
            ));
        }
        script.push_str("echo 'GraphQL: nothing stubbed' >&2\nexit 1\n");
        std::fs::write(&stub, script).unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        GH.with_borrow_mut(|gh| *gh = stub);
        let out = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(f(dir.path()));
        let mut calls = Vec::new();
        for n in 0.. {
            let Ok(args) = std::fs::read_to_string(log.join(format!("{n}.args"))) else {
                break;
            };
            let stdin = std::fs::read_to_string(log.join(format!("{n}.stdin"))).unwrap_or_default();
            calls.push((args, stdin));
        }
        (out, calls)
    }

    fn body_of(stdin: &str) -> Value {
        serde_json::from_str(stdin).unwrap()
    }

    #[test]
    fn the_threads_are_asked_of_the_checkouts_repo_and_read_back() {
        let answer = threads_answer(
            true,
            Some("PRR_9"),
            &[thread_node(
                "T_1",
                "src/a.rs",
                12,
                &[
                    comment_node("C_1", "ada", "rename this", false, false),
                    comment_node("C_2", "me", "on it", true, true),
                ],
            )],
        );
        let (answer, calls) = with_stub(&[("reviewThreads", answer)], async |dir| {
            read(dir, 42, TIMEOUT).await
        });
        let threads = answer.unwrap();
        assert_eq!(threads.pr_id, "PR_1");
        assert!(threads.mine);
        assert_eq!(
            threads.pending,
            Some(PendingReview {
                id: "PRR_9".into(),
                body: "so far".into()
            })
        );
        let thread = &threads.threads[0];
        assert_eq!((thread.path.as_str(), thread.line), ("src/a.rs", Some(12)));
        assert_eq!(thread.side, Side::Right);
        assert_eq!(thread.comments.len(), 2);
        assert!(!thread.comments[0].mine && !thread.comments[0].pending);
        assert!(thread.comments[1].mine && thread.comments[1].pending);
        assert_eq!(thread.my_pending().map(|c| c.id.as_str()), Some("C_2"));
        // One call, the number and the placeholders `gh` fills in on argv.
        assert_eq!(calls.len(), 1);
        let args = &calls[0].0;
        assert!(
            args.starts_with("api graphql -F owner={owner} -F repo={repo} -F number=42 -f query="),
            "{args}"
        );
        assert!(args.contains("reviews(states: [PENDING]"), "{args}");
    }

    #[test]
    fn a_refused_read_says_what_github_said() {
        let (answer, _) = with_stub(&[], async |dir| read(dir, 42, TIMEOUT).await);
        assert_eq!(answer.unwrap_err(), "GraphQL: nothing stubbed");
        let errors =
            json!({ "errors": [{ "message": "Could not resolve to a PullRequest" }] }).to_string();
        let (answer, _) = with_stub(&[("reviewThreads", errors)], async |dir| {
            read(dir, 42, TIMEOUT).await
        });
        assert_eq!(
            answer.unwrap_err(),
            "GraphQL: Could not resolve to a PullRequest"
        );
    }

    #[test]
    fn the_first_comment_starts_the_pending_review_then_adds_its_thread() {
        let started = json!({ "data": { "addPullRequestReview": { "pullRequestReview": { "id": "PRR_new" } } } });
        let thread = json!({ "data": { "addPullRequestReviewThread": { "thread": thread_node(
            "T_9", "src/a.rs", 14, &[comment_node("C_9", "me", "nit", true, true)],
        ) } } });
        let new = NewThread {
            path: "src/a.rs".into(),
            side: Side::Right,
            line: 14,
            start: Some((Side::Left, 12)),
            body: "nit".into(),
        };
        let (made, calls) = with_stub(
            &[
                ("addPullRequestReviewThread", thread.to_string()),
                ("addPullRequestReview(", started.to_string()),
            ],
            async |dir| comment(dir, "PR_1", None, &new).await,
        );
        let (review, thread) = made.unwrap();
        assert_eq!(review, "PRR_new");
        assert_eq!(thread.id, "T_9");
        assert!(thread.comments[0].pending);
        assert_eq!(calls.len(), 2);
        for (args, _) in &calls {
            assert_eq!(args, "api graphql -X POST --input -");
        }
        let start = body_of(&calls[0].1);
        assert!(start["query"]
            .as_str()
            .unwrap()
            .contains("addPullRequestReview(input: {pullRequestId: $pr})"));
        assert_eq!(start["variables"], json!({ "pr": "PR_1" }));
        let add = body_of(&calls[1].1);
        assert_eq!(
            add["variables"],
            json!({
                "review": "PRR_new", "path": "src/a.rs", "body": "nit", "line": 14,
                "side": "RIGHT", "startLine": 12, "startSide": "LEFT",
            })
        );
    }

    #[test]
    fn a_reply_joins_the_pending_review_already_there() {
        let replied = json!({ "data": { "addPullRequestReviewThreadReply": {
            "comment": comment_node("C_5", "me", "done", true, true),
        } } });
        let (made, calls) = with_stub(
            &[("addPullRequestReviewThreadReply", replied.to_string())],
            async |dir| reply(dir, "PR_1", Some("PRR_9".into()), "T_1", "done").await,
        );
        let (review, comment) = made.unwrap();
        assert_eq!((review.as_str(), comment.id.as_str()), ("PRR_9", "C_5"));
        assert_eq!(calls.len(), 1, "no second review started");
        assert_eq!(
            body_of(&calls[0].1)["variables"],
            json!({ "review": "PRR_9", "thread": "T_1", "body": "done" })
        );
    }

    #[test]
    fn resolve_delete_and_submit_send_what_github_takes() {
        let resolved = json!({ "data": { "resolveReviewThread": { "thread": { "id": "T_1", "isResolved": true } } } });
        let reopened = json!({ "data": { "unresolveReviewThread": { "thread": { "id": "T_1", "isResolved": false } } } });
        let ok = json!({ "data": { "x": {} } }).to_string();
        let ((now, again, gone, sent, fresh), calls) = with_stub(
            &[
                ("unresolveReviewThread", reopened.to_string()),
                ("resolveReviewThread", resolved.to_string()),
                ("deletePullRequestReviewComment", ok.clone()),
                ("submitPullRequestReview", ok.clone()),
                ("addPullRequestReview(", ok),
            ],
            async |dir| {
                (
                    resolve(dir, "T_1", true).await,
                    resolve(dir, "T_1", false).await,
                    delete(dir, "C_2").await,
                    submit(
                        dir,
                        "PR_1",
                        Some("PRR_9"),
                        Verdict::RequestChanges,
                        "please",
                    )
                    .await,
                    submit(dir, "PR_1", None, Verdict::Approve, "  ").await,
                )
            },
        );
        assert_eq!((now, again), (Ok(true), Ok(false)));
        assert_eq!((gone, sent, fresh), (Ok(()), Ok(()), Ok(())));
        let bodies: Vec<Value> = calls.iter().map(|(_, stdin)| body_of(stdin)).collect();
        assert_eq!(bodies[0]["variables"], json!({ "thread": "T_1" }));
        assert!(bodies[1]["query"]
            .as_str()
            .unwrap()
            .contains("unresolveReviewThread"));
        assert_eq!(bodies[2]["variables"], json!({ "comment": "C_2" }));
        assert_eq!(
            bodies[3]["variables"],
            json!({ "review": "PRR_9", "event": "REQUEST_CHANGES", "body": "please" })
        );
        // Nothing pending: a review sent at once, with no summary.
        assert!(bodies[4]["query"]
            .as_str()
            .unwrap()
            .contains("addPullRequestReview(input: {pullRequestId: $pr, event: $event"));
        assert_eq!(
            bodies[4]["variables"],
            json!({ "pr": "PR_1", "event": "APPROVE", "body": null })
        );
    }

    #[test]
    fn an_outdated_thread_is_named_by_where_it_was_written() {
        let mut node = thread_node(
            "T_1",
            "a.rs",
            3,
            &[comment_node("C", "ada", "x", false, false)],
        );
        node["line"] = Value::Null;
        node["isOutdated"] = json!(true);
        node["originalLine"] = json!(7);
        node["diffSide"] = json!("LEFT");
        let thread = parse_thread(&node).unwrap();
        assert_eq!(thread.line, None);
        assert!(thread.outdated);
        assert_eq!(thread.side, Side::Left);
        assert_eq!(thread.place(), "line 7");
        node["startLine"] = json!(5);
        node["line"] = json!(7);
        assert_eq!(parse_thread(&node).unwrap().place(), "lines 5–7");
    }

    #[test]
    fn only_a_comment_is_taken_on_your_own_pull_request() {
        assert!(Verdict::Comment.allowed(true));
        assert!(!Verdict::Approve.allowed(true));
        assert!(!Verdict::RequestChanges.allowed(true));
        assert!(Verdict::ALL.iter().all(|v| v.allowed(false)));
    }
}
