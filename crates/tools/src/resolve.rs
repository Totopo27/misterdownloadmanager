//! What a page holds, by `yt-dlp -J`: the title and the formats, turned into the few choices a
//! person picks from (one per resolution, plus audio only), as IDM's video panel offers them.

use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use crate::proc::{self, ProcError};

#[derive(Clone, Debug, PartialEq)]
pub struct Video {
    pub title: String,
    /// Seconds.
    pub duration: Option<f64>,
    pub formats: Vec<Format>,
    /// The uploader's subtitles, by name (#68).
    pub subtitles: Vec<Track>,
    /// The site's automatic captions (speech recognition, and their machine translations).
    pub automatic: Vec<Track>,
    /// The language the video is spoken in, when the site says (`en`, `pt-BR`).
    pub language: Option<String>,
    /// A plain file at this address, not a page or a stream (yt-dlp's `direct`). yt-dlp says
    /// nothing of its size then: the server is asked (`needs_size`, #320).
    pub direct: bool,
}

impl Video {
    /// A plain file whose size nobody has said yet.
    pub fn needs_size(&self) -> bool {
        self.direct && self.formats.iter().all(|f| f.size.is_none())
    }

    /// The size the file's server said.
    pub fn set_size(&mut self, size: u64) {
        for f in &mut self.formats {
            f.size = Some(size);
        }
    }
}

/// One language of subtitles a page has (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    /// yt-dlp's language code, what `--sub-langs` takes (`en`, `pt-BR`, `en-orig`).
    pub code: String,
    /// The site's name for it, else the code.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    pub id: String,
    pub ext: String,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub video: bool,
    pub audio: bool,
    /// Bytes, exact or estimated.
    pub size: Option<u64>,
    /// Kbit/s, when the site says (a stream's variants differ by it alone).
    pub tbr: Option<f64>,
    /// Comes as an HLS stream (yt-dlp's `protocol` `m3u8`, `m3u8_native`): its pieces make a
    /// proper file only once ffmpeg has put them into one (#569).
    pub hls: bool,
}

/// One line of the choice list: what to show and what to ask yt-dlp for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    /// yt-dlp's `-f` selector.
    pub selector: String,
    /// The container of the result (for the file name).
    pub ext: String,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
struct RawVideo {
    title: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    formats: Vec<RawFormat>,
    #[serde(default)]
    subtitles: Option<std::collections::BTreeMap<String, Vec<RawSub>>>,
    #[serde(default)]
    automatic_captions: Option<std::collections::BTreeMap<String, Vec<RawSub>>>,
    language: Option<String>,
    /// `null` from some extractors (Wikimedia Commons'): not a plain file.
    #[serde(default)]
    direct: Option<bool>,
}

/// One file of a subtitle language (its formats: srt, vtt, json3…); only the name is read.
#[derive(Deserialize)]
struct RawSub {
    name: Option<String>,
}

/// Not subtitles, though yt-dlp lists them with them: a live stream's chat replay, comments
/// laid over the picture.
const NOT_SUBTITLES: [&str; 2] = ["live_chat", "danmaku"];

/// A page's subtitle languages, by name (the code when the site gives none).
fn tracks(raw: Option<std::collections::BTreeMap<String, Vec<RawSub>>>) -> Vec<Track> {
    let mut out: Vec<Track> = raw
        .unwrap_or_default()
        .into_iter()
        .filter(|(code, files)| !files.is_empty() && !NOT_SUBTITLES.contains(&code.as_str()))
        .map(|(code, files)| Track {
            name: files
                .iter()
                .find_map(|f| f.name.clone())
                .map(|n| n.trim().to_owned())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| code.clone()),
            code,
        })
        .collect();
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// Subtitle languages that come without names, as yt-dlp lists an HLS stream's (`de`, `und`):
/// what [`tracks`] makes of them (`crate::manifest`).
pub(crate) fn unnamed_tracks(codes: impl IntoIterator<Item = String>) -> Vec<Track> {
    tracks(Some(
        codes
            .into_iter()
            .map(|code| (code, vec![RawSub { name: None }]))
            .collect(),
    ))
}

/// The size of `duration` seconds at `tbr` kbit/s (kbit/s × 125 = bytes/s), as yt-dlp's
/// `filesize_from_tbr` reckons it: the same operations in the same order, so a stream's variants
/// come out the same whether yt-dlp listed them or MDM read the stream itself
/// (`crate::manifest`).
pub(crate) fn estimate(duration: f64, tbr: f64) -> u64 {
    (duration * tbr * 125.0) as u64
}

#[derive(Deserialize)]
struct RawFormat {
    format_id: String,
    ext: Option<String>,
    height: Option<u32>,
    fps: Option<f64>,
    vcodec: Option<String>,
    acodec: Option<String>,
    filesize: Option<u64>,
    filesize_approx: Option<u64>,
    tbr: Option<f64>,
    protocol: Option<String>,
}

