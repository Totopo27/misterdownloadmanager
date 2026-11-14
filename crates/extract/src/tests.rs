use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;

fn run(
    archive: &Path,
    dest: &Path,
    password: Option<&str>,
    subfolder: Subfolder,
) -> (Result<Report, Error>, Vec<PathBuf>) {
    let mut seen = Vec::new();
    let mut on_file = |p: &Path| seen.push(p.to_path_buf());
    let mut policy = Policy {
        dest: dest.to_path_buf(),
        subfolder,
        password: password.map(str::to_owned),
        limits: Limits::default(),
        tool: find_7zip(),
        hatchzip: None,
        in_place: false,
        runs: None,
        on_file: &mut on_file,
        cancel: None,
    };
    let r = extract(archive, &mut policy);
    (r, seen)
}

fn zip_of(path: &Path, entries: &[(&str, &[u8])], aes: Option<&str>) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    for (name, data) in entries {
        let opts = zip::write::SimpleFileOptions::default();
        let opts = match aes {
            Some(p) => opts.with_aes_encryption(zip::AesMode::Aes256, p),
            None => opts,
        };
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

/// No leftover staging folder beside the destination.
fn no_staging(dest: &Path) {
    let left: Vec<_> = std::fs::read_dir(dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".mdm-extract-"))
        .collect();
    assert!(left.is_empty(), "staging left behind: {left:?}");
}

#[test]
fn entry_paths_stay_inside() {
    let d = 64;
    assert_eq!(
        safe::rel_path("a/b.txt", d).unwrap(),
        Path::new("a").join("b.txt")
    );
    assert_eq!(
        safe::rel_path("a\\b.txt", d).unwrap(),
        Path::new("a").join("b.txt")
    );
    assert_eq!(
        safe::rel_path("./a//b", d).unwrap(),
        Path::new("a").join("b")
    );
    for bad in [
        "../evil",
        "a/../../b",
        "..\\evil",
        "/abs",
        "\\abs",
        "C:\\x",
        "C:x",
        "x:ads",
        "a.txt:Zone.Identifier",
        "",
        "./",
    ] {
        assert!(safe::rel_path(bad, d).is_err(), "{bad} should be refused");
    }
    // Device names and trailing dots become safe names, not refusals.
    let con = safe::rel_path("CON.txt", d).unwrap();
    assert_ne!(con, Path::new("CON.txt"));
    assert!(safe::rel_path(&"a/".repeat(65), d).is_err());
}

#[test]
fn volume_names() {
    let v = volume_key("Movie.2024.part02.rar").unwrap();
    assert_eq!(
        (v.base.as_str(), v.kind, v.index),
        ("Movie.2024", VolumeKind::RarPart, 2)
    );
    assert_eq!(volume_key("a.PART1.RAR").unwrap().index, 1);
    let old = volume_key("a.rar").unwrap();
    assert_eq!((old.kind, old.index), (VolumeKind::RarOld, 1));
    assert_eq!(volume_key("a.r00").unwrap().index, 2);
    assert_eq!(volume_key("a.r01").unwrap().index, 3);
    let s = volume_key("pics.7z.003").unwrap();
    assert_eq!(
        (s.base.as_str(), s.kind, s.index),
        ("pics", VolumeKind::Split, 3)
    );
    assert!(volume_key("a.zip").is_none());
    assert!(volume_key("report.pdf").is_none());
    assert_eq!(base_name(Path::new("photos.tar.gz")), "photos");
    assert_eq!(base_name(Path::new("movie.part1.rar")), "movie");
}

