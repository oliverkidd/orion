//! RUN TARGETS: what `⌘⇧S` can start in a checkout, found in the checkout
//! itself — no setting to keep in step with the repo. The first of these
//! that names anything wins:
//!
//! 1. **A Justfile** (`justfile`, `Justfile`, `.justfile`): its own `start`
//!    recipe (else `dev`) as `just start`, and each of its modules'
//!    (`mod os 'apps/os/justfile'`) as `just os start` — a monorepo's apps.
//! 2. **package.json**: each workspace package's `dev` script (else
//!    `start`), run the way the repo's package manager runs one in a
//!    workspace (`npm run dev -w <name>`, `pnpm --filter <name> dev`,
//!    `yarn workspace <name> dev`, `bun --filter <name> dev`); with no
//!    workspace that has one, the root's own `dev` (else `start`).
//! 3. **A Makefile** with a `dev`, `start`, `run` or `up` target: `make dev`.
//! 4. **A compose file**: `docker compose up`.
//!
//! One target starts at once; several are a pick (`event_loop`'s
//! `toggle_stack_in`), the one picked last for the project first. Only
//! small text files are read — no `just`, no `npm` — so the key never
//! waits on a process.

use std::path::{Path, PathBuf};

/// Something to start: its name in the pick (`os`, `@riplo/slides`) and
/// the shell line that starts it (`just os start`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub command: String,
}

impl Target {
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
        }
    }
}

/// What a Justfile recipe, or a package's script, that starts an app is
/// called, in the order one is preferred.
const JUST_RECIPES: [&str; 2] = ["start", "dev"];
const SCRIPTS: [&str; 2] = ["dev", "start"];
const MAKE_TARGETS: [&str; 4] = ["dev", "start", "run", "up"];
const JUSTFILES: [&str; 3] = ["justfile", "Justfile", ".justfile"];
const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
];

/// Everything there is to start in the checkout at `dir`, its root's
/// first; `name` stands for the root's own (the project's name).
pub fn detect(dir: &Path, name: &str) -> Vec<Target> {
    let found = from_justfile(dir, name);
    if !found.is_empty() {
        return found;
    }
    let found = from_package_json(dir, name);
    if !found.is_empty() {
        return found;
    }
    if let Some(target) = from_makefile(dir, name) {
        return vec![target];
    }
    COMPOSE_FILES
        .iter()
        .any(|f| dir.join(f).is_file())
        .then(|| Target::new(name, "docker compose up"))
        .into_iter()
        .collect()
}

/// What `⌘⇧S` offers in the checkout at `dir`: [`detect`] — or, under
/// test, what [`stub`] set (one `just start` unless a test says else), as
/// the tests' checkouts are paths with nothing in them.
pub fn found(dir: &Path, name: &str) -> Vec<Target> {
    #[cfg(test)]
    if let Some(targets) = STUB.with(|s| s.borrow().clone()) {
        return targets;
    }
    detect(dir, name)
}

#[cfg(test)]
thread_local! {
    static STUB: std::cell::RefCell<Option<Vec<Target>>> =
        std::cell::RefCell::new(Some(vec![Target::new("app", "just start")]));
}

/// Under test: what [`found`] answers on this thread — None for the
/// real [`detect`].
#[cfg(test)]
pub fn stub(targets: Option<Vec<Target>>) {
    STUB.with(|s| *s.borrow_mut() = targets);
}

/// What `⌘⇧S` says when there is nothing to start.
pub const NOTHING: &str = "Nothing to start here: no start or dev recipe in a Justfile, no dev or start script in package.json, no Makefile target and no compose file";

// ---- Justfile ----

/// The first Justfile in `dir`.
fn justfile_in(dir: &Path) -> Option<PathBuf> {
    JUSTFILES.iter().map(|f| dir.join(f)).find(|p| p.is_file())
}

fn from_justfile(dir: &Path, name: &str) -> Vec<Target> {
    let Some(path) = justfile_in(dir) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let parsed = parse_justfile(&text);
    let mut out: Vec<Target> = start_recipe(&parsed.recipes)
        .map(|r| Target::new(name, format!("just {r}")))
        .into_iter()
        .collect();
    for (module, file) in parsed.modules {
        let Some(text) = module_file(dir, &module, file.as_deref())
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            continue;
        };
        if let Some(r) = start_recipe(&parse_justfile(&text).recipes) {
            out.push(Target::new(module.clone(), format!("just {module} {r}")));
        }
    }
    out
}

