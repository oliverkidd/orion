//! The mouse in a text field, the same in every one: whichever field of
//! the modal that is up was drawn under the pointer takes the press — and
//! the caret, from whatever had it — and the drag and release that follow
//! it, as [`TextInput::mouse`] spells out. One table ([`fields`]) names
//! every modal's fields; a field missing from it is one the mouse can't
//! reach.

use crate::app::{App, Overlay};
use crate::text_input::TextInput;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;
use std::cell::Cell;

/// Which field of its modal a [`fields`] entry is: what clicking it hands
/// the caret to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    /// The modal's one field, or one that holds the caret whenever it is
    /// drawn: nothing to move.
    Only,
    /// The QUICK PROMPT's (or a prompt dialog's) text: a listing row
    /// highlighted lets Enter go back to the text.
    Prompt,
    Preset(crate::preset_overlays::PresetField),
    PrQuery,
    PrCreate(crate::pr_actions::CreateField),
    PrEdit(crate::pr_actions::edit::EditRow),
    LinearQuery,
    DiffFilter,
    AutofixNote,
    ReviewNote,
    /// The PR REVIEW's summary: the bar takes the keys.
    PrReviewSummary,
}

/// Every text field the modal that is up holds, with which it is.
fn fields(overlay: &mut Overlay) -> Vec<(Field, &mut TextInput)> {
    use crate::pr_actions::{CreateField, PrForm};
    use crate::preset_overlays::PresetField;
    match overlay {
        Overlay::Prompt(p) => vec![(Field::Prompt, &mut p.input)],
        Overlay::Palette(p) => vec![(Field::Only, &mut p.query)],
        Overlay::Files(v) => vec![(Field::Only, &mut v.query)],
        Overlay::Grep(v) => vec![(Field::Only, &mut v.query)],
        Overlay::Tree(v) => vec![(Field::Only, &mut v.filter)],
        // A pull request's viewer: its review's summary on the bar too.
        Overlay::Diff(v) => {
            let mut fields = vec![(Field::DiffFilter, &mut v.filter)];
            if let Some(review) = &mut v.review {
                fields.push((Field::PrReviewSummary, &mut review.summary));
            }
            fields
        }
        Overlay::Hosts(v) => v.input.iter_mut().map(|i| (Field::Only, i)).collect(),
        // A SYSTEM PRESET's name is orion's: drawn, never a field.
        Overlay::AgentPresetEditor(e) => {
            let mut fields = vec![(Field::Preset(PresetField::Text), &mut e.text)];
            if e.system.is_none() {
                fields.insert(0, (Field::Preset(PresetField::Name), &mut e.name));
            }
            fields
        }
        Overlay::PullRequests(v) => match v.form.as_deref_mut() {
            Some(PrForm::Create(form)) => vec![
                (Field::PrCreate(CreateField::From), &mut form.from),
                (Field::PrCreate(CreateField::Into), &mut form.into),
                (Field::PrCreate(CreateField::Title), &mut form.title),
                (Field::PrCreate(CreateField::Body), &mut form.body),
            ],
            Some(PrForm::Edit(form)) => {
                use crate::pr_actions::edit::{EditRow, StateChange};
                // The comment box is drawn only while closing or reopening.
                let commenting = form.state != StateChange::Keep;
                let mut fields = vec![
                    (Field::PrEdit(EditRow::Title), &mut form.title),
                    (Field::PrEdit(EditRow::Base), &mut form.base),
                ];
                if commenting {
                    fields.push((Field::PrEdit(EditRow::Comment), &mut form.comment));
                }
                fields
            }
            Some(PrForm::Merge(_)) => Vec::new(),
            None => vec![(Field::PrQuery, &mut v.query)],
        },
        Overlay::Linear(v) => vec![(Field::LinearQuery, &mut v.query)],
        Overlay::ProjectPicker(p) => vec![(Field::Only, &mut p.query)],
        Overlay::Autofix(form) => vec![(Field::AutofixNote, &mut form.note)],
        Overlay::WeekReview(v) => match v.mode {
            crate::week_review::Mode::Screen => vec![(Field::ReviewNote, &mut v.note)],
            crate::week_review::Mode::Page => Vec::new(),
        },
        // An open item field has every key, the filter's too.
        Overlay::Todos(v) => match &mut v.input {
            Some((_, input)) => vec![(Field::Only, input)],
            None => vec![(Field::Only, &mut v.query)],
        },
        Overlay::Menu(_)
        | Overlay::Onboard(_)
        | Overlay::WhatsNew(_)
        | Overlay::Confirm(_)
        | Overlay::Help(_)
        | Overlay::Settings(_)
        | Overlay::FileTabs(_)
        | Overlay::Usage(_)
        | Overlay::Conflicts(_)
        | Overlay::CleanWorktrees(_)
        | Overlay::OpenRepo(_)
        | Overlay::CloneRepo(_)
        | Overlay::AgentPresets(_) => Vec::new(),
    }
}

