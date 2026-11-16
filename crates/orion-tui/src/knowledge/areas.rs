//! The AREAS: the fixed set every harvested topic sits in. A topic is
//! `<area>/<topic>`, and its area is one of these.
//!
//! They live in `areas.md` in [`DIR`], written by orion: seeded with
//! [`SEED`], each with a one-line scope, and each folder of hand-written
//! records under `.orion/decisions/` listed as another name for the seed
//! it is nearest ([`nearest`]). A topic filed under any other area is
//! moved to the nearest listed one ([`Areas::topic`]): by the words of
//! its area, then of its name, else to a stream's catch-all.
//!
//! [`DIR`]: super::store::DIR

use super::records::{self, Checkout};
use super::store::{topic_path, Store, DECISIONS};

/// The seed areas: name, scope, and the words that place a topic in it.
pub(crate) const SEED: &[(&str, &str, &[&str])] = &[
    (
        "editor",
        "The slide editor: canvas, shapes, text, tables, selection, toolbar and shortcuts.",
        &[
            "editor",
            "edit",
            "canvas",
            "shape",
            "shapes",
            "line",
            "lines",
            "text",
            "table",
            "tables",
            "toolbar",
            "clipboard",
            "spelling",
            "indicator",
            "indicators",
            "key",
            "keys",
            "shortcut",
            "shortcuts",
            "selection",
            "select",
            "snapping",
            "snap",
            "resize",
            "crop",
            "history",
            "undo",
            "format",
            "painter",
            "color",
            "colour",
            "picker",
            "viewport",
            "rail",
            "arrange",
            "lists",
            "elements",
            "element",
            "slide",
            "slides",
            "strip",
            "context",
            "menu",
            "chrome",
            "ui",
            "zoom",
            "grid",
            "images",
            "image",
        ],
    ),
    (
        "charts",
        "Charts on slides: chart types, data, axes, series and chart editing.",
        &[
            "chart", "charts", "graph", "graphs", "plot", "axis", "axes", "series",
        ],
    ),
    (
        "brand-and-templates",
        "Brands, themes, layouts, templates, logos and decks made from them.",
        &[
            "brand",
            "brands",
            "branding",
            "template",
            "templates",
            "layout",
            "layouts",
            "theme",
            "themes",
            "logo",
            "logos",
            "palette",
            "font",
            "fonts",
            "master",
            "decor",
        ],
    ),
    (
        "ai-and-mcp",
        "The AI assistant, its runs, prompts and evals, and the slide MCP server.",
        &[
            "ai",
            "mcp",
            "llm",
            "prompt",
            "prompts",
            "model",
            "models",
            "eval",
            "evals",
            "claude",
            "assistant",
            "chat",
            "turns",
            "copilot",
        ],
    ),
    (
        "import-export",
        "Decks in and out: PPTX and PDF import, export and server rendering.",
        &[
            "import",
            "export",
            "exports",
            "pptx",
            "pdf",
            "render",
            "rendering",
            "renderer",
            "download",
            "thumbnails",
            "thumbnail",
        ],
    ),
    (
        "workspace",
        "Projects, decks, library, search, sharing, collaboration and the home screens.",
        &[
            "workspace",
            "home",
            "project",
            "projects",
            "deck",
            "decks",
            "library",
            "sharing",
            "share",
            "search",
            "collab",
            "collaboration",
            "viewer",
            "onboarding",
            "assets",
            "asset",
            "comments",
            "presence",
            "notifications",
        ],
    ),
    (
        "access-and-accounts",
        "Sign-in, accounts, teams, roles, permissions and billing.",
        &[
            "auth",
            "login",
            "signin",
            "account",
            "accounts",
            "user",
            "users",
            "permission",
            "permissions",
            "access",
            "sso",
            "billing",
            "team",
            "teams",
            "org",
            "orgs",
            "invite",
            "invites",
            "roles",
            "role",
        ],
    ),
    (
        "platform-and-infra",
        "Hosting, databases, storage, services, deploys, releases and status.",
        &[
            "infra",
            "infrastructure",
            "platform",
            "deploy",
            "deploys",
            "aws",
            "vercel",
            "database",
            "db",
            "storage",
            "migration",
            "migrations",
            "status",
            "ops",
            "release",
            "releases",
            "ssm",
            "typesense",
            "redis",
            "postgres",
            "server",
            "services",
            "service",
            "ws",
            "flags",
            "flag",
            "secrets",
            "monitoring",
            "jobs",
            "queue",
        ],
    ),
];

