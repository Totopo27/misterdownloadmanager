//! An HLS stream's master playlist read the way yt-dlp's generic extractor reads it, for the
//! browser's quality menu (the app's `videolookup.rs`): the qualities, sizes and subtitles
//! `yt-dlp -J` lists for the stream, a second and more sooner (yt-dlp's start alone takes one).
//! Only an optimisation: it takes only what it reads exactly as yt-dlp does, a master playlist of
//! a whole video (VOD) whose every video variant says its resolution, without DRM, and anything
//! else, or any doubt, is None: yt-dlp looks the stream up as before. The parity is checked
//! against yt-dlp's own answers for the same playlists (`tests/fixtures/hls`, yt-dlp 2026.10.26).
//!
//! What yt-dlp does there (`_parse_m3u8_formats_and_subtitles`, then the generic extractor's
//! "Checking m3u8 live status"), mirrored here:
//! - each `#EXT-X-MEDIA` audio rendition with a playlist of its own (`URI`) is a format of sound
//!   alone, `<GROUP-ID>-<NAME>`, with no bitrate; a video rendition with its own (another camera)
//!   isn't mirrored (None);
//! - each variant (`#EXT-X-STREAM-INF` and the address under it) is a format: its bitrate
//!   AVERAGE-BANDWIDTH, else BANDWIDTH, / 1000; its RESOLUTION, FRAME-RATE and CODECS (none:
//!   picture and sound both unknown); a variant whose AUDIO group's first rendition has a
//!   playlist carries no sound (it is merged with one);
//! - `#EXT-X-I-FRAME-STREAM-INF` (trick play) is no format;
//! - each `SUBTITLES` rendition with a playlist is a subtitle language (its LANGUAGE, `und`
//!   without one), with no name;
//! - the duration is the whole seconds of the FIRST format's playlist (the first audio rendition
//!   when there is one, else the first variant), read only when it ends (`#EXT-X-ENDLIST`);
//! - the title is the address's last part without its extension (`master` for `master.m3u8`), of
//!   the address a redirect led to: yt-dlp looks that one up instead;
//! - every format is an `mp4`, listed worst first, as yt-dlp sorts them.
//!
//! Reading it takes two requests ([`fetch`]), the master playlist and that first playlist, with
//! the browser's context for the stream, within 5 s and 2 MB each. Neither a playlist nor its
//! address is ever logged or written anywhere.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;

use url::Url;

use crate::proc::FromBrowser;
use crate::resolve::{Format, Video, picture_rank, sound_rank};

/// The most a playlist may hold, counted on the bytes as they come (not the size the server
/// announces).
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
/// Both requests together, redirects included.
pub const BUDGET: Duration = Duration::from_secs(5);
/// Redirects followed for each request.
const MAX_REDIRECTS: usize = 5;
/// The most lines a playlist, formats a master and attributes a line may have: a bigger one is
/// nothing the menu reads.
const MAX_LINES: usize = 100_000;
const MAX_FORMATS: usize = 200;
const MAX_ATTRIBUTES: usize = 64;
/// The longest tag line whose attributes are read: a real one is well under 2 KB, and a match
/// that fails is tried again from later on the line, so a long crafted one costs its square.
const MAX_ATTRIBUTE_LINE: usize = 16 * 1024;

/// A master playlist, read: its formats as yt-dlp lists them before it knows the duration, its
/// subtitle languages, and the playlist whose length is the video's ([`Master::first`]).
#[derive(Debug)]
pub struct Master {
    title: String,
    /// In the order yt-dlp makes them (renditions, then variants).
    formats: Vec<Fmt>,
    subtitles: Vec<String>,
}

/// One format as yt-dlp makes it of a rendition or a variant.
#[derive(Clone, Debug)]
struct Fmt {
    id: String,
    url: Url,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    /// yt-dlp's codecs: `none` (not there), a name, or unknown.
    vcodec: Option<String>,
    acodec: Option<String>,
    /// Kbit/s.
    tbr: Option<f64>,
}

impl Fmt {
    fn video(&self) -> bool {
        self.vcodec.as_deref() != Some("none")
    }

    fn audio(&self) -> bool {
        self.acodec.as_deref() != Some("none")
    }
}

/// Doubt: something yt-dlp may read otherwise than MDM would. The stream is left to yt-dlp.
struct Doubt;

impl Master {
    /// The master playlist `text`, from `url` (the address it came from, after any redirect).
    /// None: not a master playlist, or one MDM doesn't read exactly as yt-dlp would.
    pub fn parse(text: &str, url: &Url) -> Option<Master> {
        let lines = lines(text)?;
        if !text.starts_with("#EXTM3U")
            || !text.contains("#EXT-X-STREAM-INF:")
            || drm(text, &lines)
            // yt-dlp's title would be the page's (`<title>`, `og:title`) if the text had one.
            || text.contains("<title")
            || text.contains("og:title")
        {
            return None;
        }
        let title = title_of(url)?;
        // The renditions first, all of them, then the variants, as yt-dlp reads them.
        let mut groups: HashMap<String, Vec<HashMap<String, String>>> = HashMap::new();
        let mut formats: Vec<Fmt> = Vec::new();
        let mut subtitles: Vec<String> = Vec::new();
        for line in lines.iter().filter(|l| l.starts_with("#EXT-X-MEDIA:")) {
            let media = attributes(line)?;
            let (Some(kind), Some(group), Some(name)) =
                (media.get("TYPE"), media.get("GROUP-ID"), media.get("NAME"))
            else {
                continue;
            };
            let (kind, group, name) = (kind.clone(), group.clone(), name.clone());
            let uri = media.get("URI").cloned();
            let language = media.get("LANGUAGE").cloned();
            groups.entry(group.clone()).or_default().push(media);
            match (kind.as_str(), uri) {
                ("SUBTITLES", Some(_)) => {
                    let code = language.unwrap_or_else(|| "und".into());
                    if !subtitles.contains(&code) {
                        subtitles.push(code);
                    }
                }
                // Another camera's picture: not mirrored.
                ("VIDEO", Some(_)) => return None,
                ("AUDIO", Some(uri)) => formats.push(Fmt {
                    id: format!("{group}-{name}"),
                    url: address_in(url, &uri)?,
                    width: None,
                    height: None,
                    fps: None,
                    vcodec: Some("none".into()),
                    acodec: None,
                    tbr: None,
                }),
                _ => {}
            }
        }
        let mut last: HashMap<String, String> = HashMap::new();
        for line in &lines {
            if line.starts_with("#EXT-X-STREAM-INF:") {
                last = attributes(line)?;
                continue;
            }
            if line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            let inf = std::mem::take(&mut last);
            // Dailymotion's plain-file twin of a variant: one more format, not mirrored.
            if inf.contains_key("PROGRESSIVE-URI") {
                return None;
            }
            let rate = inf
                .get("AVERAGE-BANDWIDTH")
                .or_else(|| inf.get("BANDWIDTH"))
                .map(|b| py_float(b))
                .transpose()
                .ok()?
                .flatten()
                .map(|b| b / 1000.0)
                // A variant without a bitrate is listed by yt-dlp all the same, unsized and
                // numbered by its place: not mirrored.
                .filter(|t| *t > 0.0)?;
            let name = inf.get("NAME").cloned().or_else(|| {
                let id = inf.get("VIDEO")?;
                Some(
                    groups
                        .get(id)
                        .and_then(|g| g.first())
                        .and_then(|r| r.get("NAME"))
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                )
            });
            let (width, height) = match inf.get("RESOLUTION") {
                Some(r) => resolution(r).ok()?.unzip(),
                None => (None, None),
            };
            let fps = match inf.get("FRAME-RATE") {
                Some(f) => py_float(f).ok()?,
                None => None,
            };
            let codecs = codecs(inf.get("CODECS").map(String::as_str)).ok()?;
            let (vcodec, mut acodec) = codecs.clone().unzip();
            // Its sound in another playlist (yt-dlp merges the two): the variant has none.
            if let (Some(group), Some(_)) = (inf.get("AUDIO"), &codecs)
                && vcodec.as_deref() != Some("none")
                && groups
                    .get(group)
                    .and_then(|g| g.first())
                    .is_some_and(|r| r.contains_key("URI"))
            {
                acodec = Some("none".into());
            }
            formats.push(Fmt {
                // Python's `'%d' % tbr`.
                id: name.unwrap_or_else(|| format!("{}", rate.trunc() as i64)),
                url: address_in(url, line.trim())?,
                width,
                height,
                fps,
                vcodec,
                acodec,
                tbr: Some(rate),
            });
            if formats.len() > MAX_FORMATS {
                return None;
            }
        }
        // Every picture has its height (no "MP4" line by type alone), and every playlist is on
        // the master's own server: one request more, to the same place.
        let home = origin(url);
        if formats.is_empty()
            || formats.len() > MAX_FORMATS
            || formats.iter().any(|f| f.video() && f.height.is_none())
            || formats.iter().any(|f| origin(&f.url) != home)
        {
            return None;
        }
        Some(Master {
            title,
            formats,
            subtitles,
        })
    }