#[test]
fn zip_smart_folder_and_marks() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("two.zip");
    zip_of(&a, &[("one.txt", b"1"), ("sub/two.txt", b"22")], None);
    let dest = t.path().join("out");
    let (r, seen) = run(&a, &dest, None, Subfolder::Smart);
    let r = r.unwrap();
    // Two things at the top: a folder named after the archive.
    assert_eq!(r.folder, dest.join("two"));
    assert_eq!(std::fs::read(dest.join("two/sub/two.txt")).unwrap(), b"22");
    assert_eq!((r.files, seen.len()), (2, 2));
    no_staging(&dest);

    // One folder at the top: straight into the destination; a second run keeps the first.
    let b = t.path().join("one.zip");
    zip_of(&b, &[("app/readme.txt", b"hi"), ("app/bin/x", b"x")], None);
    let (r, _) = run(&b, &dest, None, Subfolder::Smart);
    assert_eq!(r.unwrap().folder, dest);
    assert!(dest.join("app/readme.txt").is_file());
    let (r, _) = run(&b, &dest, None, Subfolder::Smart);
    assert_eq!(r.unwrap().renamed, 1);
    assert!(dest.join("app (1)/readme.txt").is_file());
    no_staging(&dest);
}

#[test]
fn zip_aes_passwords() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("secret.zip");
    zip_of(&a, &[("s.txt", b"secret")], Some("pw1"));
    let dest = t.path().join("out");
    assert!(matches!(
        run(&a, &dest, None, Subfolder::Never).0,
        Err(Error::NeedsPassword)
    ));
    assert!(matches!(
        run(&a, &dest, Some("nope"), Subfolder::Never).0,
        Err(Error::WrongPassword)
    ));
    no_staging(&dest);
    run(&a, &dest, Some("pw1"), Subfolder::Never).0.unwrap();
    assert_eq!(std::fs::read(dest.join("s.txt")).unwrap(), b"secret");
}

#[test]
fn hostile_zip_entries_are_left_out() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("evil.zip");
    let f = std::fs::File::create(&a).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for name in ["../outside.txt", "ok.txt", "C:/drive.txt"] {
        z.start_file(name, opts).unwrap();
        z.write_all(b"x").unwrap();
    }
    z.add_symlink("link", "../../etc", opts).unwrap();
    z.finish().unwrap();
    let dest = t.path().join("out");
    let (r, _) = run(&a, &dest, None, Subfolder::Never);
    let r = r.unwrap();
    assert!(dest.join("ok.txt").is_file());
    assert!(!t.path().join("outside.txt").exists());
    assert!(!dest.join("link").exists());
    let why: Vec<_> = r.skipped.iter().map(|(n, w)| format!("{n}: {w}")).collect();
    assert_eq!(r.skipped.len(), 3, "{why:?}");
}

#[test]
fn limits_stop_a_bomb() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("big.zip");
    let zeros = vec![0u8; 3 << 20];
    zip_of(&a, &[("z1", &zeros), ("z2", &zeros)], None);
    let dest = t.path().join("out");
    let mut on_file = |_: &Path| {};
    let mut policy = Policy {
        dest: dest.clone(),
        subfolder: Subfolder::Never,
        password: None,
        limits: Limits {
            total: 4 << 20,
            entries: 100,
            depth: 8,
        },
        tool: None,
        hatchzip: None,
        in_place: false,
        runs: None,
        on_file: &mut on_file,
        cancel: None,
    };
    assert!(matches!(
        extract(&a, &mut policy),
        Err(Error::TooBig { .. })
    ));
    // Nothing half-written is left in place.
    assert!(!dest.join("z1").exists());
    no_staging(&dest);
}

