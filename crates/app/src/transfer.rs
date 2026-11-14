//! Tasks → Export / Import (D-021): the list's addresses to a text file or an IDM export file
//! (`.ef2`), and downloads from either. IDM's import finds every web address in a file and shows
//! them to check before they are added; so does ours (in Link grabber, grabber.rs). An `.ef2` file holds one block per
//! download — `<`, the address, `referer: …` / `cookie: …` / `User-Agent: …` lines, `>` — so a
//! list moves between IDM and MDM with its request context.

use crate::model::Item;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The largest file Import reads (#805): 64 MiB. A list of addresses is small: an address
/// takes about 100 bytes and an `.ef2` block with its cookies a few hundred, so this holds
/// hundreds of thousands of downloads, more than any list or saved page. A file past it is
/// something else picked by mistake (a video, an archive, a backup of another program), and
/// reading it whole would only fill the memory to find nothing.
pub const IMPORT_LIMIT: u64 = 64 * 1024 * 1024;

/// What reading a list file found (on a thread of its own: [`Importing`]).
#[derive(Clone, Debug, PartialEq)]
pub enum ListRead {
    /// The downloads in it (none: no web address in it).
    Found(Vec<Imported>),
    /// Past [`IMPORT_LIMIT`]: its size, unread.
    TooBig(u64),
    /// It couldn't be read: why, in English (the system's words).
    Failed(String),
}

/// Read `path` as Import does: at most [`IMPORT_LIMIT`] bytes, its downloads found. May wait
/// (a sleeping disk, a network folder) and takes a moment on a large file: never on the
/// window's thread.
pub fn read_list(path: &Path) -> ListRead {
    read_list_within(path, IMPORT_LIMIT)
}

fn read_list_within(path: &Path, limit: u64) -> ListRead {
    use std::io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return ListRead::Failed(e.to_string()),
    };
    // Its size as the disk tells it, then never more than the limit read (a file that grows
    // meanwhile, or one whose size is not told, can't fill the memory either).
    if let Ok(m) = file.metadata()
        && m.len() > limit
    {
        return ListRead::TooBig(m.len());
    }
    let mut bytes = Vec::new();
    if let Err(e) = file.take(limit + 1).read_to_end(&mut bytes) {
        return ListRead::Failed(e.to_string());
    }
    if bytes.len() as u64 > limit {
        return ListRead::TooBig(bytes.len() as u64);
    }
    ListRead::Found(import(&String::from_utf8_lossy(&bytes)))
}

/// How a list file is read: [`read_list`], or a test's slow one.
pub type Reader = Arc<dyn Fn(&Path) -> ListRead + Send + Sync>;

/// The list files Import is reading (#805), each beside the window (`launch::Asked`): the
/// window asks once and takes the answer on a later frame, never waiting for the disk or the
/// search. Several files can be on their way at once (a few dropped together).
pub struct Importing {
    read: Reader,
    reading: Vec<(PathBuf, crate::launch::Asked<PathBuf, ListRead>)>,
}

impl Default for Importing {
    fn default() -> Self {
        Self::with(Arc::new(read_list))
    }
}

impl Importing {
    /// Files read with `read` (a test's).
    pub fn with(read: Reader) -> Self {
        Importing {
            read,
            reading: Vec::new(),
        }
    }

    /// Read `path` on a thread of its own; returns at once. `repaint` is repainted when its
    /// answer comes.
    pub fn start(&mut self, path: &Path, repaint: Option<&egui::Context>) {
        let asked = crate::launch::Asked::default();
        let read = self.read.clone();
        // Never waits: the answer is taken on a later frame (`finished`).
        let _ = asked.get(
            &path.to_owned(),
            std::time::Duration::MAX,
            std::time::Duration::ZERO,
            repaint,
            move |p| read(p),
        );
        self.reading.push((path.to_owned(), asked));
    }

    /// The files still being read, in the order they were started.
    pub fn pending(&self) -> impl Iterator<Item = &Path> {
        self.reading.iter().map(|(p, _)| p.as_path())
    }

    /// The files read since the last call, with what was found; never waits.
    pub fn finished(&mut self) -> Vec<(PathBuf, ListRead)> {
        let mut done = Vec::new();
        self.reading
            .retain(|(path, asked)| match asked.answer(path) {
                Some(read) => {
                    done.push((path.clone(), read));
                    false
                }
                None => true,
            });
        done
    }
}

