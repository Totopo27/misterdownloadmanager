# Roadmap

What's done for version 0.5.2, what's being built, and what's planned. Each item links to its
issue, where the details and the discussion are. Released changes are in
[CHANGELOG.md](CHANGELOG.md). Ideas and bug reports are welcome in
[Issues](https://github.com/anefzaoui/misterdownloadmanager/issues).

✅ done · 🔄 being built · 📋 planned · 🔍 looking into it

## Version 0.5.0

Everything below ships together, in version 0.5.0.

### Done

- ✅ Menus on compositors that animate windows (Hyprland): no border and no slide-in, and going from
  title to title moves one window, so nothing fades; the reporter confirmed it
  ([#277](https://github.com/anefzaoui/misterdownloadmanager/issues/277))
- ✅ A very slow first window on Windows: MDM draws with Direct3D 12 by default and keeps it through
  a slow start ([#57](https://github.com/anefzaoui/misterdownloadmanager/issues/57))
- ✅ A crash on Linux as Download File Info opens
  ([#48](https://github.com/anefzaoui/misterdownloadmanager/issues/48))
- ✅ Linux on GNOME Wayland: right-click menus open where you click, and windows fit their buttons
  ([#29](https://github.com/anefzaoui/misterdownloadmanager/issues/29))
- ✅ Windows 11 Insider builds: no black band across the top of menus, the drop target, the
  progress window and the windows whose title bar MDM draws (the style themes); the reporter
  confirmed it ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ The progress window's Download status tab: as much room above its first row as below its
  last; where the server is says "this computer" or "the local network", each with its icon
- ✅ **View → Interface size**: every window larger, 100 % to 200 %, with Ctrl + / Ctrl − / Ctrl 0
  as in a browser; a window larger than the screen scrolls. Sharper text on Windows (part of
  [#262](https://github.com/anefzaoui/misterdownloadmanager/issues/262))
- ✅ Windows: **View → Text → Smooth** draws the text anti-aliased at its own size, not as
  thick, hard-edged whole-pixel letters that read badly on a 1440p screen; **Classic**, the
  default, keeps the hinted look MDM had. **Interface size** gains 110 %, 115 % and 120 % between 100 % and
  125 %
- ✅ Windows: sharp, evenly spaced text at every interface size: letters no longer run together
  or drift apart ([#323](https://github.com/anefzaoui/misterdownloadmanager/issues/323),
  [#334](https://github.com/anefzaoui/misterdownloadmanager/issues/334))
- ✅ Linux: Update now asks the package manager which package MDM is; a copy installed by
  another package manager (pacman) is left to the system's updates, with a plain message
- ✅ Linux: the .deb and .rpm install the browsers' host files for everyone and remove them on
  uninstall, so nothing is left in the home folders for Chrome, Chromium, Edge, Brave, Vivaldi and
  Firefox (the Snap too); the guide says what the AppImage and the tar.gz leave, and how to remove
  it ([#266](https://github.com/anefzaoui/misterdownloadmanager/issues/266))
- ✅ Linux: menus and drop-down lists open under what was clicked on tiling compositors (sway)
  too, through XWayland, as on GNOME and KDE
  ([#267](https://github.com/anefzaoui/misterdownloadmanager/issues/267))
- ✅ Linux: a library the window needs that the system lacks (libxkbcommon-x11 and the other X11
  ones, from the tar.gz and the AppImage) is named at start, on the terminal and in a message box,
  with the command that installs it, instead of a crash; in a Wayland session the window opens
  on Wayland instead ([#265](https://github.com/anefzaoui/misterdownloadmanager/issues/265))
- ✅ Linux: plain HTTP downloads work on a system without CA certificates, and HTTPS ones say to
  install them ([#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264))
- ✅ Linux: torrents and magnet links work on a system without CA certificates too (peers, the
  DHT and `http://` trackers; `https://` trackers say why they can't), part of
  [#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264)
- ✅ Update now finds the Linux and macOS release files whether or not their names end in
  "-experimental"
- ✅ The Linux release files are named without "-experimental" (`…-linux-x86_64.deb` and the
  others), in the packages' descriptions too; the macOS ones keep it
- ✅ Linux packages checked in clean Ubuntu 24.04, Debian 12 and Fedora 45 systems: install, the
  command line, the window, the desktop entry and icon, browser integration and uninstall; the
  .deb and .rpm now bring the keyboard library MDM's window needs under X11 and the CA
  certificates downloads need, and require the glibc their programs were built for (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Options → Proxy's Get System and the Scheduler's "when done" drop-down fit in the last
  languages that still cut them (Bulgarian, Armenian, Macedonian, Tamil; Greek, Malayalam, and
  six the translation review lengthened), and the Proxy
  page's title no longer runs over its icon or past the window's edge in Greek, Finnish, Catalan
  and a few more: a title too long for one line beside Get System takes two
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ Linux: galleries work with the gallery-dl the distribution ships (Ubuntu 24.04's 1.26.9
  failed at once): MDM passes only the options that release accepts, from 1.20 on, and an older
  one gives way to MDM's own copy, saying why; the lab's test streams are real ones, which ffmpeg
  6.1 and 8 download to the end
  ([#236](https://github.com/anefzaoui/misterdownloadmanager/issues/236))
- ✅ Run MDM on a server without a window (Docker, Unraid, any Linux machine), and send it
  downloads from the phone or the browser extension
  ([#157](https://github.com/anefzaoui/misterdownloadmanager/issues/157))
- ✅ Better translations in every language: each language has a style guide and the terms its
  reviewers settled, and every machine translation of the program and the website was reviewed
  again against them
- ✅ Linux: browser integration reaches Brave Origin (and the Beta, Dev and Nightly versions of
  Brave, Chrome and Edge, and Vivaldi's snapshot); on macOS Brave is set up where it reads it,
  Chrome's folder. From the AppImage, the helper program the browsers start is copied out
  reliably, even while a browser still runs the old one, and put back at each start if it went
  missing; Test browser integration says when it is missing, or why setting it up failed
  ([#253](https://github.com/anefzaoui/misterdownloadmanager/issues/253))
- ✅ The extension's popup shows **Open Mister Download Manager** whole: it was cut to "Open Mister
  Do…" on Linux and in longer languages; Options moves below it when both don't fit on one line
  (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The crash report window's status line (such as "The symbols couldn't be fetched: …") stays
  inside the window: cut with "…" at its margin and shown whole under the pointer; its title is
  in MDM's language, and its title bar in MDM's light or dark (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Control MDM from a phone or another computer on the local network: **Options → Remote**, off
  by default; a paired device adds links, magnets and torrents, sees the list and pauses, starts
  and removes downloads ([#141](https://github.com/anefzaoui/misterdownloadmanager/issues/141))
- ✅ Linux: moving the pointer from an open submenu back onto its menu, or to another item, closes
  only the submenu, not the whole menu
  ([#250](https://github.com/anefzaoui/misterdownloadmanager/issues/250))
- ✅ Download File Info's Preview, Options → Proxy's Get System and the Scheduler's "when done"
  drop-down fit their translated text (Russian and others), after #212
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ A menu of the menu bar shut with Escape opens again at the first click on its title (on
  Linux it took two; on Windows, Escape with the pointer on the title left the menu open)
- ✅ A YouTube look-up that fails with "Requested format is not available" (signed in, no
  JavaScript runtime for yt-dlp) tries once more without the cookies; the video window says when
  ffmpeg would bring the better qualities a page offers
- ✅ Signed-in videos from YouTube Music and YouTube's mobile site: the browser's cookies reach
  the YouTube host yt-dlp asks, each for the domain the browser keeps it for
  ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229))
- ✅ Cookies for video sites: yt-dlp can read a browser's cookies on this computer (Firefox,
  Chrome, Edge, Brave, Opera, Vivaldi, Chromium, Whale; Safari on macOS), so a pasted YouTube
  link that asks to "confirm you're not a bot" works when you're signed in there; and the button
  over videos now passes the page's cookies to the download as well as to the look-up
  ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229))
- ✅ When the browser keeps a download, MDM says why: each one is in MDM's log with the setting
  that decided, and the extension's button shows the last one. The extension from the Chrome Web
  Store needs this release (0.2.x serves only the copy in MDM's folder)
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ Waterfox, and Firefox set to ask what to do with each file: MDM takes the download before
  the browser's *What should … do with this file?* dialog, which no longer shows for the files
  MDM takes ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ More file types taken from the browser by default: DMG, PKG, DEB, RPM, APPIMAGE, MSIX, APPX,
  CAB, JAR, XZ, ZST, WEBM, FLAC, OPUS and others; a list never edited gains them
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ Windows: Download File Info and the progress window open in front of the browser, even when
  Windows doesn't let MDM take the foreground; the keyboard stays where it was until a click
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208))
- ✅ The window no longer freezes on Linux when the system's sleep service (logind) or keyring
  (Secret Service) is slow or doesn't answer: keeping the computer awake for a woken queue is
  asked in the background, a keyring that doesn't answer at start is waited for once instead of
  once per saved password, and the tray check and Download File Info's Save As check no longer
  wait on every frame (follows [#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222))
- ✅ Linux: Dark Mode support follows the desktop's dark or light theme on XFCE, MATE and others
  without the portal's setting, and on GNOME without the portal, live
  ([#156](https://github.com/anefzaoui/misterdownloadmanager/issues/156))
- ✅ Started with the computer, MDM waits in the notification area instead of opening its window
  in front, on Windows, Linux and macOS (minimized where no notification area shows its icon);
  startup entries written before are updated, and the installer writes the new one
  ([#226](https://github.com/anefzaoui/misterdownloadmanager/issues/226))
- ✅ Open folder in Download complete no longer freezes the window on Linux when the file
  manager is slow to answer or never does: MDM asks in the background and opens the folder after
  5 seconds without an answer; every other program a window starts (Set it up, the Trash, an
  update being put in place, pages and folders) no longer holds the window either
  ([#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222))
- ✅ A download too big for its drive is refused on a very busy computer too: the free-space
  check's 5 seconds count from when it runs, not from when it was asked, and it asks only the
  folder's own drive (shared network folders too) instead of listing every drive; and the host
  connection limit test tells a reconnect from a connection too many ([#224](https://github.com/anefzaoui/misterdownloadmanager/issues/224))
- ✅ The crash report window in every language: the report keeps its lines (German and Arabic
  ran them together) and reads left to right in Arabic and Persian; **End and restart MDM** fits
  its text ([#204](https://github.com/anefzaoui/misterdownloadmanager/issues/204))
- ✅ Every window's buttons fit their text in every language: Download File Info (German
  **Später herunterladen** was cut), Download complete, Add torrent, Download a gallery, the
  progress window, the Scheduler, Options, the Help menu's windows, RSS feeds, the video helpers
  and the browser window; English is unchanged, right to left too
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192))
- ✅ Linux builds and window shots in minutes: a WSL2 machine builds any branch in about 5
  minutes (under one after a change) and shoots every window in any language in about a second
  each, never showing on the screen; the Linux desktops still check what needs them (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The Scheduler wakes the computer from sleep to run a queue, and keeps it awake while the
  queue runs: on Windows when the power plan allows wake timers (the Scheduler says when it
  doesn't), on Linux once an administrator allows it, and on macOS it says why it can't
  ([#142](https://github.com/anefzaoui/misterdownloadmanager/issues/142))
- ✅ A finished download no longer fails with "part file still in use" on a busy computer, and
  the rest of the engine tests that failed now and then there (slow connections, a prompt stop,
  speed limits, links that expire, sparse files and a few more) wait for what they check, not for
  time; the slow-connection test now fails if the engine stops reconnecting slow connections,
  which it didn't before ([#201](https://github.com/anefzaoui/misterdownloadmanager/issues/201))
- ✅ A window closed with its title bar's × keeps its contents until it goes, instead of turning
  black for a moment (seconds on Linux) ([#207](https://github.com/anefzaoui/misterdownloadmanager/issues/207))
- ✅ A freeze while MDM handles a command, or minimized, opens the crash report window and writes
  its freeze report again ([#202](https://github.com/anefzaoui/misterdownloadmanager/issues/202))
- ✅ Where a download's server is: its country's flag and name and its round trip (near, medium or
  far) in Download File Info, the progress window and Properties, from a table that comes with MDM
  (no address is sent anywhere); a proxy or a CDN said as such
  ([#140](https://github.com/anefzaoui/misterdownloadmanager/issues/140))
- ✅ Torrents: Download in order, to watch a video while it downloads (each file's first and last
  pieces first, its own choice); other torrents now take the rarest pieces first
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Every mark in the windows is drawn in every language (the browser window's ✔ Connected
  was an empty box, → on Linux), and the browser and video-helper windows put their logo at the
  start side in Arabic and Persian and fit a long German headline and subline
  ([#185](https://github.com/anefzaoui/misterdownloadmanager/issues/185),
  [#170](https://github.com/anefzaoui/misterdownloadmanager/issues/170))
- ✅ A Linux test machine for every change, scripted: an Ubuntu 24.04 desktop that builds each
  branch and drives MDM, Firefox and Chrome there (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ The Linux test machine also has KDE Plasma, Cinnamon and XFCE copies, which test a build
  without making it; store packages and theme windows are checked there too (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Firefox: reading the browser's proxy (#42) is an opt-in, a switch in the extension's popup, so
  the extension's update asks for no new permission; a VPN add-on's proxy goes with the downloads
  taken before Firefox's file dialog too, and addresses on this computer skip the proxy, as in
  Firefox
- ✅ Chrome and the other Chromium browsers: downloads go through the browser's proxy or VPN
  extension too (#42), with the same switch, off as installed: a fixed server for each scheme and
  its bypass list, or an auto-config script
- ✅ Streams whose address doesn't end in .m3u8 or .mpd, served as plain text, are found too: by
  the button over the video, the right-click menu and the media list, read only when asked
  ([#171](https://github.com/anefzaoui/misterdownloadmanager/issues/171))
- ✅ The extension in the [Chrome Web Store](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp): **Add to Chrome** beside **Add to Firefox**
  on the website, in the help, the FAQ and the README; MDM's **Add Mister Download Manager to
  your browser** window opens the store in Chrome, Edge, Brave, Opera and Vivaldi, with the
  manual install as the fallback
- ✅ Themes, the groundwork: every colour and size the controls are drawn with comes from one
  theme, and theme files can change them; the classic look is unchanged
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ View → Theme: Classic (the default), Native (the system's accent colour and high contrast,
  followed as they change) and High contrast
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Metro, Windows 8's flat look (its controls, a drawn title bar on Windows, Fluent
  icons); themes choose each control's style and bring their own icons
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Adwaita, GNOME's look (its controls, a header bar on Windows, GNOME's Adwaita icons
  in colour and one colour)
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Breeze, KDE Plasma's look (its focus frame, check boxes, buttons and title bar on
  Windows, KDE's Breeze icons)
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Luna, Aero and Aqua, the looks of Windows XP, Windows 7 and Mac OS X (shaded and
  glossy buttons, their check boxes and progress bars, their title bars on Windows, icons drawn
  for MDM); Metro, Adwaita and Breeze get MDM's own icons where theirs had none
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Base16 and pywal colour schemes imported as themes (View → Theme → Add a theme
  file…, or the themes folder); **pywal** follows `wal` as it writes new colours, text kept
  readable (4.5:1) ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Themes: Emerald (a white canvas, one near-black ink, emerald
  fills carrying near-black text, 6-point corners, Metro's one-colour icons); its theme file
  gives a few seeds per variant and derives every other colour, so a role added later arrives
  in this look rather than the classic one
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ Dark mode: grey notes and error texts readable (4.5:1), Options' failed checks no longer dark
  red on the dark dialog ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45))
- ✅ A minimized MDM keeps working: queues, the Scheduler, download limits, the kill switch and
  saving the list no longer wait for the window to be restored
- ✅ Download video: every button fits its text in every language (German **ffmpeg installieren**
  was cut), right to left too ([#172](https://github.com/anefzaoui/misterdownloadmanager/issues/172))
- ✅ The engine tests that failed now and then on a busy computer (a refused connection coming
  back, the learned connection limit, a dead address) wait for what they check, not for time
  ([#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136))
- ✅ HLS and DASH videos from the browser reach yt-dlp as their stream, by the button over the
  video, the right-click menu (it did nothing in Firefox) and the media list
  ([#155](https://github.com/anefzaoui/misterdownloadmanager/issues/155))
- ✅ A download or magnet link sent while the window is minimized opens its window at once
  ([#162](https://github.com/anefzaoui/misterdownloadmanager/issues/162))
- ✅ Linux: Ubuntu's Firefox (the Snap) reaches MDM: browser integration is set up for it, and the
  help says what the desktop asks the first time; Chromium as a Snap, which can't reach MDM, is no
  longer offered (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ A smaller download: the installer 15 % and the zips 19 % smaller, nothing removed
  ([#158](https://github.com/anefzaoui/misterdownloadmanager/issues/158))
- ✅ Linux: MDM's first window opens seconds sooner; the system's fonts are looked up all at once
  while it starts, not one by one in the window (on a busy computer that froze it, part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164))
- ✅ Firefox's add-on offered from [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/mister-download-manager/)
  first: **Add to Firefox** on the website, in the help, the FAQ and the README, and in MDM's
  **Add Mister Download Manager to your browser** window (for Zen and LibreWolf too)
- ✅ Support MDM: a way to support its creator on [GitHub Sponsors](https://github.com/sponsors/anefzaoui),
  asked for only (**Help → Support MDM**, a line in About, the website, the README, the
  extension's popup and GitHub's Sponsor button); MDM stays free, nothing changes or unlocks
- ✅ A server that can't resume a download no longer gets the file downloaded twice on a busy
  computer (part of [#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136))
- ✅ Translators: a reviewed translation is never replaced by machine translation, and CI checks it;
  Italian is the first catalog reviewed by a native speaker
  ([#150](https://github.com/anefzaoui/misterdownloadmanager/pull/150))
- ✅ Screen readers reach every row of a list, in view or not (300 around the view in long lists)
  ([#111](https://github.com/anefzaoui/misterdownloadmanager/issues/111))
- ✅ Less memory in the notification area after Minimize too, and again 2 minutes after hiding
  ([#72](https://github.com/anefzaoui/misterdownloadmanager/issues/72))
- ✅ Windows: MDM draws with Direct3D 12 by default; **View → Draw with** goes back to OpenGL, and
  either one falls back to the other when it can't start
  ([#89](https://github.com/anefzaoui/misterdownloadmanager/issues/89))
- ✅ A slow start no longer turns Direct3D off for good: its 10 seconds for the first frame count
  only Direct3D's own time, not MDM's loading (on one computer that sent MDM to OpenGL, where
  Intel's driver froze it); crash and freeze reports say near the top what drew, on which card
  and driver, and why
  ([#280](https://github.com/anefzaoui/misterdownloadmanager/issues/280))
- ✅ Linux and macOS: **View → Draw with → Vulkan** (Linux) and **Metal** (macOS), with the same
  fallback to OpenGL as Direct3D on Windows; OpenGL stays their default for now, and the menu
  says why when one can't start here
  ([#206](https://github.com/anefzaoui/misterdownloadmanager/issues/206))
- ✅ Screen readers see a window's contents the first time they read it, not only its title bar
  ([#131](https://github.com/anefzaoui/misterdownloadmanager/issues/131))
- ✅ Magnet links get their file list in seconds again: the DHT keeps going after a computer it
  asked is gone, and Add torrent shows the wait (peers found, from where, what may be in the
  way) ([#92](https://github.com/anefzaoui/misterdownloadmanager/issues/92))
- ✅ Two engine tests that failed now and then on busy machines are reliable
  ([#123](https://github.com/anefzaoui/misterdownloadmanager/issues/123))
- ✅ Numbers are grouped and pointed the interface language's way (German `10.200`, `1,50 MB`)
  ([#128](https://github.com/anefzaoui/misterdownloadmanager/issues/128))
- ✅ The extension's button lists the media the tab has loaded (HLS and DASH streams, video and
  sound files), to download a stream the page won't give up; copy-protected ones are shown as
  protected, never downloaded ([#38](https://github.com/anefzaoui/misterdownloadmanager/issues/38))
- ✅ Torrents: RSS feeds with download rules (words, episodes, each episode once, a folder per rule)
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Torrents: Recheck, Reannounce, Add trackers and Copy magnet link; choose files while a
  torrent runs, or in Torrent → Files… for any torrent (stopped or complete too); a check of the
  files says "Checking 42%"; a finished torrent that isn't shared lets go of its files; a
  `.torrent` file's web address opens Add torrent ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Link grabber: links in packages, looked up for name and size; archives (zip and tar
  built in; 7z and RAR need 7-Zip) extracted when they are downloaded
  ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37)); collecting copied links
  comes later
- ✅ Image galleries with gallery-dl: Wikimedia Commons, Flickr, DeviantArt and many more,
  verified like yt-dlp, or your own copy ([#58](https://github.com/anefzaoui/misterdownloadmanager/issues/58))
- ✅ Torrents: seeding limits (a share ratio, or a time), off by default, and a finished torrent's
  tabs while it is shared ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ A paused or finished download no longer leaves its progress file to be written again a moment
  later ([#106](https://github.com/anefzaoui/misterdownloadmanager/issues/106)), and a download
  finishing during a save no longer fails ([#116](https://github.com/anefzaoui/misterdownloadmanager/issues/116))
- ✅ Tests and test tools stay on 127.0.0.1: no Windows Firewall questions while developing
- ✅ Debrid accounts (experimental): Real-Debrid, AllDebrid and Premiumize; file-host links downloaded through your own account (Options → Accounts)
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts: a per-download "directly instead" choice, "through the service" in Download
  File Info, and the command line using the accounts
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts: torrents and magnet links through the service, their files downloaded over
  HTTP, no peer connections ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Debrid accounts (experimental): TorBox, for file-host links and torrents
  ([#391](https://github.com/anefzaoui/misterdownloadmanager/issues/391))
- ✅ Sign-ins in the system's keychain: Windows Credential Manager, the Secret Service keyring on
  Linux, the macOS keychain; for debrid accounts, Sites Logins and the proxy
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44))
- ✅ Downloads through one network adapter (a VPN's), with a kill switch: nothing goes out
  another way when it disconnects ([#35](https://github.com/anefzaoui/misterdownloadmanager/issues/35))
- ✅ Torrents on Windows through the chosen network adapter, with the same kill switch
  ([#143](https://github.com/anefzaoui/misterdownloadmanager/issues/143))
- ✅ Video pages and image galleries through the chosen network adapter too, with the same kill
  switch ([#35](https://github.com/anefzaoui/misterdownloadmanager/issues/35))
- ✅ Downloads from a browser behind a VPN or proxy extension go through the same proxy
  ([#42](https://github.com/anefzaoui/misterdownloadmanager/issues/42))
- ✅ Hide to the notification area: a download's progress window (its own button, or Minimize)
  and the main window's Minimize, with the tray's "Downloads in progress"
  ([#63](https://github.com/anefzaoui/misterdownloadmanager/issues/63))
- ✅ Whole playlists and channels, one quality for all: also as the original sound, MP3, Opus or FLAC
  ([#69](https://github.com/anefzaoui/misterdownloadmanager/issues/69))
- ✅ Choose the DNS: Cloudflare, Google, Quad9, AdGuard, NextDNS or your own, over HTTPS, TLS or
  plain, with a Test button and no silent fallback
  ([#36](https://github.com/anefzaoui/misterdownloadmanager/issues/36))
- ✅ Less memory in the notification area: under 10 MB there, and the language list's fonts let
  go when it closes ([#72](https://github.com/anefzaoui/misterdownloadmanager/issues/72))
- ✅ Video downloads: subtitles, beside the video or inside it
  ([#68](https://github.com/anefzaoui/misterdownloadmanager/issues/68)); more audio choices:
  the original track, Opus and FLAC ([#70](https://github.com/anefzaoui/misterdownloadmanager/issues/70))
- ✅ Torrent security: stricter file-name checks, no silent overwrite, files marked as downloaded
  from the internet, network switches (incoming, UPnP, port, DHT, local peers), SOCKS5 proxy and
  IP blocklist ([#40](https://github.com/anefzaoui/misterdownloadmanager/issues/40))
- ✅ Torrents: a speed limit per torrent, remembered limits, and the global cap covering torrents
  too ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Torrents: upload limits (all torrents, and each one), and a share ratio that survives pauses
  and restarts ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39))
- ✅ Google Drive: public files download by their share links
  ([#34](https://github.com/anefzaoui/misterdownloadmanager/issues/34)); folders come later
- ✅ Video downloads: Stop ends yt-dlp and everything it started
  ([#61](https://github.com/anefzaoui/misterdownloadmanager/issues/61)); no error while yt-dlp
  goes on and finishes ([#79](https://github.com/anefzaoui/misterdownloadmanager/issues/79)); a
  quality the reachable YouTube servers don't serve falls back to the nearest one
  ([#94](https://github.com/anefzaoui/misterdownloadmanager/issues/94))
- ✅ A video's sound as MP3, at the quality you pick
  ([#32](https://github.com/anefzaoui/misterdownloadmanager/issues/32))
- ✅ More than 32 connections per download, if you want them
  ([#12](https://github.com/anefzaoui/misterdownloadmanager/issues/12))
- ✅ Your own yt-dlp and ffmpeg ([#51](https://github.com/anefzaoui/misterdownloadmanager/issues/51))
- ✅ View → Always dark ([#60](https://github.com/anefzaoui/misterdownloadmanager/issues/60))
- ✅ Zen, Helium and other Firefox- and Chromium-based browsers
  ([#41](https://github.com/anefzaoui/misterdownloadmanager/issues/41))
- ✅ Linux: menus open where clicked under Wayland
  ([#50](https://github.com/anefzaoui/misterdownloadmanager/issues/50)), and the window closes on
  desktops without a tray, such as MangoWC ([#59](https://github.com/anefzaoui/misterdownloadmanager/issues/59))
- ✅ Downloads that hand out one-time links no longer fail with 403
  ([#28](https://github.com/anefzaoui/misterdownloadmanager/issues/28))
- ✅ The extension's Download video menu keeps its highlight inside its edge
  ([#81](https://github.com/anefzaoui/misterdownloadmanager/issues/81))
- ✅ No crash when a window opens while MDM is minimized or in the notification area, such as a
  download or a magnet link from the browser
  ([#22](https://github.com/anefzaoui/misterdownloadmanager/issues/22),
  [#145](https://github.com/anefzaoui/misterdownloadmanager/issues/145))
- ✅ Long file names stay inside the progress window: the Save to path is shortened in the
  middle, in every language ([#62](https://github.com/anefzaoui/misterdownloadmanager/issues/62))
- ✅ Every button translated: Download complete's Open with... no longer stays in English
  ([#80](https://github.com/anefzaoui/misterdownloadmanager/issues/80))
- ✅ A smaller download ([#25](https://github.com/anefzaoui/misterdownloadmanager/issues/25)),
  AppImage updates with zsync ([#24](https://github.com/anefzaoui/misterdownloadmanager/issues/24)),
  the Bangladesh flag for Bangla ([#27](https://github.com/anefzaoui/misterdownloadmanager/issues/27))
- ✅ Download complete shows above other windows, Options from the extension comes to the front
  ([#77](https://github.com/anefzaoui/misterdownloadmanager/issues/77)), and dragging the file out
  of Download complete works in every language
  ([#78](https://github.com/anefzaoui/misterdownloadmanager/issues/78))
- ✅ The built-in categories can be renamed, given other file types and deleted, and restored
  ([#93](https://github.com/anefzaoui/misterdownloadmanager/issues/93))
- ✅ A file-type icon before every file name, in every window that names a file (the download
  list, Download File Info, Properties, Download complete, Add torrent, a torrent's files, the
  Scheduler, Batch download, Download video and its playlists), each the system's own icon for
  its type (Windows Explorer's, macOS Finder's, the Linux desktop's icon theme), sharp at every
  display scale
  ([#122](https://github.com/anefzaoui/misterdownloadmanager/issues/122)), and every
  list's rows named for screen readers ([#95](https://github.com/anefzaoui/misterdownloadmanager/issues/95)),
  inside their list, with their place in it ([#102](https://github.com/anefzaoui/misterdownloadmanager/issues/102));
  the Categories pane, the menu bar and every right-click menu too, with Shift+F10 and the Menu key
  ([#113](https://github.com/anefzaoui/misterdownloadmanager/issues/113))
- ✅ The README's, the docs' and the website's screenshots are taken on a stock Windows (GitHub's
  runner), so the file icons in them are Windows' own
- ✅ The help shows what it describes: pictures of each window, numbered where a step points at
  one control, step strips for the usual flows, in light and dark, made from one shot list
  ([#228](https://github.com/anefzaoui/misterdownloadmanager/issues/228))
- ✅ Website in European Portuguese, and an [intended use](https://misterdownloadmanager.com/intended-use.html) page
- ✅ Website: the [free IDM alternative](https://misterdownloadmanager.com/idm-alternative.html) page compares
  MDM with IDM and with nine other free download managers, each checked on its own site, with
  their logos, a feature-by-feature form and an MDM-vs-IDM label with the benchmark's times to scale
- ✅ Website: the header fits its row in every language at every width (it hides the Sponsor heart,
  then tightens, then turns into the menu, only as far as a language needs)
  ([#168](https://github.com/anefzaoui/misterdownloadmanager/issues/168)), and no page scrolls
  sideways on a phone, Arabic and Persian included: long paths, links and tables wrap or scroll
  inside the page ([#169](https://github.com/anefzaoui/misterdownloadmanager/issues/169))
- ✅ Help opens the website's help pages, in the interface language when the website has it
  ([#227](https://github.com/anefzaoui/misterdownloadmanager/issues/227))
- ✅ Website: a page for every feature, old and new, with its real window, where it is in the app
  and what it can't do yet; tiles of them all on the home page; and a
  [Themes](https://misterdownloadmanager.com/themes.html) page with every built-in theme, light and
  dark, and how to make, try and share your own
  ([#260](https://github.com/anefzaoui/misterdownloadmanager/issues/260))
- ✅ Thanks to the people who make MDM possible: everyone who sent a pull request, opened an issue
  or commented on GitHub, as round photos in the home page's first view and the README, on a
  [Contributors](https://misterdownloadmanager.com/contributors.html) page with what each one did,
  at the end of each release on the News page, and in each release's notes on GitHub
- ✅ Themes ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45)): today's look stays the default. Every colour, size and spacing now
  comes from the chosen theme (the classic look pixel for pixel the same); **View → Theme** with
  Classic, Native (follows the system's accent and contrast on Windows, Linux and macOS) and High
  contrast; style themes with their own control shapes, a drawn title bar on Windows and their own
  icons: Metro, Adwaita, Breeze, Luna, Aero and Aqua; and theme files anyone can make, try and
  share, in TOML or JSON, built on any theme (`inherits`), with their own icons; Base16 and
  pywal colour schemes read as themes ([the guide](docs/themes.md))
- ✅ Linux, no longer experimental: every change is checked on Ubuntu (GNOME) as well as Windows,
  and on KDE Plasma, Cinnamon and Xfce; Wayland sessions (through XWayland, menus under their
  titles on sway too); the .deb and .rpm installed in clean Ubuntu, Debian and Fedora systems, the
  AppImage and the tar.gz; the tray, Dark Mode support and Launch on startup on all four desktops;
  the Snap Firefox, Google Chrome and Brave reaching MDM
  ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)), and its release files
  named without "-experimental"
- ✅ Every screenshot taken again on Windows and Linux (smaller files, WebP), and every page of the
  docs, the README and the website brought up to date
  ([#186](https://github.com/anefzaoui/misterdownloadmanager/issues/186))

## Version 0.5.2

Bug fixes from the first days of 0.5.0, a Safari extension, and more file types.

- ✅ A link from the browser no longer ends in 403 where a site refuses the browser's
  User-Agent: MDM asks once more as itself ([#372](https://github.com/anefzaoui/misterdownloadmanager/issues/372))
- ✅ The video button no longer covers a control of the page (a preview's own button): it takes
  a free corner, and dragging it keeps it where you put it
  ([#421](https://github.com/anefzaoui/misterdownloadmanager/issues/421))
- ✅ A background task stopping as MDM closes no longer shows a crash report
  ([#422](https://github.com/anefzaoui/misterdownloadmanager/issues/422))
- ✅ A freeze that was over before its dump was taken is reported as a freeze, not a crash in the
  watchdog (part of [#420](https://github.com/anefzaoui/misterdownloadmanager/issues/420))
- ✅ File names and titles in any script show their letters: the system font for a script
  outside the interface language loads the first time it is needed, instead of boxes
  ([#405](https://github.com/anefzaoui/misterdownloadmanager/issues/405))
- ✅ yt-dlp and gallery-dl use the proxy set in Options → Proxy / Socks
  ([#363](https://github.com/anefzaoui/misterdownloadmanager/issues/363))
- ✅ The window keeps answering while reading the clipboard or removing a cancelled download's
  files from a slow disk ([#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418),
  [#423](https://github.com/anefzaoui/misterdownloadmanager/issues/423))
- ✅ Fast downloads keep going on a slow disk: flushing the resume state no longer holds up
  the connections, and a connection is never replaced for the disk's wait
  ([#361](https://github.com/anefzaoui/misterdownloadmanager/issues/361))
- ✅ Freeze reports taken after the window has already answered say so, instead of blaming a deadlock
  (part of [#355](https://github.com/anefzaoui/misterdownloadmanager/issues/355))
- ✅ **Move/Rename** works on several selected downloads at once: pick a folder and they all go there
  ([#333](https://github.com/anefzaoui/misterdownloadmanager/issues/333))
- ✅ Download video shows the size of a plain file (what the server says), not "size unknown"
  (part of [#320](https://github.com/anefzaoui/misterdownloadmanager/issues/320))
- ✅ Download video on pages whose player first loads a manifest's address answering something else
  (SVT Play): the real manifest goes to MDM, not that first answer
  ([#377](https://github.com/anefzaoui/misterdownloadmanager/issues/377))
- ✅ Windows: sharp, evenly spaced text at every interface size: letters no longer run together
  or drift apart ([#323](https://github.com/anefzaoui/misterdownloadmanager/issues/323),
  [#334](https://github.com/anefzaoui/misterdownloadmanager/issues/334))
- ✅ Far more file types taken by default: AI models and datasets, 3D and CAD, design and camera
  RAW, ebooks, Office, disk images and split archives; a list never edited gains them
  ([#382](https://github.com/anefzaoui/misterdownloadmanager/issues/382))
- ✅ A save folder relative to the home folder is found where it really is
- ✅ Browser link import: tighter handling of site data
- ✅ The extension asks for a newer MDM only when a whole release separates them, not over a
  bug-fix version
- ✅ Add URL: a YouTube playlist opens the video window; Dailymotion's embedded player is looked
  up as the video's own page ([#338](https://github.com/anefzaoui/misterdownloadmanager/issues/338),
  [#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359))

## Version 0.5.1

- ✅ The Download video button on YouTube: the video playing now instead of the one before or a
  whole list, Shorts included
  ([#311](https://github.com/anefzaoui/misterdownloadmanager/issues/311))
- ✅ The Download video button on YouTube in Chrome, Edge, Brave, Opera and Vivaldi

## Done since 0.5.2

These ship in the next version.

- ✅ **Deferred Rename** on a download's **Download status** tab: save a new name while downloading to rename the finished file's stem (and video subtitles) or an exclusive finished folder on completion, leaving shared/save or pre-existing folders, split-archive volumes and single-file torrents under their original names ([#655](https://github.com/anefzaoui/misterdownloadmanager/pull/655))
- ✅ Links added by hand go with the User-Agent of your default browser, at the version you have
  (Chrome, Edge, Brave, Opera, Vivaldi, Firefox, Safari), instead of a years-old one; **Automatic**
  in Options, or one you type (part of
  [#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668))
- ✅ Linux freeze and crash reports name MDM's functions again, and the right ones: the stack of
  a program whose code begins a page after its start (0.5.0's) was read at the wrong place, so
  reports showed only system libraries, or names from elsewhere in MDM
  ([#661](https://github.com/anefzaoui/misterdownloadmanager/issues/661))
- ✅ Windows: a freeze in the C runtime's memory routines, where the window hardly ran, is
  reported as a computer too busy or slow to load MDM from disk, not as a deadlock
  ([#663](https://github.com/anefzaoui/misterdownloadmanager/issues/663))
- ✅ macOS: helpers opened from Finder find Homebrew's Deno and ffmpeg, so YouTube formats
  load ([#659](https://github.com/anefzaoui/misterdownloadmanager/issues/659))
- ✅ The website is named as the official one in the README, the browser add-on and the
  site's data for search engines
- ✅ Left click on the tray icon toggles the main window (on Windows also with the icon under
  the taskbar's ^), and browser downloads open only their dialog when the main window is
  hidden or minimized ([#520](https://github.com/anefzaoui/misterdownloadmanager/pull/520))
- ✅ Debrid accounts: signing in by code opens only the service's own page
- ✅ A speed limit raised or turned off applies at once, to the connections already waiting
  under the old one too ([#621](https://github.com/anefzaoui/misterdownloadmanager/issues/621))
- ✅ The build and release workflows: each job gets only the permissions it needs, and every
  action and release tool is pinned to an exact version
- ✅ A freeze on a slow disk, the data folder looked up on every tick
  ([#565](https://github.com/anefzaoui/misterdownloadmanager/issues/565))
- ✅ A `funding.json` file, the open format funds use to find projects and what they need
- ✅ The tools that send the browser extension to the stores, at their current versions
- ✅ macOS: downloads to a network volume (SMB, AFP, NFS), refused for "0 bytes free"
  ([#623](https://github.com/anefzaoui/misterdownloadmanager/issues/623))

## Being worked on

These ship in a later version, when they're ready.

### Being built

- 🔄 Helium: MDM offers it the extension on Linux and macOS, through Helium's own services, the
  Linux packages' helper files cover it, and the docs say how to add the extension in Helium
  ([#685](https://github.com/anefzaoui/misterdownloadmanager/issues/685)); in review
- 🔄 A freeze of the window while torrents ran on a busy or slow disk: it waited for the torrent
  engine, which holds a torrent while its writes to the disk end
  ([#693](https://github.com/anefzaoui/misterdownloadmanager/issues/693)): the torrents' numbers
  are now read beside the window; fix in review
- 🔄 Sites with bot protection (Cloudflare and similar) refusing MDM: when a site still answers
  with a bot check, asking again the way a browser connects
  ([#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668))
- 🔄 Download video: a sound-only choice (MP3, a playlist as sound) saved in the Music folder and
  listed under Music ([#611](https://github.com/anefzaoui/misterdownloadmanager/issues/611)):
  in review
- 🔄 Linux and macOS: the system's gallery-dl used when MDM has none of its own, as yt-dlp is
  ([#571](https://github.com/anefzaoui/misterdownloadmanager/issues/571)): in review
- 🔄 Videos sent in pieces (HLS, DASH) downloading several pieces at once, as many as a file's
  connections ([#531](https://github.com/anefzaoui/misterdownloadmanager/issues/531)): in review;
  a video in one file (most of YouTube's) still comes over one connection
- 🔄 Linux and macOS: the system's yt-dlp used when MDM has none of its own, with Helpers able\n  to switch to MDM's copy ([#452](https://github.com/anefzaoui/misterdownloadmanager/issues/452)): in review
- 🔄 macOS: video look-ups in a copy of MDM a browser downloaded, without the Terminal command
  ([#330](https://github.com/anefzaoui/misterdownloadmanager/issues/330),
  [#339](https://github.com/anefzaoui/misterdownloadmanager/issues/339)): yt-dlp's folder build,
  unpacked once and checked against its signed list; built and checked on macOS, still to
  confirm on a person's Mac
- 🔄 In English, the list's dates on the system's 12- or 24-hour clock
  ([#392](https://github.com/anefzaoui/misterdownloadmanager/issues/392)): in review
- 🔄 Help: when an antivirus or VirusTotal flags MDM
- 🔄 The Download video button lists the video's qualities, with their sizes, in a menu on the
  page, video and audio in their own tabs: the one picked downloads at once with only its progress
  window, MDM's main window staying where it is; "Open in Mister Download Manager…" keeps the
  Download video window. A stream the page plays (HLS) is listed at once, read by MDM itself; the
  same video asked again within 10 minutes opens at once; the pick starts without looking the
  video up a second time: in review
- 🔄 Picking a video quality already in the list (the menu over a video, the Download video window)
  follows Options' duplicate setting instead of downloading it again: in review
- 🔄 The progress window's bar glides instead of moving in steps: in review
- 🔄 Linux, what's left ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)):
  screen readers (Orca) reading MDM's windows; browsers installed as a Flatpak (Firefox from
  Flathub may reach MDM through the desktop, as the Snap does: to check); native Wayland windows
  (`MDM_WAYLAND=1`) placing their menus, which needs popup windows the window library doesn't
  have yet, so XWayland stays the default; Vulkan as the default renderer (below)

- 🔄 The Safari extension (macOS), part of the app: a click on a link to a listed file type, the
  right-click items, the button over videos, Media on this page and the toolbar button, from one
  source with the other browsers' extension. Safari gives extensions no way to see its downloads,
  so a download a page starts by itself stays with Safari. Built and its relay to the helper
  checked in the sandbox Safari uses; still to check in Safari itself, signed with a Developer ID

- 🔄 A torrent's upload speed, peers and share ratio as columns in the download list
  ([#357](https://github.com/anefzaoui/misterdownloadmanager/issues/357)): the last one in review
- 🔄 The browser extension's media list naming a file after its page, as an option: in review
- 🔄 Malay: a native speaker's check of the menus, toolbar and list columns: in review
- 🔄 Cancel in Download File Info asking whether the downloads waiting behind it are cancelled
  too (a browser selection's arrive one by one)
  ([#394](https://github.com/anefzaoui/misterdownloadmanager/issues/394)): in review

### Bugs being looked into

- 🔄 The browser's video button over a video in a feed sending the feed's address, where yt-dlp
  finds nothing (LinkedIn, Facebook, and other sites with feeds;
  [#689](https://github.com/anefzaoui/misterdownloadmanager/issues/689),
  [#643](https://github.com/anefzaoui/misterdownloadmanager/issues/643)): it now sends the post the
  video sits in (LinkedIn, Facebook, X, Instagram, Reddit, TikTok, Vimeo, Dailymotion, and a
  post's own link on other sites), else the video's own file, else it says to open the post; fix
  in review, checked on a signed-in LinkedIn feed and Facebook's video page; Facebook's feed
  posts still to check
- 🔄 Torrents: a pause landing while a piece was being checked leaving the torrent one piece short
  until MDM restarted ([#706](https://github.com/anefzaoui/misterdownloadmanager/issues/706)): the
  piece is now downloaded again after the pause; fix in review
- 🔄 Windows: the tray icon's menu light while MDM is dark
  ([#682](https://github.com/anefzaoui/misterdownloadmanager/issues/682)): it now follows MDM's
  look, dark or light, on Windows 10 and 11; fix in review
- 🔄 Torrents: magnet links without trackers finding no peers where the DHT's first nodes didn't
  answer ([#650](https://github.com/anefzaoui/misterdownloadmanager/issues/650)): four public
  routers asked together, the ones that never answer left alone once one has, the IPv6 nodes kept
  for the next start too, and the port picked kept from one start to the next; fix in review
- 🔄 Windows: the window stopping on a stalled system while it checked its title bar's colours
  ([#633](https://github.com/anefzaoui/misterdownloadmanager/issues/633),
  [#583](https://github.com/anefzaoui/misterdownloadmanager/issues/583)): the colours are now set
  once, when a window opens or the theme changes; fix in review
- 🔄 Downloads from FileHippo failing at once with 412 Precondition Failed
  ([#630](https://github.com/anefzaoui/misterdownloadmanager/issues/630)): a link the site refuses
  to MDM now goes on from where the browser's own download of it had reached; fix in review, still
  to check on FileHippo itself. A link pasted into Add URL has no such address to go on from
- 🔄 Windows: a crash when the clipboard was read in two places at once (the clipboard monitor and
  Add URL) ([#629](https://github.com/anefzaoui/misterdownloadmanager/issues/629)): every read and
  write of the clipboard now goes one at a time; fix in review
- 🔄 The window waiting on the program that owns the clipboard when it doesn't answer (Linux
  especially; follows [#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418)): a
  text box's right-click menu, Add URL, Add batch download, Add torrent and Download video now
  read the clipboard beside the window, and fill in a copied link when it comes; fix in review
- 🔄 Categories without their own folder following General's folder (`Video`, `Music`, `Torrents`
  inside it), and Add torrent starting with the Torrents category's folder instead of flipping
  from General when metadata arrives ([#556](https://github.com/anefzaoui/misterdownloadmanager/issues/556),
  [#550](https://github.com/anefzaoui/misterdownloadmanager/issues/550)): fix in review
- 🔄 The window not answering while a file was dragged out of Download complete, for as long as
  the program under the pointer took ([#561](https://github.com/anefzaoui/misterdownloadmanager/issues/561)): the drag now runs beside the window; fix
  in review, to check with a real drag
- 🔄 The video button jumping between corners over YouTube's thumbnail previews, out of the
  pointer's reach ([#560](https://github.com/anefzaoui/misterdownloadmanager/issues/560)): fix in
  review
- 🔄 A submenu staying open, its item lit beside the one under the pointer, while the pointer
  moved on through the menu ([#579](https://github.com/anefzaoui/misterdownloadmanager/issues/579)): fix in review
- 🔄 Chromium browsers: another extension's file names and folders (Save In's) lost while MDM's
  extension was on, for downloads it left to the browser
  ([#578](https://github.com/anefzaoui/misterdownloadmanager/issues/578)): fix in review
- 🔄 Without ffmpeg, YouTube's sound alone saved as `.mp4` though it was raw AAC from HLS pieces,
  which no player opened ([#569](https://github.com/anefzaoui/misterdownloadmanager/issues/569)): fix in review
- 🔄 A progress window's title bar buttons not answering
  ([#540](https://github.com/anefzaoui/misterdownloadmanager/issues/540)): a window with a title of
  256 characters or more got the system's title bar above MDM's own; that part is fixed, in review.
  Still to find: whether the reporter's window was one of them
- 🔄 macOS 15 and later: PingFang moved into the system's font assets, so Chinese text lost its
  font ([#541](https://github.com/anefzaoui/misterdownloadmanager/issues/541)): fix in review
- 🔄 The video button still showing after it was switched off, or its site listed, in Options
  ([#532](https://github.com/anefzaoui/misterdownloadmanager/issues/532)): fix in review
- 🔄 macOS: Persian and other Arabic-script file names drawn as boxes in the download list
  ([#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533)): fix in review, to
  confirm on a Mac
- 🔄 macOS: Download File Info opening behind the browser that asked for it\n  ([#484](https://github.com/anefzaoui/misterdownloadmanager/issues/484)): fix in review
- 🔄 macOS: no icon in the menu bar, so closing the window quit MDM
  ([#529](https://github.com/anefzaoui/misterdownloadmanager/issues/529)): fix in review, still to
  confirm on a Mac
- 🔄 The browser tests: the video button's focus case ran after the one that turns the button
  off for the test site, and timed out in a full run ([#480](https://github.com/anefzaoui/misterdownloadmanager/issues/480)): fix in review
- 🔄 The video button vanishing over a hover-to-play preview (YouTube's thumbnails) before it
  can be clicked ([#503](https://github.com/anefzaoui/misterdownloadmanager/issues/503)): fix in review
- 🔄 Add URL's **Use authorization** empty for an address with a login in Options → Sites
  Logins, though the download used it
  ([#511](https://github.com/anefzaoui/misterdownloadmanager/issues/511)): fix in review
- 🔄 Windows: closing the window left MDM in the taskbar instead of the notification area
  when its icon was among the hidden icons behind ^
  ([#545](https://github.com/anefzaoui/misterdownloadmanager/issues/545),
  [#534](https://github.com/anefzaoui/misterdownloadmanager/issues/534),
  [#527](https://github.com/anefzaoui/misterdownloadmanager/issues/527)): fix in review
- 🔄 Two tests that fail now and then on busy CI runners
  ([#518](https://github.com/anefzaoui/misterdownloadmanager/issues/518)): the slow-flush one
  now measures the transfer instead of how fast the runner's disk takes in the file (Windows
  runners' disks failed it again), fix in review; the crash-and-resume one found a real delay in
  saving a new download's progress, fix in review
- 🔄 The window not answering for a few seconds while Windows makes a snapshot of the disk
  (a restore point or a backup) ([#513](https://github.com/anefzaoui/misterdownloadmanager/issues/513)):
  the log and the RSS feeds are now written beside the window, as the list already was; fix in
  review, to confirm on the reporter's computer
- 🔄 The window not answering while Windows took seconds to look up the time zone for the
  queues' schedules ([#536](https://github.com/anefzaoui/misterdownloadmanager/issues/536)):
  looked up beside the window now; fix in review
- 🔄 Video look-ups failing with "WinError 448 ... untrusted mount point" when a folder on PATH
  goes through a junction Windows won't let yt-dlp cross
  ([#508](https://github.com/anefzaoui/misterdownloadmanager/issues/508)): fix in review
- 🔄 Dailymotion's embedded player from the browser's button: no formats, the embedding site's
  Referer refused ([#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359)): fix in review
- 🔄 The video button staying on screen after a click on the player, in fullscreen too\n  ([#480](https://github.com/anefzaoui/misterdownloadmanager/issues/480)): fix in review
- 🔄 A magnet link handed over while Add torrent is already open being dropped without a word
  ([#507](https://github.com/anefzaoui/misterdownloadmanager/issues/507)): fix in review
- 🔄 A freeze when a video finishes, while its file is marked as downloaded from the web
  ([#510](https://github.com/anefzaoui/misterdownloadmanager/issues/510)): fix in review
- 🔄 Linux: Dark Mode support on window managers such as Hyprland and sway, which often have no
  desktop portal to say dark or light ([#324](https://github.com/anefzaoui/misterdownloadmanager/issues/324)): fix in review
- 🔄 A bright white line under the menu bar in dark mode
  ([#379](https://github.com/anefzaoui/misterdownloadmanager/issues/379)): fix in review
- 🔄 Closing the main window follows a setting (on by default, MDM keeps running), and with no
  tray icon the window goes to the taskbar instead of vanishing: fix in review
  ([#457](https://github.com/anefzaoui/misterdownloadmanager/pull/457))
- 🔄 Download File Info staying open but invisible, with later browser downloads waiting behind
  it ([#440](https://github.com/anefzaoui/misterdownloadmanager/issues/440)): fix in review
- 🔄 The Download video window not remembering the folder you saved into
  ([#475](https://github.com/anefzaoui/misterdownloadmanager/issues/475)): fix in review
- 🔄 Arabic and other non-Latin file names saved unreadable, from Firefox and browsers built on it
  ([#447](https://github.com/anefzaoui/misterdownloadmanager/issues/447)): fix in review
- 🔄 A file renamed to avoid a clash still showing and opening under its old name
  ([#442](https://github.com/anefzaoui/misterdownloadmanager/issues/442)): fix in review
- 🔄 Download video's message when a site's bot check (Cloudflare) refuses yt-dlp
  ([#317](https://github.com/anefzaoui/misterdownloadmanager/issues/317)): fix in review
- 🔄 RSS feeds: the rule boxes losing the keyboard after each key
  ([#353](https://github.com/anefzaoui/misterdownloadmanager/issues/353)): fix in review
- 🔄 Video progress at 100 % while still downloading, and long encoded strings as file names
  ([#344](https://github.com/anefzaoui/misterdownloadmanager/issues/344),
  [#345](https://github.com/anefzaoui/misterdownloadmanager/issues/345)): fix in review
- 🔄 A page that starts a download by script being saved instead of the file
  ([#374](https://github.com/anefzaoui/misterdownloadmanager/issues/374)): fix in review
- 🔄 Google Colab: only tokenized runtime-proxy transfers stay with the browser; ordinary notebook links remain capturable ([#603](https://github.com/anefzaoui/misterdownloadmanager/issues/603)): fix in review
- 🔄 A gofile share link saving a web page instead of the file
  ([#346](https://github.com/anefzaoui/misterdownloadmanager/issues/346)): fix in review
- 🔄 A server's refusal page saved under the document's name and shown as complete, where the
  browser had the file (part of [#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533)):
  fix in review
- 🔄 "Hide tab" in a download's window only going back to Download status, the tab still
  there ([#326](https://github.com/anefzaoui/misterdownloadmanager/issues/326)): fix in review
- 🔍 A site's bot check refusing MDM's request (testfile.org, a Cloudflare challenge): the\n  message now says so ([#376](https://github.com/anefzaoui/misterdownloadmanager/issues/376)); a way to hand the download back to the browser is still to decide
- 🔄 A download starting again by itself after Stop, when it is in a running queue\n  ([#341](https://github.com/anefzaoui/misterdownloadmanager/issues/341)): fix in review
- 🔄 A crash when removing the last feed in the RSS feeds window
  ([#321](https://github.com/anefzaoui/misterdownloadmanager/issues/321)): fix in review
- 🔄 **Open folder** on a torrent still downloading
  ([#354](https://github.com/anefzaoui/misterdownloadmanager/issues/354)): fix in review
- 🔄 Export and Import for the sites the download button stays off
  ([#424](https://github.com/anefzaoui/misterdownloadmanager/issues/424)): fix in review
- 🔄 Videos in the progress window: no percentage when the size isn't known, and the real file
  size at the end: fix in review
- 🔄 X (Twitter) videos from the media list named after the tab instead of a random file name
  ([#493](https://github.com/anefzaoui/misterdownloadmanager/issues/493)): fix in review
- 🔄 **Open folder** on Windows always starting Explorer, even with another file manager set up
  to open folders ([#446](https://github.com/anefzaoui/misterdownloadmanager/issues/446)): fix in review
- 🔄 Alt and F10 opening the menu bar with a control focused
  ([#328](https://github.com/anefzaoui/misterdownloadmanager/issues/328)): fix in review
- 🔄 Crash on Windows when a window opens while MDM falls back from Direct3D to OpenGL
  ([#337](https://github.com/anefzaoui/misterdownloadmanager/issues/337)): fix in review
- 🔄 Crash on Windows when a screen goes away while MDM runs
  ([#335](https://github.com/anefzaoui/misterdownloadmanager/issues/335)): fix in review
- 🔄 A duplicate prompt for a finished file that is gone
  ([#342](https://github.com/anefzaoui/misterdownloadmanager/issues/342)): fix in review
- 🔄 No size for a stream's qualities (HLS) in the Download video window: fix in review
- 🔍 A 10-second freeze of the window after a night left open
  ([#304](https://github.com/anefzaoui/misterdownloadmanager/issues/304)): most likely the
  computer waking from sleep; waiting on the reporter's full report
- 🔄 Crash reports: a monitor that cannot open its socket exits, and an unreachable one is
  cleaned up instead of staying beside MDM: fix in review
  ([#458](https://github.com/anefzaoui/misterdownloadmanager/pull/458))

## Planned

### Look and performance

- 📋 Vulkan the default on Linux and Metal on macOS, once real graphics cards have tried them (AMD,
  Intel and NVIDIA, X11 and Wayland, older cards without Vulkan)
  ([#206](https://github.com/anefzaoui/misterdownloadmanager/issues/206))
- 📋 Benchmarks against other download managers
  ([#71](https://github.com/anefzaoui/misterdownloadmanager/issues/71))