/// 7-Zip's own archives (`7z a`): plain, encrypted, or split in 100 KB volumes.
fn seven_of(tool: &Path, src: &Path, archive: &Path, extra: &[&str]) {
    let ok = std::process::Command::new(tool)
        .arg("a")
        .arg("-bso0")
        .arg("-bsp0")
        .args(extra)
        .arg(archive)
        .arg(src.join("*"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "7-Zip couldn't make {}", archive.display());
}

#[test]
fn sevenz_plain_encrypted_and_split_through_7zip() {
    let Some(tool) = find_7zip() else {
        eprintln!("7-Zip not found: skipped (no_7zip_says_so covers its absence)");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("inner")).unwrap();
    std::fs::write(src.join("a.txt"), b"alpha").unwrap();
    // Not compressible: the split below makes several volumes.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(src.join("inner/b.bin"), &noise).unwrap();
    let plain = t.path().join("plain.7z");
    seven_of(&tool, &src, &plain, &[]);
    let dest = t.path().join("o1");
    let r = run(&plain, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(std::fs::read(r.folder.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(r.folder.join("inner/b.bin")).unwrap(), noise);
    no_staging(&dest);

    let enc = t.path().join("enc.7z");
    seven_of(&tool, &src, &enc, &["-ps3cret"]);
    let dest = t.path().join("o2");
    assert!(matches!(
        run(&enc, &dest, None, Subfolder::Smart).0,
        Err(Error::NeedsPassword | Error::WrongPassword)
    ));
    assert!(matches!(
        run(&enc, &dest, Some("bad"), Subfolder::Smart).0,
        Err(Error::WrongPassword)
    ));
    run(&enc, &dest, Some("s3cret"), Subfolder::Smart)
        .0
        .unwrap();
    no_staging(&dest);

    // 7-Zip's "Split to volumes": split.7z.001, .002…
    let split = t.path().join("split.7z");
    seven_of(&tool, &src, &split, &["-v100k"]);
    let first = t.path().join("split.7z.001");
    assert!(t.path().join("split.7z.003").is_file());
    let dest = t.path().join("o3");
    let r = run(&first, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(r.folder, dest.join("split"));
    assert_eq!(std::fs::read(r.folder.join("inner/b.bin")).unwrap(), noise);
    std::fs::remove_file(t.path().join("split.7z.002")).unwrap();
    let dest = t.path().join("o4");
    assert!(matches!(
        run(&first, &dest, None, Subfolder::Smart).0,
        Err(Error::MissingVolume(2))
    ));
    no_staging(&dest);
}

#[test]
fn no_7zip_says_so() {
    // A 7z (and a RAR) without 7-Zip: NoTool, which the app words as "7z and RAR archives need
    // 7-Zip"; nothing is left behind.
    let t = tempfile::tempdir().unwrap();
    for (name, head) in [
        ("x.7z", &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, 0, 4][..]),
        ("y.rar", &b"Rar!\x1A\x07\x01\x00"[..]),
    ] {
        let a = t.path().join(name);
        std::fs::write(&a, head).unwrap();
        let dest = t.path().join("out");
        let mut on_file = |_: &Path| {};
        let mut policy = Policy {
            dest: dest.clone(),
            subfolder: Subfolder::Smart,
            password: None,
            limits: Limits::default(),
            tool: None,
            hatchzip: None,
            in_place: false,
            runs: None,
            on_file: &mut on_file,
            cancel: None,
        };
        assert!(
            matches!(extract(&a, &mut policy), Err(Error::NoTool)),
            "{name}"
        );
        no_staging(&dest);
    }
}

#[test]
fn tar_gz_and_links() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("pack.tar.gz");
    let f = std::fs::File::create(&a).unwrap();
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(5);
    h.set_mode(0o755);
    h.set_cksum();
    b.append_data(&mut h, "pack/run.sh", &b"hello"[..]).unwrap();
    let mut l = tar::Header::new_gnu();
    l.set_entry_type(tar::EntryType::Symlink);
    l.set_size(0);
    b.append_link(&mut l, "pack/evil", "/etc/passwd").unwrap();
    b.into_inner().unwrap().finish().unwrap();
    let dest = t.path().join("out");
    let r = run(&a, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(std::fs::read(dest.join("pack/run.sh")).unwrap(), b"hello");
    assert!(std::fs::symlink_metadata(dest.join("pack/evil")).is_err());
    assert_eq!(r.skipped.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(dest.join("pack/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o777, 0o755);
    }
}

#[test]
fn single_gz() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("report.csv.gz");
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&a).unwrap(),
        flate2::Compression::fast(),
    );
    gz.write_all(b"a,b\n1,2\n").unwrap();
    gz.finish().unwrap();
    let dest = t.path().join("out");
    run(&a, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(
        std::fs::read(dest.join("report.csv")).unwrap(),
        b"a,b\n1,2\n"
    );
}

#[test]
fn not_an_archive() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("notes.zip");
    std::fs::write(&a, b"just text").unwrap();
    assert!(matches!(
        run(&a, t.path(), None, Subfolder::Smart).0,
        Err(Error::Unsupported)
    ));
}

#[test]
fn external_7zip_goes_through_the_same_checks() {
    // RAR archives can't be made without RAR's own compressor, so the external path is checked
    // with a zip handed to 7-Zip the same way. Skipped where 7-Zip isn't installed.
    let Some(tool) = find_7zip() else {
        eprintln!("7-Zip not found: skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("via7z.zip");
    zip_of(&a, &[("x/one.txt", b"1"), ("x/two.txt", b"2")], Some("pw"));
    let staging = safe::Staging::new(&t.path().join("out")).unwrap();
    let mut w = safe::Writer::new(staging.path(), Limits::default(), None);
    assert!(matches!(
        external::extract(&tool, &a, &mut w, None),
        Err(Error::NeedsPassword)
    ));
    assert!(matches!(
        external::extract(&tool, &a, &mut w, Some("bad")),
        Err(Error::WrongPassword)
    ));
    external::extract(&tool, &a, &mut w, Some("pw")).unwrap();
    let mut seen = 0;
    let (r, placed) = staging
        .place("via7z", Subfolder::Smart, &mut |_| seen += 1)
        .unwrap();
    assert_eq!((placed, seen), (2, 2));
    assert!(r.folder.join("x").join("two.txt").is_file());
}

/// Like [`run`], with HatchZip and in place as given (7-Zip as installed unless `no_7zip`),
/// MDM's records of its runs in `runs`.
fn run_hz(
    archive: &Path,
    dest: &Path,
    password: Option<&str>,
    hatchzip: Option<PathBuf>,
    in_place: bool,
    runs: &Path,
) -> (Result<Report, Error>, usize) {
    let mut seen = 0;
    let mut on_file = |_: &Path| seen += 1;
    let mut policy = Policy {
        dest: dest.to_path_buf(),
        subfolder: Subfolder::Smart,
        password: password.map(str::to_owned),
        limits: Limits::default(),
        tool: find_7zip(),
        hatchzip,
        in_place,
        runs: Some(runs.to_path_buf()),
        on_file: &mut on_file,
        cancel: None,
    };
    let r = extract(archive, &mut policy);
    (r, seen)
}

/// Nothing but the archive's own folder's contents: no work folder left over.
fn no_work_folder(dest: &Path) {
    let left: Vec<_> = std::fs::read_dir(dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".hatchzip-out"))
        .collect();
    assert!(left.is_empty(), "work folder left: {left:?}");
}

#[test]
fn hatchzip_in_place_removes_the_archive_and_places_as_ours() {
    let Some(hz) = find_hatchzip() else {
        eprintln!("HatchZip not found: skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("game.zip");
    zip_of(&a, &[("one.txt", b"1"), ("data/two.txt", b"22")], None);
    let (r, seen) = run_hz(&a, t.path(), None, Some(hz), true, runs.path());
    let r = r.unwrap();
    // Several things at its top: a folder named after it, as our own extractions do.
    assert_eq!(r.folder, t.path().join("game"));
    assert_eq!((r.files, seen), (2, 2));
    assert!(r.in_place);
    assert!(!a.exists(), "the archive went while its files came out");
    assert_eq!(
        std::fs::read(t.path().join("game/data/two.txt")).unwrap(),
        b"22"
    );
    no_work_folder(t.path());
    no_staging(t.path());
    assert_eq!(
        std::fs::read_dir(runs.path()).unwrap().count(),
        0,
        "run forgotten"
    );
}

#[test]
fn hatchzip_in_place_wrong_passwords_change_nothing() {
    let Some(hz) = find_hatchzip() else {
        eprintln!("HatchZip not found: skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("locked.zip");
    zip_of(&a, &[("x.txt", b"secret")], Some("pw"));
    let before = std::fs::read(&a).unwrap();
    let (r, _) = run_hz(&a, t.path(), None, Some(hz.clone()), true, runs.path());
    assert!(matches!(r, Err(Error::NeedsPassword)), "{r:?}");
    let (r, _) = run_hz(
        &a,
        t.path(),
        Some("bad"),
        Some(hz.clone()),
        true,
        runs.path(),
    );
    assert!(matches!(r, Err(Error::WrongPassword)), "{r:?}");
    assert_eq!(std::fs::read(&a).unwrap(), before, "untouched");
    no_work_folder(t.path());
    let (r, _) = run_hz(&a, t.path(), Some("pw"), Some(hz), true, runs.path());
    r.unwrap();
    assert_eq!(std::fs::read(t.path().join("x.txt")).unwrap(), b"secret");
    assert!(!a.exists());
}

/// A stand-in for HatchZip: records its arguments beside the archive, runs `body` (`$out` its
/// folder, `$archive` the archive) and exits with `code`.
#[cfg(unix)]
fn fake_hatchzip(dir: &Path, code: i32, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let p = dir.join(format!("fake-hatchzip-{code}-{n}"));
    std::fs::write(
        &p,
        format!(
            "#!/bin/sh\nout=\"${{2#--output=}}\"\narchive=\"$6\"\nmkdir -p \"$out\"\n\
             echo \"$@\" > \"$archive.args\"\n{body}\necho 'error: it stopped' >&2\nexit {code}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[cfg(unix)]
const WRITES: &str = "printf x > \"$out/got.txt\"";

#[cfg(unix)]
#[test]
fn hatchzip_gets_its_paths_after_a_double_dash() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("-y.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    let body = format!("{WRITES}\npwd -P > \"$archive.cwd\"");
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 0, &body)),
        true,
        runs.path(),
    );
    r.unwrap();
    let args = std::fs::read_to_string(t.path().join("-y.zip.args")).unwrap();
    let out = t.path().join("-y.hatchzip-out");
    // It runs in its output folder, not in the folder MDM was started in (#845).
    let cwd = std::fs::read_to_string(t.path().join("-y.zip.cwd")).unwrap();
    assert_eq!(
        Path::new(cwd.trim()),
        t.path().canonicalize().unwrap().join("-y.hatchzip-out")
    );
    assert_eq!(
        args.trim(),
        format!(
            "extract --output={} -y -q -- {}",
            out.display(),
            a.display()
        )
    );
}

#[cfg(unix)]
#[test]
fn without_7zip_hatchzip_is_used_only_when_asked() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("x.7z");
    std::fs::write(&a, [b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, 0, 4]).unwrap();
    let mut on_file = |_: &Path| {};
    let mut policy = Policy {
        dest: t.path().to_path_buf(),
        subfolder: Subfolder::Smart,
        password: None,
        limits: Limits::default(),
        tool: None,
        hatchzip: Some(fake_hatchzip(t.path(), 0, WRITES)),
        in_place: false,
        runs: Some(runs.path().to_path_buf()),
        on_file: &mut on_file,
        cancel: None,
    };
    let r = extract(&a, &mut policy);
    assert!(matches!(r, Err(Error::NoTool)), "{r:?}");
    assert!(!t.path().join("x.7z.args").exists(), "HatchZip wasn't run");
    assert!(a.exists());
}

#[cfg(unix)]
#[test]
fn an_archive_hatchzip_cant_extract_in_place_is_extracted_as_usual() {
    // A solid RAR: HatchZip refuses before it touches anything, asking for --keep-archive (its
    // own words, 0.1.10). Here a ZIP stands in for it, which MDM then extracts itself.
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("solid.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    let refuses = "echo 'error: a solid RAR archive can only be extracted while it is kept. Run the command again with --keep-archive to extract it and keep the archive, or extract it with another program' >&2";
    let (r, seen) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 1, refuses)),
        true,
        runs.path(),
    );
    let r = r.unwrap();
    assert!(!r.in_place);
    assert_eq!(seen, 1);
    assert_eq!(std::fs::read(t.path().join("one.txt")).unwrap(), b"1");
    assert!(a.exists(), "the archive is kept");
    no_work_folder(t.path());
    assert_eq!(std::fs::read_dir(runs.path()).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn a_stop_partway_in_place_keeps_what_came_out() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("big.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    // Stopped after writing a file: that folder stays, named, never removed.
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 1, WRITES)),
        true,
        runs.path(),
    );
    let Err(Error::Partial(folder)) = r else {
        panic!("{r:?}")
    };
    assert_eq!(folder, t.path().join("big.hatchzip-out"));
    assert!(folder.join("got.txt").exists());
    std::fs::remove_dir_all(&folder).unwrap();
    // Not enough room: nothing changed, the empty folder goes.
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 3, ":")),
        true,
        runs.path(),
    );
    assert!(
        matches!(r, Err(Error::NoRoom(ref s)) if s == "it stopped"),
        "{r:?}"
    );
    no_work_folder(t.path());
    // Damaged before anything came out: Damaged, in HatchZip's words.
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 4, ":")),
        true,
        runs.path(),
    );
    assert!(
        matches!(r, Err(Error::Damaged(ref s)) if s == "it stopped"),
        "{r:?}"
    );
    no_work_folder(t.path());
}