/// The caret to `field`, as the keys that walk the modal would put it
/// there.
fn focus(app: &mut App, field: Field) {
    match (&mut app.overlay, field) {
        (Some(Overlay::Prompt(p)), Field::Prompt) => p.hover = None,
        (Some(Overlay::AgentPresetEditor(e)), Field::Preset(to)) => {
            if e.field != to {
                e.filter.clear();
                e.field = to;
            }
        }
        (Some(Overlay::PullRequests(v)), Field::PrQuery) => {
            v.focus = crate::pr_modal::PrFocus::List;
        }
        (Some(Overlay::PullRequests(_)), Field::PrCreate(to)) => {
            crate::pr_actions::focus_create(app, to);
        }
        (Some(Overlay::PullRequests(_)), Field::PrEdit(row)) => {
            crate::pr_actions::edit::focus(app, row);
        }
        (Some(Overlay::Linear(v)), Field::LinearQuery) => {
            v.focus = crate::linear::LinearFocus::Search;
        }
        (Some(Overlay::Diff(v)), Field::DiffFilter) => {
            v.focus = crate::app::DiffFocus::Files;
        }
        (Some(Overlay::Autofix(form)), Field::AutofixNote) => {
            form.row = crate::autofix::Row::Note;
        }
        (Some(Overlay::WeekReview(view)), Field::ReviewNote) => {
            view.side = crate::week_review::Side::Form;
            view.row = crate::week_review::Row::Note;
        }
        (Some(Overlay::Diff(view)), Field::PrReviewSummary) => {
            view.focus = crate::app::DiffFocus::Bar;
            if let Some(review) = &mut view.review {
                review.focus = crate::pr_review::Focus::Bar;
                review.bar_row = crate::pr_review::bar::BarRow::Summary;
            }
        }
        _ => {}
    }
}

thread_local! {
    /// A field has the left button: a drag in it is under way.
    static HELD: Cell<bool> = const { Cell::new(false) };
}

/// Is a drag under way in a text field — the button pressed in one and
/// not yet let go (`App::mouse_held`)?
pub(crate) fn held() -> bool {
    HELD.with(Cell::get)
}

/// Hand `mouse` to the text field it is for, if any: a press on a field
/// (which takes the caret), the wheel over a box that scrolls, and the
/// drag and release of a press one took. Whether one did — the event is
/// then spent.
pub(crate) fn handle(app: &mut App, mouse: &MouseEvent) -> bool {
    let spent = route(app, mouse);
    let held = app
        .overlay
        .as_mut()
        .is_some_and(|overlay| fields(overlay).iter().any(|(_, input)| input.pressed()));
    HELD.with(|h| h.set(held));
    if spent {
        app.dirty = true;
    }
    spent
}

