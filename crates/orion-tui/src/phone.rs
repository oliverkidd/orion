//! The TUI's side of the PHONE BRIDGE (the daemon's `phone`, `away` and
//! `slack`): AWAY BY HAND (⌘⇧A, cleared by the next key), how the Slack
//! connection stands for Settings › Connections, and `c` / `o` there —
//! the three tokens pasted into the Keychain, or forgotten.

use orion_core::{ClientRequest, SlackStatus};

use crate::app::{App, Overlay, PromptKind};

use orion_core::secrets::{SLACK_ACCOUNTS, SLACK_SERVICE};

/// The three tokens, in the order `c` asks for them (`SLACK_ACCOUNTS`'s):
/// what each starts with, and where to find it.
pub const TOKENS: [(&str, &str); 3] = [
    (
        "xapp-",
        "App-level token (xapp-…): Basic Information › App-Level Tokens, with connections:write",
    ),
    (
        "xoxb-",
        "Bot token (xoxb-…): OAuth & Permissions › Bot User OAuth Token",
    ),
    (
        "xoxp-",
        "User token (xoxp-…): OAuth & Permissions › User OAuth Token",
    ),
];

/// ⌘⇧A: away by hand, or back.
pub fn toggle_away(app: &mut App) {
    let on = !app.away_by_hand;
    app.away_by_hand = on;
    app.outbox.push(ClientRequest::SetAway { on });
    let line = match (on, app.slack_status.workspace.is_some()) {
        (true, true) => {
            "Away: a session that starts waiting goes to your phone until your next key"
        }
        (true, false) => {
            "Away until your next key — connect Slack in Settings › Connections to reach your phone"
        }
        (false, _) => "Back at the desk",
    };
    app.flash = Some(crate::flash::Flash::note(line));
    app.dirty = true;
}

/// A key pressed anywhere: the first one after AWAY BY HAND went on says
/// you are back — unless it is the toggle itself, which does its own.
pub fn note_key(app: &mut App, key: &crossterm::event::KeyEvent) {
    if !app.away_by_hand {
        return;
    }
    let chord = crate::keymap::KeyChord::from_event(key);
    if app
        .keymap
        .chords(crate::keymap::Action::AwayToggle)
        .contains(&chord)
    {
        return;
    }
    app.away_by_hand = false;
    app.outbox.push(ClientRequest::SetAway { on: false });
}

/// `ServerEvent::AwayChanged`.
pub fn away_changed(app: &mut App, away: bool, by_hand: bool) {
    app.away = away;
    app.away_by_hand = by_hand;
    app.dirty = true;
}

/// `ServerEvent::SlackStatus`.
pub fn slack_changed(app: &mut App, status: SlackStatus) {
    app.slack_status = status;
    app.dirty = true;
}

/// The SLACK status row's value: the workspace, what is wrong, or that
/// there is nothing to connect.
pub fn status_label(status: &SlackStatus) -> String {
    match status {
        SlackStatus {
            problem: Some(problem),
            ..
        } => problem.clone(),
        SlackStatus {
            workspace: Some(workspace),
            ..
        } => workspace.clone(),
        SlackStatus {
            configured: true, ..
        } => "connecting…".into(),
        _ => crate::config::NOT_CONNECTED.into(),
    }
}

/// `c` on the SLACK status row: the first of the three token prompts.
pub fn connect(app: &mut App) {
    crate::event_loop::open_prompt(
        app,
        PromptKind::SlackToken {
            step: 0,
            tokens: Vec::new(),
        },
    );
}

/// The token prompt's title, label and prefill for `step`.
pub fn prompt_text(step: usize) -> (String, String) {
    let (_, what) = TOKENS[step.min(TOKENS.len() - 1)];
    (
        format!("Connect Slack · {} of {}", step + 1, TOKENS.len()),
        format!("{what} (empty = cancel)"),
    )
}

/// Enter on token prompt `step`: the next prompt, or — the third in — all
/// three saved and the daemon told to connect. A token that doesn't start
/// as it should is asked for again.
pub fn submit_token(app: &mut App, step: usize, mut tokens: Vec<String>, value: &str) {
    let value = value.trim();
    if value.is_empty() {
        crate::event_loop::reopen_settings(app);
        return;
    }
    let (prefix, _) = TOKENS[step];
    if !value.starts_with(prefix) {
        app.flash = Some(crate::flash::Flash::failed(format!(
            "that is not a {prefix}… token"
        )));
        crate::event_loop::open_prompt(app, PromptKind::SlackToken { step, tokens });
        return;
    }
    tokens.push(value.to_string());
    if step + 1 < TOKENS.len() {
        crate::event_loop::open_prompt(
            app,
            PromptKind::SlackToken {
                step: step + 1,
                tokens,
            },
        );
        return;
    }
    let saved = SLACK_ACCOUNTS
        .iter()
        .zip(&tokens)
        .try_for_each(|(account, token)| orion_core::secrets::set(SLACK_SERVICE, account, token));
    crate::event_loop::reopen_settings(app);
    match saved {
        Ok(()) => {
            app.outbox.push(ClientRequest::SlackReconnect);
            settings_note(app, "Slack: connecting…");
        }
        Err(why) => settings_note(app, format!("the tokens were not saved: {why}")),
    }
}

/// `o` on the SLACK status row: the tokens forgotten, the bridge down.
pub fn disconnect(app: &mut App) {
    for account in SLACK_ACCOUNTS {
        orion_core::secrets::delete(SLACK_SERVICE, account);
    }
    app.outbox.push(ClientRequest::SlackReconnect);
    settings_note(
        app,
        "Slack disconnected; its tokens are gone from the Keychain",
    );
}

fn settings_note(app: &mut App, text: impl Into<String>) {
    if let Some(Overlay::Settings(view)) = &mut app.overlay {
        view.info(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_row_says_where_it_stands() {
        assert_eq!(
            status_label(&SlackStatus::default()),
            crate::config::NOT_CONNECTED
        );
        let connecting = SlackStatus {
            configured: true,
            ..SlackStatus::default()
        };
        assert_eq!(status_label(&connecting), "connecting…");
        let up = SlackStatus {
            workspace: Some("Acme".into()),
            ..connecting.clone()
        };
        assert_eq!(status_label(&up), "Acme");
        let down = SlackStatus {
            problem: Some("Slack: boom".into()),
            ..up
        };
        assert_eq!(status_label(&down), "Slack: boom");
    }

    #[test]
    fn the_prompts_name_each_token_in_turn() {
        assert_eq!(prompt_text(0).0, "Connect Slack · 1 of 3");
        assert!(prompt_text(2).1.starts_with("User token (xoxp-…)"));
    }
}
