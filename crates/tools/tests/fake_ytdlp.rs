//! Subtitles a site refuses never cost the video (#68), against a fake yt-dlp that answers as the
//! real one did for YouTube's automatic English captions: `Unable to download video subtitles for
//! 'en': HTTP Error 429: Too Many Requests`. Without `--ignore-errors` the fake fails the whole
//! run, as yt-dlp does; with it, it warns and saves the video.
//!
//! One program, three roles (no test harness, `harness = false`):
//! - run as `fake-ytdlp[.exe]` (a copy of itself): it is yt-dlp;
//! - run with `MDM_SUBS_RETRY_SECS` set (the retry's pause, shortened): it runs the checks;
//! - run plainly (`cargo test`): it runs itself again with that variable, the way to set it
//!   without touching its own environment.
//!
//! #769: with `page.json` beside it, the fake's page is that one (its formats' addresses on a lab
//! server, `{EXPIRES}` replaced by a time a little ahead each look-up); asked for its choice
//! (`-O "MDMJ %()j"`) it answers as yt-dlp does after choosing; and asked to download a choice
//! whose streams are already there (MDM's engine downloaded them), it merges them as yt-dlp
//! does ("has already been downloaded"), downloading only what isn't there.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use mdm_tools::video::{State, SubReason, Subs, VideoJob};

fn main() -> ExitCode {
    let me = std::env::current_exe().expect("own path");
    let name = me.file_stem().map(|s| s.to_string_lossy().into_owned());
    if name.as_deref() == Some("fake-ytdlp") {
        return fake_ytdlp();
    }
    if std::env::var_os("MDM_SUBS_RETRY_SECS").is_some() {
        checks(&me);
        job_checks(&me);
        merge_checks(&me);
        list_checks(&me);
        preloaded_checks(&me);
        engine_checks(&me);
        println!("fake_ytdlp: ok");
        return ExitCode::SUCCESS;
    }
    let status = Command::new(&me)
        .env("MDM_SUBS_RETRY_SECS", "1")
        .status()
        .expect("run the checks");
    if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// What the fake does, from a file beside the output: `refuse-always` (English is refused every
/// time) or `refuse-once` (the first time only).
fn mode(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("mode.txt")).unwrap_or_default()
}

/// A process the fake leaves behind (as yt-dlp's launcher leaves its child, and ffmpeg): it
/// writes to `file` every 100 ms for 30 s, unless ended.
fn linger(file: &Path) -> ExitCode {
    use std::io::Write;
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
        {
            let _ = f.write_all(b"x");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    ExitCode::SUCCESS
}

/// Raw bytes on stdout (a line in another code page than UTF-8, #79).
fn out_bytes(b: &[u8]) {
    use std::io::Write;
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(b);
    let _ = o.flush();
}

/// The page the fake's look-up (`-J`) gives.
const PAGE: &str = r#"{"title": "Talk", "language": "ru", "formats": [
    {"format_id": "18", "ext": "mp4", "height": 360, "vcodec": "avc1", "acodec": "mp4a"}],
    "automatic_captions": {"en": [{"name": "English"}], "ru": [{"name": "Russian"}]}}"#;

