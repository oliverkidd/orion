//! CONTINUE ON ANOTHER ACCOUNT — carrying a Claude session onto another
//! Claude account: a `claude_accounts` entry, which is Claude's own row
//! with `CLAUDE_CONFIG_DIR` pointing at its config dir, or any
//! Claude-dialect harness whose `env` pins one. Claude Code keeps a session's whole conversation in its
//! config dir, at `projects/<cwd slug>/<session id>.jsonl`, with a
//! `<session id>/` folder beside it for what hangs off it (subagent
//! transcripts, the title `/rename` set). A resume reads it from its own
//! config dir only, so moving a session is copying those to the same
//! relative path under the other account's config dir and resuming the
//! same id there:
//!
//! - **Found where it lives.** The transcript the session's hooks last
//!   named, while the daemon remembers one; else the session id under any
//!   slug of the projects dirs its harness keeps them in.
//! - **Nothing is lost.** A file the destination already holds is left
//!   alone when it is the same, and replaced only by a copy that starts
//!   with every byte of it — an older snapshot of the same conversation,
//!   left there when the session ran on that account before. Anything
//!   else refuses the whole move before a byte is written, and the source
//!   is never touched.
//! - **The row follows.** Its PTY is stopped, the row switched to the
//!   target harness — name, worktree and session id kept, the model and
//!   effort where the target offers them — and the CLI resumed there,
//!   at its input box, for the next prompt.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use orion_core::harness::HarnessDescriptor;
use orion_core::{AgentId, AgentKind, AgentStatus, SessionRef};

use crate::pty::{PtyEvent, DEFAULT_COLS, DEFAULT_ROWS};
use crate::registry::{harness_registry, resolve_harness, Daemon, CLOUD_ROW_NO_LOCAL_SESSION};

/// Where `session_id`'s transcript sits under any of `roots` (projects
/// dirs), in any cwd slug: the root it was found under, and the file. An
/// id that is not one plain file name names no transcript.
pub(crate) fn find_transcript(roots: &[PathBuf], session_id: &str) -> Option<(PathBuf, PathBuf)> {
    let name = format!("{session_id}.jsonl");
    if Path::new(&name).file_name() != Some(std::ffi::OsStr::new(&name)) {
        return None;
    }
    roots.iter().find_map(|root| {
        std::fs::read_dir(root)
            .ok()?
            .flatten()
            .map(|slug| slug.path().join(&name))
            .find(|file| file.is_file())
            .map(|file| (root.clone(), file))
    })
}

/// The files a move writes, each `(source, destination)`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CopyPlan {
    pub copies: Vec<(PathBuf, PathBuf)>,
}

impl CopyPlan {
    /// Plan carrying `file` — a transcript under the projects dir `from`
    /// — and the `<session id>/` folder beside it to the same relative
    /// paths under the projects dir `to`. Refuses, naming the file, when
    /// the destination holds something the session's own copy does not.
    pub fn new(from: &Path, file: &Path, to: &Path) -> Result<Self> {
        let mut plan = Self::default();
        plan.add(from, file, to)?;
        let folder = file.with_extension("");
        if folder.is_dir() {
            plan.add_folder(from, &folder, to)?;
        }
        Ok(plan)
    }

    fn add_folder(&mut self, from: &Path, folder: &Path, to: &Path) -> Result<()> {
        let entries =
            std::fs::read_dir(folder).with_context(|| format!("read {}", folder.display()))?;
        for entry in entries {
            let path = entry?.path();
            // Never through a link: only what Claude wrote here moves.
            let kind = std::fs::symlink_metadata(&path)?.file_type();
            if kind.is_dir() {
                self.add_folder(from, &path, to)?;
            } else if kind.is_file() {
                self.add(from, &path, to)?;
            }
        }
        Ok(())
    }