    /// The playlist whose length is the video's, as yt-dlp reads it: the first format's.
    pub fn first(&self) -> &Url {
        &self.formats[0].url
    }

    /// The video, given [`Master::first`]'s playlist `text`: None when that is no whole video
    /// (a live stream's has no end), or DRM.
    pub fn video(&self, text: &str) -> Option<Video> {
        let lines = lines(text)?;
        if !text.contains("#EXT-X-ENDLIST")
            || text.contains("#EXT-X-STREAM-INF:")
            || drm(text, &lines)
        {
            return None;
        }
        // yt-dlp's `int(sum(float(…) for each #EXTINF))`, left to right: the same seconds.
        let mut sum = 0.0f64;
        for line in lines.iter().filter(|l| l.starts_with("#EXTINF:")) {
            let rest = &line["#EXTINF:".len()..];
            let secs = py_float(rest.split(',').next().unwrap_or_default())
                .ok()
                .flatten()
                .filter(|s| *s >= 0.0)?;
            sum += secs;
        }
        let duration = sum.trunc();
        if !(duration >= 1.0 && duration.is_finite()) {
            return None;
        }
        let mut sorted = self.formats.clone();
        sorted.sort_by(worse_first);
        let ids = unique_ids(&sorted);
        let formats: Vec<Format> = sorted
            .iter()
            .zip(ids)
            .map(|(f, id)| Format {
                id,
                ext: "mp4".into(),
                height: f.height,
                fps: f.fps,
                video: f.video(),
                audio: f.audio(),
                size: f
                    .tbr
                    .filter(|t| t.is_finite() && *t > 0.0)
                    .map(|t| crate::resolve::estimate(duration, t)),
                // Its bitrate (a peak, often) times the length: about.
                approx: true,
                tbr: f.tbr,
                // A variant of an HLS playlist: its pieces need ffmpeg to make one file (#569).
                hls: true,
                vcodec: f.vcodec.clone(),
                acodec: f.acodec.clone(),
                // SDR, as yt-dlp says of a picture of no HDR codec (the ones it reads as HDR
                // are refused: `codecs`).
                ..Default::default()
            })
            .collect();
        if !ties_are_harmless(&formats) {
            return None;
        }
        Some(Video {
            title: self.title.clone(),
            duration: Some(duration),
            formats,
            subtitles: crate::resolve::unnamed_tracks(self.subtitles.iter().cloned()),
            automatic: Vec::new(),
            language: None,
            direct: false,
            needs_js_runtime: false,
        })
    }
}

/// The lines of `text` as Python's `splitlines` makes them; None for characters it splits on
/// and Rust doesn't (or the other way), and for a playlist too long to be one.
fn lines(text: &str) -> Option<Vec<&str>> {
    let odd = |c: char| {
        (c.is_ascii_control() && !matches!(c, '\t' | '\n' | '\r'))
            || matches!(c, '\u{85}' | '\u{2028}' | '\u{2029}')
    };
    if text.chars().any(odd) {
        return None;
    }
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest.find(['\r', '\n']).unwrap_or(rest.len());
        out.push(&rest[..end]);
        let after = &rest[end..];
        rest = after
            .strip_prefix("\r\n")
            .or_else(|| after.strip_prefix('\r'))
            .or_else(|| after.strip_prefix('\n'))
            .unwrap_or(after);
        if out.len() > MAX_LINES {
            return None;
        }
    }
    Some(out)
}

/// A tag line's attributes as yt-dlp's `parse_m3u8_attributes` finds them, the whole line
/// searched for `KEY=VALUE` followed by a comma or the end (`[A-Z0-9-]+=("[^"]+"|[^",]+)(,|$)`,
/// quotes taken off), a later key winning. None for a line with too many.
fn attributes(line: &str) -> Option<HashMap<String, String>> {
    let b = line.as_bytes();
    if b.len() > MAX_ATTRIBUTE_LINE {
        return None;
    }
    let mut out = HashMap::new();
    let mut found = 0;
    let mut i = 0;
    while i < b.len() {
        match attribute_at(b, i) {
            Some((key, value, end)) => {
                found += 1;
                if found > MAX_ATTRIBUTES {
                    return None;
                }
                out.insert(line[key].to_owned(), line[value].to_owned());
                i = end;
            }
            // No match can start inside the key run that just failed (its end and what follows
            // are the same from any of its letters): on after it.
            None => i = key_run_end(b, i).max(i + 1),
        }
    }
    Some(out)
}

/// Where the run of key characters (`[A-Z0-9-]`) starting at `i` ends.
fn key_run_end(b: &[u8], i: usize) -> usize {
    let mut j = i;
    while j < b.len() && key_char(b[j]) {
        j += 1;
    }
    j
}

fn key_char(c: u8) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-'
}

type Span = std::ops::Range<usize>;

/// The attribute the pattern matches at `i`: its key, its value and where the search goes on.
fn attribute_at(b: &[u8], i: usize) -> Option<(Span, Span, usize)> {
    let j = key_run_end(b, i);
    if j == i || b.get(j) != Some(&b'=') {
        return None;
    }
    let v = j + 1;
    // The comma or the end after a value at `from..to`.
    let ends = |from: usize, to: usize, next: usize| match b.get(next) {
        None => Some((i..j, from..to, next)),
        Some(b',') => Some((i..j, from..to, next + 1)),
        _ => None,
    };
    if b.get(v) == Some(&b'"') {
        // `"[^"]+"`; failing that, `[^",]+` can't start on a quote either.
        let close = v + 1 + b[v + 1..].iter().position(|&c| c == b'"')?;
        if close == v + 1 {
            return None;
        }
        return ends(v + 1, close, close + 1);
    }
    let mut k = v;
    while k < b.len() && b[k] != b'"' && b[k] != b',' {
        k += 1;
    }
    if k == v {
        return None;
    }
    ends(v, k, k)
}