/// The recipe that starts an app, of `recipes`.
fn start_recipe(recipes: &[String]) -> Option<&'static str> {
    JUST_RECIPES
        .into_iter()
        .find(|want| recipes.iter().any(|r| r == want))
}

/// A Justfile's public recipes and its modules (each with its path when
/// one is given).
#[derive(Debug, Default, PartialEq, Eq)]
struct Justfile {
    recipes: Vec<String>,
    modules: Vec<(String, Option<String>)>,
}

/// The recipes and modules a Justfile declares, by its lines: a recipe
/// starts a line with its name and a `:` (not `:=`), past any `@`; one
/// marked `[private]` or named `_…` is left out. `mod name` and
/// `mod name 'path'` (`mod?` too) are modules.
fn parse_justfile(text: &str) -> Justfile {
    let mut out = Justfile::default();
    let mut private = false;
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let line = line.trim_end();
        if line.starts_with('[') {
            private |= line
                .trim_matches(['[', ']'])
                .split(',')
                .any(|attr| attr.trim() == "private");
            continue;
        }
        if let Some(rest) = line
            .strip_prefix("mod? ")
            .or_else(|| line.strip_prefix("mod "))
        {
            let mut words = rest.split_whitespace();
            if let Some(module) = words.next() {
                let path = words
                    .next()
                    .map(|p| p.trim_matches(['\'', '"']).to_string());
                out.modules.push((module.to_string(), path));
            }
            private = false;
            continue;
        }
        let recipe = line.strip_prefix('@').unwrap_or(line);
        let name: String = recipe
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        let after = &recipe[name.len()..];
        let is_recipe = !name.is_empty()
            && !["set", "alias", "export", "import"].contains(&name.as_str())
            && after
                .find(':')
                .is_some_and(|at| !after[at + 1..].starts_with('='));
        if is_recipe && !private && !name.starts_with('_') {
            out.recipes.push(name);
        }
        private = false;
    }
    out
}

/// Module `name`'s file: its given path (a file, or a directory holding
/// `mod.just` or a justfile), else `name.just`, `name/mod.just` or a
/// justfile in `name/` — beside the root Justfile, in `dir`.
fn module_file(dir: &Path, name: &str, given: Option<&str>) -> Option<PathBuf> {
    let in_dir = |d: PathBuf| {
        std::iter::once(d.join("mod.just"))
            .chain(JUSTFILES.iter().map(|f| d.join(f)))
            .find(|p| p.is_file())
    };
    match given {
        Some(path) => {
            let path = dir.join(path);
            match path.is_dir() {
                true => in_dir(path),
                false => path.is_file().then_some(path),
            }
        }
        None => {
            let file = dir.join(format!("{name}.just"));
            match file.is_file() {
                true => Some(file),
                false => in_dir(dir.join(name)),
            }
        }
    }
}

// ---- package.json ----

/// How the repo runs a script: its package manager, by its lockfile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Manager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl Manager {
    fn of(dir: &Path) -> Self {
        let has = |f: &str| dir.join(f).is_file();
        if has("pnpm-lock.yaml") || has("pnpm-workspace.yaml") {
            Manager::Pnpm
        } else if has("yarn.lock") {
            Manager::Yarn
        } else if has("bun.lockb") || has("bun.lock") {
            Manager::Bun
        } else {
            Manager::Npm
        }
    }

    /// `script` in workspace package `name`.
    fn in_workspace(self, name: &str, script: &str) -> String {
        match self {
            Manager::Npm => format!("npm run {script} -w {name}"),
            Manager::Pnpm => format!("pnpm --filter {name} {script}"),
            Manager::Yarn => format!("yarn workspace {name} {script}"),
            Manager::Bun => format!("bun --filter {name} {script}"),
        }
    }

    /// `script` at the root.
    fn at_root(self, script: &str) -> String {
        match self {
            Manager::Npm => format!("npm run {script}"),
            Manager::Pnpm => format!("pnpm {script}"),
            Manager::Yarn => format!("yarn {script}"),
            Manager::Bun => format!("bun run {script}"),
        }
    }
}

