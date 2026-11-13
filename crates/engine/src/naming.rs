//! Choosing the local file name for a download.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use percent_encoding::percent_decode_str;
use url::Url;

/// File name from `Content-Disposition`, else the address's ([`address_name`]), else
/// `download`. Always safe to use as a single path component on Windows and Unix.
pub fn file_name(content_disposition: Option<&str>, url: &Url) -> String {
    answer_name(
        url,
        &Answer {
            disposition: content_disposition,
            ..Answer::default()
        },
    )
}

/// What is known of a file besides its address, for its name: the server's answer
/// (`Content-Disposition`, `Content-Type`), the type its first bytes show (`sniffed`, an
/// extension from [`sniff`]), and the title of the page it was played on, when the browser sent
/// one with a page's media (#695, #800).
#[derive(Clone, Copy, Debug, Default)]
pub struct Answer<'a> {
    pub disposition: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub sniffed: Option<&'a str>,
    pub title: Option<&'a str>,
}

/// The file's name from all that is known of it (#695, #800), safe as one path component:
///
/// 1. the server's `Content-Disposition` name, as it gives it: it wins over everything;
/// 2. else the address's name ([`address_name`]: a script's `file=` value, else its last part);
/// 3. a name whose extension doesn't say its type (a script's, `stream.php`; a page's; none, a
///    bare id) takes the type's: the server's `Content-Type`, else its bytes' (`sniffed`). Only
///    the media, picture, document and archive types of [`TYPES`], never one that runs;
/// 4. a name that means nothing ([`means_nothing`]: a script's, a bare id, `videoplayback`, a
///    long hash) gives way to the page's `title` when there is one, with that extension.
///
/// A real file name (`clip.mp4`, `setup.zip`, a script's `file=` value) is never changed by the
/// type or the title.
pub fn answer_name(url: &Url, a: &Answer) -> String {
    if let Some(name) = a.disposition.and_then(disposition_name) {
        // A header may carry a path; keep only its last component.
        return sanitize(last_component(&name));
    }
    let last = last_segment(url);
    let given = query_file_name(url).filter(|_| names_nothing(&last));
    let named = given.is_some();
    let name = given.unwrap_or(last);
    let typed = if untyped(&name) {
        a.content_type
            .and_then(type_extension)
            .or_else(|| a.sniffed.filter(|e| is_typed_extension(e)))
    } else {
        None
    };
    let title = a
        .title
        .map(clean_title)
        .filter(|t| !t.is_empty() && !named && means_nothing(&name));
    let (stem, ext) = split_extension(&name);
    let stem = match title.as_deref() {
        Some(t) => t,
        None if stem.trim().is_empty() => "download",
        None => stem,
    };
    let name = match typed.or(ext) {
        Some(e) => format!("{stem}.{e}"),
        None => stem.to_owned(),
    };
    sanitize(&name)
}

/// The address's last path part, percent-decoded (empty for a folder's address).
fn last_segment(url: &Url) -> String {
    url.path_segments()
        .and_then(|mut s| s.next_back())
        .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
        .unwrap_or_default()
}

/// A path's last component (`a/b\c.zip` → `c.zip`).
fn last_component(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or_default()
}

/// The address's own name for its file, not sanitized: a script's `file=` or `filename=` value
/// when it names a file of a known type (`stream.php?file=FL262-1E01-4.mp4` →
/// `FL262-1E01-4.mp4`, #695), else its last path part.
pub fn address_name(url: &Url) -> String {
    let last = last_segment(url);
    query_file_name(url)
        .filter(|_| names_nothing(&last))
        .unwrap_or(last)
}

/// The address's `file=` / `filename=` value (its last component) when its extension is a known
/// type's ([`TYPES`]): a value naming a program, or anything else, is never taken.
fn query_file_name(url: &Url) -> Option<String> {
    url.query_pairs()
        .filter(|(k, _)| k.eq_ignore_ascii_case("file") || k.eq_ignore_ascii_case("filename"))
        .map(|(_, v)| last_component(v.trim()).trim().to_owned())
        .find(|v| match split_extension(v) {
            (stem, Some(ext)) => {
                !stem.trim().is_empty() && is_typed_extension(&ext.to_ascii_lowercase())
            }
            _ => false,
        })
}

/// `name`'s stem and extension (`clip.mp4` → `clip`, `mp4`). `.profile`, `clip` and `Mr. Smith`
/// have none.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once('.') {
        Some((stem, ext))
            if !stem.is_empty() && !ext.is_empty() && !ext.contains(char::is_whitespace) =>
        {
            (stem, Some(ext))
        }
        _ => (name, None),
    }
}

/// The extensions of programs that answer on a web server: an address ending in one names the
/// program, not the file it sends (`stream.php`, `remote_control.php`, `download.aspx`).
pub const SCRIPT_EXTENSIONS: &[&str] = &[
    "php", "php3", "php4", "php5", "phtml", "asp", "aspx", "ashx", "axd", "jsp", "jspx", "do",
    "action", "cgi", "pl", "cfm", "py", "rb",
];

/// A web page's extensions: a file of another type served at such an address takes its type's.
pub const PAGE_EXTENSIONS: &[&str] = &["htm", "html", "xhtml", "shtml", "dhtml"];

/// Whether `name`'s extension is a web server's program's ([`SCRIPT_EXTENSIONS`]).
pub fn is_script_name(name: &str) -> bool {
    split_extension(name)
        .1
        .is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A name whose extension doesn't say what the file is: a script's, a page's, or none.
fn untyped(name: &str) -> bool {
    match split_extension(name).1 {
        None => true,
        Some(e) => {
            let e = e.to_ascii_lowercase();
            SCRIPT_EXTENSIONS.contains(&e.as_str()) || PAGE_EXTENSIONS.contains(&e.as_str())
        }
    }
}

/// An address's last part that names no file, so its `file=` value does: a script's, or one
/// without an extension.
fn names_nothing(last: &str) -> bool {
    split_extension(last).1.is_none() || is_script_name(last)
}

/// A name that says nothing of what the file is, for a page's title to replace (#345, #800): a
/// script's (`remote_control.php`), none, one without an extension (a bare id, `videoplayback`),
/// or a long hash (`a3f9…e1.mp4`). The browser extension's `meansNothing` is the same test.
pub fn means_nothing(name: &str) -> bool {
    let (stem, ext) = split_extension(name);
    let hash = stem.chars().count() >= 24
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '=' | '_' | '-'))
        && stem.chars().filter(|c| matches!(c, '-' | '_')).count() <= 2;
    ext.is_none() || is_script_name(name) || hash
}