/// Where a topic with no fitting area goes.
const CATCH_ALL: &str = "workspace";

/// Areas orion once kept for how agents work in the repo: its tooling,
/// scripts, tests and CI. The harvest is product knowledge only now —
/// that kind belongs in the repo's own hand-written agent docs — so an
/// `areas.md` that lists one loses it ([`Areas::load`]), and its pages'
/// product rules move to the nearest listed area
/// (`super::store::Store::keep_product`).
pub(crate) const RETIRED: &[&str] = &["engineering-tooling", "testing-and-ci"];

/// Whether `topic` (`area/topic`) sits in a [`RETIRED`] area.
pub(crate) fn is_retired(topic: &str) -> bool {
    topic
        .split('/')
        .next()
        .is_some_and(|area| RETIRED.contains(&area))
}

/// The file the areas live in, relative to [`super::store::DIR`].
pub(crate) const FILE: &str = "areas.md";

/// One area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Area {
    pub name: String,
    pub scope: String,
    /// The hand-written record folders it stands for.
    pub records: Vec<String>,
}

/// The repo's areas, in their order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Areas {
    pub list: Vec<Area>,
}

impl Default for Areas {
    fn default() -> Self {
        Areas {
            list: SEED
                .iter()
                .map(|(name, scope, _)| Area {
                    name: name.to_string(),
                    scope: scope.to_string(),
                    records: Vec::new(),
                })
                .collect(),
        }
    }
}

/// The words of `text` worth placing a topic by.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// The seed `name`'s placing words.
fn keywords(name: &str) -> &'static [&'static str] {
    SEED.iter()
        .find(|(seed, _, _)| *seed == name)
        .map_or(&[], |(_, _, words)| words)
}

/// The seed `folder` is nearest: the first of its words that places it,
/// else the product catch-all.
pub(crate) fn nearest(folder: &str) -> &'static str {
    words(folder)
        .iter()
        .find_map(|w| {
            SEED.iter()
                .find(|(name, _, keys)| *name == w || keys.contains(&w.as_str()))
                .map(|(name, _, _)| *name)
        })
        .unwrap_or(CATCH_ALL)
}

impl Areas {
    /// The areas `store`'s checkout keeps in `areas.md`, else the seed —
    /// with every folder of hand-written records there listed under its
    /// nearest area.
    pub(crate) fn load(store: &Store) -> Areas {
        let mut areas = std::fs::read_to_string(store.dir.join(FILE))
            .ok()
            .map(|t| {
                let mut read = parse(super::store::unmarked(&t));
                read.list.retain(|a| !RETIRED.contains(&a.name.as_str()));
                read
            })
            .filter(|a| !a.list.is_empty())
            .unwrap_or_default();
        for folder in record_folders(&store.repo) {
            areas.add_record_folder(&folder);
        }
        areas
    }

    /// List the record folder `folder` under its nearest area, unless it
    /// is listed already.
    pub(crate) fn add_record_folder(&mut self, folder: &str) {
        if self
            .list
            .iter()
            .any(|a| a.records.iter().any(|r| r == folder))
        {
            return;
        }
        let at = match self.list.iter().position(|a| a.name == folder) {
            Some(at) => at,
            None => {
                let near = nearest(folder);
                self.list.iter().position(|a| a.name == near).unwrap_or(0)
            }
        };
        if let Some(area) = self.list.get_mut(at) {
            if area.name != folder {
                area.records.push(folder.to_string());
                area.records.sort();
            }
        }
    }

    pub(crate) fn names(&self) -> Vec<&str> {
        self.list.iter().map(|a| a.name.as_str()).collect()
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.list.iter().any(|a| a.name == name)
    }

