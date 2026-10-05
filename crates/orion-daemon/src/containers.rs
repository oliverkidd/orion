//! WORKTREE CONTAINERS: after orion deletes a worktree, stop or tear down
//! the docker compose projects that were started in it, as the
//! `worktree_containers` SETTING says (`orion_core::compose`). Off by
//! default. Projects are found by the directory compose recorded on their
//! containers, never by name, so a project only goes when every one of its
//! containers was started inside the deleted checkout.
//!
//! Like a WORKTREE HOOK it only reports: it runs after the git removal and
//! the row drop, and whatever goes wrong — docker unreachable, a compose
//! command failing or overrunning — is a warning in every client, never a
//! rolled-back delete. No docker CLI on the machine is nothing to do.

use orion_core::compose::{self, WorktreeContainers};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// How long `docker ps` may take — an engine that is starting up, or
/// wedged, must not hold the worktree lock.
const PS_TIMEOUT: Duration = Duration::from_secs(15);

/// How long one project's `compose stop` / `down` may take: compose gives
/// each container 10 s to stop before killing it, in parallel.
const COMPOSE_TIMEOUT: Duration = Duration::from_secs(60);

/// Apply `policy` to the compose projects started in the deleted
/// `worktree`. Returns the warnings for the clients; what it did goes to
/// the log. Skipped while `worktree` is still on disk, for the same reason
/// the delete hook is: the checkout — and what runs off it — is still
/// someone's.
pub async fn release(worktree: &Path, policy: WorktreeContainers) -> Vec<String> {
    if policy == WorktreeContainers::Off || worktree.exists() {
        return Vec::new();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let home = orion_core::env::home_dir();
    let Some(docker) = compose::find_docker(&path, home.as_deref()) else {
        tracing::info!("worktree_containers: no docker CLI, nothing to release");
        return Vec::new();
    };
    release_with(&docker, worktree, policy).await
}

/// [`release`] with the docker CLI already found.
async fn release_with(docker: &Path, worktree: &Path, policy: WorktreeContainers) -> Vec<String> {
    let dir = worktree.display();
    let listed = match run(docker, &compose::ps_args(), PS_TIMEOUT).await {
        Ok(out) => out,
        Err(e) => return vec![format!("couldn't check docker for {dir}'s containers: {e}")],
    };
    let found = compose::started_in(&compose::parse_ps(&listed), worktree);
    let mut warnings: Vec<String> = found
        .shared
        .iter()
        .map(|p| format!("left compose project {p}: it also has containers started outside {dir}"))
        .collect();
    for project in found.owned {
        let Some(args) = policy.compose_args(&project) else {
            continue;
        };
        match run(docker, &args, COMPOSE_TIMEOUT).await {
            Ok(_) => tracing::info!(
                project,
                worktree = %dir,
                "compose project {}",
                policy.past_tense()
            ),
            Err(e) => warnings.push(format!(
                "couldn't {} compose project {project}: {e}",
                policy.as_str()
            )),
        }
    }
    warnings
}

/// Run `docker args`: its stdout, or why it failed in one line — the last
/// line of its stderr, which is where docker says what went wrong.
async fn run<S: AsRef<std::ffi::OsStr>>(
    docker: &Path,
    args: &[S],
    timeout: Duration,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(docker);
    cmd.args(args)
        .env("PATH", path_with(docker))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Err(_) => return Err(format!("timed out after {}s", timeout.as_secs())),
        Ok(Err(e)) => return Err(format!("{} could not start: {e}", docker.display())),
        Ok(Ok(out)) => out,
    };
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Err(stderr
        .trim()
        .lines()
        .last()
        .map_or_else(|| format!("exited {}", out.status), str::to_string))
}

/// The DAEMON's PATH with the docker CLI's own directory in front: compose
/// and credential helpers are found beside it, and launchd's PATH names
/// neither.
fn path_with(docker: &Path) -> OsString {
    let mut dirs: Vec<PathBuf> = docker.parent().map(Path::to_path_buf).into_iter().collect();
    dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    std::env::join_paths(dirs).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stub `docker` in a temp dir, running `body` under `/bin/sh`.
    struct Stub {
        dir: tempfile::TempDir,
    }

    impl Stub {
        fn new(body: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let stub = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            let docker = stub.docker();
            std::fs::write(&docker, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
            stub
        }

        /// One that prints `ps` as given and appends every other call's
        /// arguments to `calls`, failing them with `compose_exit`.
        fn compose(ps: &str, compose_exit: i32) -> Self {
            Self::new(&format!(
                "if [ \"$1\" = ps ]; then printf '{ps}'; exit 0; fi\n\
                 echo \"$*\" >> \"$(dirname \"$0\")/calls\"\n\
                 echo 'no such project' >&2\nexit {compose_exit}"
            ))
        }

        fn docker(&self) -> PathBuf {
            self.dir.path().join("docker")
        }

        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.path().join("calls"))
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect()
        }
    }

    const PS: &str = "wt\\t/gone/wt\\nwt\\t/gone/wt/sub\\nshared\\t/gone/wt\\nshared\\t/live\\nmain\\t/live\\n\\t\\n";

    #[tokio::test]
    async fn tears_down_only_projects_wholly_inside_the_checkout() {
        let stub = Stub::compose(PS, 0);
        let warnings = release_with(
            &stub.docker(),
            Path::new("/gone/wt"),
            WorktreeContainers::RemoveVolumes,
        )
        .await;
        assert_eq!(
            stub.calls(),
            ["compose -p wt down --remove-orphans --volumes"]
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("left compose project shared"));
    }

    #[tokio::test]
    async fn stop_keeps_everything_and_a_failure_is_a_warning() {
        let stub = Stub::compose(PS, 1);
        let warnings = release_with(
            &stub.docker(),
            Path::new("/gone/wt"),
            WorktreeContainers::Stop,
        )
        .await;
        assert_eq!(stub.calls(), ["compose -p wt stop"]);
        assert!(
            warnings
                .iter()
                .any(|w| w == "couldn't stop compose project wt: no such project"),
            "{warnings:?}"
        );
    }

    #[tokio::test]
    async fn off_or_a_checkout_still_on_disk_runs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(release(dir.path(), WorktreeContainers::RemoveVolumes)
            .await
            .is_empty());
        assert!(release(Path::new("/gone/wt"), WorktreeContainers::Off)
            .await
            .is_empty());
    }

    #[tokio::test]
    async fn an_unreachable_engine_is_one_warning() {
        let stub = Stub::new("echo 'Cannot connect to the Docker daemon' >&2\nexit 1");
        let warnings = release_with(
            &stub.docker(),
            Path::new("/gone/wt"),
            WorktreeContainers::Remove,
        )
        .await;
        assert_eq!(
            warnings,
            ["couldn't check docker for /gone/wt's containers: Cannot connect to the Docker daemon"]
        );
    }
}
