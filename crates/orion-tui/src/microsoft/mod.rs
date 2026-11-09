//! MICROSOFT 365: the Connections tab's sign-in, the calendar behind the
//! footer's next meeting, and the Graph reads meeting takeaways build on.
//!
//! - [`auth`]: the device-code sign-in and the access token after it.
//! - [`graph`]: GETs of Microsoft Graph under that token.
//! - [`calendar`]: the next twelve hours of meetings, polled.
//! - [`transcripts`]: a finished Teams meeting's transcript, as text.
//!
//! Sign-in needs the organisation's Entra app: `microsoft.client_id` (and
//! `tenant`) in `config.json`. Until it is there the status row says so
//! ([`NEEDS_APP_ID`]), nothing calls Microsoft, and `c` asks for the two
//! IDs ([`ask_app_id`]) and saves them there. Signed in, the refresh
//! token and the email sit in the Keychain under `dev.orion.microsoft`.
//!
//! Everything that touches the network runs off the loop and lands as an
//! [`Answer`] ([`land`]).

pub mod auth;
pub mod calendar;
pub mod graph;
pub mod transcripts;

use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{App, Overlay, PromptKind};
use crate::config::MicrosoftSettings;

/// The Keychain service: `dev.orion.microsoft`.
pub const SERVICE: &str = "microsoft";
/// …its refresh token.
pub const REFRESH: &str = "refresh";
/// …and the email it signed in as, for the status row.
pub const EMAIL: &str = "email";

/// The status row with no Entra app to sign in through.
pub const NEEDS_APP_ID: &str = "needs your organisation's app id · c to add it";

/// The two IDs `c` asks for with no app, in order: the Entra app's
/// Overview page lists both.
pub const APP_IDS: [&str; 2] = ["Application (client) ID", "Directory (tenant) ID"];

/// Where a sign-in is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SignIn {
    #[default]
    Idle,
    /// Asking Entra for a code.
    Asking,
    /// Waiting for `code` to be typed at `uri`.
    Code { code: String, uri: String },
    /// The last sign-in failed, and why.
    Failed(String),
}

/// What runs off the loop and lands on it.
#[derive(Debug)]
pub enum Answer {
    SignIn(Step),
    Calendar(Result<Vec<calendar::Event>, String>),
}

/// A sign-in's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Code { code: String, uri: String },
    Done { email: String },
    Failed(String),
}

/// The Microsoft 365 connection's state on the app.
#[derive(Debug, Default)]
pub struct State {
    /// Where answers land: the loop's channel.
    pub tx: Option<UnboundedSender<Answer>>,
    /// Who is signed in; None while nobody is.
    pub email: Option<String>,
    pub sign_in: SignIn,
    /// The running sign-in, so `o` or another `c` can stop it.
    pub sign_in_task: Option<tokio::task::AbortHandle>,
    /// The calendar from now to twelve hours on, start order: the footer's
    /// next meeting, and the meetings takeaways watch end.
    pub events: Vec<calendar::Event>,
    /// A calendar read is out.
    pub calendar_in_flight: bool,
    /// When the calendar is next read; None reads it on the next tick.
    pub next_read: Option<tokio::time::Instant>,
    /// **Next meeting in the footer**, mirrored from the config.
    pub footer: bool,
}

/// The loop's beat for Microsoft 365 while signed in: the footer's
/// countdown repaints, and the calendar is read when it is due.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(60);

impl State {
    /// Signed in, with an app to sign in through.
    pub fn connected(&self, ms: &MicrosoftSettings) -> bool {
        self.email.is_some() && has_app(ms)
    }
}

/// The organisation's Entra app is named in the config.
pub fn has_app(ms: &MicrosoftSettings) -> bool {
    !ms.client_id.trim().is_empty()
}

/// At start: who signed in last time, from the Keychain — only once there
/// is an app to have signed in through, so a config without one asks the
/// Keychain nothing.
pub fn load(app: &mut App, ms: &MicrosoftSettings) {
    if has_app(ms) {
        app.microsoft.email = orion_core::secrets::get(SERVICE, EMAIL);
    }
}

/// The MICROSOFT 365 status row's value.
pub fn status(app: &App, ms: &MicrosoftSettings) -> String {
    if !has_app(ms) {
        return NEEDS_APP_ID.into();
    }
    match (&app.microsoft.sign_in, &app.microsoft.email) {
        (SignIn::Asking, _) => "signing in…".into(),
        (SignIn::Code { code, .. }, _) => format!("enter {code} in the browser"),
        (_, Some(email)) => email.clone(),
        (SignIn::Failed(why), None) => format!("{} · {why}", crate::config::NOT_CONNECTED),
        (SignIn::Idle, None) => crate::config::NOT_CONNECTED.into(),
    }
}