    /// The listed area `name` stands for: itself, an area a record folder
    /// of that name is listed under, or the area its words place it in.
    fn area_for(&self, name: &str) -> Option<String> {
        if self.has(name) {
            return Some(name.to_string());
        }
        if let Some(a) = self
            .list
            .iter()
            .find(|a| a.records.iter().any(|r| r == name))
        {
            return Some(a.name.clone());
        }
        words(name).iter().find_map(|w| {
            self.list
                .iter()
                .find(|a| a.name == *w || keywords(&a.name).contains(&w.as_str()))
                .map(|a| a.name.clone())
        })
    }

    /// `raw` as a topic in a listed area: `area/topic`, kebab-case; its
    /// area moved to the nearest listed one when it isn't one — by the
    /// area's words, then the topic's, else the catch-all.
    pub(crate) fn topic(&self, raw: &str) -> String {
        let path = topic_path(raw);
        let (area, name) = path.split_once('/').unwrap_or(("general", &path));
        if self.has(area) {
            return path;
        }
        let placed = self.area_for(area).or_else(|| self.area_for(name));
        let area = placed.unwrap_or_else(|| match self.has(CATCH_ALL) {
            true => CATCH_ALL.to_string(),
            false => self
                .list
                .first()
                .map_or("general".into(), |a| a.name.clone()),
        });
        format!("{area}/{name}")
    }

    /// The areas as `areas.md` has them ([`super::store::MARKER`] goes on
    /// when it is written).
    pub(crate) fn render(&self) -> String {
        let mut out = String::from(
            "# Knowledge areas\nEvery harvested topic is `<area>/<topic>`, and its area is one of these. orion keeps this list.\n\n",
        );
        for a in &self.list {
            out.push_str(&format!("- `{}` — {}", a.name, a.scope));
            if !a.records.is_empty() {
                out.push_str(&format!(" Records: {}.", a.records.join(", ")));
            }
            out.push('\n');
        }
        out
    }

    /// Write `areas.md` into `store`, when it says anything new.
    pub(crate) fn save(&self, store: &Store) -> Result<(), String> {
        let path = store.dir.join(FILE);
        let now = std::fs::read_to_string(&path).ok();
        let text = self.render();
        if now.as_deref().map(super::store::unmarked) == Some(text.as_str()) {
            return Ok(());
        }
        store.write(&path, &text)
    }

    /// The areas as a prompt lists them: name and scope a line.
    pub(crate) fn listing(&self) -> String {
        self.list
            .iter()
            .map(|a| format!("- {} — {}", a.name, a.scope))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `areas.md`'s text as areas: each `` - `name` — scope Records: a, b. ``
/// line; anything else is skipped.
pub(crate) fn parse(text: &str) -> Areas {
    let list = text
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("- `")?;
            let (name, rest) = rest.split_once('`')?;
            let rest = rest.trim_start().trim_start_matches(['—', '-']).trim();
            let (scope, records) = match rest.split_once(" Records: ") {
                Some((scope, records)) => (
                    scope,
                    records
                        .trim_end_matches('.')
                        .split(',')
                        .map(|r| r.trim().to_string())
                        .filter(|r| !r.is_empty())
                        .collect(),
                ),
                None => (rest, Vec::new()),
            };
            let name = topic_path(&format!("{name}/x"));
            let name = name.split('/').next().unwrap_or_default().to_string();
            (!name.is_empty()).then(|| Area {
                name,
                scope: scope.trim().to_string(),
                records,
            })
        })
        .collect();
    Areas { list }
}