/// A page's title as a file's stem: what a file name can't hold becomes a space, spaces are
/// collapsed, at most 150 characters, no trailing dot (the browser extension's `cleanTitle`).
pub fn clean_title(title: &str) -> String {
    let spaced: String = title
        .chars()
        .filter(|c| !is_format_char(*c))
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let words = spaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = words.chars().take(150).collect();
    cut.trim_end_matches(['.', ' ']).to_owned()
}

/// The types a file's extension is taken from, with that extension (the browser extension's
/// `TYPE_EXTENSIONS` holds the same, which `tools/extension/test.mjs` checks). Media, pictures,
/// documents and archives only: a type that runs (a program, a script, a page) never names a
/// file's extension.
pub const TYPES: &[(&str, &str)] = &[
    ("video/mp4", "mp4"),
    ("video/x-m4v", "m4v"),
    ("video/webm", "webm"),
    ("video/x-matroska", "mkv"),
    ("video/quicktime", "mov"),
    ("video/x-msvideo", "avi"),
    ("video/avi", "avi"),
    ("video/x-ms-wmv", "wmv"),
    ("video/x-flv", "flv"),
    ("video/mpeg", "mpg"),
    ("video/mp2t", "ts"),
    ("video/3gpp", "3gp"),
    ("video/3gpp2", "3g2"),
    ("video/ogg", "ogv"),
    ("audio/mpeg", "mp3"),
    ("audio/mp3", "mp3"),
    ("audio/mp4", "m4a"),
    ("audio/x-m4a", "m4a"),
    ("audio/aac", "aac"),
    ("audio/aacp", "aac"),
    ("audio/webm", "weba"),
    ("audio/ogg", "ogg"),
    ("audio/opus", "opus"),
    ("audio/flac", "flac"),
    ("audio/x-flac", "flac"),
    ("audio/wav", "wav"),
    ("audio/x-wav", "wav"),
    ("audio/wave", "wav"),
    ("audio/x-ms-wma", "wma"),
    ("application/ogg", "ogg"),
    ("image/jpeg", "jpg"),
    ("image/png", "png"),
    ("image/gif", "gif"),
    ("image/webp", "webp"),
    ("image/avif", "avif"),
    ("application/pdf", "pdf"),
    ("application/epub+zip", "epub"),
    ("application/zip", "zip"),
    ("application/x-zip-compressed", "zip"),
    ("application/x-7z-compressed", "7z"),
    ("application/vnd.rar", "rar"),
    ("application/x-rar-compressed", "rar"),
    ("application/gzip", "gz"),
    ("application/x-gzip", "gz"),
];

/// The extension a `Content-Type` value names (`video/mp4; codecs=…` → `mp4`), if [`TYPES`] has
/// it.
pub fn type_extension(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next()?.trim();
    TYPES
        .iter()
        .find(|(t, _)| t.eq_ignore_ascii_case(mime))
        .map(|(_, e)| *e)
}

/// Whether `ext` is one of [`TYPES`]' extensions (every one [`sniff`] gives is), in any case.
pub fn is_typed_extension(ext: &str) -> bool {
    TYPES.iter().any(|(_, e)| e.eq_ignore_ascii_case(ext))
}

/// How many first bytes [`sniff`] reads.
pub const SNIFF_BYTES: usize = 64;

/// The media type a file's first bytes show, as its extension: the MP4 family's (`ftyp`),
/// Matroska or WebM (EBML), Ogg, FLAC, MP3 (an ID3 tag or a Layer III frame), AAC (ADTS), WAVE,
/// AVI, FLV. None for anything else: a file is never named a program by its bytes.
pub fn sniff(head: &[u8]) -> Option<&'static str> {
    let at = |i: usize, sig: &[u8]| head.get(i..i + sig.len()) == Some(sig);
    if at(4, b"ftyp") {
        return Some(match head.get(8..12).unwrap_or_default() {
            b"M4A " | b"M4B " => "m4a",
            b"qt  " => "mov",
            b if b.starts_with(b"3g") => "3gp",
            _ => "mp4",
        });
    }
    if at(0, &[0x1A, 0x45, 0xDF, 0xA3]) {
        let webm = head.windows(4).any(|w| w == b"webm");
        return Some(if webm { "webm" } else { "mkv" });
    }
    if at(0, b"OggS") {
        return Some("ogg");
    }
    if at(0, b"fLaC") {
        return Some("flac");
    }
    if at(0, b"ID3") {
        return Some("mp3");
    }
    if at(0, b"RIFF") && at(8, b"WAVE") {
        return Some("wav");
    }
    if at(0, b"RIFF") && at(8, b"AVI ") {
        return Some("avi");
    }
    if at(0, b"FLV\x01") {
        return Some("flv");
    }
    if let [0xFF, b, c, ..] = *head {
        // ADTS: a 12-bit sync and layer 00.
        if b & 0xF6 == 0xF0 {
            return Some("aac");
        }
        // An MPEG audio frame: an 11-bit sync, a version that isn't reserved, Layer III, and a
        // bitrate and sample rate that aren't either (a UTF-16 text's FF FE isn't one).
        let (version, layer) = ((b >> 3) & 3, (b >> 1) & 3);
        if b & 0xE0 == 0xE0 && version != 1 && layer == 1 && c >> 4 != 0xF && (c >> 2) & 3 != 3 {
            return Some("mp3");
        }
    }
    None
}

/// Whether the file's bytes must be looked at for its name ([`sniff`]): it is named by its
/// address, whose name doesn't say its type, and the server's `Content-Type` doesn't either.
pub fn needs_sniff(url: &Url, disposition: Option<&str>, content_type: Option<&str>) -> bool {
    disposition.and_then(disposition_name).is_none()
        && untyped(&address_name(url))
        && content_type.and_then(type_extension).is_none()
}