use crate::claude_catalogue::read_json;

/// The most directories a workspace pattern (`packages/*`) is looked
/// through, and the most apps a pick lists: a pick of more is no use,
/// and the look runs on the keypress.
const MOST_DIRS: usize = 64;
const MOST_TARGETS: usize = 20;

/// The script of `package` that starts it.
fn start_script(package: &serde_json::Value) -> Option<&'static str> {
    SCRIPTS
        .into_iter()
        .find(|s| package["scripts"][*s].is_string())
}

fn from_package_json(dir: &Path, name: &str) -> Vec<Target> {
    let Some(root) = read_json(&dir.join("package.json")) else {
        return Vec::new();
    };
    let mut patterns: Vec<String> = match &root["workspaces"] {
        serde_json::Value::Array(list) => list.clone(),
        other => other["packages"].as_array().cloned().unwrap_or_default(),
    }
    .into_iter()
    .filter_map(|p| p.as_str().map(str::to_string))
    .collect();
    if let Ok(text) = std::fs::read_to_string(dir.join("pnpm-workspace.yaml")) {
        patterns.extend(pnpm_packages(&text));
    }
    patterns.sort();
    patterns.dedup();
    let mut found: Vec<(String, &'static str)> = Vec::new();
    for package_dir in patterns.iter().flat_map(|p| expand(dir, p)) {
        if found.len() >= MOST_TARGETS {
            break;
        }
        let Some(package) = read_json(&package_dir.join("package.json")) else {
            continue;
        };
        if let (Some(pkg), Some(script)) = (package["name"].as_str(), start_script(&package)) {
            found.push((pkg.to_string(), script));
        }
    }
    let root_script = start_script(&root).filter(|_| found.is_empty());
    if found.is_empty() && root_script.is_none() {
        return Vec::new();
    }
    // Only now is how the repo runs a script worth asking.
    let manager = Manager::of(dir);
    match root_script {
        Some(script) => vec![Target::new(name, manager.at_root(script))],
        None => found
            .into_iter()
            .map(|(pkg, script)| {
                let command = manager.in_workspace(&pkg, script);
                Target::new(pkg, command)
            })
            .collect(),
    }
}

/// The `packages:` list of a `pnpm-workspace.yaml`, read by its lines.
fn pnpm_packages(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if !line.starts_with([' ', '\t', '-']) {
            inside = line.trim_end() == "packages:";
            continue;
        }
        if let Some(item) = line.trim().strip_prefix('-').filter(|_| inside) {
            let item = item.split(" #").next().unwrap_or("");
            let item = item.trim().trim_matches(['\'', '"']);
            if !item.is_empty() && !item.starts_with('!') {
                out.push(item.to_string());
            }
        }
    }
    out
}

/// A workspace pattern's directories: `apps/*` each directory in `apps`,
/// sorted; a plain path itself. Negations and deeper globs (`**`) are
/// left out.
fn expand(dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let pattern = pattern.trim_end_matches('/');
    if pattern.starts_with('!') || pattern.contains("**") {
        return Vec::new();
    }
    match pattern.strip_suffix("/*") {
        Some(parent) => {
            let Ok(entries) = std::fs::read_dir(dir.join(parent)) else {
                return Vec::new();
            };
            // A file among them has no package.json in it: no need to
            // ask which are directories.
            let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
            dirs.sort();
            dirs.truncate(MOST_DIRS);
            dirs
        }
        None if !pattern.contains('*') => vec![dir.join(pattern)],
        None => Vec::new(),
    }
}

// ---- Makefile ----