/// The folders of hand-written records in the checkout at `repo`: the
/// first folder under `.orion/decisions/` of each record in one.
pub(crate) fn record_folders(repo: &std::path::Path) -> Vec<String> {
    let mut folders: Vec<String> = records::records(&Checkout::Dir(repo.to_path_buf()))
        .into_iter()
        .filter_map(|r| {
            let rest = r.path.strip_prefix(DECISIONS)?.strip_prefix('/')?;
            let (folder, _) = rest.split_once('/')?;
            Some(folder.to_string())
        })
        .collect();
    folders.sort();
    folders.dedup();
    folders
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_folders_are_listed_under_their_nearest_seed() {
        let mut areas = Areas::default();
        for folder in [
            "brand-layouts",
            "clipboard",
            "editor-lines",
            "infrastructure",
            "mcp-slide-text",
            "search",
            "indicator-key",
        ] {
            areas.add_record_folder(folder);
        }
        let records = |areas: &Areas, name: &str| {
            areas
                .list
                .iter()
                .find(|a| a.name == name)
                .unwrap()
                .records
                .clone()
        };
        assert_eq!(
            records(&areas, "editor"),
            vec!["clipboard", "editor-lines", "indicator-key"]
        );
        assert_eq!(
            records(&areas, "brand-and-templates"),
            vec!["brand-layouts"]
        );
        assert_eq!(
            records(&areas, "platform-and-infra"),
            vec!["infrastructure"]
        );
        assert_eq!(records(&areas, "ai-and-mcp"), vec!["mcp-slide-text"]);
        assert_eq!(records(&areas, "workspace"), vec!["search"]);
        assert_eq!(areas.list.len(), SEED.len(), "no area added");
        // Listed once, however often it is seen.
        areas.add_record_folder("clipboard");
        assert_eq!(records(&areas, "editor").len(), 3);
    }

    #[test]
    fn a_topic_must_sit_in_a_listed_area() {
        let mut areas = Areas::default();
        areas.add_record_folder("indicator-key");
        let t = |raw: &str| areas.topic(raw);
        assert_eq!(t("editor/text-boxes"), "editor/text-boxes");
        assert_eq!(t("editor-shapes/curly"), "editor/curly");
        assert_eq!(t("indicator-key/delete"), "editor/delete");
        assert_eq!(t("mcp/branding"), "ai-and-mcp/branding");
        assert_eq!(
            t("data-migrations/backfills"),
            "platform-and-infra/backfills"
        );
        // The area says nothing; the topic's name does.
        assert_eq!(t("general/billing"), "access-and-accounts/billing");
        // Nothing does: the catch-all. A retired area is never one.
        assert_eq!(t("zzz/qqq"), "workspace/qqq");
        assert_eq!(t("engineering-tooling/ui-patterns"), "editor/ui-patterns");
        assert_eq!(t("testing-and-ci/ci-workflows"), "workspace/ci-workflows");
    }

    #[test]
    fn an_areas_md_listing_a_retired_area_loses_it() {
        let repo = tempfile::tempdir().unwrap();
        let store = Store::of(repo.path());
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(
            store.dir.join(FILE),
            "# Knowledge areas\n\n- `editor` — The editor.\n- `engineering-tooling` — How agents work.\n- `testing-and-ci` — Tests and CI.\n",
        )
        .unwrap();
        assert_eq!(Areas::load(&store).names(), vec!["editor"]);
        assert!(SEED.iter().all(|(name, _, _)| !RETIRED.contains(name)));
    }

    #[test]
    fn areas_md_reads_back_as_written() {
        let mut areas = Areas::default();
        areas.add_record_folder("infrastructure");
        areas.add_record_folder("clipboard");
        let text = areas.render();
        assert!(text.contains(
            "- `platform-and-infra` — Hosting, databases, storage, services, deploys, releases and status. Records: infrastructure.\n"
        ));
        assert_eq!(parse(&text), areas);
        for a in &areas.list {
            assert!(a.scope.split_whitespace().count() < 15, "{}", a.scope);
        }
    }

    #[test]
    fn the_checkouts_record_folders_become_part_of_its_areas() {
        let repo = tempfile::tempdir().unwrap();
        crate::knowledge::records::tests::decisions(repo.path());
        let store = Store::of(repo.path());
        assert_eq!(record_folders(repo.path()), vec!["auth"]);
        let areas = Areas::load(&store);
        assert_eq!(
            areas.area_for("auth").as_deref(),
            Some("access-and-accounts")
        );
        areas.save(&store).unwrap();
        let written = std::fs::read_to_string(store.dir.join(FILE)).unwrap();
        assert!(written.starts_with(super::super::store::MARKER));
        assert_eq!(Areas::load(&store), areas);
    }
}
