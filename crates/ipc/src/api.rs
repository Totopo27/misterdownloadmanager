//! The automation API's answers, shared by the app (which writes them) and the programs that
//! read them (the command line, the MCP server), so both sides keep one shape.
//! `docs/automation-api.schema.json` is made from these types and [`crate::Command`]
//! (`MDM_WRITE_SCHEMA=1 cargo test -p mdm-ipc schema`); a test fails while it is out of date.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `{"query": "list"}`: every download, and the total speed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListAnswer {
    pub downloads: Vec<DownloadInfo>,
    /// The total download speed, bytes per second.
    pub speed: u64,
    /// The queues, in the Scheduler's order (the first is the main queue).
    #[serde(default)]
    pub queues: Vec<QueueInfo>,
    /// The categories a new download can go to (General first).
    #[serde(default)]
    pub categories: Vec<CategoryInfo>,
    /// Free space where downloads go (the General category's folder), bytes; null when unknown.
    #[serde(default)]
    pub free: Option<u64>,
}

/// One queue of the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct QueueInfo {
    /// Its name: what `queue` takes (a built-in queue's in English, as shipped).
    pub name: String,
    /// Is it running?
    pub running: bool,
    /// Its downloads' ids, in its order.
    pub ids: Vec<u64>,
}

/// One category of the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct CategoryInfo {
    /// What `category` takes and a download's `category` says (a built-in category's English
    /// name, as shipped).
    pub key: String,
    /// Its name as the user sees it: a built-in one's English name (translated where it is
    /// shown) unless the user renamed it.
    pub name: String,
}

/// An address added (`url`): the download's id, or `pending` while a torrent's files are looked
/// up (it joins the list once they are known).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct AddAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default)]
    pub pending: bool,
}

/// One download of the list.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct DownloadInfo {
    /// What the actions take (`ids`); never reused.
    pub id: u64,
    /// The file's name.
    pub file: String,
    /// Its address (a video's page, a torrent's magnet link).
    pub url: String,
    /// Where it is saved: the file, or a torrent's folder.
    pub path: String,
    /// `Queued` (added, never started), `Downloading`, `Stopped`, `Complete` or `Error`.
    pub status: String,
    /// Its size in bytes; null while unknown.
    pub size: Option<u64>,
    /// Bytes downloaded so far.
    pub downloaded: u64,
    /// Bytes per second; 0 when it isn't running.
    pub speed: u64,
    /// When it was added, as the list shows it; null for downloads from before that column.
    pub added: Option<String>,
    /// Its category's `key` (see [`CategoryInfo`]).
    #[serde(default)]
    pub category: String,
    /// Why it failed, for a download in `Error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The statuses [`DownloadInfo::status`] takes.
pub const STATUSES: [&str; 5] = ["Queued", "Downloading", "Stopped", "Complete", "Error"];

/// `{"action": …, "ids": […]}`: what was done, and what was skipped and why.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ActAnswer {
    pub done: Vec<u64>,
    pub skipped: Vec<Skipped>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Skipped {
    pub id: u64,
    /// "no such download", "not running", "already running", "complete" or "running: pause it
    /// first".
    pub why: String,
}

/// `{"pair": true}` (#157): a new pairing code for the remote control, while it is on.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct PairAnswer {
    /// The 8-digit code; it works once, for `minutes`.
    pub code: String,
    pub minutes: u32,
    /// The page's addresses, the one a phone most likely reaches first.
    pub urls: Vec<String>,
    /// A QR code of the first address with the code, drawn with text blocks for a terminal.
    pub qr: String,
}

/// `{"api": true}` / `{"api": false}`: the API's new state.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ApiAnswer {
    pub api: bool,
}

/// An unknown question or action, or a question while the API is off.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct ErrorAnswer {
    /// For people; its wording may change.
    pub error: String,
    /// For programs: `"api-off"` while the API is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// [`ErrorAnswer::code`] while the API is off.
pub const API_OFF: &str = "api-off";