/// The file name a `Content-Disposition` value suggests (RFC 6266), read as browsers do:
/// `filename*` (RFC 8187: `charset'language'percent-encoded`, UTF-8 or ISO-8859-1) wins over
/// `filename`, which is only the fallback for clients that can't decode it. A plain
/// `filename` is unquoted (with `\` escapes), and percent-encoded UTF-8 in it is decoded, as
/// Chromium does. Not sanitized.
pub fn disposition_name(value: &str) -> Option<String> {
    let params = disposition_params(value);
    let param = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    };
    // An empty `filename*` doesn't hide the plain name.
    let given = |n: &String| !n.trim().is_empty();
    let extended = param("filename*").and_then(decode_extended).filter(given);
    let plain = || param("filename").map(decode_plain).filter(given);
    extended.or_else(plain)
}

/// The `name=value` parameters after the disposition type, values unquoted. A quoted value may
/// hold `;` and `\"`. Parameters without a type before them (`filename=x`, seen in the wild)
/// count too.
fn disposition_params(value: &str) -> Vec<(String, String)> {
    let rest = match value.split_once(';') {
        Some((kind, _)) if kind.contains('=') => value,
        Some((_, rest)) => rest,
        None if value.contains('=') => value,
        None => "",
    };
    let mut out = Vec::new();
    let mut it = rest.chars().peekable();
    while it.peek().is_some() {
        let mut name = String::new();
        for c in it.by_ref() {
            match c {
                '=' => break,
                ';' => name.clear(), // a parameter without a value
                c => name.push(c),
            }
        }
        while it.next_if(|c| c.is_whitespace()).is_some() {}
        let mut value = String::new();
        if it.next_if_eq(&'"').is_some() {
            while let Some(c) = it.next() {
                match c {
                    '\\' => value.extend(it.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
            // Anything after the closing quote, up to the next parameter, is not part of it.
            for c in it.by_ref() {
                if c == ';' {
                    break;
                }
            }
        } else {
            for c in it.by_ref() {
                if c == ';' {
                    break;
                }
                value.push(c);
            }
            value.truncate(value.trim_end().len());
        }
        let name = name.trim();
        if !name.is_empty() {
            out.push((name.to_owned(), value));
        }
    }
    out
}

/// `UTF-8''na%C3%AFve.txt` → `naïve.txt` (RFC 8187). Charsets other than UTF-8 and
/// ISO-8859-1 (and its subset US-ASCII) are refused, like bytes that aren't valid UTF-8: the
/// plain `filename` is used instead.
fn decode_extended(v: &str) -> Option<String> {
    let mut parts = v.splitn(3, '\'');
    let (charset, _language, encoded) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes: Vec<u8> = percent_decode_str(encoded.trim()).collect();
    match charset.trim().to_ascii_lowercase().as_str() {
        "utf-8" | "utf8" => String::from_utf8(bytes).ok(),
        "iso-8859-1" | "iso_8859-1" | "latin1" | "us-ascii" => {
            Some(bytes.into_iter().map(char::from).collect())
        }
        _ => None,
    }
}

/// A plain `filename` as given, or its percent-encoded UTF-8 decoded when that is what it
/// holds (non-ASCII once decoded: `100%25.txt` stays as it is).
fn decode_plain(v: &str) -> String {
    if v.contains('%')
        && let Ok(decoded) = percent_decode_str(v).decode_utf8()
        && !decoded.is_ascii()
    {
        return decoded.into_owned();
    }
    v.to_owned()
}

/// Make `name` a valid single file name on Windows (the strictest target). Invisible format
/// characters are dropped: a right-to-left override shows `invoice\u{202E}fdp.exe` as
/// "invoiceexe.pdf".
pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !is_format_char(*c))
        .map(|c| {
            if c.is_control() || r#"<>:"/\|?*"#.contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = clamp(cleaned.trim().trim_end_matches(['.', ' ']));
    if cleaned.is_empty() {
        return "download".to_owned();
    }
    if is_windows_reserved(&cleaned) {
        format!("_{cleaned}")
    } else {
        cleaned
    }
}

/// Whether `name` (with or without an extension) names one of Windows' reserved DOS devices
/// (`CON`, `PRN`, `AUX`, `NUL`, `CONIN$`, `CONOUT$`, `COM0`–`COM9`, `LPT0`–`LPT9`, and the
/// superscripted `COM¹`–`COM³` / `LPT¹`–`LPT³`).
pub fn is_windows_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ((stem.starts_with("COM") || stem.starts_with("LPT"))
        && stem.chars().count() == 4
        && stem
            .chars()
            .nth(3)
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')))
}

/// Unicode's format characters (general category Cf): bidirectional marks, embeddings,
/// overrides and isolates, zero-width spaces and joiners, the byte order mark, the soft
/// hyphen, invisible operators, tags.
pub fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{AD}'
        | '\u{600}'..='\u{605}'
        | '\u{61C}'
        | '\u{6DD}'
        | '\u{70F}'
        | '\u{890}'..='\u{891}'
        | '\u{8E2}'
        | '\u{180E}'
        | '\u{200B}'..='\u{200F}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{206F}'
        | '\u{FEFF}'
        | '\u{FFF9}'..='\u{FFFB}'
        | '\u{110BD}'
        | '\u{110CD}'
        | '\u{13430}'..='\u{1343F}'
        | '\u{1BCA0}'..='\u{1BCA3}'
        | '\u{1D173}'..='\u{1D17A}'
        | '\u{E0001}'
        | '\u{E0020}'..='\u{E007F}'
    )
}

/// Longest file name we create, in UTF-16 units: NTFS allows 255, and the part file adds
/// ".mdm.part" (9) and a duplicate may add " (NNN)" (6). The extension is kept.
const MAX_NAME: usize = 230;

/// Longest full path we create, in UTF-16 units. On Windows: `MAX_PATH` (260) without the
/// terminating NUL, so Explorer and other programs can still open the file.
#[cfg(windows)]
const MAX_PATH_LEN: usize = 259;
#[cfg(not(windows))]
const MAX_PATH_LEN: usize = 4095;
/// What our files add to a destination: ".mdm.part" / ".mdm.json".
const SIDECAR: usize = 9;
/// What a duplicate's number adds: " (NNN)".
const DUPLICATE: usize = 6;
/// Shortest name worth making when a deep folder leaves little room.
const MIN_NAME: usize = 20;

fn utf16_len(s: &std::ffi::OsStr) -> usize {
    s.to_string_lossy().encode_utf16().count()
}

/// Whether `dest` and the part and state files beside it stay within the path limit.
pub fn path_fits(dest: &Path) -> bool {
    utf16_len(dest.as_os_str()) + SIDECAR <= MAX_PATH_LEN
}

/// `name`, shortened (keeping the extension) so that `dir/name`, its part and state files and a
/// duplicate's number stay within the path limit. None when `dir` alone leaves too little room.
pub fn fit_name(dir: &Path, name: &str) -> Option<String> {
    let room = MAX_PATH_LEN.checked_sub(utf16_len(dir.as_os_str()) + 1 + SIDECAR + DUPLICATE)?;
    (room >= MIN_NAME).then(|| clamp_to(name, room.min(MAX_NAME)))
}

/// Where a new download named `name` goes in `dir`: the name shortened to fit the path limit
/// (`fit_name`, keeping the extension), then numbered if taken (`unique_path`). A folder too
/// deep for any name keeps it whole, and the download says the path is too long. A name from a
/// long address (a token hundreds of characters long, #28) no longer fails the download.
pub fn destination(dir: &Path, name: &str) -> PathBuf {
    unique_path(dir, &fitted(dir, name))
}

/// `destination` before the folder is looked at: the name fitted to the path limit, not yet
/// numbered. What a window shows while the disk is asked (a sleeping drive or a network folder
/// can take seconds to answer, #743); a download started there is numbered on if the name is
/// taken by then ([`crate::Destination::Free`]). Never touches the disk.
pub fn guess(dir: &Path, name: &str) -> PathBuf {
    dir.join(fitted(dir, name))
}

/// `name` fitted to `dir` (`fit_name`), kept whole when `dir` leaves too little room.
fn fitted(dir: &Path, name: &str) -> String {
    fit_name(dir, name).unwrap_or_else(|| name.to_owned())
}

/// `destination` for a download whose helper picks the extension and takes any `<stem>.*` it
/// finds as its own (a video: yt-dlp reuses a file of the stem already there, and after an
/// MP3's conversion deletes it, #32). The stem is numbered on until no file in `dir` starts
/// with it and a dot, letter case aside, and `in_use` (the other downloads' stems in `dir`)
/// says no.
pub fn stem_destination(dir: &Path, name: &str, in_use: impl Fn(&str) -> bool) -> PathBuf {
    let name = fitted(dir, name);
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    let there: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    let free = |s: &str| {
        let prefix = format!("{}.", s.to_lowercase());
        let p = dir.join(format!("{s}{ext}"));
        !there
            .iter()
            .any(|n| n.starts_with(&prefix) || *n == s.to_lowercase())
            && !is_reserved(&p)
            && !in_use(s)
    };
    std::iter::once(stem.to_owned())
        .chain((1u32..).map(|n| format!("{stem} ({n})")))
        .find(|s| free(s))
        .map(|s| dir.join(format!("{s}{ext}")))
        .expect("unbounded search finds a free name")
}

/// [`stem_destination`] for many downloads into one folder (a playlist's videos, #69): the
/// folder is read once, not once a name, and each stem handed out counts as taken for the next.
/// The same rule: no file there starts with the stem and a dot (or is it), letter case aside,
/// and no other download there has it.
pub struct Stems {
    dir: PathBuf,
    /// Lowercase: every file name there, each of its prefixes before a dot, and the other
    /// downloads' stems.
    taken: HashSet<String>,
}

impl Stems {
    /// `in_use`: the stems of the other downloads into `dir`.
    pub fn new(dir: &Path, in_use: impl IntoIterator<Item = String>) -> Stems {
        let mut taken = HashSet::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().to_lowercase();
                for (i, _) in name.match_indices('.') {
                    taken.insert(name[..i].to_owned());
                }
                taken.insert(name);
            }
        }
        taken.extend(in_use.into_iter().map(|s| s.to_lowercase()));
        Stems {
            dir: dir.to_owned(),
            taken,
        }
    }

    /// Where `name` goes: its stem, else numbered on (` (1)`, ` (2)`…).
    pub fn pick(&mut self, name: &str) -> PathBuf {
        let name = fit_name(&self.dir, name).unwrap_or_else(|| name.to_owned());
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (&name[..i], &name[i..]),
            _ => (name.as_str(), ""),
        };
        let s = std::iter::once(stem.to_owned())
            .chain((1u32..).map(|n| format!("{stem} ({n})")))
            .find(|s| {
                !self.taken.contains(&s.to_lowercase())
                    && !is_reserved(&self.dir.join(format!("{s}{ext}")))
            })
            .expect("unbounded search finds a free name");
        self.taken.insert(s.to_lowercase());
        self.dir.join(format!("{s}{ext}"))
    }
}