fn fake_ytdlp() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--linger") {
        return linger(Path::new(&args[1]));
    }
    let after = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has = |flag: &str| args.iter().any(|a| a == flag);
    // #69: `https://list.example/playlist/<n>` (its size said) or `.../channel/<n>` (not).
    if let Some(list) = args
        .last()
        .and_then(|u| u.strip_prefix("https://list.example/"))
    {
        return fake_list(list, has("-J"), after("--playlist-items"));
    }
    // #352: yt-dlp's answer when the cookies leave it nothing to offer.
    if args
        .last()
        .is_some_and(|u| u == "https://cookies.example/none")
        && has("--cookies")
    {
        eprintln!(
            "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats"
        );
        return ExitCode::FAILURE;
    }
    let home = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_owned))
        .expect("the fake's folder");
    if has("-J") {
        // Each look-up counted beside the fake: the quality menu's is taken instead (A2).
        let file = home.join("looked.txt");
        let n = std::fs::read_to_string(&file)
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let _ = std::fs::write(&file, (n + 1).to_string());
        // #852: YouTube looked up without a JavaScript runtime, as yt-dlp says it.
        if args
            .last()
            .is_some_and(|u| u == "https://nojs.example/watch")
        {
            eprint!(
                "{}",
                include_str!("fixtures/ytdlp-js-runtime/no-runtime.stderr")
            );
        }
        // #769: a page of the test's, its links signed for a little while from now.
        match std::fs::read_to_string(home.join("page.json")) {
            Ok(page) => {
                let ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                println!("{}", page.replace("{EXPIRES}", &(ms + 1500).to_string()));
            }
            Err(_) => println!("{PAGE}"),
        }
        return ExitCode::SUCCESS;
    }
    // #769: the choice (`-O "MDMJ %()j"`), as yt-dlp prints it after choosing the formats.
    if after("-O").is_some_and(|o| o.starts_with("MDMJ")) {
        let info =
            std::fs::read_to_string(after("--load-info-json").expect("a page")).expect("the page");
        let info: serde_json::Value = serde_json::from_str(&info).expect("json");
        let template = after("-o").expect("-o").replace("%%", "%");
        let chosen = choice(&info, &after("-f").unwrap_or_default(), &template);
        println!("MDMJ {chosen}");
        return ExitCode::SUCCESS;
    }
    let template = after("-o").expect("-o");
    let dir = Path::new(&template).parent().expect("a folder").to_owned();
    let mode = mode(&dir);
    // The page it was given to download from, kept for the checks to read.
    if let Some(info) = after("--load-info-json") {
        let _ = std::fs::copy(info, dir.join("loaded.txt"));
    }
    let tries = dir.join("tries.txt");
    let tried = std::fs::read_to_string(&tries)
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok())
        .unwrap_or(0);
    std::fs::write(&tries, (tried + 1).to_string()).unwrap();
    let format = after("-f").unwrap_or_default();
    // #94: the chosen height isn't offered (the reroute left it out); a fallback is.
    if mode == "unavailable" && format.contains("[height=") {
        eprintln!(
            "ERROR: [youtube] abc: Requested format is not available. Use --list-formats for a list of available formats"
        );
        return ExitCode::FAILURE;
    }
    // #94: a media server that resolved when looked up doesn't when downloading, once.
    if mode == "dns-once" && tried == 0 {
        eprintln!(
            "ERROR: [download] Got error: HTTPSConnection(host='rr1---sn-dead.googlevideo.com', port=443): Failed to resolve 'rr1---sn-dead.googlevideo.com' ([Errno 11004] getaddrinfo failed). Giving up after 10 retries"
        );
        return ExitCode::FAILURE;
    }
    // #736: a video and its sound, merged as yt-dlp's ffmpeg does: into `<stem>.temp.mp4`,
    // growing for a while, then renamed. Said first when asked (`post_process:` print). Halfway,
    // the file is empty for a while (written again from the start): what is said of the merge
    // must not go back with it. Whole, it stays a while before the rename, long enough to be
    // read on a slow machine.
    if mode == "merge" {
        use std::io::Write;
        println!("MDMQ 137+140 1920x1080");
        println!("MDMP 3000 3000 NA 1000 137");
        println!("MDMP 1000 1000 NA 1000 140");
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        if args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMM {video}");
        }
        let temp = video.replace(".mp4", ".temp.mp4");
        let mut file = std::fs::File::create(&temp).unwrap();
        for k in 1..=10u64 {
            if k == 6 {
                file.set_len(0).unwrap();
                std::thread::sleep(Duration::from_millis(600));
                file = std::fs::File::create(&temp).unwrap();
                file.write_all(&[0u8; 2000]).unwrap();
            }
            file.write_all(&[0u8; 400]).unwrap();
            file.flush().unwrap();
            std::thread::sleep(Duration::from_millis(300));
        }
        drop(file);
        std::thread::sleep(Duration::from_millis(1200));
        std::fs::rename(&temp, &video).unwrap();
        println!("MDMF {video}");
        return ExitCode::SUCCESS;
    }
    // A merge that fails (a stream ffmpeg can't read): the streams stay, whole, as yt-dlp leaves
    // them, and the merge's file half written.
    if mode == "merge-fails" {
        println!("MDMQ 137+aud-English 1920x1080");
        println!("MDMP 3000 3000 NA 1000 137");
        println!("MDMP 1000 1000 NA 1000 aud-English");
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        std::fs::write(video.replace(".mp4", ".f137.mp4"), [0u8; 3000]).unwrap();
        // An HLS sound, named as yt-dlp names its rendition.
        std::fs::write(video.replace(".mp4", ".faud-English.mp4"), [0u8; 1000]).unwrap();
        if args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMM {video}");
        }
        std::fs::write(video.replace(".mp4", ".temp.mp4"), [0u8; 1200]).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        eprintln!("ERROR: Postprocessing: Conversion failed!");
        return ExitCode::FAILURE;
    }
    println!("MDMP 1024 4096 NA 1000 18");
    println!("MDMQ 18 640x360");
    // #79: it falls over mid-way, leaving a helper running (Windows: the launcher's child).
    if mode == "crash" {
        let me = std::env::current_exe().unwrap();
        let _ = Command::new(me)
            .args(["--linger", &dir.join("linger.log").to_string_lossy()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        std::thread::sleep(Duration::from_millis(500));
        eprintln!("ERROR: unable to download video data: HTTP Error 500");
        return ExitCode::FAILURE;
    }
    // #79: lines that aren't UTF-8, before and as the final path (a legacy code page).
    if mode == "odd-lines" {
        out_bytes(b"\xff\xfe [download] Destination: \xde\xfd.mp4\n");
        println!("not one of our lines");
        println!("MDMP NA NA NA NA");
    }
    // #79: silent for a while, as when ffmpeg merges a long video.
    if let Some(secs) = mode.strip_prefix("quiet:") {
        std::thread::sleep(Duration::from_secs(secs.trim().parse().unwrap_or(1)));
    }
    let langs = after("--sub-langs").unwrap_or_default();
    for lang in langs.split(',').filter(|l| !l.is_empty()) {
        let refused = lang == "en" && (mode == "refuse-always" || tried == 0);
        if refused {
            let why =
                "Unable to download video subtitles for 'en': HTTP Error 429: Too Many Requests";
            if has("--ignore-errors") {
                eprintln!("WARNING: {why}");
                continue;
            }
            eprintln!("ERROR: {why}");
            return ExitCode::FAILURE;
        }
        let file = template.replace("%(ext)s", &format!("{lang}.srt"));
        std::fs::write(file, "1\n00:00:00,000 --> 00:00:01,000\nHello\n").unwrap();
    }
    // #569: the format asked for, for the checks.
    std::fs::write(dir.join("format.txt"), &format).unwrap();
    // #769: a choice whose streams MDM's engine downloaded: merged as yt-dlp merges them.
    if let Some(info) = after("--load-info-json")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|i| i["formats"][0]["url"].is_string())
        && !has("--skip-download")
    {
        let template = template.replace("%%", "%");
        let chosen = choice(&info, &format, &template);
        let output = chosen["filename"].as_str().unwrap().to_owned();
        let streams: Vec<String> = match chosen["requested_formats"].as_array() {
            Some(fs) => fs
                .iter()
                .map(|f| {
                    output.replace(
                        ".mp4",
                        &format!(
                            ".f{}.{}",
                            f["format_id"].as_str().unwrap(),
                            f["ext"].as_str().unwrap()
                        ),
                    )
                })
                .collect(),
            None => vec![output.clone()],
        };
        let mut merged = Vec::new();
        for s in &streams {
            match std::fs::read(s) {
                // "has already been downloaded"
                Ok(b) => merged.extend(b),
                Err(_) => merged.extend(b"downloaded by yt-dlp"),
            }
        }
        for s in streams.iter().filter(|s| **s != output) {
            let _ = std::fs::remove_file(s);
        }
        // #736: several streams are merged as yt-dlp's ffmpeg does, said first when asked
        // (`post_process:` print), into `<stem>.temp.mp4`, growing for a while, then renamed.
        if streams.len() > 1 && args.iter().any(|a| a.starts_with("post_process:MDMM")) {
            println!("MDMQ {} 1920x1080", chosen["format_id"].as_str().unwrap());
            println!("MDMM {output}");
            let temp = output.replace(".mp4", ".temp.mp4");
            let step = merged.len().div_ceil(5);
            for k in 1..=5 {
                std::fs::write(&temp, &merged[..(k * step).min(merged.len())]).unwrap();
                std::thread::sleep(Duration::from_millis(300));
            }
            std::fs::rename(&temp, &output).unwrap();
        } else {
            std::fs::write(&output, merged).unwrap();
        }
        println!("MDMF {output}");
        return ExitCode::SUCCESS;
    }
    if !has("--skip-download") {
        let video = template.replace("%(ext)s", "mp4").replace("%%", "%");
        // #569: YouTube's HLS sound joined without ffmpeg, as yt-dlp leaves it: pieces each led
        // by an ID3 tag, then raw AAC (ADTS) frames, under the format's `.mp4` name.
        let content = if mode == "hls-audio" {
            let mut piece = b"ID3     ".to_vec();
            piece.extend([0u8; 16]);
            piece.extend([0xff, 0xf1, 0x50, 0x80, 0x2e, 0x7f, 0xfc]);
            piece.repeat(64)
        } else {
            vec![0u8; 4096]
        };
        std::fs::write(&video, content).unwrap();
        if mode == "odd-lines" {
            // The path in a one-byte code page: every letter beyond ASCII one byte, not UTF-8.
            let mut line = b"MDMF ".to_vec();
            line.extend(
                video
                    .chars()
                    .map(|c| if c.is_ascii() { c as u8 } else { 0xde }),
            );
            line.push(b'\n');
            out_bytes(&line);
        } else {
            println!("MDMF {video}");
        }
    }
    ExitCode::SUCCESS
}