    /// One file: nothing to do when the destination already holds it,
    /// a copy when it holds nothing or an older snapshot of it.
    fn add(&mut self, from: &Path, src: &Path, to: &Path) -> Result<()> {
        let dst = to.join(
            src.strip_prefix(from)
                .context("transcript outside its root")?,
        );
        match std::fs::read(&dst) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("read {}", dst.display())),
            Ok(held) => {
                let carried =
                    std::fs::read(src).with_context(|| format!("read {}", src.display()))?;
                if carried == held {
                    return Ok(());
                }
                if !carried.starts_with(&held) {
                    bail!(
                        "{} is already there and holds what this session's own copy does not — \
                         nothing was moved",
                        dst.display()
                    );
                }
            }
        }
        self.copies.push((src.to_path_buf(), dst));
        Ok(())
    }

    /// Write every planned file, each through a temporary beside it and a
    /// rename, so a destination is never left holding half a transcript.
    pub fn apply(&self) -> Result<()> {
        for (src, dst) in &self.copies {
            let dir = dst.parent().context("destination without a folder")?;
            std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
            let name = dst.file_name().context("destination without a name")?;
            let tmp = dir.join(format!(".{}.orion-tmp", name.to_string_lossy()));
            std::fs::copy(src, &tmp).with_context(|| format!("copy {}", src.display()))?;
            std::fs::rename(&tmp, dst).with_context(|| format!("write {}", dst.display()))?;
        }
        Ok(())
    }
}

/// How long a move waits for the CLI it stopped to exit before copying
/// the transcript out from under it — the kill's own grace before it
/// escalates.
const STOP_WAIT: Duration = Duration::from_secs(3);

/// How often [`wait_for_exit`] looks for the exit while it waits.
const EXIT_POLL: Duration = Duration::from_millis(10);

/// Block until the PTY whose events `exits` follows reports its exit, or
/// `limit` passes. A move runs off the async workers, so polling here
/// holds no one up.
fn wait_for_exit(exits: &mut tokio::sync::broadcast::Receiver<PtyEvent>, limit: Duration) {
    use tokio::sync::broadcast::error::TryRecvError;
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        match exits.try_recv() {
            Ok(PtyEvent::Exited { .. }) | Err(TryRecvError::Closed) => return,
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => std::thread::sleep(EXIT_POLL),
        }
    }
}

/// Why `target` cannot take a Claude session, in the words the footer
/// shows: the first of [`HarnessDescriptor::takes_claude_sessions`]'s
/// conditions it misses.
fn not_a_target(target: &HarnessDescriptor) -> String {
    let label = target.display_label();
    if let Some(problem) = target.problem() {
        return problem;
    }
    if !target.claude_like() {
        return format!(
            "{label} does not speak Claude's hook dialect — it can't take a Claude session"
        );
    }
    if !target.resumes() {
        return format!("{label} has no resume flag — it couldn't pick the conversation up");
    }
    format!(
        "{label} keeps its sessions where orion can't see — add its config dir as a Claude \
         account (Settings → Agents), or set CLAUDE_CONFIG_DIR in its `env`"
    )
}

