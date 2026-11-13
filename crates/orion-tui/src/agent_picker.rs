//! The one AGENT KIND picker behind every launch surface — the NEW AGENT
//! PICKER (`n` on the grid, which opens the QUICK PROMPT on the pick; from
//! a menu's "New agent" row it launches outright) and the QUICK PROMPT's
//! `Tab`. Each is one `ContextMenu` with a row per harness still enabled
//! on the AGENTS TAB (a disabled one is absent, not greyed), every row a
//! `MenuAction::NewAgentOfKind` carrying the surface's launch context, so
//! `→` drills into the same MODEL / EFFORT submenus everywhere.

use crate::app::{App, ContextMenu, MenuAction, MenuItem, Overlay};
use crate::config::Config;
use crate::pull_request::PrLaunch;
use crate::quick_prompt::QuickReturn;
use orion_core::{Agent, AgentKind, WorktreeId};

/// Shown instead of a picker when a hand-edited config has switched every
/// harness off (the AGENTS TAB refuses to turn off the last one).
pub(crate) const NO_HARNESS_FLASH: &str =
    "every harness is disabled — enable one in Settings › Agents";

/// The NEW AGENT PICKER's title, and the label of **New agent — choose
/// harness**, the action that opens it: the same new agent `⌘N` starts,
/// with its harness asked first. It names an AGENT, never a session — a
/// session is an agent or a terminal, and this only ever starts the first.
pub(crate) const NEW_AGENT_PICKER_TITLE: &str = "New agent — choose harness";

/// What one kind picker opens with: its title, where its rows launch, and
/// the context every row carries.
#[derive(Debug, Clone)]
pub(crate) struct KindPicker {
    pub title: String,
    pub worktree: WorktreeId,
    /// The QUICK PROMPT box owed back (its `Tab` picker).
    pub quick: Option<Box<QuickReturn>>,
    /// The row to start on: the surface's own ask (a QUICK PROMPT picker
    /// opens on its box's harness); None is the first row, which one not
    /// offered any more falls to as well.
    pub hover: Option<HarnessRow>,
}

impl KindPicker {
    /// The NEW AGENT PICKER: plain rows into `worktree`.
    pub fn new_session(worktree: WorktreeId) -> Self {
        Self {
            title: NEW_AGENT_PICKER_TITLE.into(),
            worktree,
            quick: None,
            hover: None,
        }
    }

    /// The QUICK PROMPT's `Tab` picker: every row hands the box back,
    /// and the cursor starts on the harness the box is already set to.
    /// `worktree` is the checkout the menu is built against, not where
    /// the launch lands — that stays the box's own `QuickLaunch::target`.
    pub fn quick_prompt(worktree: WorktreeId, back: QuickReturn) -> Self {
        Self {
            title: "Quick prompt agent".into(),
            ..Self::new_session_box(worktree, back)
        }
    }

    /// `n` on the grid: the NEW AGENT PICKER with no box up yet — the
    /// harness is asked first, and Enter on a row OPENS the QUICK PROMPT
    /// set to it (`back.from_box` is false: Esc closes the picker and
    /// opens nothing). The cursor starts on the harness the box would
    /// have opened on — the Settings → Accounts one — so `Enter` at once
    /// is `p`.
    pub fn new_session_box(worktree: WorktreeId, back: QuickReturn) -> Self {
        Self {
            title: NEW_AGENT_PICKER_TITLE.into(),
            worktree,
            hover: Some(HarnessRow {
                kind: back.launch.kind,
                custom: back.launch.custom.clone(),
            }),
            quick: Some(Box::new(back)),
        }
    }
}

/// One launch row: a built-in kind, or a custom registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HarnessRow {
    pub kind: AgentKind,
    /// Registry id when `kind` is [`AgentKind::Custom`].
    pub custom: Option<String>,
}

/// The harnesses the pickers offer — enabled built-ins plus usable custom
/// entries — or None, with the FLASH set, when a hand-edited config left
/// nothing to offer. An empty `ContextMenu` panics on Enter and `j`, so no
/// caller opens one.
pub(crate) fn enabled_harnesses_or_flash(app: &mut App, cfg: &Config) -> Option<Vec<HarnessRow>> {
    let rows: Vec<HarnessRow> = cfg
        .offered_harnesses()
        .into_iter()
        .map(|(kind, custom)| HarnessRow { kind, custom })
        .collect();
    if rows.is_empty() {
        app.flash = Some(crate::flash::Flash::setup(NO_HARNESS_FLASH));
        return None;
    }
    Some(rows)
}