/// The browser's quality menu (`video_lookup`, `video_poll`, `video_pick`): where a look-up is,
/// or what became of a choice. Answered whatever the automation API's switch (the browser's
/// host asks, not a program).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VideoLookupAnswer {
    /// One of [`lookup`]'s states.
    pub state: String,
    /// `pending` and `ready`: the look-up's token, to ask about it and to pick from it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// `ready`: the video's title, the file's name: the page's when yt-dlp knows the video only
    /// by its address, as the Download video window names it ([`crate::Command::video_title`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// `ready`: the qualities to choose from, labelled in MDM's language; `video_pick` takes a
    /// place in this list.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<VideoChoice>,
    /// `failed`: [`lookup::NO_YTDLP`], [`lookup::NO_CHOICES`] or [`lookup::ERROR`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// `failed` with [`lookup::ERROR`]: why, in MDM's words and language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One quality the browser's quality menu offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct VideoChoice {
    /// What the menu shows ("720p mp4"), in MDM's language.
    pub label: String,
    /// The file's type (`mp4`, `mp3`).
    pub ext: String,
    /// Bytes, exact or estimated; null when unknown.
    pub size: Option<u64>,
    /// Only the sound (an audio file, a conversion to MP3, Opus or FLAC): the menu lists these
    /// apart from the videos. False from an app older than the field.
    pub audio: bool,
    /// The picture's height in pixels (the button's chip: 1080p, 4K); none for the sound, or a
    /// file known only by its type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Frames a second, when above 31 (60 fps is worth saying, 30 isn't).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fps: Option<u32>,
    /// The sound's bitrate in kbit/s, for the sound as the site has it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kbps: Option<u32>,
}

/// [`VideoLookupAnswer::state`]'s values, and [`VideoLookupAnswer::reason`]'s.
pub mod lookup {
    /// Still looking (`token` to ask again).
    pub const PENDING: &str = "pending";
    /// Found: `title` and `choices`.
    pub const READY: &str = "ready";
    /// Not found: `reason`, with `message` for an error.
    pub const FAILED: &str = "failed";
    /// The page is a list of videos (a playlist, a channel): chosen in MDM's window.
    pub const LIST: &str = "list";
    /// Two look-ups are running already.
    pub const BUSY: &str = "busy";
    /// The video button is off on this site (Options' rules).
    pub const OFF: &str = "off";
    /// No such token (never given, or forgotten after 10 minutes).
    pub const GONE: &str = "gone";
    /// The Download video window does it instead (MDM not running, an older MDM, a Google Drive
    /// file, MDM without a window, a choice that doesn't fit its look-up).
    pub const USE_WINDOW: &str = "use-window";
    /// The choice is downloading.
    pub const STARTED: &str = "started";
    /// The choice was added to the queue (Options: "only add to the queue").
    pub const QUEUED: &str = "queued";
    /// MDM took the choice, and it isn't a new download: it is in the list already in that
    /// quality (Options' "If a duplicate download link is added": that one was opened or
    /// resumed, or MDM asks what to do in its own window). From the browser's host: MDM didn't
    /// say in time what became of it (`timeout`); the command is in its queue and will run.
    /// Either way the Download video window mustn't do it a second time; the extension takes it
    /// for `started`.
    pub const SENT: &str = "sent";
    /// `failed`: yt-dlp isn't installed.
    pub const NO_YTDLP: &str = "no-ytdlp";
    /// `failed`: the page has nothing this can download.
    pub const NO_CHOICES: &str = "no-choices";
    /// `failed`: yt-dlp couldn't look it up (`message` says why).
    pub const ERROR: &str = "error";
}

impl VideoLookupAnswer {
    /// An answer that is only a state.
    pub fn of(state: &str) -> VideoLookupAnswer {
        VideoLookupAnswer {
            state: state.into(),
            ..Default::default()
        }
    }

    /// Still looking up `token`.
    pub fn pending(token: &str) -> VideoLookupAnswer {
        VideoLookupAnswer {
            token: Some(token.into()),
            ..VideoLookupAnswer::of(lookup::PENDING)
        }
    }

    /// Not found, for `reason` (with `message` for an error).
    pub fn failed(reason: &str, message: Option<String>) -> VideoLookupAnswer {
        VideoLookupAnswer {
            reason: Some(reason.into()),
            message,
            ..VideoLookupAnswer::of(lookup::FAILED)
        }
    }
}

/// An answer as its line.
pub fn line<T: Serialize>(answer: &T) -> String {
    serde_json::to_string(answer).unwrap_or_default()
}

/// An answer read back: an [`ErrorAnswer`] or the answer asked for.
pub fn parse<T: serde::de::DeserializeOwned>(
    answer: &str,
) -> Result<Result<T, ErrorAnswer>, String> {
    if let Ok(e) = serde_json::from_str::<ErrorAnswer>(answer) {
        return Ok(Err(e));
    }
    serde_json::from_str(answer)
        .map(Ok)
        .map_err(|e| format!("unexpected answer ({e}): {answer}"))
}