/// `c` on the status row: start a sign-in. With no app to sign in
/// through, or already signed in, it says so instead. The answer is the
/// line the settings overlay shows.
pub fn connect(app: &mut App, ms: &MicrosoftSettings) -> String {
    if !has_app(ms) {
        return NEEDS_APP_ID.into();
    }
    if let Some(email) = &app.microsoft.email {
        return format!("signed in as {email} · o signs out");
    }
    let Some(tx) = app.microsoft.tx.clone() else {
        return "Microsoft 365 can't be reached from here".into();
    };
    stop_sign_in(app);
    app.microsoft.sign_in = SignIn::Asking;
    let task = tokio::spawn(sign_in(ms.clone(), tx));
    app.microsoft.sign_in_task = Some(task.abort_handle());
    "asking Microsoft for a sign-in code…".into()
}

/// `o` on the status row: sign out — or stop a sign-in under way. With
/// nobody signed in and nothing under way, the app's IDs go from the
/// config instead, so `c` asks for them again.
pub fn disconnect(app: &mut App, ms: &MicrosoftSettings) -> String {
    let signing_in = !matches!(app.microsoft.sign_in, SignIn::Idle | SignIn::Failed(_));
    if app.microsoft.email.is_none() && !signing_in && has_app(ms) {
        return forget_app(app);
    }
    let was = app.microsoft.email.take();
    stop_sign_in(app);
    app.microsoft.sign_in = SignIn::Idle;
    app.microsoft.events.clear();
    app.microsoft.next_read = None;
    std::thread::spawn(auth::forget);
    match (was, signing_in) {
        (Some(email), _) => format!("signed out {email} · o again clears the app IDs"),
        (None, true) => "sign-in stopped".into(),
        (None, false) => crate::config::NOT_CONNECTED.into(),
    }
}

/// A second `o`: `microsoft.client_id` and `tenant` back to their
/// defaults in the config, the last failed sign-in forgotten.
fn forget_app(app: &mut App) -> String {
    app.microsoft.sign_in = SignIn::Idle;
    let defaults = MicrosoftSettings::default();
    match save_app(&defaults.client_id, &defaults.tenant) {
        Ok(_) => "app IDs cleared · c adds them again".into(),
        Err(why) => format!("the app IDs were not cleared: {why}"),
    }
}

/// The client and tenant ids written into the config through its save
/// path, every other key kept; the settings they make.
fn save_app(client_id: &str, tenant: &str) -> std::io::Result<MicrosoftSettings> {
    let mut cfg = crate::config::Config::load();
    cfg.microsoft.client_id = client_id.into();
    cfg.microsoft.tenant = tenant.into();
    cfg.save()?;
    Ok(cfg.microsoft)
}

/// `text` as a GUID — `8-4-4-4-12` hex digits, spaces around it trimmed —
/// in lower case; None when it is not one.
pub fn guid(text: &str) -> Option<String> {
    let text = text.trim();
    let groups: Vec<&str> = text.split('-').collect();
    let shaped = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && g.bytes().all(|b| b.is_ascii_hexdigit()));
    shaped.then(|| text.to_ascii_lowercase())
}

/// What a typed app-id box holds.
#[derive(Debug, PartialEq, Eq)]
pub enum Typed {
    /// Nothing: nothing is saved.
    Cancel,
    /// The client id alone: the tenant is asked for next.
    Client(String),
    /// Both, ready to save.
    Both { client_id: String, tenant: String },
    /// Not what was asked for; the box stays up with this.
    Bad(&'static str),
}

const NOT_A_GUID: &str = "that is not an ID (8-4-4-4-12 hex digits)";

/// The box's `value`, `client_id` being what the first box gave (None
/// while it is the first). The first box also takes both IDs at once —
/// a line pasted from a teammate — apart by a space, comma or newline.
pub fn read_ids(client_id: Option<&str>, value: &str) -> Typed {
    let parts: Vec<&str> = value
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|p| !p.is_empty())
        .collect();
    match (client_id, parts.as_slice()) {
        (_, []) => Typed::Cancel,
        (None, [one]) => guid(one).map_or(Typed::Bad(NOT_A_GUID), Typed::Client),
        (None, [client, tenant]) => match (guid(client), guid(tenant)) {
            (Some(client_id), Some(tenant)) => Typed::Both { client_id, tenant },
            _ => Typed::Bad("those are not two IDs (8-4-4-4-12 hex digits each)"),
        },
        (Some(client_id), [one]) => {
            guid(one).map_or(Typed::Bad(NOT_A_GUID), |tenant| Typed::Both {
                client_id: client_id.into(),
                tenant,
            })
        }
        (None, _) => Typed::Bad("two IDs at most: the client ID, then the tenant ID"),
        (Some(_), _) => Typed::Bad("just the Directory (tenant) ID here"),
    }
}