/// One `NewAgentOfKind` row per harness, labelled by `label`, every row
/// carrying the same launch context (`→` on any of them drills into that
/// kind's MODEL submenu with the context intact; customs offer no EFFORT
/// submenu — see `effort_choices`). A harness whose sessions can't show
/// every status says so on its row, dim at the right edge
/// ([`orion_core::harness::HarnessDescriptor::status_note`]).
pub(crate) fn kind_rows(
    rows: &[HarnessRow],
    worktree: &WorktreeId,
    pr: Option<&PrLaunch>,
    quick: Option<&QuickReturn>,
    label: impl Fn(AgentKind, Option<&str>) -> String,
) -> Vec<MenuItem> {
    let registry = Config::load().harness_registry();
    rows.iter()
        .map(|row| {
            let item = MenuItem::new(
                label(row.kind, row.custom.as_deref()),
                MenuAction::NewAgentOfKind {
                    worktree: worktree.clone(),
                    kind: row.kind,
                    custom: row.custom.clone(),
                    model: None,
                    effort: None,
                    pr: pr.cloned(),
                    quick: quick.map(|back| Box::new(back.clone())),
                },
            );
            match entry_in(&registry, row.kind, row.custom.as_deref())
                .and_then(|entry| entry.status_note())
            {
                Some(note) => item.with_note(note),
                None => item,
            }
        })
        .collect()
}

/// The entry of `registry` a row launches, while the registry names it.
fn entry_in<'a>(
    registry: &'a [orion_core::harness::HarnessDescriptor],
    kind: AgentKind,
    custom: Option<&str>,
) -> Option<&'a orion_core::harness::HarnessDescriptor> {
    let id = orion_core::harness::registry_id(kind, custom);
    if id.is_empty() {
        return None;
    }
    registry.iter().find(|entry| entry.id == id)
}

/// The label a picker row shows: the registry entry's label for a known
/// id (falling back to the id when it carries none) — a CLAUDE ACCOUNT's
/// `Claude (a@b.co)`, built-in Claude's included — and the kind name
/// otherwise, including for a custom id the registry no longer names.
pub(crate) fn harness_label(kind: AgentKind, custom: Option<&str>) -> String {
    match entry_in(&Config::load().harness_registry(), kind, custom) {
        Some(entry) => entry.display_label().to_string(),
        None => kind_label(kind).to_string(),
    }
}

/// Open `picker` as the OVERLAY, or FLASH when no harness is enabled.
pub(crate) fn open_kind_picker(app: &mut App, picker: KindPicker) {
    let cfg = Config::load();
    let Some(rows) = enabled_harnesses_or_flash(app, &cfg) else {
        return;
    };
    let KindPicker {
        title,
        worktree,
        quick,
        hover,
    } = picker;
    let hover = hover
        .and_then(|wanted| {
            rows.iter().position(|row| {
                row.kind == wanted.kind && row.custom.as_deref() == wanted.custom.as_deref()
            })
        })
        .unwrap_or(0);
    let mut items = kind_rows(&rows, &worktree, None, quick.as_deref(), harness_label);
    // Each account's headroom at the right, `41% · 2h`, where the row has
    // no note of its own, so the pick can be made on what is left.
    let now = orion_core::clock::now_secs() as i64;
    for (item, row) in items.iter_mut().zip(&rows) {
        let id = orion_core::harness::registry_id(row.kind, row.custom.as_deref());
        let room = crate::usage::headroom(app, id).map(|room| room.short(now));
        if let Some(room) = room.filter(|r| !r.is_empty() && item.hint.is_none()) {
            item.hint = Some(room);
            item.note = true;
        }
    }
    app.overlay = Some(Overlay::Menu(ContextMenu {
        title: Some(title),
        items,
        at: None,
        hover,
        area: ratatui::layout::Rect::default(),
        parent: None,
        filter: None,
        message: None,
    }));
}