#[cfg(unix)]
#[test]
fn a_run_mdm_started_is_finished_by_hatchzip() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("half.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    // MDM starts it in place; HatchZip stops partway and keeps its progress beside the archive.
    let stops = "printf x > \"$out/one.txt\"\n: > \"$archive.hatchzip\"";
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 1, stops)),
        true,
        runs.path(),
    );
    assert!(matches!(r, Err(Error::Partial(_))), "{r:?}");
    // In place is off now: it still goes to HatchZip, in place, into the same folder.
    let ends = "printf y > \"$out/two.txt\"\nrm \"$archive.hatchzip\"";
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 0, ends)),
        false,
        runs.path(),
    );
    let r = r.unwrap();
    assert!(r.in_place);
    let args = std::fs::read_to_string(t.path().join("half.zip.args")).unwrap();
    assert!(args.contains("half.hatchzip-out"), "{args}");
    assert!(t.path().join("half/one.txt").exists(), "placed as ours");
    assert!(t.path().join("half/two.txt").exists());
    no_work_folder(t.path());
    assert_eq!(
        std::fs::read_dir(runs.path()).unwrap().count(),
        0,
        "run forgotten"
    );
}

#[cfg(unix)]
#[test]
fn a_progress_file_mdm_didnt_write_starts_nothing() {
    // A download (or a torrent) that comes with HatchZip's progress file beside the archive.
    for in_place in [false, true] {
        let t = tempfile::tempdir().unwrap();
        let runs = tempfile::tempdir().unwrap();
        let a = t.path().join("x.zip");
        zip_of(&a, &[("one.txt", b"1")], None);
        std::fs::write(t.path().join("x.zip.hatchzip"), b"").unwrap();
        let (r, _) = run_hz(
            &a,
            t.path(),
            None,
            Some(fake_hatchzip(t.path(), 0, WRITES)),
            in_place,
            runs.path(),
        );
        let r = r.unwrap();
        // Extracted as usual: the archive is kept, HatchZip never ran.
        assert!(!r.in_place);
        assert!(a.exists(), "the archive is kept");
        assert!(!t.path().join("x.zip.args").exists(), "HatchZip wasn't run");
        assert_eq!(std::fs::read(t.path().join("one.txt")).unwrap(), b"1");
    }
}