/// `c` (or Enter) on the status row with no app: the first ID's box.
pub fn ask_app_id(app: &mut App) {
    crate::event_loop::open_prompt(app, PromptKind::MicrosoftAppId { client_id: None });
}

/// The app-id box's title and label, `client_id` being the first box's
/// answer once there is one.
pub fn app_id_prompt_text(client_id: Option<&str>) -> (String, String) {
    let step = usize::from(client_id.is_some());
    let label = match step {
        0 => format!(
            "{} from the Entra app's Overview, or both IDs on one line (empty = cancel)",
            APP_IDS[0]
        ),
        _ => format!("{} from the same Overview (empty = cancel)", APP_IDS[1]),
    };
    (
        format!("Connect Microsoft 365 · {} of {}", step + 1, APP_IDS.len()),
        label,
    )
}

/// Enter in the app-id box: the tenant's box next, or — both in hand —
/// both saved to the config and the sign-in started. Something that is
/// not an ID keeps the box up, as typed, with why; empty saves nothing.
pub fn submit_app_id(app: &mut App, client_id: Option<String>, value: &str) {
    let note = match read_ids(client_id.as_deref(), value) {
        Typed::Cancel => return crate::event_loop::reopen_settings(app),
        Typed::Bad(why) => {
            app.flash = Some(crate::flash::Flash::failed(why));
            crate::event_loop::open_prompt(app, PromptKind::MicrosoftAppId { client_id });
            if let Some(Overlay::Prompt(prompt)) = &mut app.overlay {
                prompt.input = crate::text_input::TextInput::with_text(value);
            }
            return;
        }
        Typed::Client(client_id) => {
            return crate::event_loop::open_prompt(
                app,
                PromptKind::MicrosoftAppId {
                    client_id: Some(client_id),
                },
            );
        }
        Typed::Both { client_id, tenant } => match save_app(&client_id, &tenant) {
            Ok(ms) => connect(app, &ms),
            Err(why) => format!("the IDs were not saved: {why}"),
        },
    };
    crate::event_loop::reopen_settings(app);
    if let Some(Overlay::Settings(view)) = &mut app.overlay {
        view.info(note);
    }
}

fn stop_sign_in(app: &mut App) {
    if let Some(task) = app.microsoft.sign_in_task.take() {
        task.abort();
    }
}

