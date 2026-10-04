//! The two menus that reach orion's actions by name rather than by key:
//! the COMMAND PALETTE (`⌘⇧P`), every action with its key, and the OPEN
//! MENU (`⌘O`), what the cursor is on, outside orion. Both are context
//! menus with TYPE-AHEAD, and a row runs its action through
//! `event_loop::dispatch_action` — exactly what its key would have run.

use crate::app::{App, ContextMenu, MenuAction, MenuFilter, MenuItem, Overlay};
use crate::keymap::{Action, ACTIONS};

/// Actions the COMMAND PALETTE leaves out: the walk and the press keys —
/// they mean nothing without the key held under a cursor — the tab slots
/// (one row per digit is noise), the locked pane's own hatch, and the
/// palette itself.
fn listed(action: Action) -> bool {
    !matches!(
        action,
        Action::FocusLeft
            | Action::FocusRight
            | Action::MoveDown
            | Action::MoveUp
            | Action::FocusNext
            | Action::Activate
            | Action::SelectProjectTab(_)
            | Action::UnlockTerminal
            | Action::CommandPalette
    )
}

/// The keys this terminal can press for `action`, when it has any.
fn row_hint(app: &App, action: Action) -> Option<String> {
    let keys = app.keymap.shown_label(action);
    (keys != crate::keymap::UNBOUND).then_some(keys)
}

/// A centered TYPE-AHEAD menu over `items`, titled `title`.
fn filtered_menu(title: &str, items: Vec<MenuItem>) -> Overlay {
    Overlay::Menu(ContextMenu {
        title: Some(title.into()),
        filter: Some(MenuFilter {
            query: String::new(),
            all: items.clone(),
        }),
        items,
        at: None,
        hover: 0,
        area: ratatui::layout::Rect::default(),
        parent: None,
    })
}

/// `⌘⇧P`: every action orion has, by name and key, in the keymap's own
/// order — type to narrow, Enter runs it.
pub(super) fn open_command_palette(app: &mut App) {
    let items = ACTIONS
        .iter()
        .filter(|spec| listed(spec.action))
        .map(|spec| {
            // The editor row is named after the app it opens.
            let label = match spec.action {
                Action::OpenInCursor => {
                    format!("Open checkout in {}", crate::outside_editor::hint_name())
                }
                _ => spec.label.to_string(),
            };
            let mut item = MenuItem::new(label, MenuAction::RunAction(spec.action));
            if let Some(hint) = row_hint(app, spec.action) {
                item = item.with_hint(hint);
            }
            item
        })
        .collect();
    app.overlay = Some(filtered_menu("Command", items));
}

/// The OPEN MENU's rows: where `⌘O` can take what the cursor is on. The
/// editor row is named after the OPEN IN APP editor (`Checkout in VS
/// Code`) when the menu opens.
const OPEN_ROWS: &[(Action, &str)] = &[
    (Action::OpenRepo, "Repository on GitHub"),
    (Action::OpenPullRequest, "Pull request on GitHub"),
    (Action::OpenIssue, "Issue on GitHub"),
    (Action::OpenInCursor, "Checkout in"),
    (Action::OpenOutsideTerminal, "Terminal in the checkout"),
    (Action::OpenWorktree, "Open command"),
];

/// `⌘O` on the cards and panels: the OPEN MENU. Each row is the action
/// it names, so one with nothing to open says why, as its key would.
pub(super) fn open_outside_menu(app: &mut App) {
    let items = OPEN_ROWS
        .iter()
        .map(|(action, label)| {
            let label = match action {
                Action::OpenInCursor => {
                    format!("{label} {}", crate::outside_editor::hint_name())
                }
                _ => label.to_string(),
            };
            let mut item = MenuItem::new(label, MenuAction::RunAction(*action));
            if let Some(hint) = row_hint(app, *action) {
                item = item.with_hint(hint);
            }
            item
        })
        .collect();
    app.overlay = Some(filtered_menu("Open", items));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(app: &App) -> Vec<(String, MenuAction)> {
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("no menu open");
        };
        menu.items
            .iter()
            .map(|i| (i.label.clone(), i.action.clone()))
            .collect()
    }

    fn menu_hints(app: &App) -> Vec<(String, Option<String>)> {
        let Some(Overlay::Menu(menu)) = &app.overlay else {
            panic!("no menu open");
        };
        menu.items
            .iter()
            .map(|i| (i.label.clone(), i.hint.clone()))
            .collect()
    }

    #[test]
    fn the_command_palette_lists_actions_with_their_keys() {
        let mut app = App::new();
        open_command_palette(&mut app);
        let rows = rows(&app);
        assert!(rows.iter().any(|(label, action)| {
            *label == "Go to file" && *action == MenuAction::RunAction(Action::FindFile)
        }));
        assert!(menu_hints(&app)
            .iter()
            .any(|(label, hint)| label == "Go to file" && hint.as_deref() == Some("^P")));
        assert!(rows.iter().all(|(_, action)| match action {
            MenuAction::RunAction(action) => listed(*action),
            _ => false,
        }));
        assert!(!rows
            .iter()
            .any(|(_, action)| *action == MenuAction::RunAction(Action::CommandPalette)));
    }

    #[test]
    fn the_open_menu_names_every_way_out() {
        let mut app = App::new();
        open_outside_menu(&mut app);
        let actions: Vec<MenuAction> = rows(&app).into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            actions,
            OPEN_ROWS
                .iter()
                .map(|(action, _)| MenuAction::RunAction(*action))
                .collect::<Vec<_>>()
        );
    }
}