/// Whether `title` is only the last part of `page`'s address, which is what yt-dlp calls a video
/// on a site it has no page reader for (#345: on some servers that part is a long hash).
pub fn named_after_address(title: &str, page: &str) -> bool {
    let Some(last) = url::Url::parse(page).ok().and_then(|u| {
        u.path_segments()
            .and_then(|mut p| p.next_back().map(str::to_owned))
    }) else {
        return false;
    };
    let last = percent_encoding::percent_decode_str(&last).decode_utf8_lossy();
    let stem = last.rsplit_once('.').map_or(last.as_ref(), |(s, _)| s);
    !stem.is_empty() && title.trim() == stem
}

/// Parse `yt-dlp -J` output.
pub fn parse(json: &[u8]) -> Option<Video> {
    let raw: RawVideo = serde_json::from_slice(json).ok()?;
    // yt-dlp: "none" means the stream isn't there; a missing codec is just unknown (plain
    // files on generic sites carry both).
    let has = |c: &Option<String>| c.as_deref() != Some("none");
    // An HLS stream's variants give their bitrate (BANDWIDTH) but no size, though yt-dlp knows
    // the duration: about bitrate × duration (kbit/s × 125 = bytes/s), as IDM shows it. A live
    // stream has no duration, and stays without one. Never for a plain file, whose server says
    // its exact size (`needs_size`, #320).
    let duration = raw
        .duration
        .filter(|d| d.is_finite() && *d > 0.0 && raw.direct != Some(true));
    let estimate = |tbr: Option<f64>| {
        let tbr = tbr.filter(|t| t.is_finite() && *t > 0.0)?;
        Some(estimate(duration?, tbr))
    };
    Some(Video {
        title: raw.title.unwrap_or_else(|| "video".into()),
        duration: raw.duration,
        formats: raw
            .formats
            .into_iter()
            .map(|f| Format {
                video: has(&f.vcodec),
                audio: has(&f.acodec),
                id: f.format_id,
                ext: f.ext.unwrap_or_default(),
                height: f.height,
                fps: f.fps,
                size: f.filesize.or(f.filesize_approx).or_else(|| estimate(f.tbr)),
                tbr: f.tbr,
                hls: f.protocol.as_deref().is_some_and(|p| p.contains("m3u8")),
            })
            .collect(),
        subtitles: tracks(raw.subtitles),
        automatic: tracks(raw.automatic_captions),
        language: raw.language,
        direct: raw.direct == Some(true),
    })
}

/// A subtitle language offered in the video window (#68).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubChoice {
    pub code: String,
    pub name: String,
    /// The site's automatic captions, not the uploader's.
    pub automatic: bool,
}

/// The subtitle languages to offer, the uploader's first (all of them), then the automatic
/// captions that are worth a place: the ones in the language spoken (YouTube marks them
/// `-orig`), and their translations into `wanted` (the interface language) and English. A
/// site's automatic list holds a translation into every language it knows, a hundred and more:
/// offered whole, the list would be useless.
pub fn sub_choices(v: &Video, wanted: &str) -> Vec<SubChoice> {
    let mut out: Vec<SubChoice> = v
        .subtitles
        .iter()
        .map(|t| SubChoice {
            code: t.code.clone(),
            name: t.name.clone(),
            automatic: false,
        })
        .collect();
    // `pt-BR` and `pt` are the same language here.
    let base = |c: &str| c.split(['-', '_']).next().unwrap_or(c).to_ascii_lowercase();
    let spoken = v.language.as_deref().map(base);
    let worth = |t: &Track| {
        t.code.ends_with("-orig")
            || spoken.as_deref() == Some(base(&t.code).as_str())
            || base(&t.code) == base(wanted)
            || base(&t.code) == "en"
    };
    for t in v.automatic.iter().filter(|t| worth(t)) {
        // The uploader's own in that language is the better one.
        if v.subtitles.iter().any(|u| u.code == t.code) {
            continue;
        }
        out.push(SubChoice {
            code: t.code.clone(),
            name: t.name.clone(),
            automatic: true,
        });
    }
    out
}

/// A page that lists videos (#69): a playlist, a channel, an album; what one look-up says of
/// it. Its videos are listed afterwards, streamed (`crate::playlist`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListPage {
    pub title: String,
    /// How many videos, when the site says (a playlist does, a channel doesn't).
    pub total: Option<u64>,
}

/// What a page is: one video, or a list of them.
#[derive(Clone, Debug, PartialEq)]
pub enum Looked {
    Video(Video),
    List(ListPage),
}

#[derive(Deserialize)]
struct RawKind {
    #[serde(rename = "_type")]
    kind: Option<String>,
    title: Option<String>,
    playlist_count: Option<u64>,
}