/// yt-dlp's choice of formats on page `info` for `selector`: a format id the page has, that one;
/// else every format of the page merged (two or more), or the only one. With `filename` from the
/// output `template`.
fn choice(info: &serde_json::Value, selector: &str, template: &str) -> serde_json::Value {
    let formats = info["formats"].as_array().cloned().unwrap_or_default();
    let mut chosen = info.clone();
    let one = formats
        .iter()
        .find(|f| f["format_id"].as_str() == Some(selector))
        .or((formats.len() == 1).then(|| &formats[0]));
    match one {
        Some(f) => {
            for (k, v) in f.as_object().unwrap() {
                chosen[k] = v.clone();
            }
        }
        None => {
            chosen["requested_formats"] = serde_json::Value::Array(formats.clone());
            chosen["ext"] = "mp4".into();
            chosen["format_id"] = formats
                .iter()
                .map(|f| f["format_id"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
                .join("+")
                .into();
        }
    }
    let ext = chosen["ext"].as_str().unwrap_or("mp4").to_owned();
    chosen["filename"] = template.replace("%(ext)s", &ext).into();
    chosen
}

/// A list of `n` videos as yt-dlp lists them: `-J` (flat, one entry) says what it is; flat
/// `-j` prints the entries in `--playlist-items`' range, a line each, a little at a time (as
/// YouTube's pages of 100 come). Every 7th is private.
fn fake_list(list: &str, whole: bool, items: Option<String>) -> ExitCode {
    let (kind, n) = list.split_once('/').unwrap_or((list, "0"));
    // `?fail=<i>`: the site stops answering at entry i (on a listing from the start only: listed
    // again from there, as after a pause, it goes on).
    let (n, fail) = match n.split_once("?fail=") {
        Some((n, f)) => (n, f.parse::<u64>().ok()),
        None => (n, None),
    };
    let n: u64 = n.parse().unwrap_or(0);
    let count = if kind == "playlist" {
        format!(r#", "playlist_count": {n}"#)
    } else {
        String::new()
    };
    if whole {
        println!(r#"{{"_type": "playlist", "title": "A fake {kind}"{count}, "entries": []}}"#);
        return ExitCode::SUCCESS;
    }
    let (from, to) = items
        .as_deref()
        .and_then(|r| r.split_once(':'))
        .map(|(a, b)| (a.parse().unwrap_or(1), b.parse().unwrap_or(n)))
        .unwrap_or((1, n));
    for i in from..=to.min(n) {
        if fail == Some(i) && from == 1 {
            eprintln!(
                "ERROR: [youtube:tab] Unable to download API page: HTTP Error 429: Too Many Requests"
            );
            return ExitCode::FAILURE;
        }
        let title = if i % 7 == 0 {
            "[Private video]".to_owned()
        } else {
            format!("Video {i}")
        };
        println!(
            r#"{{"_type": "url", "url": "https://video.example/v{i}", "title": "{title}", "duration": {}, "playlist_index": {i}, "playlist_title": "A fake {kind}"{count}}}"#,
            60 + i % 600
        );
        if i % 100 == 0 {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    ExitCode::SUCCESS
}

/// The fake, as a copy of this program named `fake-ytdlp` in `dir`.
fn fake_in(me: &Path, dir: &Path) -> PathBuf {
    let exe = dir.join(format!("fake-ytdlp{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(me, &exe).expect("copy the fake");
    exe
}

/// Download with `subs` while the fake behaves as `mode`; the end state, its log and folder.
fn download(me: &Path, mode: &str, subs: &Subs) -> (mdm_tools::video::Progress, Vec<String>) {
    let (p, files, _out) = download_as(me, mode, subs, "b", "Talk");
    (p, files)
}

/// The same with a format `selector` and a file `stem`; also hands back the output folder.
fn download_as(
    me: &Path,
    mode: &str,
    subs: &Subs,
    selector: &str,
    stem: &str,
) -> (mdm_tools::video::Progress, Vec<String>, tempfile::TempDir) {
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("mode.txt"), mode).unwrap();
    let exe = fake_in(me, tools.path());
    let job = VideoJob::start(
        &exe,
        "https://video.example/talk",
        selector,
        out.path(),
        stem,
        None,
        None,
        None,
        &Default::default(),
        subs,
        None,
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(120);
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        assert!(Instant::now() < end, "still running: {p:?}");
        std::thread::sleep(Duration::from_millis(100));
    };
    let mut files: Vec<String> = std::fs::read_dir(out.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.ends_with(".txt") && !n.ends_with(".log"))
        .collect();
    files.sort();
    (p, files, out)
}

fn log_of(p: &mdm_tools::video::Progress) -> Vec<String> {
    p.events.iter().map(|(_, t)| t.clone()).collect()
}

/// #736: while yt-dlp's ffmpeg joins a video and its sound, the job says so, and how far it is
/// from the joined file's size; it says no more once the file is there.
fn merge_checks(me: &Path) {
    let tools = tempfile::tempdir().unwrap();
    let out = tempfile::tempdir().unwrap();
    std::fs::write(out.path().join("mode.txt"), "merge").unwrap();
    let exe = fake_in(me, tools.path());
    let job = VideoJob::start(
        &exe,
        "https://video.example/talk",
        "bv*+ba",
        out.path(),
        "Talk",
        None,
        None,
        None,
        &Default::default(),
        &Subs::default(),
        None,
    )
    .expect("started");
    let end = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    let p = loop {
        let p = job.progress();
        if p.state != State::Running {
            break p;
        }
        if let Some(m) = p.merging {
            seen.push(m);
        }
        assert!(Instant::now() < end, "still running: {p:?}");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(matches!(p.state, State::Done(_)), "merge: {:?}", p.state);
    assert_eq!(p.merging, None, "done: no longer merging");
    let known: Vec<f64> = seen.iter().filter_map(|m| *m).collect();
    assert!(!seen.is_empty(), "merging was said");
    assert!(
        known.iter().any(|f| *f > 0.0 && *f < 1.0),
        "a share done was said: {seen:?}"
    );
    // Never back, though the file was empty a while halfway; and on to its whole size, 4000 bytes
    // of the streams' 4000, held short of the end until the rename (99 %).
    assert!(
        known.windows(2).all(|w| w[0] <= w[1]),
        "it only goes forward: {known:?}"
    );
    assert_eq!(known.last(), Some(&0.99), "it reaches the end: {known:?}");

    // Joining them fails: the video fails at once (no other look-up or server mends it), says
    // why, keeps the streams for Resume and leaves nothing half joined.
    let (p, files, out) = download_as(me, "merge-fails", &Subs::default(), "bv*+ba", "Talk");
    match &p.state {
        State::Failed(e) => assert!(e.contains("Conversion failed"), "{e}"),
        other => panic!("merge fails: {other:?}"),
    }
    assert_eq!(p.merging, None);
    let kept = ["Talk.f137.mp4", "Talk.faud-English.mp4"];
    assert_eq!(files, kept, "the streams stay, the merge's file goes");
    let named: Vec<String> = p
        .merge_failed
        .expect("said to be the merge")
        .iter()
        .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(named, kept);
    assert_eq!(
        std::fs::read_to_string(out.path().join("tries.txt")).unwrap(),
        "1",
        "one run"
    );
}

/// #79 and #94: the job never gives up while yt-dlp works, never leaves it running when it does
/// give up, and mends what a new look-up or a nearby quality can mend.
fn job_checks(me: &Path) {
    let none = Subs::default();

    // Silent for 30 s (ffmpeg merging a long video): no timeout of MDM's own ends it.
    let (p, files, _o) = download_as(me, "quiet:30", &none, "b", "Talk");
    assert!(matches!(p.state, State::Done(_)), "quiet: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);

    // Lines that aren't UTF-8, the final path among them: read through, the file found by name.
    let (p, files, _o) = download_as(me, "odd-lines", &none, "b", "Şirin ğüş");
    match &p.state {
        State::Done(f) => assert_eq!(f.file_name().unwrap().to_string_lossy(), "Şirin ğüş.mp4"),
        other => panic!("odd lines: {other:?}"),
    }
    assert_eq!(files, ["Şirin ğüş.mp4"]);

    // #569: no ffmpeg (none given here), the sound alone: yt-dlp is asked for one that comes as
    // a file first, and what it joined from HLS pieces is saved for what it is, raw AAC.
    let (p, files, out) = download_as(me, "hls-audio", &none, "ba", "Talk");
    match &p.state {
        State::Done(f) => assert_eq!(f.file_name().unwrap().to_string_lossy(), "Talk.aac"),
        other => panic!("hls audio: {other:?}"),
    }
    assert_eq!(files, ["Talk.aac"]);
    assert_eq!(
        std::fs::read_to_string(out.path().join("format.txt")).unwrap(),
        "ba[protocol!*=m3u8]/ba"
    );
    // A file that is what it's named: as it is.
    let (p, files, _o) = download_as(me, "", &none, "ba", "Talk");
    assert!(matches!(p.state, State::Done(_)), "plain: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);

    // It falls over mid-way: the failure is told, and what it left running is ended with it
    // (Windows: the job; elsewhere yt-dlp is one process, so there's no child to leave).
    let (p, _files, out) = download_as(me, "crash", &none, "b", "Talk");
    match &p.state {
        State::Failed(e) => assert!(e.contains("HTTP Error 500"), "{e}"),
        other => panic!("crash: {other:?}"),
    }
    #[cfg(windows)]
    {
        let size = || std::fs::metadata(out.path().join("linger.log")).map_or(0, |m| m.len());
        std::thread::sleep(Duration::from_millis(800));
        let after = size();
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(
            size(),
            after,
            "the helper it left was ended with the download"
        );
    }
    #[cfg(not(windows))]
    let _ = out;

    // The chosen height isn't offered (a reroute left it out): the nearest below, said so.
    let (p, files, _o) = download_as(
        me,
        "unavailable",
        &none,
        "bv*[height=1080]+ba/b[height=1080]",
        "Talk",
    );
    assert!(
        matches!(p.state, State::Done(_)),
        "unavailable: {:?}",
        p.state
    );
    assert_eq!(files, ["Talk.mp4"]);
    let log = log_of(&p);
    assert!(
        log.iter()
            .any(|t| t.starts_with("The chosen quality isn't offered")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|t| t == "Downloaded 18 640x360 instead"),
        "{log:?}"
    );

    // A media server that stopped resolving: looked up again without it, then saved.
    let (p, files, _o) = download_as(me, "dns-once", &none, "b", "Talk");
    assert!(matches!(p.state, State::Done(_)), "dns: {:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);
    let log = log_of(&p);
    assert!(
        log.iter().any(|t| t
            == "The server rr1---sn-dead.googlevideo.com couldn't be reached; looking the page up again without it"),
        "{log:?}"
    );
}

/// A2: the download chosen in the browser's quality menu takes the menu's look-up (the page yt-dlp
/// read for it) in its first run, without asking yt-dlp to read the page again; with the note
/// when that look-up had to leave the cookies out. A look-up that doesn't offer the choice made,
/// a failed one, and every run after the first read the page afresh.
fn preloaded_checks(me: &Path) {
    use mdm_tools::video::{InfoSlot, Preloaded};
    let run = |mode: &str, preloaded: Option<Preloaded>| {
        let tools = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(out.path().join("mode.txt"), mode).unwrap();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start_with(
            &exe,
            "https://video.example/talk",
            "18/b[height=360]",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            preloaded,
            None,
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let p = loop {
            let p = job.progress();
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running: {p:?}");
            std::thread::sleep(Duration::from_millis(100));
        };
        let looked = std::fs::read_to_string(tools.path().join("looked.txt"))
            .ok()
            .and_then(|t| t.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let loaded = std::fs::read_to_string(out.path().join("loaded.txt")).unwrap_or_default();
        (p, looked, loaded)
    };
    let menu_of = |page: &str, label: &str, without_cookies: Option<bool>| {
        let slot = InfoSlot::new(Instant::now());
        match without_cookies {
            Some(w) => slot.fill(page.as_bytes().to_vec(), w),
            None => slot.fail(),
        }
        Some(Preloaded {
            slot,
            selector: "18/b[height=360]".into(),
            label: label.into(),
            merge: false,
        })
    };
    let menu = |label: &str, without_cookies: Option<bool>| menu_of(PAGE, label, without_cookies);
    let note =
        "The site offered no format with the browser's cookies; looked the page up without them";

    let (p, looked, _) = run("", menu("360p · H.264", Some(true)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 0, "the menu's look-up, not another");
    assert!(log_of(&p).iter().any(|t| t == note), "{:?}", log_of(&p));

    // Its own look-up when there's none, as before.
    let (p, looked, _) = run("", None);
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 1);

    // The menu's look-up doesn't offer that choice under that label, or failed: afresh.
    for menu in [
        menu("720p · H.264", Some(false)),
        menu("360p · H.264", None),
    ] {
        let (p, looked, _) = run("", menu);
        assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
        assert_eq!(looked, 1);
    }

    // The first run fails (a server stopped resolving): the second reads the page afresh.
    let (p, looked, _) = run("dns-once", menu("360p · H.264", Some(false)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 1, "the retry's");

    // The menu's look-up names a media server this computer can't reach (#94): its links are
    // moved to the alternative the site lists, as a fresh look-up's would be, and yt-dlp
    // downloads from that list without reading the page again. The servers are the test's
    // (`check_servers_with`): none is ever asked.
    mdm_tools::video::check_servers_with(
        |host| host.contains("sn-alive"),
        |link| link.contains("sn-alive"),
    );
    let dead = "https://rr1---sn-dead.googlevideo.com/videoplayback?id=1&mn=sn-dead%2Csn-alive";
    let page = PAGE.replace(
        r#""acodec": "mp4a"}"#,
        &format!(r#""acodec": "mp4a", "protocol": "https", "url": "{dead}"}}"#),
    );
    assert!(page.contains(dead));
    let (p, looked, loaded) = run("", menu_of(&page, "360p · H.264", Some(false)));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(looked, 0, "the menu's look-up, moved");
    assert!(
        loaded.contains("rr1---sn-alive.googlevideo.com") && !loaded.contains("rr1---sn-dead."),
        "{loaded}"
    );
    assert!(
        log_of(&p).iter().any(|t| t
            == "The server rr1---sn-dead.googlevideo.com can't be reached; downloading from rr1---sn-alive.googlevideo.com"),
        "{:?}",
        log_of(&p)
    );
}

/// #69: a list is told from a video, listed whole as it comes (thousands, in order), a channel a
/// page at a time, and a listing stops when asked, ending yt-dlp.
fn list_checks(me: &Path) {
    use mdm_tools::playlist::{Listing, PAGE};
    use mdm_tools::resolve::{Looked, look_up};
    let tools = tempfile::tempdir().unwrap();
    let exe = fake_in(me, tools.path());
    let none = Default::default();
    let wait = |l: &Listing| {
        let end = Instant::now() + Duration::from_secs(60);
        loop {
            let p = l.progress();
            if p.done {
                return p;
            }
            assert!(Instant::now() < end, "still listing: {p:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    match look_up(&exe, "https://list.example/playlist/6000", &none).unwrap() {
        Looked::List(l) => assert_eq!((l.title.as_str(), l.total), ("A fake playlist", Some(6000))),
        Looked::Video(_) => panic!("a playlist taken for a video"),
    }
    // #852: yt-dlp's warnings reach the look-up. A video looked up without a JavaScript runtime
    // says so (yt-dlp's own words, `fixtures/ytdlp-js-runtime`); another doesn't.
    let runtime_said = |url: &str| match look_up(&exe, url, &none).unwrap() {
        Looked::Video(v) => v.needs_js_runtime,
        Looked::List(_) => panic!("a video taken for a list"),
    };
    assert!(!runtime_said("https://video.example/talk"));
    assert!(runtime_said("https://nojs.example/watch"));

    // #352: cookies that leave yt-dlp nothing to offer: the same page without them.
    let page = "https://cookies.example/none";
    let cookies = mdm_tools::proc::FromBrowser {
        cookies: Some("LOGIN_INFO=l; SAPISID=s".into()),
        ..Default::default()
    };
    assert!(matches!(
        look_up(&exe, page, &cookies).unwrap(),
        Looked::Video(_)
    ));
    let (args, _file) = cookies.args(page).unwrap();
    let out =
        mdm_tools::proc::look_up(&exe, &["-J".to_owned()], page, Duration::from_secs(30)).unwrap();
    assert!(!out.without_cookies, "no cookies, so nothing to retry");
    let first = ["-J".to_owned()]
        .into_iter()
        .chain(args)
        .collect::<Vec<_>>();
    let out = mdm_tools::proc::look_up(&exe, &first, page, Duration::from_secs(30)).unwrap();
    assert!(out.without_cookies, "the second look-up, without them");

    // Six thousand, whole: all of them, in order, the private ones marked.
    let t = Instant::now();
    let l = Listing::start(&exe, "https://list.example/playlist/6000", &none, 1, None).unwrap();
    let p = wait(&l);
    let all = l.entries_from(0);
    assert_eq!((all.len(), p.total, p.error), (6000, Some(6000), None));
    assert!(all.iter().enumerate().all(|(i, e)| e.index == i as u64 + 1));
    assert!(all[6].unavailable && !all[7].unavailable);
    assert_eq!(l.entries_from(5990).len(), 10);
    println!("fake_ytdlp: 6000 listed in {:?}", t.elapsed());

    // A channel (no size said): a page, then the next from where it stopped, then the end.
    let url = "https://list.example/channel/120";
    let l = Listing::start(&exe, url, &none, 1, Some(PAGE)).unwrap();
    let p = wait(&l);
    assert_eq!((l.entries_from(0).len(), p.total), (PAGE as usize, None));
    let l = Listing::start(&exe, url, &none, 101, Some(150)).unwrap();
    wait(&l);
    let last = l.entries_from(0);
    assert_eq!((last.len(), last[0].index), (20, 101));

    // The site stops answering part-way: what was listed stays, and why it stopped is told;
    // listed again from the next one, it goes on to the end.
    let url = "https://list.example/playlist/300?fail=121";
    let l = Listing::start(&exe, url, &none, 1, None).unwrap();
    let p = wait(&l);
    assert_eq!(l.entries_from(0).len(), 120);
    assert!(p.error.as_deref().unwrap_or("").contains("429"), "{p:?}");
    let l = Listing::start(&exe, url, &none, 121, None).unwrap();
    let p = wait(&l);
    let rest = l.entries_from(0);
    assert_eq!((rest.len(), rest[0].index, p.error), (180, 121, None));

    // Stopped part-way: what was listed stays, and it's done at once.
    let l = Listing::start(&exe, "https://list.example/playlist/20000", &none, 1, None).unwrap();
    while l.progress().listed < 150 {
        std::thread::sleep(Duration::from_millis(10));
    }
    l.cancel();
    let p = l.progress();
    let kept = l.entries_from(0).len();
    assert!(p.done && (150..20000).contains(&kept), "{p:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        l.entries_from(0).len(),
        kept,
        "nothing listed after the stop"
    );
}

fn checks(me: &Path) {
    let subs = |langs: &[&str], embed: bool| Subs {
        langs: langs.iter().map(|l| l.to_string()).collect(),
        automatic: true,
        embed,
    };

    // English refused every time: the video and the Russian ones are saved, English is told.
    let (p, files) = download(me, "refuse-always", &subs(&["en", "ru"], false));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.mp4", "Talk.ru.srt"]);
    assert_eq!(p.sub_problems.len(), 1, "{:?}", p.sub_problems);
    let problem = &p.sub_problems[0];
    assert_eq!(
        (
            problem.code.as_str(),
            problem.name.as_str(),
            &problem.reason,
            problem.beside
        ),
        ("en", "English", &SubReason::TooManyRequests, false)
    );
    let log: Vec<&str> = p.events.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        log.iter()
            .any(|t| t.starts_with("Subtitles in en couldn't be downloaded")),
        "{log:?}"
    );

    // Refused once only: the second try, after the pause, brings them.
    let (p, files) = download(me, "refuse-once", &subs(&["en"], false));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.en.srt", "Talk.mp4"]);
    assert!(p.sub_problems.is_empty(), "{:?}", p.sub_problems);
    assert_eq!(
        p.side_files
            .iter()
            .map(|f| f.file_name().unwrap().to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["Talk.en.srt"]
    );

    // Asked into the video (no ffmpeg here, so beside it): still never the video's failure.
    let (p, files) = download(me, "refuse-always", &subs(&["en"], true));
    assert!(matches!(p.state, State::Done(_)), "{:?}", p.state);
    assert_eq!(files, ["Talk.mp4"]);
    assert_eq!(p.sub_problems.len(), 1);
}

/// #769: MDM's engine downloads what yt-dlp chose, from a lab server: each stream over several
/// connections, within yt-dlp's `http_chunk_size`, into the files yt-dlp then merges; a stopped
/// one goes on from where it stopped; an address that expires is asked of yt-dlp again; a new
/// speed limit applies at once; and a stream in pieces stays yt-dlp's to download, said so.
fn engine_checks(me: &Path) {
    use mdm_labserver::{Config, LabServer, content};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let lab = rt.block_on(LabServer::start(Config::default())).unwrap();
    let engine = |connections: u32| mdm_tools::streams::Engine {
        handle: rt.handle().clone(),
        opts: mdm_engine::Options {
            max_connections: connections,
            retry_delay: Duration::from_millis(50),
            ..Default::default()
        },
    };
    const V: u64 = 3 << 20;
    const A: u64 = 1 << 20;
    let page = |video: &str, audio: &str, protocol: &str| {
        format!(
            r#"{{"id": "talk", "title": "Talk", "ext": "mp4", "formats": [
            {{"format_id": "137", "ext": "mp4", "protocol": "{protocol}", "filesize": {V},
              "vcodec": "avc1", "acodec": "none", "url": "{}",
              "http_headers": {{"User-Agent": "FakeTube/1"}},
              "downloader_options": {{"http_chunk_size": 524288}}}},
            {{"format_id": "140", "ext": "m4a", "protocol": "https", "filesize": {A},
              "vcodec": "none", "acodec": "mp4a", "url": "{}"}}]}}"#,
            lab.url(video),
            lab.url(audio)
        )
    };
    let expected = || {
        let mut v = vec![0; V as usize];
        content::fill(1, 0, &mut v);
        let mut a = vec![0; A as usize];
        content::fill(2, 0, &mut a);
        v.extend(a);
        v
    };
    let start = |tools: &Path, out: &Path, connections: u32| {
        let exe = fake_in(me, tools);
        VideoJob::start(
            &exe,
            "https://video.example/talk",
            "bv*+ba/b",
            out,
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine(connections)),
        )
        .expect("started")
    };
    // What each run said of its merge (#736), in order.
    let merges = std::cell::RefCell::new(Vec::new());
    let wait = |job: &VideoJob| {
        let end = Instant::now() + Duration::from_secs(120);
        let mut most_connections = 0;
        merges.borrow_mut().clear();
        loop {
            let p = job.progress();
            if let Some(e) = &p.engine {
                most_connections = most_connections.max(e.connections.len());
            }
            if let Some(m) = p.merging {
                merges.borrow_mut().push(m);
            }
            if p.state != State::Running {
                return (p, most_connections);
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        }
    };

    // Merged: two streams, each over several connections, in requests of 512 KB at most.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=8M",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 4);
    let (p, most) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(p.helper.is_none());
    let saved = out.path().join("Talk.mp4");
    assert!(
        std::fs::read(&saved).unwrap() == expected(),
        "the streams merged"
    );
    assert!(most >= 2, "several connections: {most}");
    // #736: the engine downloaded the streams, yt-dlp's ffmpeg merges them: said, with how far.
    let known: Vec<f64> = merges.borrow().iter().filter_map(|m| *m).collect();
    assert!(
        known.iter().any(|f| *f > 0.0 && *f < 1.0),
        "the merge and its share done were said: {:?}",
        merges.borrow()
    );
    assert!(known.windows(2).all(|w| w[0] <= w[1]), "{known:?}");
    assert_eq!(p.merging, None, "done: no longer merging");
    assert!(
        log.iter()
            .any(|t| t == "yt-dlp's address for format 137: from 127.0.0.1"),
        "{log:?}"
    );
    rt.block_on(lab.settled());
    let video: Vec<_> = lab
        .log()
        .into_iter()
        .filter(|e| e.path.starts_with("/gen/3M/v.mp4"))
        .collect();
    assert!(video.iter().all(|e| {
        e.user_agent.as_deref() == Some("FakeTube/1")
            && e.range.as_deref().is_some_and(|r| {
                let (a, b) = r.trim_start_matches("bytes=").split_once('-').unwrap();
                b.parse::<u64>().unwrap() - a.parse::<u64>().unwrap() < 524288
            })
    }));
    assert!(
        std::fs::read_dir(out.path())
            .unwrap()
            .filter_map(Result::ok)
            .all(|e| !e.file_name().to_string_lossy().contains(".mdm.")),
        "no part file or state left"
    );

    // Stopped half-way, then started again: it goes on from the bytes already there.
    lab.clear_log();
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=1M",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    while job.progress().downloaded < 1 << 20 {
        assert!(job.progress().state == State::Running);
        std::thread::sleep(Duration::from_millis(20));
    }
    // A new limit applies at once: 256 KB/s, where the server gives 2 MB/s over two.
    job.set_speed_limit(Some(256 * 1024));
    std::thread::sleep(Duration::from_millis(500));
    let (a, t) = (job.progress().downloaded, Instant::now());
    std::thread::sleep(Duration::from_secs(2));
    let rate = (job.progress().downloaded - a) as f64 / t.elapsed().as_secs_f64();
    assert!(rate < 512.0 * 1024.0, "the limit holds: {rate:.0} B/s");
    job.stop();
    let (p, _) = wait(&job);
    assert_eq!(p.state, State::Stopped);
    let state = mdm_engine::state::state_path(&out.path().join("Talk.f137.mp4"));
    assert!(state.exists(), "the resume state is kept");
    let job = start(tools.path(), out.path(), 2);
    job.set_speed_limit(None);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "resumed whole"
    );
    assert!(log.iter().any(|t| t.starts_with("Resuming: ")), "{log:?}");

    // Addresses that expire after 1.5 s, on connections dropped every 512 KB at 512 KB/s:
    // yt-dlp is asked for today's, and the stream goes on.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page(
            "/gen/3M/v.mp4?seed=1&rate=512K&drop_after=512K&expires={EXPIRES}",
            "/gen/1M/a.m4a?seed=2",
            "https",
        ),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert!(
        std::fs::read(out.path().join("Talk.mp4")).unwrap() == expected(),
        "whole across new addresses"
    );
    assert!(
        log.iter()
            .any(|t| t == "Asking yt-dlp for a new address for format 137"),
        "{log:?}"
    );

    // A protocol the engine doesn't speak: yt-dlp downloads the choice itself, and says why.
    let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(
        tools.path().join("page.json"),
        page("/gen/3M/v.mp4?seed=1", "/gen/1M/a.m4a?seed=2", "rtmp"),
    )
    .unwrap();
    let job = start(tools.path(), out.path(), 2);
    let (p, _) = wait(&job);
    let log = log_of(&p);
    assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
    assert_eq!(
        p.helper,
        Some(mdm_tools::streams::Helper::Protocol("rtmp".into()))
    );
    assert!(
        log.iter()
            .any(|t| t == "yt-dlp downloads this video itself: its protocol (rtmp)"),
        "{log:?}"
    );
    pieces_checks(me, &rt, &lab, &engine, &expected_audio());
    println!("fake_ytdlp: the engine's checks ok");
}

/// The sound every page of the engine's checks has (format 140, the lab's `/gen/1M/a.m4a`).
fn expected_audio() -> Vec<u8> {
    let mut a = vec![0; 1 << 20];
    mdm_labserver::content::fill(2, 0, &mut a);
    a
}

/// #769: streams in pieces through the engine: an HLS playlist (plain, and AES-128 with the
/// playlist's key), DASH's pieces as yt-dlp lists them, and pieces whose addresses expire,
/// asked of yt-dlp again. Each comes whole, in order, joined as yt-dlp's own downloaders join
/// them, then merged with the sound.
fn pieces_checks(
    me: &Path,
    rt: &tokio::runtime::Runtime,
    lab: &mdm_labserver::LabServer,
    engine: &dyn Fn(u32) -> mdm_tools::streams::Engine,
    audio: &[u8],
) {
    let get = |path: &str| -> Vec<u8> {
        let url = lab.url(path);
        rt.block_on(async {
            reqwest::get(url)
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap()
                .to_vec()
        })
    };
    let audio_format = format!(
        r#"{{"format_id": "140", "ext": "m4a", "protocol": "https", "filesize": {},
          "vcodec": "none", "acodec": "mp4a", "url": "{}"}}"#,
        1 << 20,
        lab.url("/gen/1M/a.m4a?seed=2")
    );
    let run = |video: String, connections: u32| {
        let (tools, out) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        std::fs::write(
            tools.path().join("page.json"),
            format!(
                r#"{{"id": "talk", "title": "Talk", "ext": "mp4", "formats": [{video}, {audio_format}]}}"#
            ),
        )
        .unwrap();
        let exe = fake_in(me, tools.path());
        let job = VideoJob::start(
            &exe,
            "https://video.example/talk",
            "bv*+ba/b",
            out.path(),
            "Talk",
            None,
            None,
            None,
            &Default::default(),
            &Subs::default(),
            Some(engine(connections)),
        )
        .expect("started");
        let end = Instant::now() + Duration::from_secs(120);
        let mut most = 0;
        let p = loop {
            let p = job.progress();
            if let Some(e) = &p.engine {
                most = most.max(e.connections.len());
            }
            if p.state != State::Running {
                break p;
            }
            assert!(Instant::now() < end, "still running: {:?}", log_of(&p));
            std::thread::sleep(Duration::from_millis(50));
        };
        let log = log_of(&p);
        assert!(matches!(p.state, State::Done(_)), "{:?}: {log:?}", p.state);
        assert!(p.helper.is_none(), "{:?}", p.helper);
        // Named for what it holds (#569): TS pieces joined without ffmpeg are a `.ts`.
        let State::Done(file) = &p.state else {
            unreachable!()
        };
        let saved = std::fs::read(file).unwrap();
        let left: Vec<String> = std::fs::read_dir(out.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".mdm"))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
        (saved, log, most)
    };
    let hls = |playlist: &str, extra: &str| {
        format!(
            r#"{{"format_id": "hls-720p", "ext": "mp4", "protocol": "m3u8_native",
              "vcodec": "avc1", "acodec": "none", "url": "{}"{extra}}}"#,
            lab.url(playlist)
        )
    };
    let plain: Vec<u8> = (0..3)
        .flat_map(|i| get(&format!("/hls/720p/seg{i}.ts")))
        .chain(audio.iter().copied())
        .collect();

    // HLS: three pieces, two at once, joined in order.
    let (saved, log, most) = run(hls("/hls/720p/index.m3u8", ""), 2);
    assert!(saved == plain, "HLS pieces joined in order");
    assert!(most >= 2, "pieces at once: {most}");
    assert!(
        log.iter()
            .any(|t| t.starts_with("Format hls-720p: 3 pieces, 2 at once")),
        "{log:?}"
    );

    // AES-128: the playlist's key, the first piece's own IV and the others' sequence numbers.
    let (saved, log, _) = run(hls("/hls/720p/enc.m3u8", ""), 3);
    assert!(saved == plain, "decrypted as the plain pieces: {log:?}");

    // DASH: yt-dlp's list (an init section and three pieces, from its base address).
    let dash = format!(
        r#"{{"format_id": "dash-720p", "ext": "mp4", "protocol": "http_dash_segments",
          "vcodec": "avc1", "acodec": "none", "url": "{}",
          "fragment_base_url": "{}",
          "fragments": [{{"path": "init.mp4"}}, {{"path": "seg1.m4s"}}, {{"path": "seg2.m4s"}},
                        {{"url": "{}"}}]}}"#,
        lab.url("/dash/manifest.mpd"),
        lab.url("/dash/720p/"),
        lab.url("/dash/720p/seg3.m4s")
    );
    let want: Vec<u8> = ["init.mp4", "seg1.m4s", "seg2.m4s", "seg3.m4s"]
        .iter()
        .flat_map(|f| get(&format!("/dash/720p/{f}")))
        .chain(audio.iter().copied())
        .collect();
    let (saved, _, _) = run(dash, 4);
    assert!(saved == want, "DASH pieces joined in order");

    // Pieces whose addresses expire 1.5 s after each look-up, one at a time at 256 KB/s: yt-dlp
    // is asked for the list again, and the pieces go on from the new one.
    let pieces: Vec<String> = (1..=4)
        .map(|i| {
            format!(
                r#"{{"url": "{}"}}"#,
                lab.url(&format!(
                    "/gen/256K/p{i}?seed={}&rate=256K&expires={{EXPIRES}}",
                    20 + i
                ))
            )
        })
        .collect();
    let expiring = format!(
        r#"{{"format_id": "dash-exp", "ext": "mp4", "protocol": "http_dash_segments",
          "vcodec": "avc1", "acodec": "none", "url": "{}", "fragments": [{}]}}"#,
        lab.url("/dash/manifest.mpd"),
        pieces.join(", ")
    );
    let want: Vec<u8> = (1..=4u64)
        .flat_map(|i| {
            let mut b = vec![0; 256 * 1024];
            mdm_labserver::content::fill(20 + i, 0, &mut b);
            b
        })
        .chain(audio.iter().copied())
        .collect();
    let (saved, log, _) = run(expiring, 1);
    let first = saved.iter().zip(&want).position(|(a, b)| a != b);
    assert!(
        saved == want,
        "whole across new lists: {} vs {} bytes, first difference at {first:?}: {log:?}",
        saved.len(),
        want.len()
    );
    assert!(
        log.iter()
            .any(|t| t == "Format dash-exp: Asking yt-dlp for the pieces' new addresses"),
        "{log:?}"
    );
}