/// The JSON Schema of the channel: a request line and every answer.
pub fn schema() -> serde_json::Value {
    let mut generator = schemars::generate::SchemaSettings::draft2020_12().into_generator();
    let request = generator.subschema_for::<crate::Envelope>();
    let _ = generator.subschema_for::<ListAnswer>();
    let _ = generator.subschema_for::<ActAnswer>();
    let _ = generator.subschema_for::<ApiAnswer>();
    let _ = generator.subschema_for::<AddAnswer>();
    let _ = generator.subschema_for::<PairAnswer>();
    let _ = generator.subschema_for::<ErrorAnswer>();
    let _ = generator.subschema_for::<VideoLookupAnswer>();
    let defs = generator.take_definitions(true);
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://misterdownloadmanager.com/automation-api.schema.json",
        "title": "Mister Download Manager automation API",
        "description": "One JSON line to the port in ipc.json (on 127.0.0.1), one line back. \
            A request is `Request`. A command (an address to add, start_queue) is answered \
            `ok`; a question is answered with the JSON object for it (`ListAnswer`, \
            `ActAnswer`, `ApiAnswer`) or an `ErrorAnswer`. A wrong token is answered `denied`, \
            an app that doesn't answer within 2 seconds `timeout`. The browser's quality menu \
            (`video_lookup`, `video_poll`, `video_pick`, sent by its native host) is answered \
            with a `VideoLookupAnswer`. \
            Guide: https://misterdownloadmanager.com/help/command-line.html#the-automation-api",
        "$ref": request.get("$ref").cloned().unwrap_or(serde_json::Value::Null),
        "$defs": defs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_file_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/automation-api.schema.json");
        let made = serde_json::to_string_pretty(&schema()).unwrap() + "\n";
        if std::env::var_os("MDM_WRITE_SCHEMA").is_some() {
            std::fs::write(&path, &made).unwrap();
        }
        let file = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            file == made,
            "docs/automation-api.schema.json is out of date: MDM_WRITE_SCHEMA=1 cargo test -p mdm-ipc schema"
        );
    }

    #[test]
    fn errors_and_answers_read_back() {
        let off = r#"{"error":"the automation API is off","code":"api-off"}"#;
        let e = parse::<ListAnswer>(off).unwrap().unwrap_err();
        assert_eq!(e.code.as_deref(), Some(API_OFF));
        let list = parse::<ListAnswer>(r#"{"downloads":[],"speed":5}"#)
            .unwrap()
            .unwrap();
        assert_eq!(list.speed, 5);
        assert!(parse::<ListAnswer>("ok").is_err());
    }

    /// The quality menu's answers as the browser's host reads them: only what each state has,
    /// a size unknown said as null, and back the same.
    #[test]
    fn video_lookup_answers_round_trip() {
        assert_eq!(
            line(&VideoLookupAnswer::pending("abc")),
            r#"{"state":"pending","token":"abc"}"#
        );
        assert_eq!(
            line(&VideoLookupAnswer::of(lookup::USE_WINDOW)),
            r#"{"state":"use-window"}"#
        );
        assert_eq!(
            line(&VideoLookupAnswer::failed(lookup::NO_YTDLP, None)),
            r#"{"state":"failed","reason":"no-ytdlp"}"#
        );
        let ready = VideoLookupAnswer {
            token: Some("abc".into()),
            title: Some("A talk".into()),
            choices: vec![
                VideoChoice {
                    label: "720p mp4".into(),
                    ext: "mp4".into(),
                    size: Some(187_500_000),
                    audio: false,
                    ..Default::default()
                },
                VideoChoice {
                    label: "Audio only mp3".into(),
                    ext: "mp3".into(),
                    size: None,
                    audio: true,
                    ..Default::default()
                },
            ],
            ..VideoLookupAnswer::of(lookup::READY)
        };
        let json = line(&ready);
        assert_eq!(
            json,
            r#"{"state":"ready","token":"abc","title":"A talk","choices":[{"label":"720p mp4","ext":"mp4","size":187500000,"audio":false},{"label":"Audio only mp3","ext":"mp3","size":null,"audio":true}]}"#
        );
        assert_eq!(
            serde_json::from_str::<VideoLookupAnswer>(&json).unwrap(),
            ready
        );
        // An older app's choices, without `audio`: none is sound alone.
        let old: VideoLookupAnswer = serde_json::from_str(
            r#"{"state":"ready","choices":[{"label":"Audio only mp3","ext":"mp3","size":null}]}"#,
        )
        .unwrap();
        assert!(!old.choices[0].audio);
        let failed = VideoLookupAnswer::failed(lookup::ERROR, Some("Video unavailable".into()));
        assert_eq!(
            serde_json::from_str::<VideoLookupAnswer>(&line(&failed)).unwrap(),
            failed
        );
        // Not one of these (an error answer, an older app's `ok`): no state to read.
        let other: VideoLookupAnswer =
            serde_json::from_str(r#"{"error":"the automation API is off"}"#).unwrap();
        assert!(other.state.is_empty());
        assert!(serde_json::from_str::<VideoLookupAnswer>("ok").is_err());
    }
}
