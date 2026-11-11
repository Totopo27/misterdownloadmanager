//! Choosing the local file name for a download.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use percent_encoding::percent_decode_str;
use url::Url;

/// File name from `Content-Disposition`, else the last URL path segment, else `download`.
/// Always safe to use as a single path component on Windows and Unix.
pub fn file_name(content_disposition: Option<&str>, url: &Url) -> String {
    let from_url = || {
        url.path_segments()
            .and_then(|mut s| s.next_back())
            .map(|s| percent_decode_str(s).decode_utf8_lossy().into_owned())
    };
    let name = content_disposition
        .and_then(disposition_name)
        .or_else(from_url)
        .unwrap_or_default();
    // A header may carry a path; keep only its last component.
    let name = name.rsplit(['/', '\\']).next().unwrap_or_default();
    sanitize(name)
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
        let t = std::time::Instant::now();
        for i in 0..5000 {
            stems.pick(&format!("Video {}.mp4", i % 2500));
        }
        assert_eq!(stems.pick("Video 7.mp4"), d.join("Video 7 (2).mp4"));
        assert!(
            t.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            t.elapsed()
        );
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