#[cfg(unix)]
#[test]
fn entries_left_out_in_place_stay_in_its_folder() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("links.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    let body = "printf 1 > \"$out/one.txt\"\nmkdir \"$out/sub\"\nprintf 2 > \"$out/sub/two.txt\"\n\
                ln -s /etc/hostname \"$out/top-link\"\nln -s /etc \"$out/sub/dir-link\"";
    let (r, seen) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 0, body)),
        true,
        runs.path(),
    );
    let r = r.unwrap();
    let out = t.path().join("links.hatchzip-out");
    assert_eq!((r.files, seen), (2, 2));
    assert_eq!(
        std::fs::read(t.path().join("links/sub/two.txt")).unwrap(),
        b"2"
    );
    let mut skipped: Vec<_> = r.skipped.iter().map(|(n, w)| (n.as_str(), *w)).collect();
    skipped.sort();
    assert_eq!(skipped, [("sub/dir-link", "link"), ("top-link", "link")]);
    // Never deleted: still where HatchZip put them, and the report says where.
    assert_eq!(r.left.as_deref(), Some(out.as_path()));
    assert!(
        out.join("top-link")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert!(
        out.join("sub/dir-link")
            .symlink_metadata()
            .unwrap()
            .is_symlink()
    );
    assert!(!out.join("one.txt").exists() && !out.join("sub/two.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_failed_move_in_place_keeps_every_file() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("stuck.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    // `z` holds a link (left out), so its files are moved one by one; it can't be written to
    // (a file held open, say), so moving `z/c.txt` out of it fails after `a.txt` went.
    let body = "printf a > \"$out/a.txt\"\nmkdir \"$out/z\"\nprintf c > \"$out/z/c.txt\"\n\
                ln -s /etc \"$out/z/link\"\nchmod 555 \"$out/z\"";
    let (r, _) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 0, body)),
        true,
        runs.path(),
    );
    let out = t.path().join("stuck.hatchzip-out");
    let back = || std::fs::set_permissions(out.join("z"), std::fs::Permissions::from_mode(0o755));
    let Err(Error::Unplaced { to, left, .. }) = r else {
        back().unwrap();
        panic!("{r:?}")
    };
    back().unwrap();
    assert_eq!(to, t.path().join("stuck"));
    assert_eq!(left, out);
    // What moved stays where it went, the rest where HatchZip put it: nothing deleted.
    assert_eq!(std::fs::read(to.join("a.txt")).unwrap(), b"a");
    assert_eq!(std::fs::read(out.join("z/c.txt")).unwrap(), b"c");
    assert!(out.join("z/link").symlink_metadata().unwrap().is_symlink());
    assert!(!out.join("a.txt").exists());
}