fn clamp(name: &str) -> String {
    clamp_to(name, MAX_NAME)
}

fn clamp_to(name: &str, max: usize) -> String {
    let name = name.trim_end_matches(['.', ' ']);
    let len = |s: &str| s.encode_utf16().count();
    if len(name) <= max {
        return name.to_owned();
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i)
            if i > 0
                && len(&name[i..]) > 1
                && len(&name[i..]) <= 16
                && !name[i + 1..].contains(' ') =>
        {
            (&name[..i], &name[i..])
        }
        _ => (name, ""),
    };
    let mut out = String::new();
    for ch in stem.chars() {
        if len(&out) + ch.len_utf16() + len(ext) > max {
            break;
        }
        out.push(ch);
    }
    let trimmed = out.trim_end_matches(['.', ' ']);
    if trimmed.is_empty() {
        let mut plain = String::new();
        for ch in name.chars() {
            if len(&plain) + ch.len_utf16() > max {
                break;
            }
            plain.push(ch);
        }
        return plain.trim_end_matches(['.', ' ']).to_owned();
    }
    format!("{trimmed}{ext}")
        .trim_end_matches(['.', ' '])
        .to_owned()
}

/// The part file a download is written to before it is complete.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".mdm.part");
    PathBuf::from(s)
}

/// `dir/name`, or `dir/stem (N).ext` with the lowest N that is free — IDM's default for a
/// duplicate link is "add the duplicate with a numbered file name" (notes/ENGINE.md §6).
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || part_path(p).exists() || is_reserved(p);
    let first = dir.join(name);
    if !taken(&first) {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1u32..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("unbounded search finds a free name")
}