fn route(app: &mut App, mouse: &MouseEvent) -> bool {
    let Some(overlay) = &mut app.overlay else {
        return false;
    };
    let at = Position::new(mouse.column, mouse.row);
    let mut all = fields(overlay);
    let pressed = all.iter().position(|(_, input)| input.pressed());
    // Motion while a field has the button is the drag going on, whatever
    // button the host put on the report — as the panes take it.
    let mouse = match (mouse.kind, pressed) {
        (MouseEventKind::Moved, Some(_)) => MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            ..*mouse
        },
        (MouseEventKind::Moved, None) => return false,
        _ => *mouse,
    };
    let edit = match mouse.kind {
        MouseEventKind::Down(button) => {
            // A press anywhere lets every earlier press go: a release that
            // never came (the button let up off the window) ends there.
            for (_, input) in all.iter_mut() {
                input.release();
            }
            let Some(index) = all.iter().position(|(_, input)| input.under(at)) else {
                return false;
            };
            if button != MouseButton::Left {
                return false;
            }
            let field = all[index].0;
            drop(all);
            focus(app, field);
            // The caret moved; the fields, and their order, are as they were.
            let Some(overlay) = &mut app.overlay else {
                return false;
            };
            fields(overlay).swap_remove(index).1.mouse(&mouse)
        }
        MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left) => {
            match pressed {
                Some(index) => all[index].1.mouse(&mouse),
                None => return false,
            }
        }
        _ => match all.iter().position(|(_, input)| input.under(at)) {
            Some(index) => all[index].1.mouse(&mouse),
            None => return false,
        },
    };
    edit.consumed()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::FileFinder;
    use crate::preset_overlays::{AgentPresetEditor, PresetField};
    use crossterm::event::KeyModifiers;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn frame(app: &mut App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        terminal
    }

    /// The first cell of `needle` on screen.
    fn cell_of(terminal: &Terminal<TestBackend>, needle: &str) -> (u16, u16) {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            let line: Vec<&str> = (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect();
            let text = line.concat();
            if let Some(byte) = text.find(needle) {
                return (text[..byte].chars().count() as u16, y);
            }
        }
        panic!("{needle:?} is not on screen");
    }

    fn mouse(app: &mut App, kind: MouseEventKind, (column, row): (u16, u16)) -> bool {
        let event = MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        handle(app, &event)
    }

    fn editor(app: &App) -> &AgentPresetEditor {
        match &app.overlay {
            Some(Overlay::AgentPresetEditor(e)) => e,
            _ => panic!("the preset editor is not up"),
        }
    }

    /// In the PRESET EDITOR (Settings → Presets) a click on a box the caret
    /// is not in hands it the caret where it points, and a drag there
    /// selects; a click on the name takes the caret back up — the boxes
    /// type, click and select as the QUICK PROMPT's does.
    #[test]
    fn a_click_in_an_idle_box_takes_the_caret_there_and_a_drag_selects() {
        let mut app = App::new();
        let mut form = AgentPresetEditor::new(0);
        form.text.set_text("hello world");
        app.overlay = Some(Overlay::AgentPresetEditor(form));
        let terminal = frame(&mut app);
        assert_eq!(editor(&app).field, PresetField::Name);

        let (x, y) = cell_of(&terminal, "hello world");
        assert!(mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            (x + 6, y)
        ));
        assert_eq!(editor(&app).field, PresetField::Text);
        assert_eq!(editor(&app).text.cursor_chars(), 6);
        frame(&mut app);
        assert!(mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            (x + 11, y)
        ));
        assert!(mouse(
            &mut app,
            MouseEventKind::Up(MouseButton::Left),
            (x + 11, y)
        ));
        assert_eq!(editor(&app).text.selected(), Some("world"));

        let terminal = frame(&mut app);
        let (x, y) = cell_of(&terminal, "(required)");
        assert!(mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            (x, y)
        ));
        assert_eq!(editor(&app).field, PresetField::Name);
        // Off every field the mouse is the modal's.
        assert!(!mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            (0, 0)
        ));
    }

    /// A filter row — every fuzzy modal's — clicks and drags like any
    /// other field.
    #[test]
    fn a_drag_selects_in_a_filter_row() {
        let mut app = App::new();
        let mut finder = FileFinder::new(
            "/nonexistent-orion-field-mouse-test".into(),
            "main".into(),
            "vim".into(),
            vec!["src/alpha.rs".into()],
        );
        finder.query.set_text("alpha beta");
        app.overlay = Some(Overlay::Files(finder));
        let terminal = frame(&mut app);
        let (x, y) = cell_of(&terminal, "alpha beta");
        assert!(
            !mouse(&mut app, MouseEventKind::Moved, (x, y)),
            "no press, no drag"
        );
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), (x, y));
        assert!(held() && app.mouse_held(), "the button is the field's");
        assert!(mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            (x + 5, y)
        ));
        // A report that lost the button is the drag going on.
        assert!(mouse(&mut app, MouseEventKind::Moved, (x + 4, y)));
        let query = |app: &App| match &app.overlay {
            Some(Overlay::Files(finder)) => finder.query.selected().map(str::to_string),
            _ => panic!("the finder closed"),
        };
        assert_eq!(query(&app).as_deref(), Some("alph"));
        mouse(&mut app, MouseEventKind::Up(MouseButton::Left), (x + 5, y));
        assert!(!held() && !app.mouse_held());
        assert_eq!(
            query(&app).as_deref(),
            Some("alph"),
            "kept past the release"
        );
    }
}
