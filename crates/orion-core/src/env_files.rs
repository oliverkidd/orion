//! Which files are a checkout's ENV FILES — `.env`, `.env.local` and their
//! kind, gitignored by design: the ones the DAEMON links into every new
//! worktree (`link_env_files`) and the FILE FINDER lists despite the
//! gitignore.

use std::path::Path;

/// The pathspec `git ls-files --others --ignored` is narrowed by: `.env`
/// and every `.env.*` at any depth.
pub const PATHSPEC: &str = ":(glob)**/.env*";

/// Whether `path` (as git lists it) is an env file: its name starts with
/// `.env`, and nothing under a `node_modules` directory counts.
pub fn is_env_file(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".env"))
        && !path.components().any(|c| c.as_os_str() == "node_modules")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_at_any_depth_but_not_in_node_modules() {
        assert!(is_env_file(Path::new(".env")));
        assert!(is_env_file(Path::new("apps/web/.env.local")));
        assert!(!is_env_file(Path::new("node_modules/pkg/.env")));
        assert!(!is_env_file(Path::new("docs/env.md")));
    }
}