/// Destinations of downloads running in this process: one destination, one download.
fn active() -> &'static Mutex<HashSet<PathBuf>> {
    static ACTIVE: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}

fn is_reserved(p: &Path) -> bool {
    active().lock().unwrap().contains(p)
}

/// Holds a destination for one download; released when dropped.
#[derive(Debug)]
pub struct Reservation(PathBuf);

impl Drop for Reservation {
    fn drop(&mut self) {
        active().lock().unwrap().remove(&self.0);
    }
}

/// Reserve `dest` for one download. None when another download in this process has it.
pub fn reserve(dest: &Path) -> Option<Reservation> {
    active()
        .lock()
        .unwrap()
        .insert(dest.to_owned())
        .then(|| Reservation(dest.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #32: a video's stem is free for every extension, on disk (any letter case) and among
    /// the other downloads.
    #[test]
    fn a_stem_is_free_for_every_extension() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let none = |_: &str| false;
        assert_eq!(stem_destination(d, "Song.mp3", none), d.join("Song.mp3"));
        std::fs::write(d.join("song.M4A"), b"the user's").unwrap();
        assert_eq!(
            stem_destination(d, "Song.mp3", none),
            d.join("Song (1).mp3")
        );
        std::fs::write(d.join("Song (1).webm.part"), b"").unwrap();
        assert_eq!(
            stem_destination(d, "Song.mp3", none),
            d.join("Song (2).mp3")
        );
        // Another download in the list already has "Song (2)".
        assert_eq!(
            stem_destination(d, "Song.mp3", |s| s == "Song (2)"),
            d.join("Song (3).mp3")
        );
        // "Song extra.mp3" or "Songs.mp3" don't take "Song".
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("Song extra.mp3"), b"").unwrap();
        std::fs::write(other.path().join("Songs.m4a"), b"").unwrap();
        assert_eq!(
            stem_destination(other.path(), "Song.mp3", none),
            other.path().join("Song.mp3")
        );
    }

    /// #69: a playlist's names, the folder read once: the same rule as `stem_destination`, each
    /// name handed out taken for the next, and thousands quickly.
    #[test]
    fn many_stems_into_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("song.M4A"), b"the user's").unwrap();
        std::fs::write(d.join("Song (1).webm.part"), b"").unwrap();
        std::fs::write(d.join("Songs.m4a"), b"").unwrap();
        let mut stems = Stems::new(d, ["Song (2)".to_owned()]);
        assert_eq!(stems.pick("Song.mp4"), d.join("Song (3).mp4"));
        assert_eq!(stems.pick("Song.mp4"), d.join("Song (4).mp4"));
        assert_eq!(stems.pick("Songs.mp3"), d.join("Songs (1).mp3"));
        assert_eq!(stems.pick("Song extra.mp4"), d.join("Song extra.mp4"));
        // Many names, each twice: each looked up, not compared with every other one.
        fn pick_all((mut stems, n): (Stems, usize)) -> Stems {
            for i in 0..n {
                stems.pick(&format!("Video {}.mp4", i % (n / 2)));
            }
            stems
        }
        stems = pick_all((stems, 5000));
        assert_eq!(stems.pick("Video 7.mp4"), d.join("Video 7 (2).mp4"));
        // 8 times the names cost about 8 times as long (not 64), on any machine.
        mdm_labserver::growth::assert_linear(1000, |n| (Stems::new(d, []), n), pick_all);
    }

    #[test]
    fn names_shrink_to_fit_a_deep_folder() {
        let deep = PathBuf::from(format!(r"C:\{}", "d".repeat(200)));
        let name = fit_name(&deep, &format!("{}.zip", "n".repeat(100))).unwrap();
        assert!(name.ends_with(".zip"), "{name}");
        let dest = deep.join(&name);
        assert!(path_fits(&dest));
        // Room for the duplicate's number too.
        assert!(utf16_len(dest.as_os_str()) + SIDECAR + DUPLICATE <= MAX_PATH_LEN);
        assert_eq!(
            fit_name(Path::new(r"C:\dl"), "a.zip").as_deref(),
            Some("a.zip")
        );
    }

    /// #28: a file host's address ends in a token of hundreds of characters; the download's
    /// path still fits Windows' limit, extension kept.
    #[test]
    fn a_token_named_download_fits_its_folder() {
        let dir = PathBuf::from(r"C:\Users\someone\Downloads");
        let token = format!("ag1.{}", "eyJhcyI6MTMzMTY5LCJjIjoiQU41WUMybVpz".repeat(12));
        let dest = destination(&dir, &sanitize(&token));
        assert!(path_fits(&dest), "{}", dest.display());
        assert_eq!(dest.parent(), Some(dir.as_path()));
        let named = destination(&dir, &format!("{}.zip", "x".repeat(300)));
        assert!(path_fits(&named) && named.extension().is_some_and(|e| e == "zip"));
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_too_deep_for_any_name_is_refused() {
        let deeper = PathBuf::from(format!(r"C:\{}", "d".repeat(240)));
        assert_eq!(fit_name(&deeper, "a.zip"), None);
        assert!(!path_fits(&deeper.join("a-longer-file-name.zip")));
    }

    #[test]
    fn long_names_are_clamped_keeping_the_extension() {
        let long = format!("{}.mp4", "a".repeat(400));
        let s = sanitize(&long);
        assert!(s.encode_utf16().count() <= MAX_NAME);
        assert!(s.ends_with(".mp4"));
    }

    #[test]
    fn fit_name_never_leaves_trailing_space_or_dot() {
        let dir = Path::new("/tmp/downloads");
        for raw in [
            "Folder. ".to_owned(),
            format!("{}   ...", "Folder ".repeat(40)),
            format!("{}. Vol 2 ", "Episode ".repeat(35)),
            format!("{}.", "Archive".repeat(40)),
            format!("{} .mp4", "Video ".repeat(40)),
        ] {
            let fitted = fit_name(dir, &raw).unwrap();
            assert!(
                !fitted.ends_with(['.', ' ']),
                "shortened name {fitted:?} ended with a dot or space"
            );
        }
    }

    #[test]
    fn all_reserved_device_names() {
        for n in [
            "CONIN$.txt",
            "conout$",
            "COM¹.log",
            "LPT³",
            "com9.txt",
            "nul",
        ] {
            assert!(sanitize(n).starts_with('_'), "{n}");
        }
        assert_eq!(sanitize("COMPUTER.txt"), "COMPUTER.txt");
    }

    #[test]
    fn one_destination_one_download() {
        let p = std::env::temp_dir().join("mdm-reserve-test.bin");
        let a = reserve(&p).expect("free");
        assert!(reserve(&p).is_none(), "second download refused");
        let other = unique_path(&std::env::temp_dir(), "mdm-reserve-test.bin");
        assert_ne!(other, p, "unique_path skips reserved names");
        drop(a);
        assert!(reserve(&p).is_some(), "released when dropped");
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn header_wins_over_url() {
        let u = url("http://x/a/b.zip");
        assert_eq!(
            file_name(Some("attachment; filename=\"movie.mp4\""), &u),
            "movie.mp4"
        );
        assert_eq!(
            file_name(
                Some("attachment; filename*=UTF-8''na%C3%AFve%20file.txt"),
                &u
            ),
            "naïve file.txt"
        );
        assert_eq!(file_name(None, &u), "b.zip");
        assert_eq!(file_name(Some("inline"), &u), "b.zip");
    }

    #[test]
    fn the_encoded_name_wins_over_its_ascii_fallback() {
        let u = url("http://x/get?id=7");
        let name = "تقرير-été-日本.zip";
        let encoded: String =
            percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
                .to_string();
        // Either order, any case, quoted or not, with a language tag.
        for cd in [
            format!("attachment; filename=\"report.zip\"; filename*=UTF-8''{encoded}"),
            format!("attachment; filename*=utf-8''{encoded}; filename=report.zip"),
            format!("attachment; FILENAME*=UTF-8'ar'{encoded}; Filename=\"report.zip\""),
            format!("attachment; filename*=\"UTF-8''{encoded}\""),
        ] {
            assert_eq!(file_name(Some(&cd), &u), name, "{cd}");
            assert_eq!(disposition_name(&cd).as_deref(), Some(name), "{cd}");
        }
        assert_eq!(
            file_name(
                Some("attachment; filename*=ISO-8859-1''caf%E9%20cr%E8me.txt"),
                &u
            ),
            "café crème.txt"
        );
        // An encoding it can't read, or bytes that aren't UTF-8: the plain name.
        for cd in [
            "attachment; filename=\"plain.txt\"; filename*=KOI8-R''%C1%C2.txt",
            "attachment; filename=\"plain.txt\"; filename*=UTF-8''%FF%FE.txt",
            "attachment; filename=\"plain.txt\"; filename*=broken",
        ] {
            assert_eq!(file_name(Some(cd), &u), "plain.txt", "{cd}");
        }
    }

    #[test]
    fn plain_names_are_unquoted_and_decoded() {
        let u = url("http://x/a.bin");
        let n = |cd: &str| file_name(Some(cd), &u);
        assert_eq!(n("attachment; filename=\"a; b.zip\"; size=3"), "a; b.zip");
        assert_eq!(
            n(r#"attachment; filename="say \"hi\".txt""#),
            "say _hi_.txt"
        );
        assert_eq!(n("attachment; filename=  spaced.txt  ; x=1"), "spaced.txt");
        assert_eq!(n("filename=no-type.pdf"), "no-type.pdf");
        // Percent-encoded UTF-8 in the plain name (Chromium decodes it); ASCII stays as sent.
        assert_eq!(
            n("attachment; filename=\"%E6%97%A5%E6%9C%AC.txt\""),
            "日本.txt"
        );
        assert_eq!(n("attachment; filename=\"100%25.txt\""), "100%25.txt");
        // Raw UTF-8 in the header (the download reads its bytes as UTF-8).
        assert_eq!(n("attachment; filename=\"été.mp3\""), "été.mp3");
        // Nothing usable: the URL's name.
        assert_eq!(n("attachment; filename=\"\""), "a.bin");
        assert_eq!(n("attachment"), "a.bin");
    }

    #[test]
    fn format_characters_are_removed() {
        // A right-to-left override would show this as "invoiceexe.pdf".
        assert_eq!(sanitize("invoice\u{202E}fdp.exe"), "invoicefdp.exe");
        for c in [
            '\u{200E}',
            '\u{200F}',
            '\u{202A}',
            '\u{202B}',
            '\u{202C}',
            '\u{202D}',
            '\u{2066}',
            '\u{2067}',
            '\u{2068}',
            '\u{2069}',
            '\u{200B}',
            '\u{200C}',
            '\u{200D}',
            '\u{FEFF}',
            '\u{061C}',
            '\u{00AD}',
            '\u{E0041}',
        ] {
            assert_eq!(
                sanitize(&format!("a{c}b.txt")),
                "ab.txt",
                "U+{:04X}",
                c as u32
            );
        }
        // Only invisible characters: nothing is left.
        assert_eq!(sanitize("\u{202E}\u{200B}"), "download");
        // From a header too.
        assert_eq!(
            file_name(
                Some("attachment; filename*=UTF-8''photo%E2%80%AEgnp.scr"),
                &url("http://x/")
            ),
            "photognp.scr"
        );
        // Visible text in any script is kept.
        assert_eq!(sanitize("تقرير-été-日本.zip"), "تقرير-été-日本.zip");
    }

    fn named(u: &str, a: Answer) -> String {
        answer_name(&url(u), &a)
    }

    fn typed(content_type: &str) -> Answer<'_> {
        Answer {
            content_type: Some(content_type),
            ..Answer::default()
        }
    }

    /// #695, #800: a video a script sends is named by its type, not the script's extension.
    #[test]
    fn a_scripts_answer_takes_its_types_extension() {
        let mp4 = typed("video/mp4");
        assert_eq!(named("http://x/stream.php?id=7", mp4), "stream.mp4");
        assert_eq!(
            named(
                "http://x/v/remote_control.php?a=1",
                typed("video/mp4; codecs=avc1")
            ),
            "remote_control.mp4"
        );
        assert_eq!(named("http://x/get.aspx", typed("audio/mpeg")), "get.mp3");
        assert_eq!(named("http://x/dl.cgi", typed("application/pdf")), "dl.pdf");
        assert_eq!(named("http://x/watch.html", mp4), "watch.mp4");
        // A bare id and Google's `videoplayback` have no extension: the type gives one.
        assert_eq!(named("http://x/media/123456", mp4), "123456.mp4");
        assert_eq!(
            named("http://x/videoplayback?itag=18", mp4),
            "videoplayback.mp4"
        );
        assert_eq!(named("http://x/", mp4), "download.mp4");
        // A type that isn't a file's (a page, nothing said) leaves the name as it is.
        assert_eq!(
            named("http://x/stream.php", typed("text/html")),
            "stream.php"
        );
        assert_eq!(
            named("http://x/stream.php", typed("application/octet-stream")),
            "stream.php"
        );
        assert_eq!(
            named("http://x/stream.php", Answer::default()),
            "stream.php"
        );
    }

    /// #695: the script's `file=` (or `filename=`) value names it, before and after the answer.
    #[test]
    fn a_scripts_file_value_names_it() {
        let u = "http://x/baigiang/stream.php?file=FL262-1E01-4.mp4&token=abc";
        assert_eq!(file_name(None, &url(u)), "FL262-1E01-4.mp4");
        assert_eq!(named(u, typed("video/mp4")), "FL262-1E01-4.mp4");
        assert_eq!(
            file_name(None, &url("http://x/get.php?filename=a%2Fb%2Fclip.webm")),
            "clip.webm"
        );
        // No type known from the value: the script's name stays.
        assert_eq!(
            file_name(None, &url("http://x/get.php?file=notes")),
            "get.php"
        );
        // A real name in the path is the file's: the query doesn't change it.
        assert_eq!(
            file_name(None, &url("http://x/files/movie.mkv?file=other.mp4")),
            "movie.mkv"
        );
        // The page's title doesn't replace a real name.
        let titled = Answer {
            title: Some("Lecture one"),
            ..typed("video/mp4")
        };
        assert_eq!(named(u, titled), "FL262-1E01-4.mp4");
    }

    /// Content-Disposition still wins over the type, the `file=` value and the title.
    #[test]
    fn the_servers_name_wins_over_type_and_title() {
        let a = Answer {
            disposition: Some("attachment; filename=\"Lecture 1.mp4\""),
            content_type: Some("video/webm"),
            sniffed: Some("mkv"),
            title: Some("A page"),
        };
        assert_eq!(
            named("http://x/stream.php?file=other.mp4", a),
            "Lecture 1.mp4"
        );
    }

    /// A real file name is never changed by the type, the bytes or a title.
    #[test]
    fn real_names_are_left_alone() {
        let a = Answer {
            content_type: Some("video/webm"),
            sniffed: Some("mkv"),
            title: Some("A page"),
            ..Answer::default()
        };
        assert_eq!(named("http://x/clip.mp4", a), "clip.mp4");
        assert_eq!(named("http://x/setup.zip", typed("video/mp4")), "setup.zip");
        assert_eq!(named("http://x/my%20file.iso", a), "my file.iso");
    }

    /// The type's extension is only ever a media, picture, document or archive one: a script
    /// serving a program, a script or a page keeps its own name, and a `file=` naming a program
    /// is not taken.
    #[test]
    fn a_dangerous_extension_is_never_introduced() {
        for t in [
            "application/x-msdownload",
            "application/x-msdos-program",
            "application/x-sh",
            "application/javascript",
            "application/hta",
            "application/x-ms-shortcut",
            "text/html",
        ] {
            assert_eq!(named("http://x/get.php", typed(t)), "get.php", "{t}");
            assert_eq!(named("http://x/123", typed(t)), "123", "{t}");
        }
        for v in [
            "setup.exe",
            "run.bat",
            "x.scr",
            "a.lnk",
            "b.ps1",
            "c.js",
            "d.hta",
            "e.msi",
        ] {
            assert_eq!(
                file_name(None, &url(&format!("http://x/get.php?file={v}"))),
                "get.php"
            );
        }
        let runs = [
            "exe", "bat", "cmd", "com", "scr", "msi", "lnk", "ps1", "js", "vbs", "hta",
        ];
        for (_, e) in TYPES {
            assert!(!runs.contains(e), "{e}");
        }
        assert_eq!(
            named(
                "http://x/v.php",
                Answer {
                    sniffed: Some("exe"),
                    ..Answer::default()
                }
            ),
            "v.php"
        );
    }

    /// #800: a page's media with a name that means nothing takes the page's title.
    #[test]
    fn a_meaningless_name_takes_the_pages_title() {
        let t = |title| Answer {
            title: Some(title),
            ..typed("video/mp4")
        };
        assert_eq!(
            named("http://x/remote_control.php?id=9", t("A clip: part 1/2")),
            "A clip part 1 2.mp4"
        );
        assert_eq!(named("http://x/videoplayback", t("Talk")), "Talk.mp4");
        assert_eq!(
            named("http://x/v/aGVsbG8gd29ybGQgdGhpcyBpcyBh.mp4", t("Talk")),
            "Talk.mp4"
        );
        // A real name keeps its own; a blank title is none.
        assert_eq!(named("http://x/clip.mp4", t("Talk")), "clip.mp4");
        assert_eq!(named("http://x/stream.php", t("  ")), "stream.mp4");
        // Its type unknown: the title, and the script's extension (nothing says it is another).
        let untyped = Answer {
            title: Some("Talk"),
            ..Answer::default()
        };
        assert_eq!(named("http://x/stream.php", untyped), "Talk.php");
        assert_eq!(named("http://x/12345", untyped), "Talk");
        // Its bytes said: that type.
        let sniffed = Answer {
            sniffed: Some("webm"),
            ..untyped
        };
        assert_eq!(named("http://x/stream.php", sniffed), "Talk.webm");
    }

    /// The names the browser extension gives a page's media (background.js' `mediaName`) are
    /// the engine's: both check the same cases (`tools/extension/test.mjs`).
    #[test]
    fn the_extension_names_media_as_the_engine_does() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/data/media-names.json")).unwrap();
        for c in cases["cases"].as_array().unwrap() {
            let text = |i: usize| c[i].as_str().unwrap();
            let (mime, title) = (text(1).to_owned(), text(2).to_owned());
            let a = Answer {
                content_type: Some(mime.as_str()).filter(|s| !s.is_empty()),
                title: Some(title.as_str()).filter(|s| !s.is_empty()),
                ..Answer::default()
            };
            assert_eq!(named(text(0), a), text(3), "{c}");
        }
    }

    #[test]
    fn titles_are_cleaned() {
        assert_eq!(clean_title("  a  b\t c  "), "a b c");
        assert_eq!(clean_title("Who? Me: <yes>|no*"), "Who Me yes no");
        assert_eq!(clean_title("End..."), "End");
        assert_eq!(clean_title("x\u{202E}y"), "xy");
        assert_eq!(clean_title(&"é".repeat(200)).chars().count(), 150);
    }

    #[test]
    fn bytes_say_their_type() {
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom\0\0\x02\0");
        assert_eq!(sniff(&mp4), Some("mp4"));
        assert_eq!(sniff(b"\0\0\0\x20ftypM4A \0\0"), Some("m4a"));
        assert_eq!(sniff(b"\0\0\0\x14ftypqt  "), Some("mov"));
        assert_eq!(
            sniff(b"\x1A\x45\xDF\xA3\x9F\x42\x86\x81\x01\x42\x82\x84webm"),
            Some("webm")
        );
        assert_eq!(sniff(b"\x1A\x45\xDF\xA3\x42\x82\x88matroska"), Some("mkv"));
        assert_eq!(sniff(b"OggS\0\x02"), Some("ogg"));
        assert_eq!(sniff(b"fLaC\0\0\0\x22"), Some("flac"));
        assert_eq!(sniff(b"ID3\x04\0\0"), Some("mp3"));
        assert_eq!(sniff(&[0xFF, 0xFB, 0x90, 0x64]), Some("mp3"));
        assert_eq!(sniff(&[0xFF, 0xF1, 0x50, 0x80]), Some("aac"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WAVEfmt "), Some("wav"));
        assert_eq!(sniff(b"RIFF\0\0\0\0AVI LIST"), Some("avi"));
        assert_eq!(sniff(b"FLV\x01\x05"), Some("flv"));
        // Not media: a page, a program, UTF-16 text, nothing.
        assert_eq!(sniff(b"<!doctype html>"), None);
        assert_eq!(sniff(b"MZ\x90\0\x03\0"), None);
        assert_eq!(sniff(&[0xFF, 0xFE, 0x3C, 0x00]), None);
        assert_eq!(sniff(b""), None);
        // Every type the bytes give is one a name may take.
        for e in [
            "mp4", "m4a", "mov", "3gp", "webm", "mkv", "ogg", "flac", "mp3", "aac", "wav",
        ] {
            assert!(is_typed_extension(e), "{e}");
        }
        assert!(is_typed_extension("avi") && is_typed_extension("flv"));
    }

    #[test]
    fn bytes_are_looked_at_only_when_nothing_else_says() {
        let u = url("http://x/stream.php?id=1");
        assert!(needs_sniff(&u, None, None));
        assert!(needs_sniff(&u, None, Some("application/octet-stream")));
        assert!(!needs_sniff(&u, None, Some("video/mp4")));
        assert!(!needs_sniff(&u, Some("attachment; filename=a.bin"), None));
        assert!(!needs_sniff(&url("http://x/a.zip"), None, None));
        assert!(!needs_sniff(&url("http://x/s.php?file=c.mp4"), None, None));
    }

    #[test]
    fn url_fallbacks() {
        assert_eq!(
            file_name(None, &url("http://x/dir/my%20file.iso?x=1")),
            "my file.iso"
        );
        assert_eq!(file_name(None, &url("http://x/")), "download");
    }

    #[test]
    fn sanitizes() {
        assert_eq!(
            file_name(
                Some("attachment; filename=\"../../evil.exe\""),
                &url("http://x/")
            ),
            "evil.exe"
        );
        assert_eq!(sanitize("a<b>c:d?.txt"), "a_b_c_d_.txt");
        assert_eq!(sanitize("con.txt"), "_con.txt");
        assert_eq!(sanitize("COM1"), "_COM1");
        assert_eq!(sanitize("compact.txt"), "compact.txt");
        assert_eq!(sanitize("name. . "), "name");
        assert_eq!(sanitize("..."), "download");
    }

    /// #743: the guess a window shows before the disk answers is `destination`'s name, fitted
    /// the same way, never numbered (and never looked up).
    #[test]
    fn the_guess_is_the_destination_before_numbering() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(guess(d.path(), "a.zip"), destination(d.path(), "a.zip"));
        std::fs::write(d.path().join("a.zip"), b"").unwrap();
        assert_eq!(guess(d.path(), "a.zip"), d.path().join("a.zip"));
        assert_eq!(destination(d.path(), "a.zip"), d.path().join("a (1).zip"));
        let long = format!("{}.zip", "x".repeat(400));
        assert_eq!(
            guess(d.path(), &long),
            d.path().join(fit_name(d.path(), &long).unwrap())
        );
        // A folder that isn't there: nothing to look up, the same name.
        let gone = d.path().join("gone");
        assert_eq!(guess(&gone, "a.zip"), gone.join("a.zip"));
    }

    #[test]
    fn numbered_duplicates() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(unique_path(d.path(), "a.zip"), d.path().join("a.zip"));
        std::fs::write(d.path().join("a.zip"), b"").unwrap();
        assert_eq!(unique_path(d.path(), "a.zip"), d.path().join("a (1).zip"));
        std::fs::write(part_path(&d.path().join("a (1).zip")), b"").unwrap();
        assert_eq!(unique_path(d.path(), "a.zip"), d.path().join("a (2).zip"));
        std::fs::write(d.path().join("README"), b"").unwrap();
        assert_eq!(unique_path(d.path(), "README"), d.path().join("README (1)"));
    }
}
