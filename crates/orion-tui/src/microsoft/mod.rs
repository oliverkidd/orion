//! MICROSOFT 365: the Connections tab's sign-in, the calendar behind the
//! footer's next meeting, and the Graph reads meeting takeaways build on.
//!
//! - [`auth`]: the device-code sign-in and the access token after it.
//! - [`graph`]: GETs of Microsoft Graph under that token.
//! - [`calendar`]: the next twelve hours of meetings, polled.
//!
//! Sign-in needs the organisation's Entra app: `microsoft.client_id` (and
//! `tenant`) in `config.json`. Until it is there the status row says so
//! ([`NEEDS_APP_ID`]) and nothing calls Microsoft. Signed in, the refresh
//! token and the email sit in the Keychain under `dev.orion.microsoft`.
//!
//! Everything that touches the network runs off the loop and lands as an
//! [`Answer`] ([`land`]).

pub mod auth;
pub mod calendar;
pub mod graph;

use chrono::Utc;
use tokio::sync::mpsc::UnboundedSender;

use crate::app::App;
use crate::config::MicrosoftSettings;

/// The Keychain service: `dev.orion.microsoft`.
pub const SERVICE: &str = "microsoft";
/// …its refresh token.
pub const REFRESH: &str = "refresh";
/// …and the email it signed in as, for the status row.
pub const EMAIL: &str = "email";

/// The status row with no Entra app to sign in through.
pub const NEEDS_APP_ID: &str = "needs your organisation's app id (see setup)";

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

/// `o` on the status row: sign out — or stop a sign-in under way.
pub fn disconnect(app: &mut App) -> String {
    let was = app.microsoft.email.take();
    let signing_in = !matches!(app.microsoft.sign_in, SignIn::Idle | SignIn::Failed(_));
    stop_sign_in(app);
    app.microsoft.sign_in = SignIn::Idle;
    app.microsoft.events.clear();
    app.microsoft.next_read = None;
    std::thread::spawn(auth::forget);
    match (was, signing_in) {
        (Some(email), _) => format!("signed out {email}"),
        (None, true) => "sign-in stopped".into(),
        (None, false) => crate::config::NOT_CONNECTED.into(),
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
    if !app.microsoft.footer || app.microsoft.email.is_none() {
        return None;
    }
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