/// A sign-in, start to end: a code, then polls until it is used, then the
/// tokens kept and who signed in asked.
async fn sign_in(ms: MicrosoftSettings, tx: UnboundedSender<Answer>) {
    let send = |step| {
        let _ = tx.send(Answer::SignIn(step));
    };
    let asked = ms.clone();
    let code = match blocking(move || auth::request_code(&asked)).await {
        Ok(code) => code,
        Err(why) => return send(Step::Failed(why)),
    };
    send(Step::Code {
        code: code.user_code.clone(),
        uri: code.verification_uri.clone(),
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(code.expires_in);
    let mut interval = std::time::Duration::from_secs(code.interval.max(1));
    let tokens = loop {
        tokio::time::sleep(interval).await;
        if tokio::time::Instant::now() > deadline {
            return send(Step::Failed(auth::EXPIRED.into()));
        }
        let (ms, code) = (ms.clone(), code.clone());
        match blocking(move || Ok(auth::poll_once(&ms, &code)))
            .await
            .unwrap_or_else(auth::Poll::Failed)
            .next(interval)
        {
            auth::Next::Wait(next) => interval = next,
            auth::Next::Finish(Ok(tokens)) => break tokens,
            auth::Next::Finish(Err(why)) => return send(Step::Failed(why)),
        }
    };
    let step = blocking(move || {
        auth::keep(&tokens)?;
        let email = graph::email(&ms)?;
        orion_core::secrets::set(SERVICE, EMAIL, &email)?;
        Ok(email)
    })
    .await;
    send(match step {
        Ok(email) => Step::Done { email },
        Err(why) => Step::Failed(why),
    });
}

/// `job` on the blocking pool.
async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(job)
        .await
        .unwrap_or_else(|e| Err(e.to_string()))
}

/// The calendar should be polled: signed in, and the footer or takeaways
/// want it.
pub fn calendar_wanted(app: &App, ms: &MicrosoftSettings) -> bool {
    app.microsoft.connected(ms) && (ms.footer_meeting || ms.takeaways)
}

/// The loop's [`TICK`] while signed in: repaint a countdown on show, and
/// read the calendar once every `every`.
pub fn tick(app: &mut App, every: std::time::Duration) {
    if footer_text(app).is_some() {
        app.dirty = true;
    }
    let now = tokio::time::Instant::now();
    if app.microsoft.next_read.is_some_and(|at| now < at) {
        return;
    }
    let ms = crate::config::Config::load().microsoft;
    if calendar_wanted(app, &ms) {
        spawn_calendar(app, &ms);
        app.microsoft.next_read = Some(now + every);
    }
}

/// Read the calendar off the loop, unless a read is already out.
fn spawn_calendar(app: &mut App, ms: &MicrosoftSettings) {
    let Some(tx) = app.microsoft.tx.clone() else {
        return;
    };
    if app.microsoft.calendar_in_flight {
        return;
    }
    app.microsoft.calendar_in_flight = true;
    let ms = ms.clone();
    tokio::spawn(async move {
        let events = blocking(move || calendar::fetch(&ms, Utc::now())).await;
        let _ = tx.send(Answer::Calendar(events));
    });
}

/// An answer from off the loop.
pub fn land(app: &mut App, answer: Answer) {
    app.dirty = true;
    match answer {
        Answer::SignIn(Step::Code { code, uri }) => {
            crate::open_url::open_url(&uri, None);
            let note = format!("code {code} copied · paste it in the browser that opened");
            crate::event_loop::copy_and_flash(app, &code, &note);
            app.microsoft.sign_in = SignIn::Code { code, uri };
        }
        Answer::SignIn(Step::Done { email }) => {
            app.microsoft.sign_in_task = None;
            app.microsoft.sign_in = SignIn::Idle;
            app.flash = Some(crate::flash::Flash::done(format!(
                "Microsoft 365 signed in as {email}"
            )));
            app.microsoft.email = Some(email);
            // The loop's tick reads the calendar straight away.
            app.microsoft.next_read = None;
        }
        Answer::SignIn(Step::Failed(why)) => {
            app.microsoft.sign_in_task = None;
            app.microsoft.sign_in = SignIn::Failed(why);
        }
        Answer::Calendar(events) => {
            app.microsoft.calendar_in_flight = false;
            match events {
                // Signed out while it was out: drop it.
                Ok(_) if app.microsoft.email.is_none() => {}
                Ok(events) => app.microsoft.events = events,
                // A read that failed keeps the meetings already known, so a
                // blip doesn't blank the footer.
                Err(why) => tracing::debug!(%why, "calendar read failed"),
            }
        }
    }
}

/// The footer's meeting segment, when it is on and a meeting is within
/// the hour.
pub fn footer_text(app: &App) -> Option<String> {
    app.microsoft.footer.then(|| next_meeting(app)).flatten()
}

/// The meeting running or next within the hour, as the footer words it —
/// `▸ Standup in 12m · Teams` — while signed in; what the NEEDS YOU
/// board's header shows whatever the footer's row says.
pub fn next_meeting(app: &App) -> Option<String> {
    app.microsoft.email.as_ref()?;
    calendar::footer_text(&app.microsoft.events, Utc::now())
}

/// A URL component or form value, percent-encoded: everything but the
/// unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_app() -> MicrosoftSettings {
        MicrosoftSettings {
            client_id: "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_status_row_says_where_the_connection_is() {
        let mut app = App::new();
        assert_eq!(status(&app, &MicrosoftSettings::default()), NEEDS_APP_ID);
        let ms = with_app();
        assert_eq!(status(&app, &ms), "not connected");
        app.microsoft.sign_in = SignIn::Asking;
        assert_eq!(status(&app, &ms), "signing in…");
        app.microsoft.sign_in = SignIn::Code {
            code: "FBQR3SHV9".into(),
            uri: "https://microsoft.com/devicelogin".into(),
        };
        assert_eq!(status(&app, &ms), "enter FBQR3SHV9 in the browser");
        app.microsoft.sign_in = SignIn::Failed(auth::DECLINED.into());
        assert_eq!(
            status(&app, &ms),
            "not connected · sign-in was declined in the browser"
        );
        app.microsoft.sign_in = SignIn::Idle;
        app.microsoft.email = Some("ada@riplo.ai".into());
        assert_eq!(status(&app, &ms), "ada@riplo.ai");
    }

    #[test]
    fn connect_without_an_app_id_asks_for_it_and_calls_nobody() {
        let mut app = App::new();
        assert_eq!(
            connect(&mut app, &MicrosoftSettings::default()),
            NEEDS_APP_ID
        );
        assert_eq!(app.microsoft.sign_in, SignIn::Idle);
    }

    #[test]
    fn an_id_is_a_guid_in_either_case_spaces_trimmed() {
        const ID: &str = "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f";
        assert_eq!(guid(ID).as_deref(), Some(ID));
        assert_eq!(
            guid(&format!("  {}\t", ID.to_uppercase())).as_deref(),
            Some(ID)
        );
        for bad in [
            "",
            "organizations",
            "3f1c2a7e9b4d4e218c551a2b3c4d5e6f",
            "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6",
            "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f0",
            "3f1c2a7g-9b4d-4e21-8c55-1a2b3c4d5e6f",
            "3f1c2a7e-9b4d-4e21-8c55-1a2b-3c4d5e6f",
            "{3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f}",
        ] {
            assert_eq!(guid(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_first_box_takes_one_id_or_both_on_a_line() {
        const C: &str = "3f1c2a7e-9b4d-4e21-8c55-1a2b3c4d5e6f";
        const T: &str = "72f988bf-86f1-41af-91ab-2d7cd011db47";
        let both = Typed::Both {
            client_id: C.into(),
            tenant: T.into(),
        };
        assert_eq!(read_ids(None, "   "), Typed::Cancel);
        assert_eq!(read_ids(None, C), Typed::Client(C.into()));
        for line in [
            format!("{C} {T}"),
            format!("{C},{T}"),
            format!("{C}, {T}"),
            format!("{C}\n{T}\n"),
            format!(" {} \r\n {} ", C.to_uppercase(), T.to_uppercase()),
        ] {
            assert_eq!(read_ids(None, &line), both, "{line:?}");
        }
        assert!(matches!(read_ids(None, "nope"), Typed::Bad(_)));
        assert!(matches!(
            read_ids(None, &format!("{C} nope")),
            Typed::Bad(_)
        ));
        assert!(matches!(
            read_ids(None, &format!("{C} {T} {C}")),
            Typed::Bad(_)
        ));
        // The second box: the tenant alone.
        assert_eq!(read_ids(Some(C), T), both);
        assert_eq!(read_ids(Some(C), ""), Typed::Cancel);
        assert!(matches!(read_ids(Some(C), "organizations"), Typed::Bad(_)));
        assert!(matches!(
            read_ids(Some(C), &format!("{C} {T}")),
            Typed::Bad(_)
        ));
    }

    #[test]
    fn the_boxes_name_each_id_in_turn() {
        let (title, label) = app_id_prompt_text(None);
        assert_eq!(title, "Connect Microsoft 365 · 1 of 2");
        assert!(label.starts_with("Application (client) ID"));
        let (title, label) = app_id_prompt_text(Some("x"));
        assert_eq!(title, "Connect Microsoft 365 · 2 of 2");
        assert!(label.starts_with("Directory (tenant) ID"));
    }

    #[test]
    fn the_calendar_is_read_only_when_signed_in_and_wanted() {
        let mut app = App::new();
        let mut ms = with_app();
        assert!(!calendar_wanted(&app, &ms), "nobody signed in");
        app.microsoft.email = Some("ada@riplo.ai".into());
        assert!(calendar_wanted(&app, &ms));
        ms.footer_meeting = false;
        ms.takeaways = false;
        assert!(!calendar_wanted(&app, &ms), "nothing wants it");
        assert!(!calendar_wanted(&app, &MicrosoftSettings::default()));
    }

    #[test]
    fn encode_keeps_only_the_unreserved() {
        assert_eq!(encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode("a b/c?d=é"), "a%20b%2Fc%3Fd%3D%C3%A9");
    }
}
