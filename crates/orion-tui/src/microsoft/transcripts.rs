//! TEAMS TRANSCRIPTS: what meeting takeaways read once a meeting ends.
//!
//! A meeting on the calendar is found by its join link
//! (`/me/onlineMeetings?$filter=JoinWebUrl eq '…'`, the link URL-encoded as
//! Graph asks), its transcripts listed (`…/transcripts`), and the one this
//! sitting made fetched as WebVTT (`…/content?$format=text/vtt`), then read
//! down to `Speaker: what they said` lines ([`vtt_to_text`]).
//!
//! Every call blocks: run it under `spawn_blocking`. The delegated
//! permissions are `OnlineMeetings.Read` and
//! `OnlineMeetingTranscript.Read.All`.

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::Deserialize;

use super::{encode, graph};
use crate::config::MicrosoftSettings;

/// How long before the meeting's start a transcript may be dated and still
/// be this sitting's: a recurring meeting keeps one meeting id, and every
/// sitting's transcript sits under it.
const EARLY: chrono::Duration = chrono::Duration::minutes(15);

/// The ask that finds the meeting a join link belongs to.
pub fn meeting_path(join_url: &str) -> String {
    let filter = format!("JoinWebUrl eq '{}'", join_url.replace('\'', "''"));
    format!("/me/onlineMeetings?$filter={}", encode(&filter))
}

/// The ask that lists a meeting's transcripts.
pub fn transcripts_path(meeting: &str) -> String {
    format!("/me/onlineMeetings/{}/transcripts", encode(meeting))
}

/// The ask for one transcript's content, as WebVTT.
pub fn content_path(meeting: &str, transcript: &str) -> String {
    format!(
        "/me/onlineMeetings/{}/transcripts/{}/content?$format=text/vtt",
        encode(meeting),
        encode(transcript)
    )
}

#[derive(Deserialize)]
struct List<T> {
    #[serde(default = "Vec::new")]
    value: Vec<T>,
}

#[derive(Deserialize)]
struct Meeting {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transcript {
    id: String,
    #[serde(default)]
    created_date_time: Option<String>,
}

/// The transcript of the meeting at `join_url` that started at `start`, as
/// WebVTT — None while there is none yet.
pub fn look(
    ms: &MicrosoftSettings,
    join_url: &str,
    start: DateTime<Utc>,
) -> Result<Option<String>, String> {
    let meetings: List<Meeting> = graph::get_json(ms, &meeting_path(join_url), &[])?;
    let Some(meeting) = meetings.value.into_iter().next() else {
        return Err("Microsoft Graph doesn't know this meeting by its join link".into());
    };
    let body = graph::get(ms, &transcripts_path(&meeting.id), &[])?;
    let Some(transcript) = pick(&body, start)? else {
        return Ok(None);
    };
    let vtt = graph::get(ms, &content_path(&meeting.id, &transcript), &[])?;
    Ok(Some(String::from_utf8_lossy(&vtt).into_owned()))
}

/// Of a meeting's transcripts (a `…/transcripts` answer), the one this
/// sitting made: the latest dated at or after `start` (less [`EARLY`]).
pub fn pick(body: &[u8], start: DateTime<Utc>) -> Result<Option<String>, String> {
    let list: List<Transcript> = serde_json::from_slice(body)
        .map_err(|e| format!("Microsoft Graph's transcripts didn't read: {e}"))?;
    let mut sittings: Vec<(DateTime<Utc>, String)> = list
        .value
        .into_iter()
        .filter_map(|t| Some((utc(t.created_date_time.as_deref()?)?, t.id)))
        .filter(|(at, _)| *at >= start - EARLY)
        .collect();
    sittings.sort();
    Ok(sittings.pop().map(|(_, id)| id))
}

/// `2026-11-08T10:32:41.1234567Z`, Graph's dates.
fn utc(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(text.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|t| t.and_utc())
        })
}

