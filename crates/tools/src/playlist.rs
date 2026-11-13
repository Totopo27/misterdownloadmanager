//! A page's list of videos (#69): a playlist, a channel, an album. Listed by yt-dlp flat (each
//! entry its address, title and length, no video looked up) and lazily (`--lazy-playlist`: each
//! entry printed as yt-dlp reads it, one JSON line, never the whole list as one document), so a
//! list of six and one of six thousand cost the same to start, the window shows them as they
//! come, and the listing can be stopped at any point.
//!
//! Bounded on purpose: at most [`MAX_ENTRIES`] entries are kept, a line longer than
//! [`MAX_LINE`] is skipped, and nothing here recurses.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::proc::{self, ProcError, Spawned};

/// Entries kept at most (about 200 bytes each: a few MB). YouTube's own playlists stop at
/// 5,000; a channel is listed a page at a time ([`PAGE`]).
pub const MAX_ENTRIES: usize = 20_000;
/// A flat entry is a few hundred bytes; a line far past that is not one.
const MAX_LINE: usize = 256 * 1024;
/// A list whose size isn't known (a channel: its newest first) is listed this many at a time.
pub const PAGE: u64 = 50;

/// One video of the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Its place in the list, from 1.
    pub index: u64,
    /// The video's own page (what a download looks up).
    pub url: String,
    pub title: String,
    /// Seconds, when the site says.
    pub duration: Option<f64>,
    /// Private, deleted or otherwise not downloadable, by what the site lists.
    pub unavailable: bool,
}

/// Where a listing is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    /// Entries listed so far.
    pub listed: usize,
    /// The list's size, when the site says.
    pub total: Option<u64>,
    /// The list's title, from its entries.
    pub title: Option<String>,
    /// yt-dlp has finished (or was stopped).
    pub done: bool,
    /// More were there than [`MAX_ENTRIES`]: the listing stopped there.
    pub capped: bool,
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct RawEntry {
    id: Option<String>,
    url: Option<String>,
    webpage_url: Option<String>,
    title: Option<String>,
    duration: Option<f64>,
    playlist_index: Option<u64>,
    playlist_count: Option<u64>,
    playlist_title: Option<String>,
    availability: Option<String>,
}

/// Titles YouTube gives the entries it won't show.
const GONE: [&str; 3] = ["[Private video]", "[Deleted video]", "[Unavailable video]"];