/// A download read from a file: its address and what the browser sent with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    pub url: String,
    pub referer: Option<String>,
    pub cookies: Option<String>,
    pub user_agent: Option<String>,
    /// From a private window's page: kept out of the saved list.
    pub private: bool,
    /// The package a site packed it in (Click'n'Load): Link grabber's package.
    pub package: Option<String>,
    /// That package's archive password.
    pub password: Option<String>,
    /// Sent by a web page (Click'n'Load): Link grabber doesn't look it up; nothing is fetched
    /// for it before the user adds it to the queue.
    pub from_page: bool,
}

/// One address per line.
pub fn export_text(items: &[Item]) -> String {
    items.iter().map(|i| format!("{}\r\n", i.url)).collect()
}

/// IDM's export format.
pub fn export_ef2(items: &[Item]) -> String {
    let mut out = String::new();
    for i in items {
        out.push_str("<\r\n");
        out.push_str(&format!("{}\r\n", i.url));
        let c = &i.context;
        if let Some(r) = c.referer.as_deref().filter(|r| !r.is_empty()) {
            out.push_str(&format!("referer: {r}\r\n"));
        }
        if let Some(k) = c.cookies.as_deref().filter(|k| !k.is_empty()) {
            out.push_str(&format!("cookie: {k}\r\n"));
        }
        if let Some(u) = c.user_agent.as_deref().filter(|u| !u.is_empty()) {
            out.push_str(&format!("User-Agent: {u}\r\n"));
        }
        out.push_str(">\r\n");
    }
    out
}

/// The downloads in a file or a copied text: `.ef2` blocks when it has them, else every
/// download address in the text ([`addresses`]), each once, in order.
pub fn import(text: &str) -> Vec<Imported> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let blocks = ef2_blocks(text);
    if !blocks.is_empty() {
        return blocks;
    }
    let mut seen = std::collections::HashSet::new();
    addresses(text)
        .into_iter()
        .filter(|u| seen.insert(same_key(u)))
        .map(|url| Imported {
            url,
            ..Default::default()
        })
        .collect()
}