/// Parse `yt-dlp -J` output: a video, or a list's first page (`_type: playlist`).
pub fn parse_looked(json: &[u8]) -> Option<Looked> {
    let kind: RawKind = serde_json::from_slice(json).ok()?;
    if kind.kind.as_deref() == Some("playlist") {
        return Some(Looked::List(ListPage {
            title: kind
                .title
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "playlist".into()),
            total: kind.playlist_count,
        }));
    }
    parse(json).map(Looked::Video)
}

/// How long one yt-dlp look-up may take. `proc::look_up` may run it twice (the second time
/// without the cookies, #352).
pub const LOOK_UP_TIMEOUT: Duration = Duration::from_secs(90);

/// Look `url` up with yt-dlp: one video (`--no-playlist`: a video in a playlist is that video),
/// or, for a page that is a list, only what it is and its first entry, flat (#69: never every
/// video of a 6,000-long playlist at once; `--flat-playlist -I 1:1` doesn't change a video's
/// look-up).
pub fn look_up(exe: &Path, url: &str, browser: &proc::FromBrowser) -> Result<Looked, ProcError> {
    look_up_kept(exe, url, browser).map(|(looked, _)| looked)
}

/// [`look_up`], with yt-dlp's answer as it gave it: the browser's quality menu keeps it for the
/// download chosen there, which then doesn't read the page again (`video::Preloaded`).
pub fn look_up_kept(
    exe: &Path,
    url: &str,
    browser: &proc::FromBrowser,
) -> Result<(Looked, proc::LookedUp), ProcError> {
    let mut args: Vec<String> = [
        "-J",
        "--no-playlist",
        "--flat-playlist",
        "--playlist-items",
        "1:1",
        "--no-warnings",
    ]
    .map(String::from)
    .to_vec();
    // Kept until yt-dlp has answered.
    let (theirs, _cookies) = browser.args(url)?;
    args.extend(theirs);
    let out = proc::look_up(exe, &args, url, LOOK_UP_TIMEOUT)?;
    let looked = parse_looked(&out.json).ok_or_else(|| ProcError::Failed {
        tool: "yt-dlp".into(),
        message: "it gave no video information".into(),
    })?;
    Ok((looked, out))
}

/// Look `url` up as one video (a list is an error here).
pub fn resolve(exe: &Path, url: &str, browser: &proc::FromBrowser) -> Result<Video, ProcError> {
    match look_up(exe, url, browser)? {
        Looked::Video(v) => Ok(v),
        Looked::List(l) => Err(ProcError::Failed {
            tool: "yt-dlp".into(),
            message: format!("the address is a list of videos ({})", l.title),
        }),
    }
}

/// The audio choice's label starts with this (the app translates it).
pub const AUDIO_ONLY: &str = "Audio only";
/// The sound as the site has it, not converted (#70): its label starts with this.
pub const AUDIO_ORIGINAL: &str = "Audio only original";

/// Opus and FLAC choices' selectors (#70): the best sound converted by ffmpeg (Opus as is when
/// it already is Opus: yt-dlp then only moves it into an `.opus` file).
pub const OPUS: &str = "opus:";
pub const FLAC: &str = "flac:";

/// What an audio choice converts to: the file type and yt-dlp's `--audio-quality`, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioOut {
    pub format: &'static str,
    pub quality: Option<String>,
}

/// The conversion an audio choice's selector asks for (MP3 #32, Opus and FLAC #70); none for a
/// video or the original sound.
pub fn audio_conversion(selector: &str) -> Option<AudioOut> {
    if let Some(q) = mp3_quality(selector) {
        return Some(AudioOut {
            format: "mp3",
            quality: Some(q),
        });
    }
    match selector {
        OPUS => Some(AudioOut {
            format: "opus",
            quality: None,
        }),
        FLAC => Some(AudioOut {
            format: "flac",
            quality: None,
        }),
        _ => None,
    }
}

/// An MP3 choice's selector: `mp3:<kbps>`, the best sound converted to MP3 by ffmpeg at that
/// constant bitrate, or `mp3:0` at its best variable one (`VideoJob::start`; #32).
pub const MP3: &str = "mp3:";

/// The MP3 bitrates offered, best first.
pub const MP3_RATES: [u32; 4] = [320, 256, 192, 128];

/// yt-dlp's `--audio-quality` for an MP3 choice's `<kbps>` (`MP3`): `0` (the best variable
/// bitrate) or a bitrate MDM offers, written exactly (`192`, not `+192` or `0192`); none for
/// anything else.
pub fn mp3_quality(selector: &str) -> Option<String> {
    let kbps = selector.strip_prefix(MP3)?;
    if kbps == "0" {
        return Some("0".into());
    }
    MP3_RATES
        .iter()
        .find(|k| k.to_string() == kbps)
        .map(|k| format!("{k}K"))
}