fn from_makefile(dir: &Path, name: &str) -> Option<Target> {
    let text = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())?;
    let targets: Vec<&str> = text
        .lines()
        .filter(|l| !l.starts_with([' ', '\t', '#', '.']))
        .filter_map(|l| l.split_once(':').filter(|(_, rest)| !rest.starts_with('=')))
        .flat_map(|(names, _)| names.split_whitespace())
        .collect();
    MAKE_TARGETS
        .into_iter()
        .find(|want| targets.contains(want))
        .map(|t| Target::new(name, format!("make {t}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, path: &str, text: &str) {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn a_justfile_reads_its_recipes_and_modules() {
        let parsed = parse_justfile(
            "set shell := [\"bash\", \"-cu\"]\nmod os 'apps/os/justfile'\nmod? docs\n\n[group('private-api')]\napi:\n[private]\ndefault:\n    @just --list\n\nversion := \"1\"\n# start: not a recipe\nstart *args:\n    echo\n@dev port=\"3000\": build\n_hidden:\nbuild:\n",
        );
        assert_eq!(parsed.recipes, ["api", "start", "dev", "build"]);
        assert_eq!(
            parsed.modules,
            [
                ("os".to_string(), Some("apps/os/justfile".to_string())),
                ("docs".to_string(), None)
            ]
        );
    }

    /// A monorepo's Justfile: each module with a `start` is an app to
    /// pick, run as `just <module> start`; one without starts nothing.
    #[test]
    fn a_monorepos_justfile_modules_are_its_apps() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(
            d,
            "Justfile",
            "mod os 'apps/os/justfile'\nmod slides 'apps/slides/justfile'\nmod infra\nsetup-worktree:\n",
        );
        write(d, "apps/os/justfile", "start *args:\n    bash x\nstop:\n");
        write(d, "apps/slides/justfile", "build:\ndev:\n");
        write(d, "infra.just", "plan:\n");
        // package.json is never reached: the Justfile named apps.
        write(d, "package.json", r#"{"scripts": {"dev": "next dev"}}"#);
        assert_eq!(
            detect(d, "riplo-os"),
            [
                Target::new("os", "just os start"),
                Target::new("slides", "just slides dev")
            ]
        );
        write(d, "Justfile", "start:\n");
        assert_eq!(detect(d, "app"), [Target::new("app", "just start")]);
    }

    /// Workspaces each with a dev script are the apps, run the way the
    /// lockfile's manager runs a workspace's; with none, the root's.
    #[test]
    fn package_json_workspaces_are_the_apps_else_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        write(
            d,
            "package.json",
            r#"{"workspaces": ["apps/*", "services/api"], "scripts": {"start": "x"}}"#,
        );
        write(
            d,
            "apps/web/package.json",
            r#"{"name": "@me/web", "scripts": {"dev": "next dev"}}"#,
        );
        write(
            d,
            "apps/lib/package.json",
            r#"{"name": "lib", "scripts": {"build": "tsc"}}"#,
        );
        write(
            d,
            "services/api/package.json",
            r#"{"name": "api", "scripts": {"start": "node ."}}"#,
        );
        assert_eq!(
            detect(d, "mono"),
            [
                Target::new("@me/web", "npm run dev -w @me/web"),
                Target::new("api", "npm run start -w api")
            ]
        );
        write(d, "pnpm-lock.yaml", "");
        assert_eq!(detect(d, "mono")[0].command, "pnpm --filter @me/web dev");

        let plain = tempfile::tempdir().unwrap();
        write(
            plain.path(),
            "package.json",
            r#"{"scripts": {"dev": "vite", "start": "x"}}"#,
        );
        write(plain.path(), "yarn.lock", "");
        assert_eq!(
            detect(plain.path(), "site"),
            [Target::new("site", "yarn dev")]
        );
    }

    #[test]
    fn pnpm_workspace_yaml_lists_its_packages() {
        assert_eq!(
            pnpm_packages(
                "packages:\n  - 'apps/*' # the apps\n  - \"tools/cli\"\n  - '!apps/old'\ncatalog:\n  - x\n"
            ),
            ["apps/*", "tools/cli"]
        );
    }

    #[test]
    fn a_makefile_or_a_compose_file_is_the_last_resort() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(detect(d, "x").is_empty());
        write(d, "compose.yaml", "services: {}\n");
        assert_eq!(detect(d, "x"), [Target::new("x", "docker compose up")]);
        write(
            d,
            "Makefile",
            "VAR := 1\n.PHONY: run\nbuild:\n\tcc\nup run: build\n\t./a\n",
        );
        assert_eq!(detect(d, "x"), [Target::new("x", "make run")]);
    }
}