impl Daemon {
    /// **Continue on** another account — see the module docs. Refused, with the
    /// footer's reason and before anything stops, for a Cloud row, an
    /// archived one, a harness off Claude's dialect on either side, a
    /// session with no conversation or no transcript to move, and a
    /// destination that holds something the move would lose.
    pub fn continue_agent_on(self: &Arc<Self>, id: &AgentId, target_id: &str) -> Result<()> {
        let agent = self.store.get_agent(id)?.context("agent not found")?;
        if agent.cloud_session_id.is_some() {
            bail!("{CLOUD_ROW_NO_LOCAL_SESSION}");
        }
        if agent.archived {
            bail!("agent is archived — unarchive it first");
        }
        let source = resolve_harness(agent.kind, agent.custom_harness.as_deref())?;
        if agent.kind != AgentKind::Claude && !source.claude_like() {
            bail!(
                "only a Claude session moves to another account — {} runs {}",
                agent.name,
                source.display_label()
            );
        }
        let all = harness_registry();
        let target_id = target_id.trim();
        let Some(target) = all.iter().find(|entry| entry.id == target_id) else {
            bail!("no harness `{target_id}` to continue on");
        };
        if target.id == source.id {
            bail!("{} already runs on {}", agent.name, target.display_label());
        }
        if !target.takes_claude_sessions() {
            bail!("{}", not_a_target(target));
        }
        let Some(session_id) = agent.session_id.clone() else {
            bail!(
                "{} has no conversation yet — nothing to continue",
                agent.name
            );
        };
        let roots = self.continue_roots(&agent, &source, &session_id);
        let Some((from, file)) = find_transcript(&roots, &session_id) else {
            let looked: Vec<String> = roots.iter().map(|r| r.display().to_string()).collect();
            bail!(
                "no transcript for {} under {} — nothing to move",
                agent.name,
                looked.join(", ")
            );
        };
        let to = target
            .claude_config_dir()
            .context("no Claude config dir to continue in")?
            .join("projects");
        // Everything that could refuse is asked before the session stops.
        CopyPlan::new(&from, &file, &to)?;
        let worktree = self
            .store
            .get_worktree(&agent.worktree_id)?
            .context("worktree not found")?;

        // `ensure_session`'s gate, held from the kill to the respawn: an
        // Attach reaching for the stopped session meanwhile must not boot
        // it again on the account it is leaving.
        let _gate = self.spawn_gate.lock().unwrap();
        let sref = SessionRef::Agent(id.clone());
        let mut exits = self.session(&sref).map(|s| s.events.subscribe());
        self.kill_session(&sref);
        // A kill only signals: give the CLI its moment to go, so what it
        // last wrote is what moves.
        if let Some(exits) = exits.as_mut() {
            wait_for_exit(exits, STOP_WAIT);
        }
        let copied = match CopyPlan::new(&from, &file, &to)
            .and_then(|plan| plan.apply().map(|()| plan.copies.len()))
        {
            Ok(copied) => copied,
            Err(e) => {
                // Still on its own account: the next attach brings it
                // back there.
                self.try_broadcast_agent(id);
                return Err(e);
            }
        };
        let (kind, custom) = match AgentKind::parse(&target.id) {
            Some(kind) => (kind, None),
            None => (AgentKind::Custom, Some(target.id.as_str())),
        };
        let model = agent.model.as_deref().filter(|m| target.offers_model(m));
        let effort = agent.effort.as_deref().filter(|e| target.offers_effort(e));
        self.store
            .set_agent_harness(id, kind, custom, model, effort)?;
        // Whatever stopped it was the old account's. The CLI comes back
        // at its input box on the new one: a turn over, nothing unread.
        // The row goes out as one upsert, never a StatusChanged, so
        // moving a session rings no DONE SOUND.
        self.store.set_agent_usage_limit(id, None)?;
        self.store.set_agent_status(id, AgentStatus::Finished)?;
        self.store.mark_agent_seen(id)?;
        self.status_machines.lock().unwrap().remove(id);
        // The old account's transcript path: the respawn's hooks report
        // the new one.
        self.transcripts.lock().unwrap().remove(id);
        let moved = self.store.get_agent(id)?.context("agent not found")?;
        let spawned = self.spawn_agent_session(&moved, &worktree, DEFAULT_COLS, DEFAULT_ROWS);
        self.try_broadcast_agent(id);
        spawned?;
        tracing::info!(
            agent = %id,
            from = %source.id,
            to = %target.id,
            copied,
            "session continued on another account"
        );
        Ok(())
    }

    /// Where to look for `session_id`'s transcript before moving it: the
    /// file the session's hooks last named, while the daemon remembers it,
    /// then wherever its harness keeps them — the resume safeguards' roots,
    /// else (a wrapper orion can't see into) this process's default.
    fn continue_roots(
        &self,
        agent: &orion_core::Agent,
        source: &HarnessDescriptor,
        session_id: &str,
    ) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = self
            .transcripts
            .lock()
            .unwrap()
            .get(&agent.id)
            .filter(|t| t.session_id == session_id)
            .and_then(|t| Some(t.transcript_path.parent()?.parent()?.to_path_buf()))
            .into_iter()
            .collect();
        let known = self
            .claude_projects_roots(agent, source)
            .unwrap_or_else(|| {
                orion_core::paths::claude_config_dir()
                    .map(|dir| vec![dir.join("projects")])
                    .unwrap_or_default()
            });
        for root in known {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
        roots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn a_transcript_is_found_in_any_slug_of_any_root() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a/projects");
        let b = tmp.path().join("b/projects");
        write(&b.join("-repo-feat/sid-1.jsonl"), "{}\n");
        std::fs::create_dir_all(a.join("-repo")).unwrap();
        let roots = vec![a.clone(), b.clone(), tmp.path().join("missing")];
        assert_eq!(
            find_transcript(&roots, "sid-1"),
            Some((b.clone(), b.join("-repo-feat/sid-1.jsonl")))
        );
        assert_eq!(find_transcript(&roots, "sid-2"), None);
        // An id that would climb out of its slug names nothing.
        write(&a.join("x.jsonl"), "{}\n");
        assert_eq!(find_transcript(&roots, "../x"), None);
    }