/// Containers that only ever hold sound.
fn audio_container(ext: &str) -> bool {
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "m4a" | "mp3" | "opus" | "aac" | "flac" | "wav" | "weba" | "oga" | "mka"
    )
}

/// A choice that saves only the sound (no picture to show subtitles on, #68).
pub fn is_audio(c: &Choice) -> bool {
    sound_only(&c.selector, &c.ext)
}

/// A selector and file type that save only the sound: a choice ([`is_audio`]) or a playlist's
/// quality (saved in the Music category's folder, #611).
pub fn sound_only(selector: &str, ext: &str) -> bool {
    selector == "ba" || audio_conversion(selector).is_some() || audio_container(ext)
}

/// A choice whose file can hold subtitles put into it (yt-dlp's `--embed-subs` with SRT).
pub fn holds_subtitles(c: &Choice) -> bool {
    matches!(
        c.ext.to_ascii_lowercase().as_str(),
        "mp4" | "mkv" | "mov" | "m4v"
    )
}

/// Would merging (ffmpeg) bring a better video than the ones already carrying their sound (#351)?
/// YouTube's only such format is 360p, and without a JavaScript runtime yt-dlp often has none:
/// the list then holds the sound alone, so the window must say why.
pub fn needs_merging(v: &Video) -> bool {
    let best = |merging: bool| {
        v.formats
            .iter()
            .filter(|f| f.video && (merging || f.audio))
            .filter_map(|f| f.height)
            .max()
    };
    best(true) > best(false)
}