/// A length as a clock reads it: `9:18`, `1:02:05`.
pub fn clock(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// One line of `--flat-playlist -j`: the entry, and what it says of the list.
fn parse_line(line: &[u8], fallback_index: u64) -> Option<(Entry, Option<u64>, Option<String>)> {
    let raw: RawEntry = serde_json::from_slice(line).ok()?;
    let url = raw
        .webpage_url
        .or(raw.url)
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"))?;
    let title = raw
        .title
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .or(raw.id)
        .unwrap_or_else(|| url.clone());
    let unavailable = GONE.contains(&title.as_str())
        || matches!(
            raw.availability.as_deref(),
            Some("private" | "premium_only" | "subscriber_only" | "needs_auth")
        );
    Some((
        Entry {
            index: raw.playlist_index.unwrap_or(fallback_index),
            url,
            title,
            duration: raw.duration.filter(|d| d.is_finite() && *d >= 0.0),
            unavailable,
        },
        raw.playlist_count,
        raw.playlist_title,
    ))
}

#[derive(Default)]
struct Shared {
    entries: Vec<Entry>,
    progress: Progress,
}

/// A listing running in the background: entries come in as yt-dlp prints them.
pub struct Listing {
    shared: Arc<Mutex<Shared>>,
    child: Arc<Mutex<Option<Spawned>>>,
}

impl Listing {
    /// List `url`'s entries `from` to `to` (from 1; `to` None: to the end), flat and lazily.
    pub fn start(
        exe: &Path,
        url: &str,
        browser: &proc::FromBrowser,
        from: u64,
        to: Option<u64>,
    ) -> Result<Listing, ProcError> {
        let from = from.max(1);
        let range = match to {
            Some(t) => format!("{from}:{}", t.max(from)),
            None => format!("{from}:"),
        };
        let mut args: Vec<String> = [
            "--flat-playlist",
            "--lazy-playlist",
            "-j",
            "--playlist-items",
            &range,
            "--no-warnings",
        ]
        .map(String::from)
        .to_vec();
        let (theirs, cookies) = browser.args(url)?;
        args.extend(theirs);
        let cmd = proc::ytdlp(exe, &[], &args, url)?;
        let mut spawned = proc::spawn(cmd, "yt-dlp")?;
        let stdout = spawned.child.stdout.take().expect("piped");
        let shared = Arc::new(Mutex::new(Shared::default()));
        let child = Arc::new(Mutex::new(Some(spawned)));
        {
            let (shared, child) = (shared.clone(), child.clone());
            std::thread::spawn(move || {
                let _cookies = cookies;
                read(stdout, &shared, from);
                // yt-dlp's end: its own failure (a wrong address, a site that stopped answering).
                let capped = shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .progress
                    .capped;
                // Capped, nobody reads it any more: ended, not waited for.
                if capped && let Some(s) = child.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
                {
                    s.kill();
                }
                // Polled with the lock free in between, so `cancel` never waits on it.
                let failure = loop {
                    let mut slot = child.lock().unwrap_or_else(|e| e.into_inner());
                    let Some(s) = slot.as_mut() else {
                        break None;
                    };
                    match s.child.try_wait() {
                        // Its last words read outside the lock (`failure` may wait for them).
                        Ok(Some(st)) => {
                            let ended = slot.take().expect("checked");
                            drop(slot);
                            break (!st.success()).then(|| ended.failure().to_string());
                        }
                        Ok(None) => {}
                        Err(_) => break None,
                    }
                    drop(slot);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                };
                child.lock().unwrap_or_else(|e| e.into_inner()).take();
                let mut sh = shared.lock().unwrap_or_else(|e| e.into_inner());
                // Told even part-way (a site that stopped answering): what was listed stays, and
                // the window says where it stopped. Stopped by us (capped, cancelled): no error.
                if !sh.progress.capped {
                    sh.progress.error = failure;
                }
                sh.progress.done = true;
            });
        }
        Ok(Listing { shared, child })
    }

    pub fn progress(&self) -> Progress {
        self.shared.lock().unwrap().progress.clone()
    }

    /// The entries listed after the first `seen` (the window takes them as they come).
    pub fn entries_from(&self, seen: usize) -> Vec<Entry> {
        let sh = self.shared.lock().unwrap();
        sh.entries
            .get(seen..)
            .map(<[Entry]>::to_vec)
            .unwrap_or_default()
    }

    /// Stop listing (what was listed stays).
    pub fn cancel(&self) {
        if let Some(mut s) = self.child.lock().unwrap().take() {
            s.kill();
        }
        self.shared.lock().unwrap().progress.done = true;
    }
}

impl Drop for Listing {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Read yt-dlp's lines into `shared` until it ends, the list is capped, or it's stopped.
fn read(stdout: impl Read, shared: &Mutex<Shared>, from: u64) {
    let mut reader = BufReader::new(stdout);
    let mut buf = Vec::new();
    let mut next_index = from;
    loop {
        buf.clear();
        // A line read up to MAX_LINE; the rest of a longer one is skipped.
        let n = match (&mut reader)
            .take(MAX_LINE as u64)
            .read_until(b'\n', &mut buf)
        {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        if n == MAX_LINE && buf.last() != Some(&b'\n') {
            let mut skip = Vec::new();
            let _ = reader.read_until(b'\n', &mut skip);
            continue;
        }
        let Some((entry, total, title)) = parse_line(&buf, next_index) else {
            continue;
        };
        next_index = entry.index + 1;
        let mut sh = shared.lock().unwrap();
        if sh.progress.done {
            return;
        }
        if sh.entries.len() >= MAX_ENTRIES {
            sh.progress.capped = true;
            return;
        }
        if sh.progress.total.is_none() {
            sh.progress.total = total;
        }
        if sh.progress.title.is_none() {
            sh.progress.title = title.filter(|t| !t.trim().is_empty());
        }
        sh.entries.push(entry);
        sh.progress.listed = sh.entries.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"{"_type": "url", "ie_key": "Youtube", "id": "Zezt6EsG9WE", "url": "https://www.youtube.com/watch?v=Zezt6EsG9WE", "title": "NASA Moon Base", "duration": 558, "playlist_index": 7, "playlist_count": 3912, "playlist_title": "NASA - Videos"}"#;

    #[test]
    fn lengths_read_as_clocks() {
        assert_eq!(clock(558.0), "9:18");
        assert_eq!(clock(3725.4), "1:02:05");
        assert_eq!(clock(7.0), "0:07");
    }

    #[test]
    fn an_entry_and_what_it_says_of_its_list() {
        let (e, total, title) = parse_line(LINE.as_bytes(), 1).unwrap();
        assert_eq!(e.index, 7);
        assert_eq!(e.url, "https://www.youtube.com/watch?v=Zezt6EsG9WE");
        assert_eq!(e.title, "NASA Moon Base");
        assert_eq!(e.duration, Some(558.0));
        assert!(!e.unavailable);
        assert_eq!(
            (total, title.as_deref()),
            (Some(3912), Some("NASA - Videos"))
        );
        // A private one is kept, marked; a line that is no entry (or no web address) isn't.
        let private = r#"{"url": "https://www.youtube.com/watch?v=x", "title": "[Private video]"}"#;
        let (p, _, _) = parse_line(private.as_bytes(), 9).unwrap();
        assert!(p.unavailable);
        assert_eq!(p.index, 9, "no index of its own: the one after the last");
        assert!(parse_line(b"not json", 1).is_none());
        assert!(parse_line(br#"{"url": "file:///etc/passwd", "title": "x"}"#, 1).is_none());
    }

    /// Thousands of lines, a giant one among them, and garbage: read in order, the giant one
    /// skipped, the list capped at MAX_ENTRIES, and quickly (no quadratic step).
    #[test]
    fn a_long_list_is_read_in_order_and_capped() {
        let lines = |n: usize, giant: bool| {
            let mut text = String::new();
            for i in 1..=n {
                text.push_str(&format!(
                    r#"{{"url": "https://v.example/{i}", "title": "Video {i}", "playlist_index": {i}}}"#
                ));
                text.push('\n');
                if giant && i == 3 {
                    text.push_str(&format!(
                        "{{\"title\": \"{}\"}}\n",
                        "x".repeat(MAX_LINE * 2)
                    ));
                    text.push_str("garbage\n");
                }
            }
            text
        };
        let text = lines(MAX_ENTRIES + 500, true);
        let shared = Mutex::new(Shared::default());
        read(text.as_bytes(), &shared, 1);
        let sh = shared.lock().unwrap();
        assert_eq!(sh.entries.len(), MAX_ENTRIES);
        assert!(sh.progress.capped);
        assert_eq!(
            sh.entries[3].title, "Video 4",
            "the giant line skipped, order kept"
        );
        // 8 times the lines cost about 8 times as long (not 64), on any machine.
        mdm_labserver::growth::assert_linear(
            MAX_ENTRIES / 10,
            |n| (lines(n, false), Mutex::new(Shared::default())),
            |(text, shared)| read(text.as_bytes(), &shared, 1),
        );
    }
}