/// The same badge against a config already in hand. A CLAUDE ACCOUNT
/// says what it says on every card — the email, with more than one
/// account about ([`crate::claude_accounts::short_name`]), else its id —
/// never its whole `Claude (a@b.co)` label, which a card has no room for.
pub(crate) fn session_harness_badge_in(agent: &Agent, cfg: &Config) -> String {
    if let Some(name) =
        crate::claude_accounts::short_name(agent.kind, agent.custom_harness.as_deref())
    {
        return name;
    }
    match (agent.kind, agent.custom_harness.as_deref()) {
        (AgentKind::Claude, Some(id)) => id.to_string(),
        (AgentKind::Custom, Some(id)) => {
            let entry = cfg.effective_harness_by_id(id);
            if entry.is_claude_account() {
                return id.to_string();
            }
            entry.display_label().to_string()
        }
        _ => agent.kind.as_str().to_string(),
    }
}

pub(crate) fn kind_label(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Claude => "Claude",
        AgentKind::Codex => "Codex",
        AgentKind::Cursor => "Cursor",
        AgentKind::Grok => "Grok Build",
        AgentKind::OpenCode => "OpenCode",
        // Custom rows label through `harness_label` (the entry's own
        // label); this is only the fallback when its entry is gone.
        AgentKind::Custom => "Custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SubmenuKind;
    use crate::pull_request::OpenPr;
    use crate::quick_prompt::QuickLaunch;

    const PR_URL: &str = "https://github.com/o/r/pull/7";
    const PR_HEAD: &str = "attach-links";

    /// Pin the config to a temp file holding `json`: every picker reads
    /// `Config::load`, and the dev's real file must stay out of it.
    fn pinned<T>(json: &str, f: impl FnOnce() -> T) -> T {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, json).unwrap();
        crate::config::with_config_path(path, f)
    }

    fn open_pr() -> OpenPr {
        OpenPr {
            number: 7,
            title: "Attach links".into(),
            url: PR_URL.into(),
            answered_draft: false,
            answered: Default::default(),
            head: PR_HEAD.into(),
            mine: false,
            head_sha: String::new(),
            meta: Default::default(),
        }
    }

    fn labels(menu: &ContextMenu) -> Vec<&str> {
        menu.items.iter().map(|item| item.label.as_str()).collect()
    }

    /// Every row is the same launch context under a different harness, so
    /// a submenu drilled from any row keeps that context — including the
    /// custom registry id.
    #[test]
    fn kind_rows_carry_the_launch_context_into_every_row() {
        let worktree = WorktreeId("w1".into());
        let pr = PrLaunch::of(&open_pr());
        let harness_rows: Vec<HarnessRow> = AgentKind::ALL
            .into_iter()
            .filter(|kind| *kind != AgentKind::Custom)
            .map(|kind| HarnessRow { kind, custom: None })
            .chain([HarnessRow {
                kind: AgentKind::Custom,
                custom: Some("agy".into()),
            }])
            .collect();
        let rows = kind_rows(&harness_rows, &worktree, Some(&pr), None, |kind, custom| {
            format!("New {} session", harness_label(kind, custom))
        });
        assert!(rows.iter().any(|row| row.label == "New Codex session"));
        assert!(rows.iter().any(|row| row.label == "New Custom session"));
        for (row, expected) in rows.iter().zip(harness_rows.iter()) {
            assert!(
                matches!(
                    &row.action,
                    MenuAction::NewAgentOfKind {
                        worktree,
                        kind,
                        custom,
                        model: None,
                        effort: None,
                        pr: Some(pr),
                        quick: None,
                    } if worktree.as_str() == "w1"
                        && *kind == expected.kind
                        && custom.as_deref() == expected.custom.as_deref()
                        && pr.url == PR_URL
                        && pr.head == PR_HEAD
                ),
                "{row:?}"
            );
            assert_eq!(row.action.submenu(), Some(SubmenuKind::Models));
        }
    }

    /// `harnesses` map entries ride after the built-ins under their own labels;
    /// disabled, invalid and (under the hide switch) missing ones stay
    /// out, and their rows carry the registry id into the launch.
    #[test]
    fn picker_lists_usable_custom_entries_after_the_builtins() {
        let json = r#"{"harnesses": {
            "agy": {"label": "Agy"},
            "off": {"label": "Off", "enabled": false},
            "broken": {"label": "Broken", "env": {"MY-VAR": "x"}}
        }}"#;
        pinned(json, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            let names = labels(menu);
            assert_eq!(names.last(), Some(&"Agy"));
            assert!(!names.contains(&"Off"), "{names:?}");
            assert!(!names.contains(&"Broken"), "{names:?}");
            let agy = menu.items.iter().find(|item| item.label == "Agy").unwrap();
            assert!(matches!(
                &agy.action,
                MenuAction::NewAgentOfKind {
                    kind: AgentKind::Custom,
                    custom: Some(id),
                    ..
                } if id == "agy"
            ));
            assert_eq!(agy.action.submenu(), Some(SubmenuKind::Models));
        });
    }

    /// A row says what its sessions can't show: Grok never shows that it
    /// waits on you, a CLI with no hook dialect shows only its process —
    /// and the rest say nothing. The note is the row's dim edge, never
    /// part of its name.
    #[test]
    fn picker_rows_note_the_status_their_sessions_cannot_show() {
        use orion_core::harness::{NO_WAITING_NOTE, PROCESS_STATUS_NOTE};
        let json = r#"{"harnesses": {"agy": {"label": "Agy", "program": "agy"}}}"#;
        pinned(json, || {
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(WorktreeId("w1".into())));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            let note = |label: &str| {
                let item = menu.items.iter().find(|i| i.label == label).unwrap();
                assert_eq!(item.note, item.hint.is_some(), "{label}");
                item.hint.clone()
            };
            assert_eq!(note("Grok Build").as_deref(), Some(NO_WAITING_NOTE));
            assert_eq!(note("Agy").as_deref(), Some(PROCESS_STATUS_NOTE));
            for label in ["Codex", "Cursor", "OpenCode"] {
                assert_eq!(note(label), None, "{label}");
            }
            assert_eq!(note(&harness_label(AgentKind::Claude, None)), None);

            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
            terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();
            let shot: String = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(shot.contains(NO_WAITING_NOTE), "drawn on the row");
        });
    }

    /// The Accounts tab's per-entry Enabled row is the toggle now: flip
    /// it, save, and the picker drops the entry until it flips back.
    #[test]
    fn accounts_tab_toggles_custom_entries_off_the_picker() {
        use crate::config::{locate_agent, HarnessField};
        let json = r#"{"harnesses": {
            "agy": {"label": "Agy", "program": "agy"}
        }}"#;
        pinned(json, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert!(labels(menu).contains(&"Agy"));

            let mut cfg = Config::load();
            let (tab, row) = locate_agent("agy", HarnessField::Enabled).unwrap();
            cfg.cycle(tab, row, 0);
            cfg.save().unwrap();

            open_kind_picker(&mut app, KindPicker::new_session(worktree));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert!(!labels(menu).contains(&"Agy"), "{:?}", labels(menu));

            let mut cfg = Config::load();
            let (tab, row) = locate_agent("agy", HarnessField::Enabled).unwrap();
            cfg.cycle(tab, row, 0);
            cfg.save().unwrap();
            assert!(
                Config::load()
                    .offered_harnesses()
                    .contains(&(AgentKind::Custom, Some("agy".into()))),
                "flipping back re-offers the entry"
            );
        });
    }

    /// The surfaces differ only in title, context and starting row;
    /// a harness switched off on the AGENTS TAB is missing from all of them.
    #[test]
    fn every_surface_opens_the_same_rows_minus_disabled_harnesses() {
        pinned(r#"{"codex_enabled": false}"#, || {
            let worktree = WorktreeId("w1".into());
            let mut app = App::new();
            // Every built-in harness but the one switched off, in the ALL
            // order. A bare Custom kind is never offered (entries come
            // from the registry), and the pinned config defines none.
            let offered: Vec<&str> = AgentKind::ALL
                .iter()
                .filter(|kind| **kind != AgentKind::Codex && **kind != AgentKind::Custom)
                .map(|kind| kind_label(*kind))
                .collect();
            assert!(offered.starts_with(&["Claude", "Cursor"]), "{offered:?}");

            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New agent — choose harness"));
            assert_eq!(labels(menu), offered);
            assert_eq!(menu.hover, 0);

            let back = QuickReturn {
                launch: QuickLaunch {
                    target: crate::quick_prompt::QuickTarget::Worktree(worktree.clone()),
                    kind: AgentKind::Cursor,
                    custom: None,
                    model: None,
                    effort: None,
                    preset: None,
                    pr: None,
                    linear: None,
                    todo: None,
                    under: None,
                    mode: orion_core::AgentMode::Edit,
                    automation: None,
                },
                text: "typed so far".into(),
                from_box: true,
                into_pane: None,
            };
            open_kind_picker(
                &mut app,
                KindPicker::quick_prompt(worktree.clone(), back.clone()),
            );
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("Quick prompt agent"));
            assert_eq!(labels(menu), offered);
            assert_eq!(menu.hover, 1, "starts on the box's own harness");
            assert!(menu.items.iter().all(|item| matches!(
                &item.action,
                MenuAction::NewAgentOfKind { pr: None, quick: Some(back), .. }
                    if back.text == "typed so far"
            )));

            // `n` on the grid: the same rows under the NEW AGENT PICKER's title,
            // owing a box that is not up yet.
            open_kind_picker(
                &mut app,
                KindPicker::new_session_box(
                    worktree.clone(),
                    QuickReturn {
                        text: String::new(),
                        from_box: false,
                        ..back
                    },
                ),
            );
            let Some(Overlay::Menu(menu)) = &app.overlay else {
                panic!("{:?}", app.overlay);
            };
            assert_eq!(menu.title.as_deref(), Some("New agent — choose harness"));
            assert_eq!(labels(menu), offered);
            assert_eq!(menu.hover, 1, "starts on the harness the box would open on");
            assert!(menu.items.iter().all(|item| matches!(
                &item.action,
                MenuAction::NewAgentOfKind { pr: None, quick: Some(back), .. }
                    if !back.from_box && back.text.is_empty()
            )));
        });
    }

    /// The NEW AGENT picker opens on its first row whatever
    /// `quick_prompt_kind` says — a pick is one session's — and the QUICK
    /// PROMPT's picker on its own box's harness.
    #[test]
    fn pickers_open_on_the_first_row_or_the_boxs_own_harness() {
        let worktree = WorktreeId("w1".into());
        fn hover_of(app: &App) -> usize {
            match &app.overlay {
                Some(Overlay::Menu(menu)) => menu.hover,
                other => panic!("expected a picker, got {other:?}"),
            }
        }

        pinned(r#"{"quick_prompt_kind": "cursor"}"#, || {
            let mut app = App::new();
            open_kind_picker(&mut app, KindPicker::new_session(worktree.clone()));
            assert_eq!(hover_of(&app), 0, "the quick prompt's harness is its own");

            let back = QuickReturn {
                launch: QuickLaunch {
                    target: crate::quick_prompt::QuickTarget::Worktree(worktree.clone()),
                    kind: AgentKind::Codex,
                    custom: None,
                    model: None,
                    effort: None,
                    preset: None,
                    pr: None,
                    linear: None,
                    todo: None,
                    under: None,
                    mode: orion_core::AgentMode::Edit,
                    automation: None,
                },
                text: String::new(),
                from_box: true,
                into_pane: None,
            };
            open_kind_picker(&mut app, KindPicker::quick_prompt(worktree.clone(), back));
            assert_eq!(hover_of(&app), 1, "the box's own harness");
        });
    }

    /// Only a hand-edited config reaches an empty list: the picker flashes
    /// instead of opening.
    #[test]
    fn no_harness_flashes_the_picker() {
        pinned(
            r#"{"claude_enabled": false, "codex_enabled": false, "cursor_enabled": false, "opencode_enabled": false, "harnesses":{"grok":{"enabled":false}}}"#,
            || {
                let worktree = WorktreeId("w1".into());
                let mut app = App::new();
                open_kind_picker(&mut app, KindPicker::new_session(worktree));
                assert!(app.overlay.is_none(), "{:?}", app.overlay);
                assert_eq!(app.flash.as_deref(), Some(NO_HARNESS_FLASH));
            },
        );
    }
}