#[cfg(unix)]
#[test]
fn a_folder_that_cant_be_read_says_where_the_files_are() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    let a = t.path().join("shut.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    // HatchZip finished, and then its folder can't be listed (its rights taken away): nothing
    // was moved, and the error names the folder, as a move that failed does.
    let body = "printf a > \"$out/a.txt\"\nchmod 000 \"$out\"";
    let (r, seen) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 0, body)),
        true,
        runs.path(),
    );
    let out = t.path().join("shut.hatchzip-out");
    std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).unwrap();
    // Root reads any folder: nothing to check then.
    if r.is_ok() {
        return;
    }
    let Err(Error::Unplaced { to, left, .. }) = r else {
        panic!("{r:?}")
    };
    assert_eq!((to, left), (t.path().to_path_buf(), out.clone()));
    assert_eq!(seen, 0);
    assert_eq!(std::fs::read(out.join("a.txt")).unwrap(), b"a", "kept");
}

#[cfg(unix)]
#[test]
fn only_hatchzips_own_refusal_with_nothing_changed_falls_back() {
    let t = tempfile::tempdir().unwrap();
    let runs = tempfile::tempdir().unwrap();
    // Another error that mentions the flag isn't the refusal: no second extraction.
    let a = t.path().join("other.zip");
    zip_of(&a, &[("one.txt", b"1")], None);
    let mentions = "echo 'error: unexpected argument near --keep-archive' >&2";
    let (r, seen) = run_hz(
        &a,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 1, mentions)),
        true,
        runs.path(),
    );
    assert!(matches!(r, Err(Error::Damaged(_))), "{r:?}");
    assert_eq!(seen, 0);
    assert!(!t.path().join("one.txt").exists());
    // The refusal's words, but the archive is shorter than it was: not extracted as usual.
    let b = t.path().join("cut.zip");
    zip_of(&b, &[("two.txt", b"2")], None);
    let cuts =
        "printf x > \"$archive\"\necho 'error: Run the command again with --keep-archive' >&2";
    let (r, seen) = run_hz(
        &b,
        t.path(),
        None,
        Some(fake_hatchzip(t.path(), 1, cuts)),
        true,
        runs.path(),
    );
    assert!(matches!(r, Err(Error::Damaged(_))), "{r:?}");
    assert_eq!(seen, 0);
    assert!(!t.path().join("two.txt").exists());
}
