# Architecture

How Mister Download Manager's pieces fit together.

## Crates

| Path | Crate | Role |
|---|---|---|
| `crates/engine` | `mdm-engine` | Downloads one file over HTTP(S) or FTP(S): probe, segment planner, connection workers, resume state, per-host limits, speed limiter, checksums, proxy scripts |
| `crates/app` | `mdm-app` | The desktop app (`misterdownloadmanager.exe`, egui): list, categories, queues, scheduler, dialogs, tray, settings |
| `crates/cli` | `mdm-cli` | Headless downloads with progress (`get`, `resume`), videos, torrents and the automation API's commands: a library the app's program runs (`misterdownloadmanager --cli …`), and `misterdownloadmanager-cli`, a small launcher that starts it with the same arguments, terminal and exit code |
| `crates/mcp` | `mdm-mcp` | The MCP server (`misterdownloadmanager-mcp`): AI assistants list, add and control the running app's downloads through the automation API |
| `crates/remote` | `mdm-remote` | The remote control (#141): the HTTPS server a paired phone or computer on the local network uses (its page, pairing codes, device tokens, the self-signed certificate); every request becomes an automation API command the app answers |
| `crates/nmhost` | `mdm-nmhost` | The browser's native messaging host: takes captured downloads from the extension and hands them to the app |
| `crates/ipc` | `mdm-ipc` | The single-instance channel (a token-protected local socket), the data folder, and the capture rules shared by the app and the host |
| `crates/debrid` | `mdm-debrid` | Debrid accounts (#44): the services' official APIs (Real-Debrid, AllDebrid, Premiumize, TorBox) turn a file host's link into a direct one the engine downloads |
| `crates/extract` | `mdm-extract` | Archive extraction (#37): zip, tar, gz and xz in process; 7z and RAR through an installed 7-Zip, run as its own program |
| `crates/geo` | `mdm-geo` | Where a download's server is (#140): an IP-to-country range table built from DB-IP's Lite data (`tools/geo/build.py`), looked up on this computer only, and the countries' flags |
| `crates/tools` | `mdm-tools` | Video pages through yt-dlp and ffmpeg, image galleries through gallery-dl (run as separate programs); while a network adapter is chosen, their connections go through a proxy of MDM's on 127.0.0.1 that binds them to it (`relay.rs`); each runs in a folder it can write, also its TEMP (`workdir.rs`) |
| `crates/crash` | `mdm-crash` | Crash and freeze reports: the monitor process, out-of-process minidumps, symbolicated stacks, the report's text and the release logs ([crash-reports.md](crash-reports.md)) |
| `crates/torrent` | `mdm-torrent` | BitTorrent and magnet links through librqbit |
| `crates/labserver` | `mdm-labserver` | An HTTP Range server with a request log and fault injection, and an FTP/FTPS server with its own faults, for tests and benchmarks |
| `extension/` | — | The Chromium (MV3) and Firefox extension |
| `docs/` | — | The user guide and tutorials, this file, the benchmark data; the website makes the guide its help pages, which Help opens |
| `website/` | — | misterdownloadmanager.com's own pages (home, download, features, themes, FAQ); `tools/website/build.py` adds the help from `docs/` and the news from the CHANGELOG, in 22 languages, and GitHub Pages publishes it |

Dependencies point inward: `app`, `cli`, `mcp`, `nmhost` and `remote` use `engine` and `ipc`; the engine knows
nothing of the UI. The app sees the engine only through its events (progress, segment table
snapshots, finished, failed).

## Engine

```
Download
 ├─ probe()              size, resumable?, validators (ETag / Last-Modified), file name
 ├─ Planner (pure)       segment table: when a connection is free, the largest remaining
 │                       part is split in half (planner.rs, tested by random simulation)
 ├─ Worker × N           one HTTP/1.1 connection each, reused across segments; slow ones
 │                       are dropped and their part re-split
 │    └─ writes at its offset into <name>.mdm.part (created sparse)
 ├─ HostLimits           learns how many connections each server accepts
 ├─ Limiter (shared)     per-download and global token buckets
 ├─ State                segment table saved atomically (craft-atomic) for crash-safe resume
 └─ events → channel     Progress, Done, Failed
```

When every segment is done, the part file is checked against the expected size and renamed into
place; the file's date can follow the server's.

A download's first request (`get`) follows redirects and the pages that lead on to a file by
themselves (a waiting page's refresh). Sites whose links aren't the file's address have a module
in `sites/` (Google Drive: `sites/gdrive.rs`): it turns the link into the file's address, reads
the site's own pages (a confirmation form, an error page) and names its refusals
(`Error::Refused`), with pure functions tested without a network. The lab server plays such a
site on its real host names for the end-to-end tests (`labserver/src/drive.rs`).

FTP and FTPS (`ftp.rs`) go through the same planner, workers and state: a segment is `REST` +
`RETR` on a control session instead of a ranged `GET`, `SIZE` and `MDTM` stand in for the
headers, and sessions are pooled per download and reused between segments.

## App

One egui window painted to fixed pixel coordinates, plus one immediate viewport per dialog.
`app.rs` holds the state (downloads, queues, settings, the list's selection) and turns menu,
toolbar, key and IPC commands into actions. Each dialog is its own module and returns an
`Outcome` (open, closed, done with a value). Windows-only calls are confined to `win32.rs` and
their Linux and macOS counterparts to `unix.rs` (starting with the session, the browsers' host,
sounds, show in folder, the crash handler); every other file is safe, portable Rust. Fonts come from the system:
Windows' Fonts folder, macOS's system fonts, fontconfig on Linux (`theme.rs`). On Windows, Latin,
Greek, Cyrillic and single-codepoint CJK text is drawn by DirectWrite (`win32.rs`, through egui's glyph rasterizer hook):
View → Text → Smooth in its natural mode, anti-aliased at the size asked for and placed at quarter
pixels with the font's own advances; Classic (the default) in GDI-classic mode, so each letter's
hinted shape and whole-pixel width come together. CJK uses the interface language's system face;
combining marks, multi-codepoint clusters and missing glyphs stay with egui, as do other scripts.

### Themes

Every colour and size a control is drawn with comes from the active theme (#45). A theme is a
light and a dark palette, metrics, control styles and an icon style (`palette.rs`):

- **Palette roles.** `palette.rs` declares each role with a stable key (`"button.hot"`,
  `"list.row"`, `"caption"`…), a description and the classic look's light and dark values,
  written out (the classic look isn't derived). The shared painters (`winui.rs`, `chrome.rs`,
  `list.rs`, `scrollbar.rs`) read them through `theme::colors()` and `theme::metrics()` and decide
  each control's states from them; no painting code holds a colour of its own.
- **Derived palettes.** `derive.rs` makes a whole palette from a few seeds (background, surface,
  text, accent…), mixing every other role in OKLab and keeping WCAG contrast (4.5:1 for text, 3:1
  for edges); High contrast strengthens every edge on top. `native.rs` seeds the Native theme from
  the system's accent and contrast (Windows' settings, the freedesktop portal on Linux, macOS's
  accent).
- **Control styles.** `palette::Controls` names each control's shape (text boxes, check boxes,
  buttons, progress bars, scroll bars, the focus mark, the title bar); the style themes (Metro,
  Adwaita, Breeze, Luna, Aero, Aqua, Emerald) pick theirs, and on Windows MDM then draws the
  title bar itself (the `caption.*` roles).
- **Theme files.** `themefile.rs` reads TOML or JSON from the data folder's `themes` (one file,
  or `<id>/theme.toml` with icons beside it). A file names only what it changes; the rest comes
  from the theme it `inherits` (a built-in's id or another file, the classic look by default),
  down a chain in which a cycle is refused. Unknown keys are warnings, not errors. The built-in
  style themes are such files too, carried in the program (`crates/app/assets/themes`).
- **Icon sets.** `themeicons.rs` draws a theme's icons (named by what they're for: `tb-add-url`,
  `cat-music`…, SVG or PNG at 16, 24, 32 and 48 px, `-symbolic` ones inked in the text colour)
  where the classic look draws its own; a need the set lacks keeps the classic icon.

Where a window puts its controls stays its layout. So do the icons
before file names (`fileicons.rs`): the Windows shell's system image lists, macOS Finder's
(NSWorkspace), the Linux desktop's icon theme (`xdgicons.rs`, shared-mime-info and the Icon
Theme Specification). Each type is looked up once per pixel size, on a background thread. Tango's
icons (bundled) stand in until the system's icon is ready, and wherever the system has none.

### Drawing

The windows are drawn with OpenGL (eframe's glow) or with the system's own graphics API through
eframe's wgpu, with that one backend only: Direct3D 12 on Windows (the default there), Vulkan on
Linux, Metal on macOS (choices there; OpenGL stays their default until real graphics cards have
tried them, #206). View → Draw with picks one, from the next start. One code path does it on
every system (`crates/app/src/renderer.rs`): when the chosen one can't start, the other one is
tried, so the windows always open. A watched start with the system's API leaves a marker until
its first frame is drawn: a start that ends before it (a crash inside the driver), one whose
first frame doesn't come within 10 seconds (MDM then starts again with OpenGL), or a graphics
card lost twice in 10 minutes is remembered with its reason, which the menu shows.

View → Interface size (`scale.rs`, #262) is egui's zoom factor over the display's own scale:
every window is laid out in points, so it opens that much larger; one larger than the screen
scrolls.

### Data

The download list and settings are saved atomically to the data folder (`downloads.json`):
`%APPDATA%\misterdownloadmanager\` on Windows, `~/.local/share/misterdownloadmanager` on Linux,
`~/Library/Application Support/misterdownloadmanager` on macOS, a `data` folder beside a portable
copy, or `MDM_DATA_DIR` (`mdm_ipc::data_dir`). The browser host reads the same file for the
capture rules, so a change in Options applies to the next captured download.

Sign-in secrets (debrid accounts, Sites Logins' and the proxy's passwords) live in the system's
keychain (`secrets.rs`, #44): Windows Credential Manager (`win32.rs`), the Secret Service on Linux
or the macOS keychain (`unix.rs`). The settings keep a `keychain:<service>/<scope>/<key>`
reference in their place, written only once the keychain has confirmed the secret; without a
keychain (or with a locked one, never unlocked for MDM) the secret stays in the settings as
before.

A download's server location (#140) is the country of the address it connected to, looked up in
`mdm-geo`'s table on this computer (`countries.rs` names it in the interface language); the
release job builds the table again from the month's data (`tools/geo/build.py --release`).

## Browser integration

```
page ──► extension ──native messaging──► mdm-nmhost ──ipc──► app
```

The extension asks the host which file types to take, checks each download against them, and
cancels the browser's download only after the app has accepted it; otherwise the browser keeps
it, and the extension says why.

The host serves only MDM's own extension. Its IDs are in one place, `mdm_ipc::extension`
(`crates/ipc/src/extension.rs`): the unpacked copy's (`UNPACKED_ID`, from the manifest's public
key), the Chrome Web Store's (`STORE_ID`, signed with the store's own key: `pack.ps1` leaves the
manifest's key out of the store's copy), the Firefox add-on's (`FIREFOX_ID`) and its
addons.mozilla.org page; the Edge and Opera stores' are placeholders, never served, until the
extension is listed there. The host writes each browser's manifest with those IDs, and the app
offers the store's copy (Add to Chrome, Add to Firefox) before the unpacked one.

## Remote control

```
phone ──HTTPS (paired token)──► mdm-remote ──ipc::Sink──► app (answer, remote_add)
```

`--headless` (`headless.rs`, #157) runs the same `App` without eframe: a loop calls its
`tick` (what `logic` runs at every frame) on a bare `egui::Context`, woken by the command
channel, and never its `ui`; commands are added as the remote control adds them (no window).
The `Dockerfile` and `docker/` (Compose, Unraid's template) run it on a server
([server.md](server.md)).

The server runs on the app's Tokio runtime, one listener per local network address. Each request
is checked (`Host`, `Origin`, token), turned into an `mdm_ipc::Command` marked `remote` (a field
no request line can set) and handed to the app through the same channel as the command line's;
the app answers it on its next frame, as it answers the automation API. Its page is built in
(`crates/remote/web`) and its texts come from the app's catalogs (`remote.rs`).

## Translations

Every text the UI shows goes through `crate::i18n` (craft-i18n catalogs, `i18n/<locale>.tsv`).
`tools/i18n/extract.py` rebuilds `i18n/en.tsv` from the source, with a note for translators on
each text (what it is, where it shows, its placeholders). `tools/i18n/translate.py` fills what
the other catalogs lack (rows marked `#, machine`), following each language's style guide
(`i18n/style/`) and the glossary; a row a person reviewed is never replaced, and CI
(`reviewed_guard.py`) fails a change that drops one. The website has its own catalogs
(`i18n/site/`, `tools/website/translate_site.py`).

## Pictures

The help pages' pictures (`docs/images/help/`) come from one list, `tools/shots/shots.toml`:
`tools/shots/make.py` opens each window through MDM's own `/show <window>` command line in a test
instance off the screen, crops and annotates it from the window's accessibility tree, and writes
lossless WebP; `make.py check` (CI) keeps the list, the pages and the files in step. The
published ones are taken on GitHub's Windows runner (`docs-shots.yml`).

## Contributors

`tools/contributors.py` reads who opened pull requests and issues, and who commented, from
GitHub's public API (the maintainer, Claude and bots left out) for every place that thanks them:
the website's home page, `/contributors.html` and each release on the News page (`build.py`, on
every build), each release's notes (`release.yml`, the people active since the release before)
and the README (`python tools/contributors.py readme` at each release's preparation: its round
photos are committed in `art/contributors/`, since GitHub can't round a picture in a README).

## Tests and benchmarks

`cargo test` runs unit tests and end-to-end downloads against `mdm-labserver` with injected
faults (slow, dropped and refused connections, servers without ranges, changing files).
`tools/bench` runs the published speed comparison; its raw data is in `docs/bench/`.