/// A WebVTT transcript as plain lines, `Speaker: what they said`, one per
/// turn: a speaker's cues in a row are joined, timings and cue ids go,
/// and so do tags.
pub fn vtt_to_text(vtt: &str) -> String {
    let mut turns: Vec<(String, String)> = Vec::new();
    let text = vtt.replace("\r\n", "\n");
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().map(str::trim).collect();
        // A cue is the lines after its timing; the header, NOTEs and
        // STYLE blocks have none.
        let Some(timing) = lines.iter().position(|l| l.contains("-->")) else {
            continue;
        };
        for line in &lines[timing + 1..] {
            let (speaker, said) = cue_line(line);
            if said.is_empty() {
                continue;
            }
            match turns.last_mut() {
                Some((who, words)) if *who == speaker => {
                    words.push(' ');
                    words.push_str(&said);
                }
                _ => turns.push((speaker, said)),
            }
        }
    }
    turns
        .into_iter()
        .map(|(who, words)| match who.is_empty() {
            true => words,
            false => format!("{who}: {words}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One cue line's speaker (its `<v Name>`, else none) and words.
fn cue_line(line: &str) -> (String, String) {
    let speaker = line
        .strip_prefix("<v")
        .and_then(|rest| rest.split_once('>'))
        .map(|(name, _)| match name.strip_prefix('.') {
            // `<v.loud Name>`: a class before the name.
            Some(classed) => classed.split_once(' ').map_or("", |(_, n)| n),
            None => name,
        })
        .map(|name| name.trim().to_string())
        .unwrap_or_default();
    (speaker, unescape(&strip_tags(line)).trim().to_string())
}

fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_tag = false;
    for c in line.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&apos;", "'")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Teams' transcript shape, as Graph's `calltranscript-get` documents it.
    pub(crate) const VTT: &str = "WEBVTT\r\n\r\n\
        0f5c8a1e-3d2b-4c55-9b1a-1d2e3f4a5b6c/17-0\r\n\
        00:00:16.246 --> 00:00:17.726\r\n\
        <v Ada Lovelace>Morning. Quick one on the invoices.</v>\r\n\r\n\
        0f5c8a1e-3d2b-4c55-9b1a-1d2e3f4a5b6c/17-1\r\n\
        00:00:17.900 --> 00:00:19.100\r\n\
        <v Ada Lovelace>I&apos;ll fix the &lt;empty&gt; address 500.</v>\r\n\r\n\
        NOTE nothing to read here\r\n\r\n\
        0f5c8a1e-3d2b-4c55-9b1a-1d2e3f4a5b6c/18-0\r\n\
        00:00:20.000 --> 00:00:22.500\r\n\
        <v Grace Hopper>Thanks &amp; I'll write the onboarding email.</v>\r\n";

    #[test]
    fn vtt_reads_as_one_line_a_turn() {
        assert_eq!(
            vtt_to_text(VTT),
            "Ada Lovelace: Morning. Quick one on the invoices. I'll fix the <empty> address 500.\n\
             Grace Hopper: Thanks & I'll write the onboarding email."
        );
        // A cue with no voice keeps its words.
        assert_eq!(
            vtt_to_text("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHello <b>there</b>\n"),
            "Hello there"
        );
        assert_eq!(vtt_to_text("WEBVTT\n"), "");
    }

    #[test]
    fn the_asks_are_encoded_as_graph_wants() {
        assert_eq!(
            meeting_path("https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc"),
            "/me/onlineMeetings?$filter=JoinWebUrl%20eq%20%27https%3A%2F%2Fteams.microsoft.com%2Fl%2Fmeetup-join%2F19%253ameeting_abc%27"
        );
        assert_eq!(
            content_path("MSo1N2Y5=", "MSMjMCMj"),
            "/me/onlineMeetings/MSo1N2Y5%3D/transcripts/MSMjMCMj/content?$format=text/vtt"
        );
    }

    #[test]
    fn the_sittings_own_transcript_is_picked() {
        let start = Utc.with_ymd_and_hms(2026, 11, 8, 10, 0, 0).unwrap();
        let body = br#"{"@odata.count": 3, "value": [
            {"id": "last-week", "createdDateTime": "2026-11-01T10:31:00Z"},
            {"id": "today", "createdDateTime": "2026-11-08T10:32:41.1234567Z"},
            {"id": "undated"}
        ]}"#;
        assert_eq!(pick(body, start).unwrap().as_deref(), Some("today"));
        let older =
            br#"{"value": [{"id": "last-week", "createdDateTime": "2026-11-01T10:31:00Z"}]}"#;
        assert_eq!(pick(older, start).unwrap(), None, "not yet");
        assert_eq!(pick(br#"{"value": []}"#, start).unwrap(), None);
        assert!(pick(b"nope", start).is_err());
    }
}