    /// The transcript and its folder — a subagent's transcript and the
    /// title, nested — land at the same relative paths under the target
    /// account, and the source keeps every byte.
    #[test]
    fn a_move_copies_the_transcript_and_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("claude/projects");
        let to = tmp.path().join("claude-b/projects");
        let file = from.join("-repo/sid.jsonl");
        write(&file, "{\"a\":1}\n");
        write(
            &from.join("-repo/sid/custom-title.json"),
            "{\"customTitle\":\"Fix\"}",
        );
        write(
            &from.join("-repo/sid/subagents/agent-1.jsonl"),
            "{\"b\":2}\n",
        );
        // Another session in the same slug stays where it is.
        write(&from.join("-repo/other.jsonl"), "{}\n");

        let plan = CopyPlan::new(&from, &file, &to).unwrap();
        assert_eq!(plan.copies.len(), 3, "{plan:?}");
        plan.apply().unwrap();
        assert_eq!(read(&to.join("-repo/sid.jsonl")), "{\"a\":1}\n");
        assert_eq!(
            read(&to.join("-repo/sid/custom-title.json")),
            "{\"customTitle\":\"Fix\"}"
        );
        assert_eq!(
            read(&to.join("-repo/sid/subagents/agent-1.jsonl")),
            "{\"b\":2}\n"
        );
        assert!(!to.join("-repo/other.jsonl").exists());
        assert_eq!(read(&file), "{\"a\":1}\n", "the source is never touched");
        assert!(
            !to.join("-repo/.sid.jsonl.orion-tmp").exists(),
            "no temporary is left behind"
        );

        // Moved again with nothing new: nothing to write.
        assert_eq!(CopyPlan::new(&from, &file, &to).unwrap().copies, Vec::new());
    }

    /// Carried back to an account it ran on before: the copy there is an
    /// older snapshot of the same conversation, and the newer one replaces
    /// it. A copy that went its own way refuses the whole move.
    #[test]
    fn an_older_copy_is_replaced_and_a_different_one_refuses() {
        let tmp = tempfile::tempdir().unwrap();
        let from = tmp.path().join("claude-b/projects");
        let to = tmp.path().join("claude/projects");
        let file = from.join("-repo/sid.jsonl");
        write(&file, "{\"a\":1}\n{\"b\":2}\n");
        write(&to.join("-repo/sid.jsonl"), "{\"a\":1}\n");
        CopyPlan::new(&from, &file, &to).unwrap().apply().unwrap();
        assert_eq!(read(&to.join("-repo/sid.jsonl")), "{\"a\":1}\n{\"b\":2}\n");

        write(&to.join("-repo/sid.jsonl"), "{\"a\":1}\n{\"c\":3}\n");
        let err = CopyPlan::new(&from, &file, &to).unwrap_err().to_string();
        assert!(err.contains("nothing was moved"), "{err}");
        assert_eq!(
            read(&to.join("-repo/sid.jsonl")),
            "{\"a\":1}\n{\"c\":3}\n",
            "a different file is never overwritten"
        );

        // A conflict deep in the folder refuses it all too, before the
        // transcript itself is written.
        let fresh = tmp.path().join("claude-c/projects");
        write(&from.join("-repo/sid/subagents/a.jsonl"), "{\"x\":1}\n");
        write(&fresh.join("-repo/sid/subagents/a.jsonl"), "{\"y\":1}\n");
        assert!(CopyPlan::new(&from, &file, &fresh).is_err());
        assert!(!fresh.join("-repo/sid.jsonl").exists());
    }

    #[test]
    fn only_a_claude_harness_that_resumes_where_orion_can_see_is_a_target() {
        let mut wrapper = orion_core::harness::builtin("claude").unwrap();
        wrapper.id = "claude-b".into();
        wrapper.label = "Claude B".into();
        wrapper.program = "/home/me/bin/claude-b".into();
        assert!(!wrapper.takes_claude_sessions());
        assert!(
            not_a_target(&wrapper).contains("CLAUDE_CONFIG_DIR"),
            "{}",
            not_a_target(&wrapper)
        );
        wrapper
            .env
            .insert("CLAUDE_CONFIG_DIR".into(), "~/.claude-b".into());
        assert!(wrapper.takes_claude_sessions());
        wrapper.resume.flag = None;
        assert!(not_a_target(&wrapper).contains("no resume flag"));
        let codex = orion_core::harness::builtin("codex").unwrap();
        assert!(not_a_target(&codex).contains("hook dialect"));
    }
}