/// A format selector for yt-dlp without ffmpeg (#569): each of its "best" picks (`ba`,
/// `b[height=720]`) tried first among the formats that come as a file, then as it was. An HLS
/// stream's pieces are joined as they come: without ffmpeg to put them into the container they
/// were named for, YouTube's HLS sound is raw AAC under an `.mp4` name, which no player opens
/// as one. A merge (`bv*+ba`, which needs ffmpeg anyway) and a format named by its ID (the one
/// asked for) are left as they are.
pub fn without_remuxing(selector: &str) -> String {
    const BEST: [&str; 10] = [
        "b",
        "ba",
        "bv",
        "b*",
        "ba*",
        "bv*",
        "best",
        "bestaudio",
        "bestvideo",
        "best*",
    ];
    selector
        .split('/')
        .map(|alt| {
            let base = alt.split('[').next().unwrap_or(alt).trim();
            if BEST.contains(&base) && !alt.contains('+') && !alt.contains("protocol") {
                format!("{alt}[protocol!*=m3u8]/{alt}")
            } else {
                alt.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The choices, best first: each resolution (merged with the best audio when ffmpeg is there,
/// else only formats that carry both), then audio only.
pub fn choices(v: &Video, can_merge: bool) -> Vec<Choice> {
    let mut heights: Vec<u32> = v
        .formats
        .iter()
        .filter(|f| f.video && (can_merge || f.audio))
        .filter_map(|f| f.height)
        .collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    // Without ffmpeg a sound that comes as a file (#569): an HLS one stays its raw pieces.
    let best_audio = v
        .formats
        .iter()
        .filter(|f| f.audio && !f.video)
        .max_by_key(|f| (can_merge || !f.hls, f.size.unwrap_or(0)));
    let mut out: Vec<Choice> = heights
        .into_iter()
        .map(|h| {
            // The largest format of that height stands for its size.
            let best = v
                .formats
                .iter()
                .filter(|f| f.video && f.height == Some(h) && (can_merge || f.audio))
                .max_by_key(|f| f.size.unwrap_or(0));
            let fps = best
                .and_then(|f| f.fps)
                .filter(|fps| *fps > 31.0)
                .map(|fps| format!(" {}fps", fps.round()))
                .unwrap_or_default();
            let (selector, ext, size) = if can_merge {
                let audio = best.filter(|f| !f.audio).and(best_audio);
                (
                    format!("bv*[height={h}]+ba/b[height={h}]"),
                    "mp4".to_owned(),
                    best.and_then(|f| f.size)
                        .map(|s| s + audio.and_then(|a| a.size).unwrap_or(0)),
                )
            } else {
                (
                    format!("b[height={h}]"),
                    best.map(|f| f.ext.clone()).unwrap_or_else(|| "mp4".into()),
                    best.and_then(|f| f.size),
                )
            };
            Choice {
                label: format!("{}p{fps} {}", h, ext.to_uppercase()),
                selector,
                ext,
                size,
            }
        })
        .collect();
    // Files whose resolution isn't known (generic sites, a stream's variants with a bitrate
    // only): offered by type, and by bitrate when there are several, best first. Audio
    // containers, and streams with no video codec named, aren't videos.
    let mut plain: Vec<&Format> = v
        .formats
        .iter()
        .filter(|f| f.video && f.audio && f.height.is_none() && !audio_container(&f.ext))
        .collect();
    plain.sort_by(|a, b| b.tbr.unwrap_or(0.0).total_cmp(&a.tbr.unwrap_or(0.0)));
    let several = plain.len() > 1;
    let mut labels = std::collections::HashSet::new();
    for f in plain {
        let label = match f.tbr.filter(|_| several) {
            Some(kbps) => format!("{} {} kbps", f.ext.to_uppercase(), kbps.round()),
            None => f.ext.to_uppercase(),
        };
        // Twins (the same type and rate under two IDs) once.
        if !labels.insert(label.clone()) {
            continue;
        }
        out.push(Choice {
            label,
            selector: f.id.clone(),
            ext: f.ext.clone(),
            size: f.size,
        });
    }
    if let Some(a) = best_audio {
        out.push(Choice {
            // The app shows it in its language (videodlg::choice_label): the site's own file,
            // not converted (#70).
            label: format!("{AUDIO_ORIGINAL} {}", a.ext.to_uppercase()),
            selector: "ba".into(),
            ext: a.ext.clone(),
            size: a.size,
        });
    }
    // MP3, converted by ffmpeg from the best sound (#32): its best variable bitrate, then each
    // constant one. Sizes from the duration (a variable one's is about 245 kbit/s).
    let has_audio = v.formats.iter().any(|f| f.audio);
    if can_merge && has_audio {
        let size = |kbps: u32| v.duration.map(|s| (s * f64::from(kbps) * 125.0) as u64);
        out.push(Choice {
            label: format!("{AUDIO_ONLY} MP3 VBR"),
            selector: format!("{MP3}0"),
            ext: "mp3".into(),
            size: size(245),
        });
        for kbps in MP3_RATES {
            out.push(Choice {
                label: format!("{AUDIO_ONLY} MP3 {kbps} kbps"),
                selector: format!("{MP3}{kbps}"),
                ext: "mp3".into(),
                size: size(kbps),
            });
        }
        // Opus (#70): about the size of the best sound when that is Opus already (it is only
        // moved into an .opus file), else of a 160 kbit/s encoding.
        let opus_size = best_audio
            .filter(|a| a.ext.eq_ignore_ascii_case("webm") || a.ext.eq_ignore_ascii_case("opus"))
            .and_then(|a| a.size)
            .or_else(|| size(160));
        out.push(Choice {
            label: format!("{AUDIO_ONLY} OPUS"),
            selector: OPUS.into(),
            ext: "opus".into(),
            size: opus_size,
        });
        // FLAC (#70): lossless from a lossy source, about 900 kbit/s for 48 kHz stereo.
        out.push(Choice {
            label: format!("{AUDIO_ONLY} FLAC"),
            selector: FLAC.into(),
            ext: "flac".into(),
            size: size(900),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_title_that_is_only_the_address() {
        let hash = "SFp1NUcrZFRkRUVrUWlqM3paMitkakJIUW9JSFhOeEZkd0ZMNEc5UHpnZm5hRkY4Vis5MjlJU00xdDVqTk1vaw";
        let page = format!("https://cdn.example/v/{hash}.mp4?sig=1");
        assert!(named_after_address(hash, &page));
        assert!(!named_after_address("A real title", &page));
        assert!(!named_after_address("watch", "not an address"));
        assert!(named_after_address(
            "my clip",
            "https://x.example/my%20clip.mp4"
        ));
    }

    use super::*;

    #[test]
    fn a_streams_variants_by_bitrate() {
        let fmt = |id: &str, tbr: f64| Format {
            id: id.into(),
            ext: "mp4".into(),
            height: None,
            fps: None,
            video: true,
            audio: true,
            size: None,
            tbr: Some(tbr),
            hls: false,
        };
        let v = Video {
            title: "t".into(),
            duration: None,
            formats: vec![
                fmt("hls-640", 640.0),
                fmt("hls-1280", 1280.0),
                fmt("hls-1280b", 1280.0),
            ],
            subtitles: Vec::new(),
            automatic: Vec::new(),
            language: None,
            direct: false,
        };
        let labels: Vec<String> = choices(&v, true)
            .into_iter()
            .map(|c| c.label)
            .filter(|l| !l.starts_with("Audio only"))
            .collect();
        assert_eq!(
            labels,
            ["MP4 1280 kbps", "MP4 640 kbps"],
            "best first, twins once"
        );
    }

    const SAMPLE: &str = r#"{
        "title": "A talk", "duration": 61.5,
        "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 1000000},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 5000000},
            {"format_id": "136", "ext": "mp4", "height": 720, "fps": 30, "vcodec": "avc1", "acodec": "none", "filesize": 20000000},
            {"format_id": "299", "ext": "mp4", "height": 1080, "fps": 60, "vcodec": "avc1", "acodec": "none", "filesize_approx": 60000000}
        ]}"#;

    /// An HLS master playlist as yt-dlp reads it (checked with yt-dlp 2026.10.26): each variant's
    /// bitrate and the duration, no size; sized as bitrate × duration. A live one stays unsized.
    #[test]
    fn hls_variants_are_sized_from_their_bitrate() {
        const HLS: &str = r#"{"title": "master", "duration": 600, "formats": [
            {"format_id": "800", "ext": "mp4", "height": 360, "tbr": 800.0, "protocol": "m3u8_native"},
            {"format_id": "2500", "ext": "mp4", "height": 720, "tbr": 2500.0, "protocol": "m3u8_native"}]}"#;
        let v = parse(HLS.as_bytes()).unwrap();
        let sizes: Vec<_> = choices(&v, true)
            .into_iter()
            .filter(|c| !c.label.starts_with("Audio only"))
            .map(|c| (c.label, c.size))
            .collect();
        assert_eq!(
            sizes,
            [
                ("720p MP4".to_owned(), Some(187_500_000)),
                ("360p MP4".to_owned(), Some(60_000_000))
            ]
        );
        let live = HLS.replace(r#""duration": 600, "#, "");
        let v = parse(live.as_bytes()).unwrap();
        assert!(v.formats.iter().all(|f| f.size.is_none()), "live: no size");
        // A size yt-dlp gives wins over the estimate.
        let given = HLS.replace(r#""tbr": 800.0"#, r#""tbr": 800.0, "filesize": 123"#);
        let v = parse(given.as_bytes()).unwrap();
        assert_eq!(v.formats[0].size, Some(123));
        // A plain file's size is its server's to say (#320), never estimated.
        let direct = HLS.replace(
            r#""duration": 600, "#,
            r#""duration": 600, "direct": true, "#,
        );
        assert!(parse(direct.as_bytes()).unwrap().needs_size());
        // Wikimedia Commons' extractor says `"direct": null`: a page like any other, not "no
        // video information".
        let commons = HLS.replacen(
            r#""duration": 600, "#,
            r#""duration": 600, "direct": null, "#,
            1,
        );
        let v = parse(commons.as_bytes()).expect("a direct of null");
        assert!(!v.direct);
    }

    #[test]
    fn choices_with_and_without_merging() {
        let v = parse(SAMPLE.as_bytes()).unwrap();
        assert_eq!(v.title, "A talk");
        let merged = choices(&v, true);
        let labels: Vec<&str> = merged.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "1080p 60fps MP4",
                "720p MP4",
                "360p MP4",
                "Audio only original M4A",
                "Audio only MP3 VBR",
                "Audio only MP3 320 kbps",
                "Audio only MP3 256 kbps",
                "Audio only MP3 192 kbps",
                "Audio only MP3 128 kbps",
                "Audio only OPUS",
                "Audio only FLAC",
            ]
        );
        // Opus from M4A is encoded (160 kbit/s); FLAC about 900 kbit/s (61.5 s).
        let opus = &merged[merged.len() - 2];
        assert_eq!((opus.selector.as_str(), opus.size), (OPUS, Some(1_230_000)));
        let flac = &merged[merged.len() - 1];
        assert_eq!((flac.selector.as_str(), flac.size), (FLAC, Some(6_918_750)));
        assert_eq!(merged[0].size, Some(61_000_000), "video + best audio");
        // MP3 (#32): converted from the best sound, sized from the duration (61.5 s).
        let mp3 = &merged[5];
        assert_eq!(
            (mp3.selector.as_str(), mp3.ext.as_str()),
            ("mp3:320", "mp3")
        );
        assert_eq!(mp3.size, Some(2_460_000));
        // Without ffmpeg only the format that has both survives, and no MP3 (it converts).
        let plain = choices(&v, false);
        let labels: Vec<&str> = plain.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["360p MP4", "Audio only original M4A"]);
        assert_eq!(plain[0].selector, "b[height=360]");
    }

    /// Live (#352): `MDM_LIVE_YTDLP=<yt-dlp> MDM_LIVE_PAGE=<video page> cargo test -p mdm-tools
    /// live_look_up -- --ignored --nocapture`. A look-up given cookies; with a yt-dlp that fails
    /// the way a signed-in session does (a wrapper forcing `web_embedded,tv_samsung,web` when it
    /// sees `--cookies`), the second look-up, without them, lists the page's videos.
    #[test]
    #[ignore]
    fn live_look_up_with_cookies() {
        let exe =
            std::path::PathBuf::from(std::env::var("MDM_LIVE_YTDLP").expect("MDM_LIVE_YTDLP"));
        let page = std::env::var("MDM_LIVE_PAGE").expect("MDM_LIVE_PAGE");
        let browser = proc::FromBrowser {
            cookies: Some("YSC=test".into()),
            ..Default::default()
        };
        let (mut args, _file) = browser.args(&page).unwrap();
        args.insert(0, "-J".to_owned());
        args.push("--no-playlist".to_owned());
        let out = proc::look_up(&exe, &args, &page, Duration::from_secs(120)).unwrap();
        let v = parse(&out.json).unwrap();
        let heights: std::collections::BTreeSet<u32> = v
            .formats
            .iter()
            .filter(|f| f.video)
            .filter_map(|f| f.height)
            .collect();
        println!(
            "without_cookies: {}, {} formats, video heights {heights:?}",
            out.without_cookies,
            v.formats.len()
        );
        assert!(!heights.is_empty(), "no video in {:?}", v.title);
    }

    /// #320: yt-dlp's answer for a plain file (real output, 2026.10.26) says `direct` and no size.
    #[test]
    fn a_direct_file_takes_the_size_the_server_said() {
        let json = r#"{"title": "clip", "direct": true, "ext": "mp4", "formats": [
            {"format_id": "mp4", "ext": "mp4", "vcodec": null, "acodec": null}]}"#;
        let mut v = parse(json.as_bytes()).unwrap();
        assert!(v.needs_size());
        assert_eq!(choices(&v, true)[0].size, None);
        v.set_size(430_019_588);
        assert!(!v.needs_size());
        assert_eq!(choices(&v, true)[0].size, Some(430_019_588));
        // A page's video: no such look (its sizes are the site's to say, or estimated).
        let page = r#"{"title": "t", "formats": [{"format_id": "18", "ext": "mp4"}]}"#;
        assert!(!parse(page.as_bytes()).unwrap().needs_size());
        // A file whose size yt-dlp said keeps it.
        let sized =
            r#"{"title": "t", "direct": true, "formats": [{"format_id": "mp4", "filesize": 9}]}"#;
        assert!(!parse(sized.as_bytes()).unwrap().needs_size());
    }

    /// #351: what yt-dlp lists on YouTube with no JavaScript runtime (real output, 2026.10.26):
    /// video-only streams and sound, nothing carrying both. Without ffmpeg only the sound is
    /// offered, and the window has to say that ffmpeg brings the qualities.
    #[test]
    fn only_the_sound_without_ffmpeg_and_a_muxed_format() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "sb0", "ext": "mhtml", "vcodec": "none", "acodec": "none"},
            {"format_id": "140-0", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "filesize": 900},
            {"format_id": "312", "ext": "mp4", "height": 1080, "vcodec": "avc1.64002A", "acodec": "none", "filesize": 9000},
            {"format_id": "230", "ext": "mp4", "height": 360, "vcodec": "avc1.4D401E", "acodec": "none", "filesize": 1000}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let without: Vec<String> = choices(&v, false).into_iter().map(|c| c.label).collect();
        assert_eq!(without, ["Audio only original M4A"]);
        assert!(needs_merging(&v));
        let with: Vec<String> = choices(&v, true).into_iter().map(|c| c.label).collect();
        assert_eq!(&with[..2], ["1080p MP4", "360p MP4"]);
        // A format carrying both, as good as any video: nothing more to get from merging.
        let muxed = r#"{"title": "T", "formats": [
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"},
            {"format_id": "230", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "none"}]}"#;
        assert!(!needs_merging(&parse(muxed.as_bytes()).unwrap()));
        // Sound alone: nothing to merge.
        let sound = r#"{"title": "T", "formats": [
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a"}]}"#;
        assert!(!needs_merging(&parse(sound.as_bytes()).unwrap()));
    }

    /// #569: YouTube on a Mac with neither ffmpeg nor a JavaScript runtime: its sound as HLS
    /// streams (the larger) and as a file. Without ffmpeg the file is the choice, and yt-dlp is
    /// asked for a file first; with ffmpeg the best, HLS or not (ffmpeg makes it a proper file).
    #[test]
    fn without_ffmpeg_a_sound_that_comes_as_a_file() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "233", "ext": "mp4", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "m3u8_native", "filesize_approx": 2000},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "https", "filesize": 900}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        assert!(v.formats[0].hls && !v.formats[1].hls);
        let without = choices(&v, false);
        assert_eq!(without[0].label, "Audio only original M4A");
        assert_eq!(without[0].size, Some(900));
        assert_eq!(choices(&v, true)[0].size, Some(2000));
        assert_eq!(without_remuxing("ba"), "ba[protocol!*=m3u8]/ba");
        assert_eq!(
            without_remuxing("b[height=360]"),
            "b[height=360][protocol!*=m3u8]/b[height=360]"
        );
        assert_eq!(
            without_remuxing("bv*[height=720]+ba/b[height=720]"),
            "bv*[height=720]+ba/b[height=720][protocol!*=m3u8]/b[height=720]"
        );
        // A format by its ID, and one already choosing its way, as they are.
        assert_eq!(without_remuxing("hls-1280"), "hls-1280");
        assert_eq!(without_remuxing("18"), "18");
        assert_eq!(without_remuxing("ba[protocol=https]"), "ba[protocol=https]");
        // Only HLS sound: still offered (it is saved for what it is, video.rs).
        let hls_only = r#"{"title": "T", "formats": [
            {"format_id": "233", "ext": "mp4", "vcodec": "none", "acodec": "mp4a.40.2", "protocol": "m3u8_native"}]}"#;
        let only = choices(&parse(hls_only.as_bytes()).unwrap(), false);
        assert_eq!(only[0].selector, "ba");
    }

    /// #70: what each audio choice converts to; the original and videos convert nothing.
    #[test]
    fn audio_conversions() {
        let c = |s: &str| audio_conversion(s).map(|a| (a.format, a.quality));
        assert_eq!(c("mp3:320"), Some(("mp3", Some("320K".into()))));
        assert_eq!(c(OPUS), Some(("opus", None)));
        assert_eq!(c(FLAC), Some(("flac", None)));
        assert_eq!(c("ba"), None);
        assert_eq!(c("bv*[height=720]+ba/b[height=720]"), None);
        assert_eq!(c("opus:x"), None);
        let flac = Choice {
            label: "x".into(),
            selector: FLAC.into(),
            ext: "flac".into(),
            size: None,
        };
        assert!(is_audio(&flac));
    }

    #[test]
    fn mp3_selectors_are_the_offered_ones_only() {
        assert_eq!(mp3_quality("mp3:0").as_deref(), Some("0"));
        assert_eq!(mp3_quality("mp3:192").as_deref(), Some("192K"));
        for other in [
            "mp3:64",
            "mp3:",
            "mp3:-1",
            "mp3:320; rm",
            "mp3:+192",
            "mp3:0320",
            "mp3:000",
            "mp3: 192",
            "MP3:192",
            "ba",
            "b[height=360]",
        ] {
            assert_eq!(mp3_quality(other), None, "{other}");
        }
    }

    #[test]
    fn plain_files_without_codec_or_resolution() {
        let json = r#"{"title": "Bunny", "formats": [
            {"format_id": "ogv", "ext": "ogv", "filesize": 100},
            {"format_id": "mp4", "ext": "mp4", "filesize": 200}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let c = choices(&v, false);
        let labels: Vec<&str> = c.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["OGV", "MP4"]);
        assert_eq!(c[1].selector, "mp4");
    }

    #[test]
    fn audio_without_codec_info_is_not_a_video_choice() {
        let json = r#"{"title": "T", "formats": [
            {"format_id": "773", "ext": "m4a", "filesize": 100},
            {"format_id": "140", "ext": "m4a", "vcodec": "none", "acodec": "mp4a", "filesize": 90},
            {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a", "filesize": 500}]}"#;
        let v = parse(json.as_bytes()).unwrap();
        let labels: Vec<String> = choices(&v, false).into_iter().map(|c| c.label).collect();
        assert_eq!(labels, ["360p MP4", "Audio only original M4A"]);
    }

    /// A page's subtitles (#68), as yt-dlp -J lists them: the uploader's by name, the chat
    /// replay left out; of the automatic ones only the spoken language's and the translations
    /// into the interface language and English, and none the uploader already has.
    #[test]
    fn subtitle_languages_worth_offering() {
        let json = r#"{"title": "T", "language": "en", "formats": [],
            "subtitles": {
                "live_chat": [{"ext": "json"}],
                "fr": [{"ext": "vtt", "name": "French"}, {"ext": "srt", "name": "French"}],
                "de": [{"ext": "vtt"}],
                "empty": []
            },
            "automatic_captions": {
                "en-orig": [{"ext": "vtt", "name": "English (Original)"}],
                "en": [{"ext": "vtt", "name": "English"}],
                "fr": [{"ext": "vtt", "name": "French"}],
                "ar": [{"ext": "vtt", "name": "Arabic"}],
                "ja": [{"ext": "vtt", "name": "Japanese"}],
                "zu": [{"ext": "vtt", "name": "Zulu"}]
            }}"#;
        let v = parse(json.as_bytes()).unwrap();
        let names: Vec<&str> = v.subtitles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["de", "French"], "by name; no name: the code");
        assert_eq!(v.language.as_deref(), Some("en"));
        let offered: Vec<(String, bool)> = sub_choices(&v, "ar")
            .into_iter()
            .map(|c| (c.code, c.automatic))
            .collect();
        let want: Vec<(String, bool)> = [
            ("de", false),
            ("fr", false),
            ("ar", true),
            ("en", true),
            ("en-orig", true),
        ]
        .iter()
        .map(|(c, a)| (c.to_string(), *a))
        .collect();
        assert_eq!(offered, want);
        // A page without any: nothing to offer.
        let none = parse(br#"{"title": "T", "formats": []}"#).unwrap();
        assert!(sub_choices(&none, "en").is_empty());
    }

    #[test]
    fn junk_is_not_a_video() {
        assert!(parse(b"not json").is_none());
    }
}