/// Python's `float()` of a value as yt-dlp's `float_or_none` takes it: `Ok(None)` when Python
/// can't read it either; `Err` for what Python reads and Rust doesn't (underscores between
/// digits, other scripts' digits), and for infinities and NaN, which nothing here mirrors.
fn py_float(s: &str) -> Result<Option<f64>, Doubt> {
    let s = s.trim();
    if !s.is_ascii() || s.contains('_') {
        return Err(Doubt);
    }
    match s.parse::<f64>() {
        Ok(f) if f.is_finite() => Ok(Some(f)),
        Ok(_) => Err(Doubt),
        Err(_) => Ok(None),
    }
}

/// RESOLUTION's width and height as yt-dlp searches them (`(\d+)[xX](\d+)`); none when it
/// finds none.
fn resolution(s: &str) -> Result<Option<(u32, u32)>, Doubt> {
    if !s.is_ascii() {
        return Err(Doubt);
    }
    let b = s.as_bytes();
    let digits = |from: usize| from + b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let x = digits(i);
        if matches!(b.get(x), Some(b'x' | b'X')) && b.get(x + 1).is_some_and(u8::is_ascii_digit) {
            let end = digits(x + 1);
            let width = s[i..x].parse().map_err(|_| Doubt)?;
            let height = s[x + 1..end].parse().map_err(|_| Doubt)?;
            return Ok(Some((width, height)));
        }
        i = x;
    }
    Ok(None)
}

/// CODECS as yt-dlp's `parse_codecs` reads it: (picture, sound), `none` for the one that isn't
/// there; None when it names nothing yt-dlp knows (both unknown). `Err` for what it reads into
/// more than that (HDR and subtitle codecs, which yt-dlp sorts and lists apart).
fn codecs(s: Option<&str>) -> Result<Option<(String, String)>, Doubt> {
    let Some(s) = s else { return Ok(None) };
    if !s.is_ascii() {
        return Err(Doubt);
    }
    let split: Vec<&str> = s
        .trim()
        .trim_matches(',')
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect();
    const PICTURE: [&str; 16] = [
        "avc1", "avc2", "avc3", "avc4", "vp9", "vp8", "hev1", "hev2", "h263", "h264", "mp4v",
        "hvc1", "av1", "theora", "dvh1", "dvhe",
    ];
    const SOUND: [&str; 14] = [
        "flac", "mp4a", "opus", "vorbis", "mp3", "aac", "ac-4", "ac-3", "ec-3", "eac3", "dtsc",
        "dtse", "dtsh", "dtsl",
    ];
    let (mut picture, mut sound): (Option<String>, Option<String>) = (None, None);
    for codec in &split {
        // The name lowercased up to its first dot; zeros before a digit dropped to tell it
        // (`av01` is `av1`, `vp09.02` is `vp9.2`).
        let (head, tail) = codec.split_at(codec.find('.').unwrap_or(codec.len()));
        let full = format!("{}{tail}", head.to_ascii_lowercase());
        let parts: Vec<String> = no_leading_zeros(&full)
            .split('.')
            .map(str::to_owned)
            .collect();
        let kind = parts[0].as_str();
        if PICTURE.contains(&kind) {
            if picture.is_some() {
                continue;
            }
            let hdr = matches!(kind, "dvh1" | "dvhe")
                || (kind == "av1" && parts.get(3).map(String::as_str) == Some("10"))
                || (kind == "vp9" && parts.get(1).map(String::as_str) == Some("2"));
            if hdr {
                return Err(Doubt);
            }
            picture = Some(full);
        } else if SOUND.contains(&kind) {
            sound = sound.or(Some(full));
        } else if matches!(kind, "stpp" | "wvtt") {
            return Err(Doubt);
        }
    }
    Ok(match (picture, sound) {
        (None, None) if split.len() == 2 => Some((split[0].to_owned(), split[1].to_owned())),
        (None, None) => None,
        (p, s) => Some((
            p.unwrap_or_else(|| "none".into()),
            s.unwrap_or_else(|| "none".into()),
        )),
    })
}