fn ef2_blocks(text: &str) -> Vec<Imported> {
    let mut out = Vec::new();
    let mut cur: Option<Imported> = None;
    for line in text.lines().map(str::trim) {
        match line {
            "<" => cur = Some(Imported::default()),
            ">" => {
                if let Some(d) = cur.take().filter(|d| downloadable(&d.url)) {
                    out.push(d);
                }
            }
            _ => {
                let Some(d) = cur.as_mut() else { continue };
                if d.url.is_empty() {
                    d.url = line.to_owned();
                } else if let Some((k, v)) = line.split_once(':') {
                    let v = v.trim().to_owned();
                    match k.trim().to_ascii_lowercase().as_str() {
                        "referer" => d.referer = Some(v),
                        "cookie" => d.cookies = Some(v),
                        "user-agent" => d.user_agent = Some(v),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

fn downloadable(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| mdm_engine::supported(&u))
}

/// Every download address in free text (a page saved as text, a list, a mail, a forum post):
/// http(s), ftp(s) and magnet links, one per line or inside sentences, Markdown links
/// (`[name](address)`), angle brackets (`<address>`) or HTML (`href="address"`). What ends a
/// sentence (`.`, `,`, `!`…), a closing bracket the address didn't open and Markdown's `**` are
/// not part of the address; a pair of brackets inside it is (`…/wiki/Name_(film)`).
fn addresses(text: &str) -> Vec<String> {
    // ASCII lower case keeps every byte where it was.
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut at = 0;
    // The next of each, found again only once passed: a long list without magnet links (or
    // with nothing but them) is read once, not once per address (#805).
    let (mut web, mut magnet) = (Next::default(), Next::default());
    loop {
        let web = web.from(&lower, "://", at);
        let magnet = magnet.from(&lower, "magnet:?", at);
        let (pos, is_magnet) = match (web, magnet) {
            (Some(w), Some(m)) if m < w => (m, true),
            (Some(w), _) => (w, false),
            (None, Some(m)) => (m, true),
            (None, None) => break,
        };
        let start = if is_magnet {
            pos
        } else {
            scheme_start(&lower, pos)
        };
        let rest = &text[start..];
        let len = address_len(rest);
        let before = text[..start].chars().next_back();
        let cand = trim_address(&rest[..len], before);
        let found = if is_magnet {
            magnet_link(cand)
        } else {
            web_address(cand)
        };
        out.extend(found);
        at = (start + len).max(pos + 3);
    }
    out
}

/// Where a text's next match of a needle is, kept between looks.
#[derive(Default)]
struct Next(Option<Option<usize>>);

impl Next {
    /// The first match at or after `at`: the one found last time while it isn't behind `at`.
    fn from(&mut self, text: &str, needle: &str, at: usize) -> Option<usize> {
        match self.0 {
            Some(None) => None,
            Some(Some(p)) if p >= at => Some(p),
            _ => {
                let p = text[at..].find(needle).map(|i| at + i);
                self.0 = Some(p);
                p
            }
        }
    }
}

/// Where the scheme before the "://" at `sep` starts: the letters right before it, or only
/// their end when it is a scheme MDM downloads glued to a word (`Linkhttps://…`).
fn scheme_start(lower: &str, sep: usize) -> usize {
    let start = lower[..sep]
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_ascii_alphabetic())
        .map_or(0, |(i, c)| i + c.len_utf8());
    let word = &lower[start..sep];
    ["https", "http", "ftps", "ftp"]
        .iter()
        .find(|s| word.len() > s.len() && word.ends_with(*s))
        .map_or(start, |s| sep - s.len())
}

/// How long the address at the start of `rest` is: up to a space, a quote, an angle bracket,
/// a closing bracket it didn't open, or a sign of punctuation from another script (`，`, `」`).
fn address_len(rest: &str) -> usize {
    let (mut round, mut square) = (0u32, 0u32);
    for (i, c) in rest.char_indices() {
        match c {
            '(' => round += 1,
            '[' => square += 1,
            ')' if round == 0 => return i,
            ')' => round -= 1,
            ']' if square == 0 => return i,
            ']' => square -= 1,
            // The double quote by its code: theme.rs's glyph check pairs the source's quotes.
            '\u{22}' | '\'' | '<' | '>' | '`' | '{' | '}' | '|' | '\\' | '^' => return i,
            '\u{200b}' | '\u{feff}' | '“' | '”' | '‘' | '’' | '«' | '»' | '‹' | '›' | '「'
            | '」' | '『' | '』' | '（' | '）' | '【' | '】' | '《' | '》' | '〈' | '〉' | '，'
            | '。' | '、' | '；' | '：' | '！' | '？' | '…' => return i,
            c if c.is_whitespace() || c.is_control() => return i,
            _ => {}
        }
    }
    rest.len()
}

/// The address without what ends the sentence round it (`.`, `,`, `:`, `!`, `?`, `;`), nor the
/// closing `**`, `__` or `~~` of the Markdown that opened before it (`before`).
fn trim_address(cand: &str, before: Option<char>) -> &str {
    let mut cand = cand;
    loop {
        let trimmed = cand.trim_end_matches(['.', ',', ';', ':', '!', '?']);
        let trimmed = match before {
            Some(m @ ('*' | '_' | '~')) => trimmed.trim_end_matches(m),
            _ => trimmed,
        };
        if trimmed.len() == cand.len() {
            return cand;
        }
        cand = trimmed;
    }
}

/// An http(s) or ftp(s) address MDM downloads, as the address bar writes it; `&amp;` read as
/// `&` (an address copied out of a page's HTML).
fn web_address(cand: &str) -> Option<String> {
    let cand = cand.replace("&amp;", "&");
    url::Url::parse(&cand)
        .ok()
        .filter(|u| mdm_engine::supported(u) && u.host_str().is_some_and(|h| !h.is_empty()))
        .map(String::from)
}

/// A magnet link to a torrent (one with its BitTorrent hash, `xt=urn:btih:` or `urn:btmh:`).
fn magnet_link(cand: &str) -> Option<String> {
    let cand = cand.replace("&amp;", "&");
    (mdm_torrent::Source::magnet(&cand).is_some() && magnet_hash(&cand).is_some()).then_some(cand)
}

/// A magnet link's torrent hash (`xt=urn:btih:<hash>`), in lower case.
pub fn magnet_hash(magnet: &str) -> Option<String> {
    let lower = magnet.to_ascii_lowercase();
    let query = lower.strip_prefix("magnet:?")?;
    query.split('&').find_map(|kv| {
        let hash = kv
            .strip_prefix("xt=urn:btih:")
            .or_else(|| kv.strip_prefix("xt=urn:btmh:"))?;
        (!hash.is_empty()).then(|| hash.to_owned())
    })
}

/// What makes two found addresses one download: the same torrent for magnet links, else the
/// same address but for a `#fragment`.
fn same_key(address: &str) -> String {
    if let Some(hash) = magnet_hash(address) {
        return format!("magnet:{hash}");
    }
    match url::Url::parse(address) {
        Ok(mut u) => {
            u.set_fragment(None);
            u.into()
        }
        Err(_) => address.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idm_export_files_round_trip() {
        let mut a = Item::new(
            1,
            "https://x.org/a.zip".parse().unwrap(),
            "C:\\d\\a.zip".into(),
            crate::model::Category::Compressed,
        );
        a.context.referer = Some("https://x.org/".into());
        a.context.cookies = Some("sid=1".into());
        let b = Item::new(
            2,
            "https://y.org/b.iso".parse().unwrap(),
            "C:\\d\\b.iso".into(),
            crate::model::Category::General,
        );
        let text = export_ef2(&[a, b]);
        assert_eq!(
            import(&text),
            vec![
                Imported {
                    url: "https://x.org/a.zip".into(),
                    referer: Some("https://x.org/".into()),
                    cookies: Some("sid=1".into()),
                    user_agent: None,
                    private: false,
                    ..Default::default()
                },
                Imported {
                    url: "https://y.org/b.iso".into(),
                    ..Default::default()
                },
            ]
        );
    }

    #[test]
    fn addresses_are_found_in_any_text() {
        let text = "See https://a.org/x.zip, and (http://b.org/y.rar). Again: https://a.org/x.zip\n\
                    <a href=\"https://c.org/z.7z\">z</a> ftp://f.org/f mailto://x httpnot";
        let urls: Vec<String> = import(text).into_iter().map(|d| d.url).collect();
        assert_eq!(
            urls,
            [
                "https://a.org/x.zip",
                "http://b.org/y.rar",
                "https://c.org/z.7z",
                "ftp://f.org/f"
            ]
        );
        assert_eq!(export_text(&[]), "");
    }

    fn found(text: &str) -> Vec<String> {
        import(text).into_iter().map(|d| d.url).collect()
    }

    /// #709: a forum post or a list copied from a page: the addresses inside Markdown, angle
    /// brackets, brackets and sentences, magnet links among them, each once.
    #[test]
    fn addresses_in_a_forum_post() {
        let post = "Mirror 1: **https://dl.example.org/pack/part1.rar**\n\
            Mirror 2: [part 2](https://dl.example.org/pack/part2.rar \"title\")\n\
            [https://dl.example.org/pack/part3.rar](https://dl.example.org/pack/part3.rar)\n\
            Old: <https://old.example.org/notes.txt>, and the wiki (https://wiki.example.org/Film_(1999)).\n\
            Torrent: magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01&dn=Show&amp;tr=udp://t.example:80/announce.\n\
            Same torrent: magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01\n\
            Glued:https://glued.example.org/a.iso! Linkhttps://word.example.org/b.iso?\n\
            Twice: https://DL.example.org/pack/part1.rar#again and HTTPS://dl.example.org:443/pack/part2.rar\n\
            Quoted «https://q.example.org/q.zip» and 「https://cjk.example.org/c.zip」，next\n\
            Query: https://h.example.org/get?id=7&amp;key=x; done\n\
            Not these: mailto:a@b.org, magnet:?dn=no-hash, https:// nothing, file:///C:/x.zip";
        assert_eq!(
            found(post),
            [
                "https://dl.example.org/pack/part1.rar",
                "https://dl.example.org/pack/part2.rar",
                "https://dl.example.org/pack/part3.rar",
                "https://old.example.org/notes.txt",
                "https://wiki.example.org/Film_(1999)",
                "magnet:?xt=urn:btih:ABCDEF0123456789ABCDEF0123456789ABCDEF01&dn=Show&tr=udp://t.example:80/announce",
                "https://glued.example.org/a.iso",
                "https://word.example.org/b.iso",
                "https://q.example.org/q.zip",
                "https://cjk.example.org/c.zip",
                "https://h.example.org/get?id=7&key=x",
            ]
        );
        assert_eq!(
            magnet_hash("MAGNET:?dn=x&xt=urn:btih:ABC").as_deref(),
            Some("abc")
        );
        assert_eq!(found(""), Vec::<String>::new());
        assert_eq!(found("no links here, only words."), Vec::<String>::new());
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mdm-transfer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_past_the_limit_is_not_read_and_one_within_it_is() {
        let dir = temp("limit");
        let list = dir.join("list.txt");
        std::fs::write(&list, "https://a.org/x.zip\nhttps://b.org/y.rar\n").unwrap();
        let size = std::fs::metadata(&list).unwrap().len();
        assert_eq!(
            read_list_within(&list, size),
            ListRead::Found(vec![
                Imported {
                    url: "https://a.org/x.zip".into(),
                    ..Default::default()
                },
                Imported {
                    url: "https://b.org/y.rar".into(),
                    ..Default::default()
                },
            ])
        );
        // One byte over: unread, its size told.
        assert_eq!(read_list_within(&list, size - 1), ListRead::TooBig(size));
        // A file that isn't text (bytes that aren't UTF-8): read, nothing found, no failure.
        let bin = dir.join("video.mp4");
        std::fs::write(
            &bin,
            (0..=255u8).cycle().take(64 * 1024).collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(read_list(&bin), ListRead::Found(Vec::new()));
        // The real limit, on a sparse file past it: refused from its size alone, at once.
        let big = dir.join("backup.ibf");
        std::fs::File::create(&big)
            .unwrap()
            .set_len(IMPORT_LIMIT + 1)
            .unwrap();
        let t = std::time::Instant::now();
        assert_eq!(read_list(&big), ListRead::TooBig(IMPORT_LIMIT + 1));
        assert!(t.elapsed() < std::time::Duration::from_secs(1));
        assert!(matches!(
            read_list(&dir.join("missing.txt")),
            ListRead::Failed(_)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_read_that_blocks_never_holds_a_frame() {
        use std::sync::{Mutex, mpsc};
        use std::time::{Duration, Instant};
        // The read waits until the test lets it go (a disk that doesn't answer).
        let (go, wait) = mpsc::channel::<()>();
        let wait = Mutex::new(wait);
        let mut importing = Importing::with(Arc::new(move |p: &Path| {
            let _ = wait.lock().unwrap().recv();
            ListRead::Failed(p.display().to_string())
        }));
        let path = PathBuf::from("slow-list.txt");
        let t = Instant::now();
        importing.start(&path, None);
        assert!(t.elapsed() < Duration::from_millis(200), "start waited");
        // Frames meanwhile: at once, nothing finished, the file still pending.
        for _ in 0..50 {
            let t = Instant::now();
            assert!(importing.finished().is_empty());
            assert!(t.elapsed() < Duration::from_millis(50), "a frame waited");
            assert_eq!(importing.pending().collect::<Vec<_>>(), [path.as_path()]);
        }
        go.send(()).unwrap();
        let start = Instant::now();
        let done = loop {
            let done = importing.finished();
            if !done.is_empty() {
                break done;
            }
            assert!(start.elapsed() < Duration::from_secs(5), "never answered");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            done,
            [(path.clone(), ListRead::Failed("slow-list.txt".into()))]
        );
        // Taken once: gone from the pending ones.
        assert_eq!(importing.pending().count(), 0);
        assert!(importing.finished().is_empty());
    }

    #[test]
    fn a_large_list_is_searched_in_linear_time() {
        // Addresses and as much text that only looks like them: the search goes through it
        // once, 8 times the list costing about 8 times as long (not 64), on any machine.
        let list = |n: usize| {
            let mut text = String::new();
            for i in 0..n {
                text.push_str(&format!("https://dl.example.com/files/{i}.zip\n"));
                text.push_str("abc:// not an address ://\n");
            }
            text
        };
        assert_eq!(import(&list(1_000)).len(), 1_000);
        mdm_labserver::growth::assert_linear(5_000, list, |text| import(&text));
    }
}