/// Python's `re.sub(r'0+(?=\d)', '', s)`: runs of zeros before a digit taken out.
fn no_leading_zeros(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'0' {
            // A run of zeros goes whole before a digit; before anything else all but its last
            // zero go (that one is the digit the others are before).
            let run = b[i..].iter().take_while(|&&c| c == b'0').count();
            if !b.get(i + run).is_some_and(u8::is_ascii_digit) {
                out.push('0');
            }
            i += run;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// Whether the master's text says DRM: yt-dlp's own signs of it (FairPlay, PlayReady, Flash
/// Access, which it drops the formats for), and any key that isn't plain AES-128 (`SAMPLE-AES`,
/// or a key format other than `identity`), in a key or a session key.
fn drm(text: &str, lines: &[&str]) -> bool {
    [
        "skd://",
        "com.apple.streamingkeydelivery",
        "com.microsoft.playready",
        "#EXT-X-FAXS-CM:",
    ]
    .iter()
    .any(|s| text.contains(s))
        || lines
            .iter()
            .filter(|l| l.starts_with("#EXT-X-KEY:") || l.starts_with("#EXT-X-SESSION-KEY:"))
            .any(|l| {
                let Some(a) = attributes(l) else { return true };
                a.get("METHOD").is_some_and(|m| m.starts_with("SAMPLE-AES"))
                    || a.get("KEYFORMAT").is_some_and(|f| f != "identity")
            })
}

/// The title yt-dlp's generic extractor gives a stream it knows only by its address: the last
/// part of the address's path without its extension (Python's `os.path.splitext`), then decoded.
/// None for a part that isn't UTF-8 once decoded (yt-dlp would put replacement characters).
fn title_of(url: &Url) -> Option<String> {
    let last = url.path().trim_matches('/').rsplit('/').next()?;
    let stem = match last.rfind('.') {
        Some(dot) if last[..dot].chars().any(|c| c != '.') => &last[..dot],
        _ => last,
    };
    let title = percent_encoding::percent_decode_str(stem)
        .decode_utf8()
        .ok()?
        .into_owned();
    (!title.trim().is_empty()).then_some(title)
}

/// Characters an address may hold as it is (RFC 3986), so that yt-dlp's and MDM's readings of
/// it can't differ.
fn plain_address(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~:/?#[]@!$&'()*+,;=%".contains(&c))
}

/// A playlist's address `uri` as written in the master at `base`.
fn address_in(base: &Url, uri: &str) -> Option<Url> {
    if !plain_address(uri) {
        return None;
    }
    let u = base.join(uri).ok()?;
    matches!(u.scheme(), "http" | "https").then_some(u)
}

/// Scheme, host and port: the browser's cookie and referrer go only where all three are the
/// stream's.
fn origin(u: &Url) -> (String, Option<String>, Option<u16>) {
    (
        u.scheme().to_owned(),
        u.host_str().map(str::to_ascii_lowercase),
        u.port_or_known_default(),
    )
}

/// yt-dlp's order of a stream's formats, worst first (its format sorter's fields that tell them
/// apart: picture or not, resolution, frame rate, codecs, bitrate, sound or not, ID). Ties left
/// between formats that would make different choices are refused ([`ties_are_harmless`]).
fn worse_first(a: &Fmt, b: &Fmt) -> Ordering {
    let maybe = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => x.total_cmp(&y),
        (x, y) => x.is_some().cmp(&y.is_some()),
    };
    // The smaller side, as the sorter's `res`.
    let res = |f: &Fmt| {
        [f.height, f.width]
            .into_iter()
            .flatten()
            .filter(|v| *v > 0)
            .min()
            .unwrap_or(0)
    };
    a.video()
        .cmp(&b.video())
        .then(res(a).cmp(&res(b)))
        .then(maybe(a.fps, b.fps))
        .then(picture_rank(a.vcodec.as_deref()).cmp(&picture_rank(b.vcodec.as_deref())))
        .then(sound_rank(a.acodec.as_deref()).cmp(&sound_rank(b.acodec.as_deref())))
        .then(maybe(a.tbr, b.tbr))
        .then(a.audio().cmp(&b.audio()))
        .then(a.id.cmp(&b.id))
}

/// The formats' IDs made unique as yt-dlp makes them (`-0`, `-1`… after each of a repeated
/// one; `f` before one that is a file type's name).
fn unique_ids(formats: &[Fmt]) -> Vec<String> {
    const TYPES: [&str; 17] = [
        "avi", "flv", "mkv", "mov", "mp4", "webm", "3gp", "aiff", "alac", "flac", "m4a", "mka",
        "mp3", "ogg", "opus", "wav", "mhtml",
    ];
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let count = |id: &str| formats.iter().filter(|f| f.id == id).count();
    formats
        .iter()
        .map(|f| {
            let id = if count(&f.id) > 1 {
                let n = seen.entry(&f.id).or_default();
                *n += 1;
                format!("{}-{}", f.id, *n - 1)
            } else {
                f.id.clone()
            };
            if id != "mp4" && TYPES.contains(&id.as_str()) {
                format!("f{id}")
            } else {
                id
            }
        })
        .collect()
}

/// Whether the choices made of `formats` (`resolve::choices`, which keeps the last of the
/// formats equal in size) come out the same in any order: formats tied for a height's place, or
/// for the sound's, must make the same line (frame rate shown, sound or not, type). MDM's order
/// ([`worse_first`]) is yt-dlp's for every stream of the corpus, but isn't proven equal: where
/// it would decide something, the stream is left to yt-dlp.
fn ties_are_harmless(formats: &[Format]) -> bool {
    let shown_fps = |f: &Format| f.fps.filter(|x| *x > 31.0).map(|x| x.round().to_bits());
    let same = |tied: &[&Format]| {
        tied.windows(2).all(|w| {
            shown_fps(w[0]) == shown_fps(w[1]) && w[0].audio == w[1].audio && w[0].ext == w[1].ext
        })
    };
    fn top(group: Vec<&Format>) -> Vec<&Format> {
        let best = group.iter().map(|f| f.size.unwrap_or(0)).max();
        group
            .into_iter()
            .filter(|f| Some(f.size.unwrap_or(0)) == best)
            .collect()
    }
    for merge in [false, true] {
        let mut heights: Vec<u32> = formats
            .iter()
            .filter(|f| f.video && (merge || f.audio))
            .filter_map(|f| f.height)
            .collect();
        heights.sort_unstable();
        heights.dedup();
        for h in heights {
            let group = formats
                .iter()
                .filter(|f| f.video && f.height == Some(h) && (merge || f.audio))
                .collect();
            if !same(&top(group)) {
                return false;
            }
        }
    }
    same(&top(formats
        .iter()
        .filter(|f| f.audio && !f.video)
        .collect()))
}

/// Read the stream at `url` as yt-dlp would list it, asked as the browser asked for it (`from`:
/// its page as the Referer, its User-Agent, its Cookie header) through `opts` (Options' proxy,
/// DNS and network adapter, as MDM's own requests). None whenever the stream isn't one this
/// reads exactly as yt-dlp does, or anything goes wrong: within [`BUDGET`] for both requests.
pub async fn fetch(url: &str, from: &FromBrowser, opts: &mdm_engine::Options) -> Option<Video> {
    fetch_within(url, from, opts, BUDGET).await
}

async fn fetch_within(
    url: &str,
    from: &FromBrowser,
    opts: &mdm_engine::Options,
    budget: Duration,
) -> Option<Video> {
    tokio::time::timeout(budget, read(url, from, opts))
        .await
        .ok()
        .flatten()
}

async fn read(url: &str, from: &FromBrowser, opts: &mdm_engine::Options) -> Option<Video> {
    // An address yt-dlp and MDM can only read alike: a playlist's on the web, as written.
    let start = Url::parse(url).ok()?;
    if !matches!(start.scheme(), "http" | "https")
        || start.as_str() != url
        || !plain_address(url)
        || !start.path().ends_with(".m3u8")
    {
        return None;
    }
    let mut opts = opts.clone();
    if let Some(agent) = from.user_agent.as_deref().filter(|a| !a.trim().is_empty()) {
        opts.user_agent = agent.to_owned();
    }
    let mut hops = Hops {
        origin: origin(&start),
        credentials: true,
        from,
        opts: &opts,
    };
    let (at, text) = hops.get(start).await?;
    let master = Master::parse(&text, &at)?;
    let (_, text) = hops.get(master.first().clone()).await?;
    master.video(&text)
}

/// The requests for one stream, each redirect followed by hand.
struct Hops<'a> {
    /// The stream's scheme, host and port.
    origin: (String, Option<String>, Option<u16>),
    /// The browser's cookie, referrer and origin still go: every hop so far was to the stream's
    /// origin. Once one wasn't, never again, for this request or the next.
    credentials: bool,
    from: &'a FromBrowser,
    opts: &'a mdm_engine::Options,
}

impl Hops<'_> {
    /// `url`'s text and the address it came from, after at most [`MAX_REDIRECTS`] redirects.
    async fn get(&mut self, mut url: Url) -> Option<(Url, String)> {
        use reqwest::header::{CONTENT_TYPE, COOKIE, LOCATION, ORIGIN, REFERER};
        let given = |s: &Option<String>| {
            s.as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        for _ in 0..=MAX_REDIRECTS {
            if !matches!(url.scheme(), "http" | "https") {
                return None;
            }
            if origin(&url) != self.origin {
                self.credentials = false;
            }
            // A client for this hop's server: its proxy and network adapter are this server's.
            let client = mdm_engine::hop_client(self.opts, &url).ok()?;
            let mut req = client.get(url.clone());
            if self.credentials {
                // The Cookie header the browser sent for this address: never a cookie jar
                // (a whole site's cookies) made into one.
                if let Some(c) = given(&self.from.cookies) {
                    req = req.header(COOKIE, c);
                }
                if let Some(r) = given(&self.from.referer) {
                    req = req.header(REFERER, r);
                }
                // The page's Origin, as its player sent it (#900).
                if let Some(o) = given(&self.from.origin) {
                    req = req.header(ORIGIN, o);
                }
            }
            let mut resp = req.send().await.ok()?;
            match resp.status().as_u16() {
                301 | 302 | 303 | 307 | 308 => {
                    let to = resp.headers().get(LOCATION)?.to_str().ok()?;
                    url = address_in(&url, to)?;
                }
                200 => {
                    // UTF-8, as yt-dlp reads a playlist without another charset.
                    if let Some(t) = resp.headers().get(CONTENT_TYPE) {
                        let t = t.to_str().ok()?.to_ascii_lowercase();
                        if let Some((_, charset)) = t.split_once("charset=")
                            && !matches!(charset.trim(), "utf-8" | "utf8")
                        {
                            return None;
                        }
                    }
                    let mut body = Vec::new();
                    while let Some(chunk) = resp.chunk().await.ok()? {
                        if body.len() + chunk.len() > MAX_BYTES {
                            return None;
                        }
                        body.extend_from_slice(&chunk);
                    }
                    // A byte-order mark, or a `<meta charset>` yt-dlp would read the text by.
                    if body.starts_with(b"\xef\xbb\xbf")
                        || body[..body.len().min(1024)]
                            .windows(5)
                            .any(|w| w == b"<meta")
                    {
                        return None;
                    }
                    return Some((url, String::from_utf8(body).ok()?));
                }
                _ => return None,
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::{choices, sub_choices};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    fn fixture(case: &str, file: &str) -> String {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/hls")
            .join(case)
            .join(file);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
    }

    /// The parity corpus: master playlists served from this computer (the lab server's stream,
    /// and playlists made for each case, served by Python's http.server), each with the playlist
    /// yt-dlp read its length from (it says so in its log: "Checking m3u8 live status") and
    /// `yt-dlp -J`'s answer (2026.10.26), trimmed to what `resolve::parse` reads. Each case: the
    /// address it was served at, and the playlist yt-dlp asked for after it.
    const CORPUS: [(&str, &str, &str); 9] = [
        // An audio rendition group, two variants with sound in it (the lab server's).
        (
            "lab",
            "http://127.0.0.1:8093/hls/master.m3u8",
            "http://127.0.0.1:8093/hls/audio/index.m3u8",
        ),
        // Two audio renditions, three variants (AVERAGE-BANDWIDTH, frame rates), subtitles in
        // two languages and one without.
        (
            "group",
            "http://127.0.0.1:8094/group.m3u8",
            "http://127.0.0.1:8094/a/en.m3u8",
        ),
        // Pictures alone, a variant of sound alone, I-frame playlists.
        (
            "videoonly",
            "http://127.0.0.1:8094/videoonly.m3u8",
            "http://127.0.0.1:8094/a/en.m3u8",
        ),
        // Heights twice, at two bitrates and frame rates; a bitrate that isn't a whole kbit/s.
        (
            "repeated",
            "http://127.0.0.1:8094/repeated.m3u8",
            "http://127.0.0.1:8094/v/v720.m3u8",
        ),
        // No CODECS: the audio group is ignored, the variants carry both.
        (
            "nocodecs",
            "http://127.0.0.1:8094/nocodecs.m3u8",
            "http://127.0.0.1:8094/a/en.m3u8",
        ),
        // AES-128 with the identity key format: no DRM.
        (
            "aes",
            "http://127.0.0.1:8094/aes.m3u8",
            "http://127.0.0.1:8094/v/aes.m3u8",
        ),
        // Upright video, an audio group without playlists, a name with a space.
        (
            "portrait",
            "http://127.0.0.1:8094/my%20clip.m3u8",
            "http://127.0.0.1:8094/v/v720.m3u8",
        ),
        // The duration from the audio rendition's playlist, though a variant comes first.
        (
            "order",
            "http://127.0.0.1:8094/order.m3u8",
            "http://127.0.0.1:8094/a/en.m3u8",
        ),
        // Reached by a redirect (the lab's /redirect?to=…): named after where it led.
        (
            "redirect",
            "http://127.0.0.1:8094/repeated.m3u8",
            "http://127.0.0.1:8094/v/v720.m3u8",
        ),
    ];

    /// What MDM reads of each stream is what yt-dlp listed: the same title, duration, formats in
    /// the same order (IDs, heights, frame rates, codecs present, bitrates) and sizes, exactly
    /// (the duration is yt-dlp's `int(sum(EXTINF))`, summed in the same order, and the size the
    /// same `tbr × duration × 125`: no tolerance), the same subtitle languages; so the same
    /// choices, with and without merging. Only `language` isn't compared: yt-dlp takes it from
    /// the format it would pick, and it matters only for automatic captions, which a stream
    /// hasn't.
    #[test]
    fn streams_are_read_as_yt_dlp_lists_them() {
        for (case, address, first) in CORPUS {
            let url = Url::parse(address).unwrap();
            let master = Master::parse(&fixture(case, "master.m3u8"), &url)
                .unwrap_or_else(|| panic!("{case}: not read"));
            assert_eq!(
                master.first().as_str(),
                first,
                "{case}: the length's playlist"
            );
            let ours = master
                .video(&fixture(case, "media.m3u8"))
                .unwrap_or_else(|| panic!("{case}: no video"));
            let theirs = crate::resolve::parse(fixture(case, "ytdlp.json").as_bytes()).unwrap();
            let ids = |v: &Video| v.formats.iter().map(|f| f.id.clone()).collect::<Vec<_>>();
            assert_eq!(ids(&ours), ids(&theirs), "{case}: order");
            assert_eq!(
                ours,
                Video {
                    language: None,
                    ..theirs.clone()
                },
                "{case}"
            );
            for merge in [false, true] {
                assert_eq!(
                    choices(&ours, merge),
                    choices(&theirs, merge),
                    "{case}, merge {merge}"
                );
            }
            assert_eq!(
                sub_choices(&ours, "de"),
                sub_choices(&theirs, "de"),
                "{case}"
            );
        }
    }

    const MASTER: &str = "#EXTM3U\n\
        #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,CODECS=\"avc1.4d401f,mp4a.40.2\"\n\
        v720.m3u8\n\
        #EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360,CODECS=\"avc1.42e01e,mp4a.40.2\"\n\
        v360.m3u8\n";
    const MEDIA: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:10\n#EXTINF:10.0,\na.ts\n#EXTINF:10.0,\nb.ts\n#EXT-X-ENDLIST\n";

    fn read(master: &str, media: &str) -> Option<Video> {
        let url = Url::parse("https://cdn.test/v/master.m3u8").unwrap();
        Master::parse(master, &url)?.video(media)
    }

    #[test]
    fn a_plain_stream_is_read() {
        let v = read(MASTER, MEDIA).unwrap();
        assert_eq!(v.title, "master");
        assert_eq!(v.duration, Some(20.0));
        let labels: Vec<String> = choices(&v, false).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["720p · H.264", "360p · H.264"]);
    }

    /// DRM, a live stream, a picture without its resolution, a playlist on another server, a
    /// second camera, a variant without a bitrate, a media playlist given as the master: none
    /// is read here (yt-dlp looks them up).
    #[test]
    fn what_is_left_to_yt_dlp() {
        let with = |line: &str| MASTER.replace("#EXTM3U\n", &format!("#EXTM3U\n{line}\n"));
        for drm in [
            "#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES,URI=\"skd://key\",KEYFORMAT=\"com.apple.streamingkeydelivery\"",
            "#EXT-X-SESSION-KEY:METHOD=SAMPLE-AES-CTR,URI=\"https://cdn.test/k\"",
            "#EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"https://cdn.test/k\",KEYFORMAT=\"urn:uuid:edef8ba9\"",
            "#EXT-X-KEY:METHOD=AES-128,URI=\"data:text/plain;base64,AAAA\",KEYFORMAT=\"com.microsoft.playready\"",
            "#EXT-X-FAXS-CM:abc",
        ] {
            assert!(read(&with(drm), MEDIA).is_none(), "{drm}");
        }
        // Plain AES-128 is no DRM.
        assert!(
            read(
                &with("#EXT-X-SESSION-KEY:METHOD=AES-128,URI=\"k.bin\",KEYFORMAT=\"identity\""),
                MEDIA
            )
            .is_some()
        );
        // DRM in the media playlist.
        let keyed = MEDIA.replace(
            "#EXT-X-TARGETDURATION:10\n",
            "#EXT-X-TARGETDURATION:10\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"k\"\n",
        );
        assert!(read(MASTER, &keyed).is_none());
        // Live: no end to its playlist.
        assert!(read(MASTER, &MEDIA.replace("#EXT-X-ENDLIST\n", "")).is_none());
        // A picture without its resolution.
        assert!(read(&MASTER.replace("RESOLUTION=640x360,", ""), MEDIA).is_none());
        assert!(
            read(
                &MASTER
                    .replace("RESOLUTION=640x360,", "")
                    .replace(",CODECS=\"avc1.42e01e,mp4a.40.2\"", ""),
                MEDIA
            )
            .is_none()
        );
        // Sound alone needs none.
        let sound = MASTER.replace(
            "RESOLUTION=640x360,CODECS=\"avc1.42e01e,mp4a.40.2\"",
            "CODECS=\"mp4a.40.2\"",
        );
        assert!(read(&sound, MEDIA).is_some());
        // A variant on another server, scheme or port.
        for other in [
            "https://other.test/v/v360.m3u8",
            "http://cdn.test/v/v360.m3u8",
            "https://cdn.test:8443/v/v360.m3u8",
        ] {
            assert!(
                read(&MASTER.replace("v360.m3u8", other), MEDIA).is_none(),
                "{other}"
            );
        }
        assert!(
            read(
                &MASTER.replace("v360.m3u8", "https://cdn.test/w/v360.m3u8"),
                MEDIA
            )
            .is_some()
        );
        // Another camera's picture.
        assert!(
            read(
                &with("#EXT-X-MEDIA:TYPE=VIDEO,GROUP-ID=\"cam\",NAME=\"Side\",URI=\"side.m3u8\""),
                MEDIA
            )
            .is_none()
        );
        // No bitrate.
        assert!(read(&MASTER.replace("BANDWIDTH=800000,", ""), MEDIA).is_none());
        // A media playlist, or nothing of the kind.
        assert!(read(MEDIA, MEDIA).is_none());
        assert!(read("<html><title>x</title></html>", MEDIA).is_none());
        // HDR, sorted apart by yt-dlp.
        assert!(read(&MASTER.replace("avc1.4d401f", "dvh1.05.06"), MEDIA).is_none());
        // Dailymotion's plain-file twins.
        assert!(
            read(
                &MASTER.replace(
                    "CODECS=\"avc1.42e01e,mp4a.40.2\"",
                    "CODECS=\"avc1.42e01e,mp4a.40.2\",PROGRESSIVE-URI=\"https://cdn.test/v.mp4\""
                ),
                MEDIA
            )
            .is_none()
        );
    }

    /// Two variants of one height, the same size and different frame rates: which one
    /// `resolve::choices` shows depends on the order, and MDM's isn't proven to be yt-dlp's
    /// there.
    #[test]
    fn ties_that_matter_are_left_to_yt_dlp() {
        let twin = "#EXTM3U\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,FRAME-RATE=60,CODECS=\"avc1.4d401f,mp4a.40.2\"\n\
            a.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,FRAME-RATE=25,CODECS=\"hvc1.1.6.L93.B0,mp4a.40.2\"\n\
            b.m3u8\n";
        assert!(read(twin, MEDIA).is_none());
        // The same frame rate shown: either is the same line.
        let same = twin.replace("FRAME-RATE=25", "FRAME-RATE=60");
        let v = read(&same, MEDIA).unwrap();
        assert_eq!(
            v.formats.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(),
            ["2000-0", "2000-1"]
        );
    }

    #[test]
    fn attributes_as_yt_dlp_finds_them() {
        let a = attributes(
            "#EXT-X-STREAM-INF:BANDWIDTH=800000,CODECS=\"avc1.4d401f,mp4a.40.2\",NAME=\"\",RESOLUTION=640x360,lower=1,X=\"a\"b,Y=2",
        )
        .unwrap();
        assert_eq!(a["BANDWIDTH"], "800000");
        assert_eq!(a["CODECS"], "avc1.4d401f,mp4a.40.2");
        assert_eq!(a["RESOLUTION"], "640x360");
        assert_eq!(a["Y"], "2");
        // An empty quoted value, a lowercase key and a quote not followed by a comma are no match.
        assert!(!a.contains_key("NAME") && !a.contains_key("lower") && !a.contains_key("X"));
        assert_eq!(resolution("1280x720").ok().flatten(), Some((1280, 720)));
        assert_eq!(
            resolution("w12 1920X1080").ok().flatten(),
            Some((1920, 1080))
        );
        assert_eq!(resolution("none").ok().flatten(), None);
        assert!(resolution("99999999999x1").is_err());
        assert!(attributes(&"A=1,".repeat(MAX_ATTRIBUTES + 1)).is_none());
    }

    /// A crafted line can't make the search cost its square: a long run of key characters with
    /// no `=`, or an unclosed quote again and again, comes back at once (refused when longer
    /// than any real line), and a line just under the cap is read in linear time.
    #[test]
    fn crafted_attribute_lines_cost_no_square() {
        // Longer than any real line: refused from its length alone.
        assert!(attributes(&format!("#EXT-X-KEY:{}", "A".repeat(MAX_BYTES))).is_none());
        assert!(attributes(&format!("#EXT-X-KEY:{}", "A=\"".repeat(MAX_BYTES / 3))).is_none());
        // Under the cap: 8 times the line costs about 8 times as long (not 64), on any machine.
        let crafted = |n: usize| {
            [
                format!("#EXT-X-KEY:{}", "A".repeat(n)),
                format!("#EXT-X-KEY:{}", "A=\"".repeat(n / 3)),
                format!("#EXT-X-KEY:METHOD=AES-128,{}", "B".repeat(n)),
            ]
        };
        let small = (MAX_ATTRIBUTE_LINE - 40) / mdm_labserver::growth::SCALE;
        mdm_labserver::growth::assert_linear(small, crafted, |lines| {
            for _ in 0..20 {
                for line in &lines {
                    std::hint::black_box(attributes(line));
                }
            }
        });
        // The run's skip changes nothing: a key after a failed run is still found.
        let a = attributes("#EXT-X-KEY:AAAA BBB-9 METHOD=NONE").unwrap();
        assert_eq!(a.get("METHOD").map(String::as_str), Some("NONE"));
    }

    #[test]
    fn codecs_as_yt_dlp_reads_them() {
        let c = |s: &str| codecs(Some(s)).ok().flatten();
        let pair = |v: &str, a: &str| Some((v.to_owned(), a.to_owned()));
        assert_eq!(c("avc1.4d401f,mp4a.40.2"), pair("avc1.4d401f", "mp4a.40.2"));
        assert_eq!(c("AVC1.4D401F"), pair("avc1.4D401F", "none"));
        assert_eq!(c(" mp4a.40.2, "), pair("none", "mp4a.40.2"));
        assert_eq!(c("av01.0.05M.08,opus"), pair("av01.0.05M.08", "opus"));
        assert_eq!(c("foo,bar"), pair("foo", "bar"));
        assert_eq!(c("foo"), None);
        assert!(codecs(Some("av01.0.09M.10")).is_err(), "HDR");
        assert!(codecs(Some("vp09.02.10.10")).is_err(), "HDR");
        assert!(codecs(Some("wvtt")).is_err());
        assert_eq!(no_leading_zeros("vp09.02.10"), "vp9.2.10");
        assert_eq!(no_leading_zeros("av01.0.05M"), "av1.0.5M");
        assert_eq!(no_leading_zeros("a00.000"), "a0.0");
    }

    #[test]
    fn titles_as_yt_dlp_names_streams() {
        let t = |s: &str| title_of(&Url::parse(s).unwrap());
        assert_eq!(
            t("https://x.test/v/master.m3u8?t=1").as_deref(),
            Some("master")
        );
        assert_eq!(
            t("https://x.test/my%20clip.m3u8").as_deref(),
            Some("my clip")
        );
        // The extension is taken off before decoding (`a%2Eb` keeps its dot).
        assert_eq!(t("https://x.test/a%2Eb.m3u8").as_deref(), Some("a.b"));
        assert_eq!(t("https://x.test/a.b.m3u8/").as_deref(), Some("a.b"));
        assert_eq!(t("https://x.test/.m3u8").as_deref(), Some(".m3u8"));
        assert_eq!(t("https://x.test/%FF.m3u8"), None);
    }

    /// No input makes the reading panic: lines, attributes, numbers and codecs out of order,
    /// cut short, repeated, or random bytes.
    #[test]
    fn nothing_makes_it_panic() {
        let bits = [
            "#EXTM3U",
            "\n",
            "\r\n",
            "\r",
            "#EXT-X-STREAM-INF:",
            "#EXT-X-MEDIA:",
            "#EXTINF:",
            "#EXT-X-ENDLIST",
            "#EXT-X-KEY:",
            "#EXT-X-I-FRAME-STREAM-INF:",
            "BANDWIDTH=",
            "AVERAGE-BANDWIDTH=",
            "RESOLUTION=",
            "FRAME-RATE=",
            "CODECS=",
            "AUDIO=",
            "VIDEO=",
            "TYPE=AUDIO",
            "TYPE=SUBTITLES",
            "TYPE=VIDEO",
            "GROUP-ID=",
            "NAME=",
            "URI=",
            "LANGUAGE=",
            "METHOD=",
            "\"",
            ",",
            "=",
            "x",
            "X",
            "0",
            "00",
            "1280",
            "720",
            "-1",
            "1e309",
            "nan",
            "1_0",
            ".",
            "..",
            "/",
            "//",
            "http://",
            "a.m3u8",
            "%",
            "%FF",
            "é",
            "\u{0}",
            "\t",
            " ",
            "avc1.4d401f",
            "mp4a.40.2",
            "av01.0.05M.10",
            "dvh1",
            "vp09.02",
            "foo",
            ";",
        ];
        let url = Url::parse("https://cdn.test/v/master.m3u8").unwrap();
        let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let n = (next() % 60) as usize;
            let mut text = String::new();
            for _ in 0..n {
                text.push_str(bits[(next() % bits.len() as u64) as usize]);
            }
            let bytes: Vec<u8> = (0..(next() % 80)).map(|_| next() as u8).collect();
            for t in [
                text.clone(),
                String::from_utf8_lossy(&bytes).into_owned(),
                format!("#EXTM3U\n#EXT-X-STREAM-INF:{text}"),
            ] {
                if let Some(m) = Master::parse(&t, &url) {
                    let _ = m.video(&t);
                    let _ = m.video(MEDIA);
                }
                let _ = attributes(&t);
                let _ = codecs(Some(&t));
                let _ = resolution(&t);
                let _ = py_float(&t);
            }
        }
    }

    /// One request as a test server saw it: its path and headers (lowercase names).
    #[derive(Clone, Debug)]
    struct Seen {
        path: String,
        headers: Vec<(String, String)>,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        }
    }

    /// An answer: status, headers, body, sent after `wait`; `length`: with a Content-Length
    /// (else the body ends with the connection).
    struct Answer {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        wait: Duration,
        length: bool,
    }

    fn ok(body: &str) -> Answer {
        Answer {
            status: 200,
            headers: vec![("Content-Type", "application/vnd.apple.mpegurl".into())],
            body: body.as_bytes().to_vec(),
            wait: Duration::ZERO,
            length: true,
        }
    }

    fn moved(to: &str) -> Answer {
        Answer {
            status: 302,
            headers: vec![("Location", to.to_owned())],
            body: Vec::new(),
            wait: Duration::ZERO,
            length: true,
        }
    }

    type Route = Arc<dyn Fn(&str) -> Answer + Send + Sync>;

    /// A server on 127.0.0.1 answering each request by its path, keeping what it saw.
    struct Server {
        port: u16,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Server {
        fn start(route: Route) -> Server {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
            let kept = seen.clone();
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let (route, kept) = (route.clone(), kept.clone());
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut first = String::new();
                        if reader.read_line(&mut first).is_err() {
                            return;
                        }
                        let path = first.split(' ').nth(1).unwrap_or_default().to_owned();
                        let mut headers = Vec::new();
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                break;
                            }
                            let line = line.trim_end();
                            if line.is_empty() {
                                break;
                            }
                            if let Some((n, v)) = line.split_once(':') {
                                headers.push((n.trim().to_ascii_lowercase(), v.trim().to_owned()));
                            }
                        }
                        kept.lock().unwrap().push(Seen {
                            path: path.clone(),
                            headers,
                        });
                        let a = route(&path);
                        std::thread::sleep(a.wait);
                        let mut head = format!("HTTP/1.1 {} X\r\nConnection: close\r\n", a.status);
                        for (n, v) in &a.headers {
                            head.push_str(&format!("{n}: {v}\r\n"));
                        }
                        if a.length {
                            head.push_str(&format!("Content-Length: {}\r\n", a.body.len()));
                        }
                        head.push_str("\r\n");
                        let _ = stream.write_all(head.as_bytes());
                        for chunk in a.body.chunks(64 * 1024) {
                            if stream.write_all(chunk).is_err() {
                                return;
                            }
                        }
                        // The body ends here (none has a length of its own then).
                        let _ = stream.flush();
                        let _ = stream.shutdown(std::net::Shutdown::Write);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
                        let _ = reader.read_to_end(&mut Vec::new());
                    });
                }
            });
            Server { port, seen }
        }

        fn url(&self, path: &str) -> String {
            format!("http://127.0.0.1:{}{path}", self.port)
        }

        fn seen(&self) -> Vec<Seen> {
            self.seen.lock().unwrap().clone()
        }
    }

    fn browser() -> FromBrowser {
        FromBrowser {
            referer: Some("https://videos.test/watch".into()),
            origin: Some("https://videos.test".into()),
            user_agent: Some("UA/1".into()),
            cookies: Some("sid=secret".into()),
            jar: vec![mdm_ipc::BrowserCookie {
                name: "jar".into(),
                value: "never".into(),
                domain: "127.0.0.1".into(),
                ..Default::default()
            }],
        }
    }

    fn fetch_now(url: &str, budget: Duration) -> Option<Video> {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let opts = crate::net(mdm_engine::Proxy::None);
        rt.block_on(fetch_within(url, &browser(), &opts, budget))
    }

    /// The stream as the browser asked for it: its cookie header (never the jar), its page as
    /// the referrer, its player's Origin (#900), its User-Agent, on both requests to the
    /// stream's server.
    #[test]
    fn the_stream_is_asked_for_as_the_browser_did() {
        let s = Server::start(Arc::new(|path: &str| match path {
            "/v/master.m3u8" => ok(MASTER),
            "/v/v720.m3u8" => ok(MEDIA),
            _ => Answer {
                status: 404,
                ..ok("")
            },
        }));
        let v = fetch_now(&s.url("/v/master.m3u8"), BUDGET).unwrap();
        assert_eq!(v.title, "master");
        let seen = s.seen();
        assert_eq!(
            seen.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(),
            ["/v/master.m3u8", "/v/v720.m3u8"]
        );
        for r in &seen {
            assert_eq!(r.header("cookie"), Some("sid=secret"));
            assert_eq!(r.header("referer"), Some("https://videos.test/watch"));
            assert_eq!(r.header("origin"), Some("https://videos.test"));
            assert_eq!(r.header("user-agent"), Some("UA/1"));
        }
        // Not a playlist's address: not even asked.
        assert!(fetch_now(&s.url("/v/master.mpd"), BUDGET).is_none());
        assert_eq!(s.seen().len(), 2);
    }

    /// A redirect to another port (another server) takes the browser's cookie and referrer
    /// away, for good: not even back on the stream's own server, nor for the second request.
    /// The stream is named after where the redirects led, as yt-dlp names it.
    #[test]
    fn a_redirect_elsewhere_carries_no_cookie() {
        let other = Server::start(Arc::new(|path: &str| match path {
            "/repeated.m3u8" => ok(&fixture("redirect", "master.m3u8")),
            "/v/v720.m3u8" => ok(&fixture("redirect", "media.m3u8")),
            _ => Answer {
                status: 404,
                ..ok("")
            },
        }));
        let to = other.url("/repeated.m3u8");
        let first = Server::start(Arc::new(move |path: &str| match path {
            "/r/start.m3u8" => moved("/r/same.m3u8"),
            "/r/same.m3u8" => moved(&to),
            _ => Answer {
                status: 404,
                ..ok("")
            },
        }));
        let v = fetch_now(&first.url("/r/start.m3u8"), BUDGET).unwrap();
        let theirs = crate::resolve::parse(fixture("redirect", "ytdlp.json").as_bytes()).unwrap();
        assert_eq!(
            v,
            Video {
                language: None,
                ..theirs
            }
        );
        // On the stream's own server, through a redirect there: the cookie and the referrer.
        for r in first.seen() {
            assert_eq!(r.header("cookie"), Some("sid=secret"), "{}", r.path);
            assert_eq!(r.header("referer"), Some("https://videos.test/watch"));
        }
        let elsewhere = other.seen();
        assert_eq!(elsewhere.len(), 2);
        for r in &elsewhere {
            assert_eq!(r.header("cookie"), None, "{}", r.path);
            assert_eq!(r.header("referer"), None, "{}", r.path);
            assert_eq!(r.header("origin"), None, "{}", r.path);
            assert_eq!(r.header("user-agent"), Some("UA/1"));
        }
        // Back to the stream's server after another: still none.
        let back = Arc::new(Mutex::new(String::new()));
        let back2 = back.clone();
        let bouncer = Server::start(Arc::new(move |_: &str| moved(&back2.lock().unwrap())));
        let bounce = bouncer.url("/x.m3u8");
        let home = Server::start(Arc::new(move |path: &str| match path {
            "/start.m3u8" => moved(&bounce),
            "/master.m3u8" => ok(MASTER),
            "/v720.m3u8" => ok(MEDIA),
            _ => Answer {
                status: 404,
                ..ok("")
            },
        }));
        *back.lock().unwrap() = home.url("/master.m3u8");
        assert!(fetch_now(&home.url("/start.m3u8"), BUDGET).is_some());
        let seen = home.seen();
        assert_eq!(seen[0].header("cookie"), Some("sid=secret"));
        assert!(
            seen[1..]
                .iter()
                .all(|r| r.header("cookie").is_none() && r.header("referer").is_none()),
            "{seen:?}"
        );
        assert_eq!(seen.len(), 3);
        // Five redirects at most.
        let looping = Server::start(Arc::new(|path: &str| {
            let n: u32 = path
                .trim_start_matches("/r")
                .trim_end_matches(".m3u8")
                .parse()
                .unwrap_or(0);
            if n < 6 {
                moved(&format!("/r{}.m3u8", n + 1))
            } else {
                ok(MASTER)
            }
        }));
        assert!(fetch_now(&looping.url("/r0.m3u8"), BUDGET).is_none());
        assert_eq!(looping.seen().len(), 6);
    }

    /// More than 2 MB, counted as it comes (with no Content-Length to go by), and 5 s for both
    /// requests together, are too much: nothing then.
    #[test]
    fn too_big_or_too_slow_is_nothing() {
        let padded = |size: usize| {
            let mut text = MASTER.to_owned();
            while text.len() < size {
                text.push_str(
                    "#EXT-X-PADDING:xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\n",
                );
            }
            text
        };
        let s = Server::start(Arc::new(move |path: &str| match path {
            "/small.m3u8" => Answer {
                length: false,
                ..ok(&padded(MAX_BYTES - 1000))
            },
            "/big.m3u8" => Answer {
                length: false,
                ..ok(&padded(MAX_BYTES + 1))
            },
            "/v720.m3u8" => ok(MEDIA),
            "/slow/master.m3u8" => Answer {
                wait: Duration::from_millis(250),
                ..ok(MASTER)
            },
            "/slow/v720.m3u8" => Answer {
                wait: Duration::from_millis(250),
                ..ok(MEDIA)
            },
            _ => Answer {
                status: 404,
                ..ok("")
            },
        }));
        assert!(
            fetch_now(&s.url("/small.m3u8"), BUDGET).is_some(),
            "under 2 MB"
        );
        assert!(
            fetch_now(&s.url("/big.m3u8"), BUDGET).is_none(),
            "over 2 MB"
        );
        // Each request alone fits in 400 ms; both don't.
        assert!(fetch_now(&s.url("/slow/master.m3u8"), Duration::from_millis(1000)).is_some());
        let at = std::time::Instant::now();
        assert!(fetch_now(&s.url("/slow/master.m3u8"), Duration::from_millis(400)).is_none());
        assert!(at.elapsed() < Duration::from_millis(900));
    }
}
