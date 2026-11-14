# Changelog

Each release's notes on GitHub come from its section here (`## [X.Y.Z]`, see
`.github/workflows/release.yml`).

## [Unreleased]

### Added

- The browser's **Download panels** (the video button) site exclusions list supports wildcards (`*`) like `*.youtube.*`, `youtube.*` or `*dailymotion*` to hide the button across whole domain families and regional TLDs ([#759](https://github.com/anefzaoui/misterdownloadmanager/issues/759)).
- **Extract in place with HatchZip** ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37)): when [HatchZip](https://hatchzip.dev) is installed (its command line program, `hatchzip`), **Options → Archives → Extract in place with HatchZip** removes the archive while its files come out, so a large download on a nearly full disk needs room for its files only. Off by default. HatchZip checks that everything fits and proves a password before it changes anything; when it stops partway, the files extracted so far stay in a folder beside the archive and **Extract** continues (only a run MDM started: a HatchZip progress file that comes with a download starts nothing). Nothing HatchZip extracted is ever deleted: entries left out, or files a failed move couldn't put in place, stay in that folder and the log says where. 7z and RAR archives still need 7-Zip unless the option is on.
- Windows and macOS: **Options → General → Shortcut to show or hide window**: a 2-key or 3-key shortcut (such as `Cmd+U`, `Shift+Cmd+M`, or `Ctrl+Alt+D`) brings MDM's window to the front when hidden in the notification area, minimized, or behind another application, and hides it again when pressed while focused ([#558](https://github.com/anefzaoui/misterdownloadmanager/issues/558)).
- **Add torrent**: a **Rename** row under **Save to** with separate boxes for the main name and the file extension (for a single-file torrent, both the stem and the extension can be edited before the download starts; for a multi-file torrent, the main box renames the torrent's folder and the extension box is disabled) ([#517](https://github.com/anefzaoui/misterdownloadmanager/issues/517)).
- **Click'n'Load** ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37)): a link site's **Click'n'Load** button sends its links to Link grabber, in the site's packages with their archive passwords, once **Options → Archives and Link grabber → Accept Click'n'Load from web pages** is on (off by default; port 9666, this computer only). The first time a site sends links, MDM asks whether to allow it (the site your browser names; an unknown site is asked about every time). Links a page sent aren't looked up, links to this computer or your network are left out, and nothing is downloaded before you add it; posts are limited for each site. MDM doesn't answer the check for JDownloader some sites make first, so their button may say no program was found. See [Link grabber](docs/link-grabber.md#clickn-load).
- **Mega-Debrid accounts** ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44)): in **Options → Accounts → Add...**, sign in with your user name and password, or with an application's API key from your Mega-Debrid account (then MDM keeps no password). File-host links it supports are then downloaded through it, as with the other services. Torrents don't go through Mega-Debrid yet; they go to the first account that takes them. Experimental: built from Mega-Debrid's public API documentation.
- **Embed chapters** in the Download video window, under **Subtitles**, ticked to begin with: a video's chapters (YouTube's list of parts under the player, with their titles and start times) go into the downloaded file, so VLC, MPV, MPC-HC and Windows' Media Player list them and skip from one to the next. It needs ffmpeg, stays as you leave it for the next video, applies to a playlist's videos too, and isn't offered for an MP3, Opus or FLAC choice. The command line's `video-get` and `playlist-get` put them in too ([#827](https://github.com/anefzaoui/misterdownloadmanager/issues/827)).
- **Add batch download from clipboard** reads every link you copied: a list, a forum post, a page's text. Every `http`, `https`, `ftp` and magnet link in it (one per line or among other words, in Markdown or between `<` and `>`, without the sentence's full stop or bracket) opens in **Link grabber**, each once, ticked and sorted into packages, where you pick each package's folder and **Start** them or **Add to queue**; they end with one **Downloads complete** summary. Magnet links are added as torrents with all their files once those are known. One copied address with a number in its name (`…/ep01.mp4`) opens **Batch download** with the asterisk in the number's place (`…/ep*.mp4`), From and the wildcard size set, ready for the last number in To. An empty clipboard, or one without links, says so. It is also in the tray icon's menu, and **Ctrl+B** runs it. Until now it opened the batch window, empty unless the clipboard held a single address ([#709](https://github.com/anefzaoui/misterdownloadmanager/issues/709), [#780](https://github.com/anefzaoui/misterdownloadmanager/issues/780)).
- **Video qualities with their codec, frame rate and HDR:** the Download video window and the menu over a video show each quality's codec (AV1, VP9, H.264), frame rate and HDR (`2160p · VP9 · 60 fps · HDR`), and a resolution that comes in several (AV1 and VP9, HDR and SDR) lists each as its own choice, downloaded as listed. The best one stays first, as before. The sound as the site has it is offered in each type it comes in (YouTube's WEBM and M4A) ([#755](https://github.com/anefzaoui/misterdownloadmanager/issues/755)).
- **Fewer Download complete windows:** downloads added together (a playlist, **Add batch download**, a list from the clipboard, the links you add from **Link grabber**, the browser's page links among them) no longer open **Download complete** one by one. Once all of them are done, one **Downloads complete** window sums them up: **12 downloads finished · 1 failed**, their size together and their folder, the failed ones named with why, **Open folder** and **Show in list**, which selects them in the list. A download added alone keeps its own window, and so does a group of which only one finished ([#766](https://github.com/anefzaoui/misterdownloadmanager/issues/766)).
- **Snooze** in **Download complete** (and in the summary): no such window for 30 minutes, 1 hour, 8 hours, or until MDM restarts, without unticking **Show download complete dialog**. The snooze ends by itself; meanwhile the tray icon's tip says how many downloads finished, and **Options → Downloads** says **Download complete is snoozed until 14:30** with **Resume now** ([#741](https://github.com/anefzaoui/misterdownloadmanager/issues/741)).
- **The Download video button, redesigned** (building on the qualities' menu below): a small light tab at the video's top corner (dark in Windows' dark mode) with MDM's logo, a green **Download video** part, an arrow for its list and **✕** to hide it on this video. MDM looks the video up as soon as the button appears, so the button names the best quality before you click (**1080p**, **4K**), and one click downloads it at once, even while MDM is still looking; a video that plays a plain file is downloaded as that file, with no look-up. Its list (the arrow) starts with **Download best**, then one numbered line per quality, video and sound apart, with its type, frame rate or bitrate and size, opens the Download video window for the rest (**All qualities and subtitles…**), and hides it or keeps it off the site. After a click it says what happened in its own colour, with the one action that helps: **Added to MDM** with **Show**, **Open the post to download it** with **Open post**, **MDM didn't answer** with **Get MDM** and **Retry**, **Off on this site** with **Turn on**, and **Protected video · can't be saved**. It mirrors in right-to-left languages, shows a ring around it for the keyboard, shrinks to its logo, arrow and ✕ on a small video or where its words wouldn't fit, and shows the edge and the spots it snaps to while you drag it ([#703](https://github.com/anefzaoui/misterdownloadmanager/issues/703)).
- **The video's qualities in the page:** a click on the Download video button opens a menu over the video that lists its qualities with their sizes, video and audio in their own tabs (looked up by MDM with yt-dlp). The one you pick downloads at once with only its progress window; MDM's main window stays where it is. **Open in Mister Download Manager…** in the same menu opens the Download video window as before. A stream the page plays (HLS) is listed at once, read by MDM itself; the same video asked again within 10 minutes opens at once; and the pick starts downloading without looking the video up a second time.
- **Deferred Rename** on a download's **Download status** tab: save a new name while a download runs (**Confirm**, **Enter**, or 3 seconds idle) to rename the finished file's stem (along with yt-dlp subtitle files) or an exclusive finished folder on completion, without touching the live stream or `.mdm.part` state; shared/save or pre-existing folders, split-archive volumes, and single-file torrents keep their original names ([#655](https://github.com/anefzaoui/misterdownloadmanager/pull/655)).
- Helium: on Linux and macOS, MDM offers the extension to Helium, through Helium's own services: with them allowed, it arrives at Helium's next start (on macOS Helium asks first). It names the store as Helium does: Helium asks Google's update service without its version, and Google answers it "no update", which is why the offer MDM writes for Chrome on Windows, which Helium reads there too, never brought it the extension. On Windows MDM's window still opens the store's page in Helium. On Linux the .deb's and .rpm's helper files for everyone cover Helium too, so MDM writes none in its folder in your home. [Browser integration](docs/browser-integration.md#helium) says how to add the extension in Helium, and what to turn on when Helium says **Extension downloads are disabled** ([#685](https://github.com/anefzaoui/misterdownloadmanager/issues/685)).
- Links you add by hand go with the User-Agent of your default browser, at the version you have installed (Chrome, Edge, Brave, Opera, Vivaldi, Firefox or Safari, on Windows, Linux and macOS), instead of the Internet Explorer 11 one MDM sent until now, which some sites no longer let through. MDM looks the browser up once as it starts. **Options → Downloads** shows it with **Automatic** ticked; untick it to type your own, and a User-Agent you had set stays yours. When MDM doesn't recognise your default browser, it sends a current Chrome's; when a site turns a browser's User-Agent away, it asks once more with its former one, as for downloads from the browser (part of [#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668)).
- Sites that let only browsers through: when a site's bot check (Cloudflare's challenge) or its refusal of programs (FileHippo's *412 Precondition Failed*) turns MDM's own connection away, MDM asks once more connecting the way your browser does (its handshake, HTTP/2 settings and order of headers), with the download's User-Agent and your browser's cookies for that site only, and downloads on that one connection. Proxy, network adapter (it isn't tried when it can't be held to the adapter), speed limits, stop and resume apply as usual, and every other download keeps MDM's own connection. It answers no challenge: a site that still wants one solved in a browser refuses again, and MDM says so as before (Alt+click leaves the download to the browser) ([#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668), [#645](https://github.com/anefzaoui/misterdownloadmanager/issues/645), [#630](https://github.com/anefzaoui/misterdownloadmanager/issues/630)).
- Linux and macOS: a gallery-dl installed by the system (a package manager, Homebrew, pip) is found and used when MDM has none of its own, as yt-dlp is; **Helpers** can switch to MDM's own copy where MDM has one (Windows and Linux x86-64) ([#571](https://github.com/anefzaoui/misterdownloadmanager/issues/571)).
- Videos a site sends in pieces (HLS and DASH streams, as many sites other than YouTube do) download several pieces at once, as many as **Options → Connection** gives a file (8 by default), through MDM's own downloader: against a test server that slows each connection, 4 at once took under a third of the time, 8 under a fifth. They keep to the speed limits, a change applying at once; pieces encrypted with AES-128 are decrypted with their playlist's key, and pieces whose addresses expire are asked of yt-dlp again. The command line's `video-get` and `playlist-get` take `-c` for it ([#531](https://github.com/anefzaoui/misterdownloadmanager/issues/531), [#769](https://github.com/anefzaoui/misterdownloadmanager/issues/769)).
- **TorBox** debrid accounts (experimental), in **Options → Accounts**, signed in in the browser or with your API key: file-host links, torrents and magnet links through your account ([#391](https://github.com/anefzaoui/misterdownloadmanager/issues/391)).
- Linux and macOS: a yt-dlp installed by the system (a package manager, Homebrew) is found and used when MDM has none of its own; **Helpers** can still switch to MDM's own copy, kept up to date ([#452](https://github.com/anefzaoui/misterdownloadmanager/issues/452)).
- **Upload speed for torrents**, next to **Transfer rate** (part of [#357](https://github.com/anefzaoui/misterdownloadmanager/issues/357)): what a torrent sends to peers, in bytes per second, blank for anything but a torrent. A layout saved before it gets it between **Transfer rate** and **Peers**.

### Changed

- **Bangla, reviewed by a native speaker** for the main window, the download list, the progress window, the dialogs, the tray and the language picker: one word for each thing (Stop, Close and Turn off no longer share a word; nor do Remove and Move), Status and Invalid in the words Bangladesh uses, its spelling as written there, and the menus' keyboard letters on English letters in brackets, as in Hindi and Japanese: the Bangla letters they were on can't be pressed as shortcuts ([#927](https://github.com/anefzaoui/misterdownloadmanager/pull/927); thanks to [@geniusmonir](https://github.com/geniusmonir)).
- **Videos come down through MDM's own downloader:** yt-dlp now finds a video and its sound, and MDM downloads them the way it downloads any file: over as many connections as **Options → Connection** gives (8 by default; the progress window's connections table and segment map show them), under the speed limits (a change applies at once), paused and resumed like a file, also after quitting MDM, and with a fresh address asked of yt-dlp when one expires. yt-dlp then joins them and adds the subtitles as before. YouTube's qualities, its sound, most sites' MP4s and streams in pieces (HLS, DASH) come this way, and so do an image gallery's pictures, which gallery-dl lists (gallery-dl 1.32 and later; it then fetches only what MDM couldn't have); live streams and RTMP, RTSP, MMS or FTP streams are still yt-dlp's to download, and the progress window says so ([#769](https://github.com/anefzaoui/misterdownloadmanager/issues/769)).
- Building from source needs Rust 1.98 or later, CMake and libclang (and NASM on Windows): the browser-like connection is built on BoringSSL ([#668](https://github.com/anefzaoui/misterdownloadmanager/issues/668)).
- Debrid accounts: signing in by code opens the page only when it is the service's own site, over https.
- An idle MDM no longer redraws its window twice a second to watch the clipboard for copied links (**Options → General → Offer to download links copied to the clipboard**, on by default): the clipboard is looked at from a thread of its own, on Windows only when it changed, and the window is woken only when a new text was copied. A graphics card hardly noticed those frames, but Windows' software renderer (WARP, which draws where a card can't) spent most of a processor core on them: idle, MDM drawing with WARP now uses about a sixth of what it did ([#847](https://github.com/anefzaoui/misterdownloadmanager/issues/847)).
- Windows: on a graphics card without Direct3D 12 (Intel's HD Graphics 3000 to 5500, older AMD Radeon cards), MDM still draws with OpenGL, and once OpenGL fails there it draws with Direct3D 12 on Windows' own software renderer (WARP) from the next start. OpenGL fails when it can't start, is too old to draw MDM, its first window doesn't show within 10 seconds, or MDM freezes inside the graphics driver while OpenGL shows a frame, as the freeze reports from those cards showed: the crash report's window reads that from the freeze and remembers it. **View → Draw with** says what draws and why (**Direct3D 12 (WARP): this graphics card has no Direct3D 12 driver**, **OpenGL froze inside the graphics driver here**); choosing **OpenGL** there tries it again. WARP failing too goes back to OpenGL, and is remembered. WARP draws with the processor, so OpenGL comes first where it works ([#847](https://github.com/anefzaoui/misterdownloadmanager/issues/847)).

### Fixed

- Windows: the window no longer stops while Windows starts another program: Update now's installer, a browser chosen in **Open in browser**, the power options and the dial-up hang-up start beside the window, so an antivirus checking the program can't freeze it. Closing the Updates window while the update is put in place still starts nothing ([#846](https://github.com/anefzaoui/misterdownloadmanager/issues/846), thanks to [@arifrhm](https://github.com/arifrhm)).
- Browser extension, Firefox: one **Download video** button over a video, no longer two (seen on YouTube in full screen). When the extension was updated while its button showed, Firefox started the new copy apart from the old one, which could no longer take its button away: it stayed on the page where it was, beside the new one. A new copy now takes away any button an older one left, and a copy cut off from the extension takes itself away instead of showing ([#336](https://github.com/anefzaoui/misterdownloadmanager/issues/336)).
- Browser extension: **Alt+click** on a link (**Option+click** on a Mac) leaves its download to the browser in every browser. Firefox does nothing itself with such a click (it saves a link on Alt+click only with `browser.altClickSave` turned on), so nothing was downloaded at all; the extension now follows the link there as a plain click would, once it has noted that the browser keeps what comes of it. In every browser the download is matched to the click by the link, by where its redirects end, or, once, by the download a page's script starts after it (in the same tab, or sent from the link's page) in the next ten seconds; and the extension hears of the click as the mouse button goes down, before the browser can start anything ([#332](https://github.com/anefzaoui/misterdownloadmanager/issues/332)).
- **Download File Info** and **Add URL**: Left and Right in the **Save As** and **Address** boxes move the caret along the text. They moved the keyboard focus to the button beside the box instead (**...** in Save As, **OK** in Add URL) ([#812](https://github.com/anefzaoui/misterdownloadmanager/pull/812); thanks to [@void-lawliet](https://github.com/void-lawliet) for the fix).
- Linux: downloads to an NTFS drive (a Windows disk mounted with the kernel's `ntfs3` driver) no longer stand at 0% while the whole computer slows down. A large download's file is made at its full size before the first byte, and `ntfs3` filled all of it with zeros first: 30 GB of zeros for a 30 GB file, written before the download moved, while every other program waited on the disk. MDM now marks the file sparse there, as it already did on Windows, and it starts at once: a 30 GB download to an NTFS hard disk went from 0% after a minute to more than half done in 30 seconds.
- Linux on Wayland with NVIDIA's own driver (GNOME, KDE and others, where MDM's windows go through XWayland): typing and clicking no longer lag by a second or two. Each frame waited for the screen's refresh on every window XWayland wasn't showing at that moment (the main window behind a dialog, for one), and MDM draws all its windows in one pass, so a letter typed in **Download video**'s **File name** showed 1 to 2 seconds later and every other window answered as slowly. With NVIDIA's driver there, MDM's windows no longer wait for the refresh and MDM spaces its frames itself, 60 a second at most (the log says so): a letter now shows in about 45 ms, and an idle window still draws nothing. Other drivers (Mesa: Intel, AMD, nouveau) keep waiting for the refresh as before.
- Linux: MDM no longer closes with an internal error (`Error parsing ... TTF/OTF font file`) when the system offers it a font it can't draw with, such as an old bitmap font (`.pcf.gz`, `.bdf`) fontconfig named for Japanese or another script, as on Arch Linux with `xorg-fonts-misc`. MDM now asks fontconfig for fonts that scale first (Noto Sans CJK before a bitmap font it prefers for Japanese) and takes the first TrueType or OpenType one in its list, passing any other over (the log names it); on Windows and macOS a damaged font file is passed over the same way ([#931](https://github.com/anefzaoui/misterdownloadmanager/issues/931)).
- Where a download's server is (the progress window's Download status tab, Properties): shown once the server answered, with its address and its round trip together. Between the connection's opening and the server's answer it showed a round trip without an address, twice too long over https, for as long as the server took to answer, and a download stopped meanwhile kept it so ([#933](https://github.com/anefzaoui/misterdownloadmanager/issues/933)).
- Menus' keyboard letters: in **View → Arrange files** (also in the list's right-click menu on blank space), **A** reaches **By Date Added**, as **By Status** now has a letter of its own, **U**; it had **A** too, and an open menu runs the first item with the letter pressed. In the same way, in the list's right-click menu on blank space **N** now adds a new download, so **A** reaches **Select all**, and **View → Text** (Windows) has **X**, apart from **Tray icon**. A test now checks every menu MDM builds for two items with the same letter, in English and in the languages that keep its letters ([#926](https://github.com/anefzaoui/misterdownloadmanager/issues/926); thanks to [@geniusmonir](https://github.com/geniusmonir) for the report and the fix for Arrange files, [#928](https://github.com/anefzaoui/misterdownloadmanager/pull/928)).
- Windows 10: the taskbar's preview and tip of a progress window, and the notification-area icon's tip, no longer show boxes marked FSI and PDI around a file's name (`71% [FSI]file.apk[PDI]`). MDM keeps a name apart from the text around it with Unicode marks that its own windows draw as nothing, and Windows 10's taskbar drew them; what MDM gives Windows (window titles, the icon's tip, a file dialog's title) now has none, and in Arabic, Persian, Hebrew and Urdu reads in the same order with the older marks Windows draws as nothing ([#919](https://github.com/anefzaoui/misterdownloadmanager/issues/919)).
- Windows: **Open** starts the program Windows has for the file's type, as a double-click in Explorer does: a `.zip` opens in 7-Zip when 7-Zip opens `.zip` files there. It handed the file to `explorer.exe`, which failed ("Launcher "explorer.exe" ... failed with ExitStatus(1)") and showed the file's folder instead ([#811](https://github.com/anefzaoui/misterdownloadmanager/issues/811); thanks to [@arifrhm](https://github.com/arifrhm) for the fix, [#854](https://github.com/anefzaoui/misterdownloadmanager/pull/854)).
- Windows, with a style theme (Metro, Adwaita, Aero and the others after Classic, Native and High Contrast): a window hidden in the notification area (closed with **Keep running when closing the window**, or minimized with **Minimize to the notification area**) fades out as it does with Classic, and so does a download's progress window minimized there; they just disappeared. Windows fades out only a window with a title bar style, so they get it for that moment, as for minimizing (below). On Windows 10, the system's own title bar no longer shows above MDM's for a few frames while a window minimizes, maximizes, restores or fades: Windows draws the frame of that title bar style there, so MDM turns it off for those frames, and a thin edge shows instead ([#874](https://github.com/anefzaoui/misterdownloadmanager/issues/874)).
- The download list's column lines run in one straight line from the header down: each line between the rows sat one pixel to the right of the header's divider above it (one or two at 125 % and 150 % interface sizes, to the left in Arabic and Hebrew), on the next column's first pixel instead of the column's last ([#924](https://github.com/anefzaoui/misterdownloadmanager/issues/924)).
- Windows, with a style theme (Metro, Adwaita, Aero and the others after Classic, Native and High Contrast): MDM's windows minimize, maximize and restore with Windows' own animation again, as they do with Classic, whether from their title bar's buttons, the taskbar, **Win+Up** and **Win+Down**, a double-click on the title bar or the window menu. They popped in and out: Windows animates only a window with a title bar style, and those windows have none under MDM's own title bar, since the hidden one put a black band above it on Windows 10 ([#620](https://github.com/anefzaoui/misterdownloadmanager/issues/620), [#802](https://github.com/anefzaoui/misterdownloadmanager/issues/802)). Now a window gets the style for the length of the animation only (while it sits in the taskbar too), framed the whole time as without it, so nothing moves under the title bar. With **Minimize to the notification area** on, Minimize still goes straight to the tray, with any theme ([#874](https://github.com/anefzaoui/misterdownloadmanager/issues/874)).
- Windows 10, with a style theme (Metro, Adwaita, Aero and the others after Classic, Native and High Contrast): the main window no longer shows a black band above its title bar, with its menus opening about 31 px too high and every click landing above the button it showed. Like the progress window before it ([#620](https://github.com/anefzaoui/misterdownloadmanager/issues/620)), the main window kept the system's title bar style underneath MDM's own, hidden, and the picture was placed where that hidden title bar would have ended. It is now framed on Windows 10 as on Windows 11, with no title bar style at all, and still resizes from every edge, maximizes to the screen above the taskbar, snaps, and opens its window menu. Windows 10 shows its sizing frame's top edge, a few pixels above MDM's title bar, in its own title bar colour (white, your accent colour, or black in dark mode): unlike Windows 11, it lets no app colour it ([#802](https://github.com/anefzaoui/misterdownloadmanager/issues/802)).
- When a video's picture and sound can't be joined into one file (ffmpeg fails on a damaged stream, a full disk), the progress window says so in full under **Error**: why, and that what was downloaded stays in the folder (`Talk.f1280.mp4`, `Talk.faud-English.mp4`, named in the message) so **Resume** joins it again without downloading it; deleting the download with **Also delete the file from disk** now removes those parts too. Before, the window showed yt-dlp's bare words cut at its edge (`yt-dlp failed: Postprocessing: Error opening input files: Invalid data found whe…`), and deleting the download left the parts in the folder. What ffmpeg half wrote, if anything, is removed. While a video is joined, its percentage also never goes back.
- Torrents: **File Properties** closes with **OK** again. It said "The address must be an http(s) or ftp(s) address" for a torrent that wasn't finished or running, as OK checked its magnet link (or .torrent) as a web address it could be given in its place. A torrent's address is now shown read only there, as a video's is, and OK only checks an address that can be changed ([#904](https://github.com/anefzaoui/misterdownloadmanager/issues/904)).
- macOS: a .torrent file opens in MDM from Finder (a double-click, **Open With → Mister Download Manager**, or dropped on MDM's icon in the Dock), and a magnet link clicked in a browser opens in MDM: **Add torrent** opens for it, whether MDM is running or not. Finder said "Mister Download Manager cannot open files in the “BitTorrent Document” format": the app didn't tell macOS it opens .torrent files and magnet links, and didn't receive the files and links macOS hands an app (as Apple events, not on its command line). **Torrent settings → Open magnet links and .torrent files with Mister Download Manager** now makes MDM their default app on macOS too ([#698](https://github.com/anefzaoui/misterdownloadmanager/issues/698)).
- Windows: a menu closes the moment you click outside MDM (on the desktop, the taskbar or another program), as Windows' own menus do. It stayed on the screen for 0.4 seconds first: MDM waited that long for the focus to come back to one of its windows, which a submenu's window can take for a moment on Linux (#250), while on Windows a menu never takes it. Windows now says the window in front is another program's, and the menu closes on the frame MDM learns it ([#859](https://github.com/anefzaoui/misterdownloadmanager/issues/859)).
- No more **The window stopped responding** report just after the computer wakes from sleep, with MDM waiting in the notification area. Its window hadn't answered for 16 to 23 seconds after the wake, and MDM's separate crash monitor, which only waits, couldn't take its snapshot until the same moment, so most likely the computer wasn't running MDM yet while its disk, network and screen came back; the report's stack, taken once the window had answered again, said nothing of what held it. The freeze watchdog now leaves the first minute after a wake to the computer: a stall then is written to the log, with what the window was doing, and reported only if it lasts past that minute. Every freeze report now also says what the window was doing when it stopped answering (the tray icon, the downloads' progress, drawing the main window), and a snapshot taken in the pause after a hidden window's frame says the freeze was over before it ([#574](https://github.com/anefzaoui/misterdownloadmanager/issues/574), [#613](https://github.com/anefzaoui/misterdownloadmanager/issues/613)).
- A video stream the browser played (an `.m3u8` from a site's player) is looked up and downloaded as the browser asked for it: the extension keeps the **Referer**, the **Origin** and the **cookies** the browser sent with the stream, and MDM sends them again when it reads the stream and with each of its playlists and pieces. A site's video server that serves its own pages' players only (Dailymotion's, for one) answered MDM with **403 Forbidden**: MDM sent the page's address as the Referer and no Origin at all, and a player in another site's frame can have cookies the browser keeps for that page only, which the extension couldn't list ([#900](https://github.com/anefzaoui/misterdownloadmanager/issues/900)).
- A video whose size nobody knows beforehand (a Dropbox video, many sites' streams in pieces) no longer reads 99.9 % with 0 seconds left from its first second while it still downloads, its size growing with what came, when yt-dlp downloads it itself: yt-dlp's first figure for such a stream is an estimate as big as what came so far, and the progress bar held on to it. Such a figure now leaves no percentage, and the bar follows yt-dlp's estimates as they come. A video named after its page's title no longer gets its extension twice when that title is the file's name (Dropbox's: `Episode 31.mp4.mp4`) ([#622](https://github.com/anefzaoui/misterdownloadmanager/issues/622)).
- On a touch screen (a Surface, a touch laptop), tapping an item of a menu chooses it: the menu bar's menus, right-click menus, a toolbar button's drop-down and a drop-down list's choices. A tap opened the menu, but a tap on its items did nothing, so only a mouse could use them ([#733](https://github.com/anefzaoui/misterdownloadmanager/issues/733)).
- A video's **Address** in the progress window, **Download complete** and **File Properties** is where its file comes from (the stream MDM downloads, `googlevideo.com` for YouTube), and the page it was found on shows under **The web page from which this file was obtained**, as for a file. They showed the page as the address and "Unknown" for the page. File Properties also shows where that file's server is, as the progress window does, instead of "not one server". A download's page and **Referer** are also kept with the list now: after MDM was closed and opened again, File Properties said "Unknown" and an empty Referer for every download, and a download resumed then went without its Referer ([#795](https://github.com/anefzaoui/misterdownloadmanager/issues/795)).
- The progress window's connections table is no longer empty while a video downloads. MDM's own downloader now brings videos down (0.5.2 left them to yt-dlp, whose connections MDM couldn't see), so the table lists its connections, as many as **Options → Connection** gives (8 by default; a stream in pieces as many pieces at once). While yt-dlp looks the page up first, and for a video it still downloads itself (a live stream, an RTMP stream), the table shows that one connection and what it is doing instead of nothing ([#793](https://github.com/anefzaoui/misterdownloadmanager/issues/793)).
- After a download's last byte, the progress window and the list say what MDM is doing and how far it is: **Verifying… (45%)** while the file is checked against its checksum, and **Merging… (45%)** while a video's picture and sound are joined into one file (read from the size of the file ffmpeg writes); the list's Status column, wider by default for it (132 px instead of IDM's 65, which fitted no language's word beside a percentage, and more where the list's font needs it, as German does on Linux), says **Checking 45%** while the file is checked and **Merging 45%** while a video is joined, with the whole words in its tip; a list saved with Status at the old 65 px gets the new width, a width you chose stays. A torrent's **Checking 45%** there is no longer cut in German either. Meanwhile the progress window's bar starts again in the accent colour and fills as the check or the merge goes, and the transfer rate, the time left and the connections show nothing coming in, as for a finished download, rather than the download's last rate and connections still receiving. While the last of a file is written out, the progress window says **Finishing…**, not **Connecting…** as before. The map of the connections' parts also fills each finished part up to the next one: a finished download showed small gaps at the end of each part ([#736](https://github.com/anefzaoui/misterdownloadmanager/issues/736)).
- Windows: the file you drag out of **Download complete** (its small box at the bottom right) is moved or copied as when you drag a file in Explorer: moved to a folder on the same drive (or with **Shift** held), copied to another drive (or with **Ctrl** held), a shortcut with **Alt**. MDM offered only a copy, so the file was always copied and the original stayed in the download folder. A moved file is followed: its download points to the new place, so the list, **Download complete** and **File Properties** show it there, and **Open** and **Open folder** open it there. Where MDM can't learn where a moved file went (a program that took it, the Recycle Bin), **Open** and **Open folder** say it was moved elsewhere, and where to look, instead of failing. MDM never deletes the file itself: a program that only copies it leaves it where it was ([#869](https://github.com/anefzaoui/misterdownloadmanager/issues/869)).
- Windows: MDM's windows are no longer drawn as garbage on a graphics card whose OpenGL is older than 3.0 and that has no Direct3D 12 driver (Intel's first HD Graphics, whose OpenGL is 2.1). MDM fell back from Direct3D 12 to OpenGL there, which can't draw it. It now reads OpenGL's version as it starts: one older than 3.0 is passed over, and MDM starts again with Direct3D 12 on Windows' own software renderer (WARP), which draws it right. That is remembered, so the next starts go straight to WARP, with OpenGL as the last way should WARP fail. **View → Draw with** says that WARP draws and why; choosing **OpenGL** there tries it again. Other graphics cards draw as before ([#835](https://github.com/anefzaoui/misterdownloadmanager/issues/835)).
- When yt-dlp, ffmpeg or gallery-dl can't be downloaded, MDM says which server it couldn't reach and why, instead of a bare "network error": its name wasn't found, the connection was refused or cut, it didn't answer in time, the secure connection failed (an antivirus's web filter or a proxy reading HTTPS), it turned the request away (GitHub's limit on requests from one network, 403 or 429), or the download was cut short. It also says what to do: set your proxy in **Options → Proxy / Socks**, try again later, or download the program yourself and choose it in **Helpers → Use another program**. The helpers' release lists and checksum lists, and those of **Update now**, now go through the proxy set in **Options → Proxy / Socks**, as the downloads themselves did; they went through Windows' proxy settings instead ([#816](https://github.com/anefzaoui/misterdownloadmanager/issues/816)).
- **Link grabber** lists a web page's links when you give it the page's address. Pasting `https://www.epa.gov/water-research/epanet` (or copying it for **Add batch download from clipboard**) showed only the page itself: Link grabber took every address for a file to download and only looked each one up, so a page stayed one link. Now an address that came on its own and turns out to be a web page is read, and the links on it join as that page's package, as the browser's **Download all links** brings them: the page's `<a>` links and its videos and sounds, relative ones read against the page's address or its `<base>`, each once, without what sits in its scripts, styles and comments. The page itself is left unticked, and **Hide web pages** now also hides a link whose look-up found a page, whatever its address ends in. Links found on a page aren't read in turn, and with **Look links up when they arrive** off nothing is read ([#705](https://github.com/anefzaoui/misterdownloadmanager/issues/705)).
- Windows: **Open with...**, in a finished download's right-click menu and in **Download complete**, shows Windows' own **Select an app to open this file** again. It showed nothing when the file's name or folder had a space in it: MDM handed the file to Windows' old `OpenAs_RunDLL` command, which takes everything after it for the file's name, the quotes round a name with spaces included, and found no such file. MDM now asks Windows for the chooser itself, as Explorer's **Open with → Choose another app** does, beside the window, and a file moved or deleted since then says so, as **Open** does ([#720](https://github.com/anefzaoui/misterdownloadmanager/issues/720)).
- A file host that answers with its own web page (DropMeFiles, dailyuploads, datavaults: a page that counts down, asks you to solve a captcha or to press a button before the file) no longer leaves a "corrupted" archive of a few kilobytes marked as finished. MDM saved that page under the archive's name whenever the site sent it as the file's own type, under the file's name, or without a type, and the download looked complete. Now the download stops before anything is saved and says the site sent a web page instead of the file: open the link in your browser, finish there, and the download comes back to MDM (through MDM's browser extension). The progress window's **Open in browser** opens it. MDM tells the page by its first bytes as well as by its type, wherever an archive, a program, a disk image, a PDF, a picture, audio or video was expected (from its name, the browser's type for it, or the size the browser announced), and a download resumed onto such a page stops the same way, its data on disk kept as it was. A page you download on purpose (`.html`, a page's link) is saved as before, and so is a report "exported to Excel" as a page under an `.xls` name. A link the browser handed over that now gives the page is also asked once where the browser's own download ended, as one the site refuses is ([#701](https://github.com/anefzaoui/misterdownloadmanager/issues/701), [#692](https://github.com/anefzaoui/misterdownloadmanager/issues/692)).
- A YouTube video that lists only **Audio only** (and maybe one low quality) now says why: yt-dlp needs a JavaScript runtime (Deno, Node.js or Bun) to solve YouTube's challenges, and without one it finds only part of the video's qualities. The Download video window, the menu over a video and the command line now say **YouTube needs a JavaScript runtime to list its video qualities: install Node.js or Deno, then look it up again**, with a link to the [help page's section](docs/video.md#javascript-runtime) that gives the command for Windows, Ubuntu and Debian, Fedora, Arch and macOS. MDM notices a runtime installed while it runs at the next look-up, and also finds Deno from its own installer (`~/.deno/bin`) and, on Windows, Deno or Node.js just installed with winget or Node.js's installer ([#852](https://github.com/anefzaoui/misterdownloadmanager/issues/852)).
- Linux: the window no longer freezes where the session bus doesn't answer (a session started without D-Bus, some minimal window managers, a bus that hangs). MDM made its tray icon on the window's own thread, and the tray library waits for the bus as the icon is made, so the window waited too, and reported "The window stopped responding". The icon is now set up beside the window, which opens at once and behaves meanwhile as it does with no tray (closing quits). A bus that answers later, however late, still brings the icon, and closing then hides the window in it ([#828](https://github.com/anefzaoui/misterdownloadmanager/issues/828)).
- Looking up a video put online in the last two days no longer fails with "[Errno 13] Permission denied: 'C:\Program Files\Mister Download Manager\tmp…'" when MDM was opened from the Start menu. yt-dlp checks such a video's formats with a small file in the folder it was started in, and MDM started its helpers in its own, which is the install folder Windows doesn't let you write. yt-dlp, gallery-dl, ffmpeg and 7-Zip now run in your temporary folder; if that one can't be written, in a `work` folder of MDM's data folder, which the log names ([#834](https://github.com/anefzaoui/misterdownloadmanager/issues/834)).
- A video a site sends through a program's address (`stream.php?file=lecture.mp4`, `remote_control.php`, many lecture and video sites do) is named by what it is, not by the program: MDM took the name and the type from the address's last part, so the file was saved as `stream.php`, the browser's media list showed **remote_control.php · PHP**, and the Download video window offered **PHP** and then failed ("The extracted extension (php) is unusual and will be skipped for safety reasons"). Now a name whose ending is a program's (`.php`, `.asp`, `.aspx`, `.jsp`, `.cgi` and the like), a page's or none at all takes its type from the address's `file=` name, else from what the server says it sends (video/mp4 is `.mp4`), else from the file's first bytes; a video from the browser whose address names nothing (a program, a bare number, `videoplayback`) is named by its page's title, with the **Prefix media file names with the page title** switch on or off; and a name the server gives (Content-Disposition) still wins. The media list shows that name and type (**Lecture 1.mp4 · MP4**), the button over a video says MP4 instead of PHP, and the Download video window saves such a video as an MP4. Only media, picture, document and archive types are taken this way, never a program's ([#695](https://github.com/anefzaoui/misterdownloadmanager/issues/695), [#800](https://github.com/anefzaoui/misterdownloadmanager/issues/800)).
- Copying several links at once no longer opens **Download File Info** with all of them run together as one address (with **Offer to download links copied to the clipboard** on): a copied text with spaces or line breaks in it is not one link, and is left for **Add batch download from clipboard** ([#709](https://github.com/anefzaoui/misterdownloadmanager/issues/709)).
- Windows: MDM follows the Group Policy **Do not preserve zone information in file attachments**, as browsers do: with it on, for the computer or for you, downloads no longer get the mark that says they came from the internet (the Mark of the Web). MDM added it to every download whatever the policy said. A new option, **Options → Save to → Mark downloaded files as coming from the internet** (on by default), turns the mark off for MDM alone, on every system: Windows' Mark of the Web, macOS' quarantine and Linux's origin attributes ([#813](https://github.com/anefzaoui/misterdownloadmanager/issues/813)).
- **Tasks → Import** no longer freezes the window on a large file, or on one that isn't a list of addresses (a video, an archive, another program's backup picked by mistake): MDM read the whole file and looked through it for web addresses on the window's own thread, so a backup of several gigabytes kept the window frozen. The file is now read and searched beside the window, the status strip says **Reading** and the file's name meanwhile, and a file over 64 MB, far more than any list of links holds, is refused at once with a message that says so. Link grabber also takes a long list without a pause: it compared each new link with every link already in it, so 10,000 links made 50 million comparisons on the window's thread ([#805](https://github.com/anefzaoui/misterdownloadmanager/issues/805)).
- A video quality's size is now the size of what downloads. The list showed one stream's size and downloaded another of the same resolution: for a portrait 4K video on YouTube, **3840p 60fps MP4** said 3.79 GB (an estimate from the peak bitrate of a stream in pieces) and the download was 532 MB (AV1 and Opus, the formats yt-dlp picked). Each quality is now the very video and sound streams it was sized from, sized from their bytes, and a size that is only an estimate shows as about ("~532 MB"), in the Download video window, the menu over a video and the command line's `video-info` ([#801](https://github.com/anefzaoui/misterdownloadmanager/issues/801)).
- Windows: the window no longer freezes while it checks whether Windows is in light or dark mode. Every 2 seconds the window read Windows' app mode (and, with the **Native** theme, its accent and contrast colours) from the registry, and looked at the themes folder and pywal's colours on the disk, on its own thread; where the registry took seconds to answer, the window waited that long ("freeze in misterdownloadmanager::theme::update"). A thread beside the window now reads them, at once when Windows says one of those settings changed, and the window shows its last answer without waiting: switching Windows between light and dark, a new accent, a theme file added to the themes folder and pywal's new colours are followed as before ([#719](https://github.com/anefzaoui/misterdownloadmanager/issues/719)).
- Wikimedia Commons videos download again: their quality list carries a negative size estimate, which MDM took for a page with no video at all ("it gave no video information"), and so did one yt-dlp says is neither a plain file nor not (`"direct": null`).
- The Download video button no longer comes back when you play or pause the video: only moving the pointer over it shows it. It fades 1.5 seconds after the pointer stops (2.6 before), and at once when the pointer leaves the video (part of [#741](https://github.com/anefzaoui/misterdownloadmanager/issues/741)).
- Windows 10: the download's progress window no longer shows a black band above its title bar, with its contents moved down and every click landing above the button it showed (you had to click above a button for it to work); the same goes for the dialogs with a Metro, Luna, Aero or Aqua title bar. These windows kept the system's title bar style underneath MDM's own, hidden, and the picture was placed where that hidden title bar would have ended, as it was on a Windows 11 Insider build, fixed there in 0.5.0 ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)). They are now framed on Windows 10 as on Windows 11, with no title bar style at all, and a thin border round them ([#620](https://github.com/anefzaoui/misterdownloadmanager/issues/620)).
- Fast downloads to a hard disk no longer freeze for 7 to 10 seconds once they are a few gigabytes in, and MDM no longer replaces healthy connections as "much slower than the other connections". Past the first gigabytes the system's cache is full and the disk sets the pace; each save of the resume state then had more to write than the last (spaced out by how long the last one took), until one took over 10 seconds and the download stood still. And a connection that had waited for the disk read what the system kept for it meanwhile in a burst, which MDM took for its speed, so every other connection looked four times slower and was cut, often all at once. Now the connections keep receiving into memory while the disk takes their writes (up to 256 MB per download), the resume state is saved to the disk only when that won't stop them, a connection's speed counts all the time it took (waits for the disk included), and no connection is replaced while the disk sets the pace. On a disk slower than the network the download goes at the disk's speed, evenly: with a test disk of 70 MB/s behind a 1 GB cache, a 6 GB download that froze for 7 to 12 seconds now never drops under 69 MB/s once the cache is full, and on Linux with the disk held to 60 MB/s, where MDM cut 67 connections in a typical 6 GB download (up to 111), it now cuts none or one. What a crash or power cut makes MDM download again is what came since the last save: on a disk that falls behind, everything since it did; pausing and stopping still save everything ([#782](https://github.com/anefzaoui/misterdownloadmanager/issues/782)).
- Windows: MDM's programs carry Microsoft's C runtime inside them instead of loading it from Windows (`VCRUNTIME140.dll`), so a PC without that runtime, or with a 32-bit copy where the 64-bit one should be, no longer stops them: setup's last step failed there with "The application was unable to start correctly (0xc000007b)" ([#785](https://github.com/anefzaoui/misterdownloadmanager/issues/785)).
- Sites behind Akamai that let only browsers through (a 403 "Access Denied" page for MDM, the file for a browser, such as HPE's documentation PDFs) now download: MDM takes Akamai's refusal for the bot check it is and tries again the way a browser connects, as it does for Cloudflare's ([#771](https://github.com/anefzaoui/misterdownloadmanager/issues/771)).
- Crash and freeze reports keep the log lines that explain them: 25 of the file-type icons carried no-op styles (`marker:none`, `mask:none` and the like) that MDM's SVG reader warned about each time they were drawn, 1,385 lines for one look at them all, and those warnings filled the end of the log a report includes ([#767](https://github.com/anefzaoui/misterdownloadmanager/issues/767)). The icons look the same.
- The download windows no longer freeze while a drive is slow to answer (a hard disk waking up, a USB drive, a network or mapped folder). **Download File Info** asked the disk whether its file's name was taken (to number it `(1)`) on the window's own thread, and so did **Show in folder**, **Open folder**, **Open** and the drag handle in **Download complete**, the question to create a missing folder, the look for files a torrent would write into, and the progress window's check of a folder for **Rename**; the window waited for the disk, 8 seconds and more in the reports. They now ask beside the window. Meanwhile Save As shows the name as it is, without `(1)`, and takes the numbered one when the answer comes; a download started before then is numbered as it starts, so nothing is overwritten ([#743](https://github.com/anefzaoui/misterdownloadmanager/issues/743), [#751](https://github.com/anefzaoui/misterdownloadmanager/issues/751)).
- **Sizes for a stream's qualities:** HLS qualities showed no size; each is now estimated from its bitrate and the video's length, in the Download video window and in the download list.
- **A video already in the list is no longer downloaded again without a word:** picking the same quality of the same video, from the menu over a video or the Download video window, now follows Options → Downloads ("If a duplicate download link is added": ask, a numbered name, overwrite, or open / resume the one there). Another quality of the same video is not a duplicate.
- The progress window's bar glides as the download goes, instead of moving in steps four times a second.
- Video downloads (and image galleries) keep to the speed limits you set: a video's own limit (its progress window's **Speed Limiter** tab) and the limit for all downloads together, not only the limit for one file, and turning the limiter on or off, changing its settings or a video's own limit applies to the videos under way (at once for a video MDM's own downloader brings in; one yt-dlp still downloads by itself, and a gallery, take a limit only as they start, so they stop and start again with the new one, from where they were). Several such videos under the limit for all downloads share it evenly: when another starts, the ones running with more than their share start again with it. A video or a gallery started while the Scheduler's download limit is reached now waits, as other downloads do ([#708](https://github.com/anefzaoui/misterdownloadmanager/issues/708)).
- File names whose stem is a Windows reserved device name followed by spaces before the dot (`CON .txt`, `LPT1 .log`) are now prefixed with `_` (`_CON .txt`), as `CON.txt` already was ([#655](https://github.com/anefzaoui/misterdownloadmanager/pull/655)).
- The progress window no longer shows scroll bars over content that fits: at an interface size above 100 % (125 %, say) the window's client comes back a pixel short of its content, which the pane read as content to scroll to, put up a horizontal bar and then a vertical one beside it ([#672](https://github.com/anefzaoui/misterdownloadmanager/issues/672)).
- The browser's **video button** over a video in a feed (a site's home page, a timeline, search results) sends that video's own address instead of the page's, which yt-dlp read as no video at all ("No video found on this page"). It finds the post the video sits in, as you would: on LinkedIn its `/feed/update/urn:li:activity:…/` (or `urn:li:ugcPost:…`) address, read from the post in LinkedIn's 2026 feed too, on Facebook the reel's, the video's or the post's own link, on X the post's (`/status/…`), on Instagram `/p/…` or `/reel/…`, on Reddit the post's comments page, on TikTok `/@…/video/…`, on Vimeo and Dailymotion the clip's page, and elsewhere a post's own link, such as its timestamp. Only a post that names that one video counts. When nothing does, the video's own file goes (when it plays one, not pieces), and otherwise nothing goes and the button says to open the post and use the button there ([#689](https://github.com/anefzaoui/misterdownloadmanager/issues/689), [#643](https://github.com/anefzaoui/misterdownloadmanager/issues/643)).
- Torrents: pausing a torrent just as one of its pieces was being checked no longer leaves it one piece short for good. That piece was neither counted as downloaded nor asked for again, so the torrent stopped just before the end, and pausing and starting it again didn't help, only restarting MDM did; a pause now puts that piece back to be downloaded again ([#706](https://github.com/anefzaoui/misterdownloadmanager/issues/706)).
- Torrents: the window no longer freezes while torrents run on a busy or slow disk (an antivirus scanning each write, a disk that is full of other work). The window read each torrent's numbers from the torrent engine on every frame, and the engine holds a torrent while it is paused, removed or added until its writes to the disk have ended, seconds on such a disk; the window waited that long ("freeze in mdm_torrent::Torrents::stats"). The numbers are now read beside the window, five times a second and at once after each change, and the list, the progress window's tabs, **Files...** and the right-click menu show the last ones without waiting ([#693](https://github.com/anefzaoui/misterdownloadmanager/issues/693)).
- The window no longer waits for the program that owns the clipboard. A text box's right-click menu (to offer **Paste**) and **Add URL**, **Add batch download**, **Add torrent** and **Download video** (to fill in a copied link) read the clipboard on the window's own thread, so a program that held the clipboard without answering (on Linux, where a read waited up to 4 seconds each time, and while the right-click menu was open at every frame; on Windows, a program that had stopped responding) held up MDM's window too. They now read it beside the window, as the clipboard monitor has since 0.5.1; a link copied just before still fills the box as the window opens (follows [#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418)).
- Linux: freeze and crash reports show MDM's own functions, files and lines again, and the right ones. When MDM's code began a page after the start of the program in memory (as 0.5.0's did), the report read the whole stack at the wrong place: it showed only system libraries (`freeze in libc.so.6+…`), named other functions of MDM once the release's symbols were fetched, and couldn't find an existing issue for the same problem. A `.dmp` from such a report reads right too ([#661](https://github.com/anefzaoui/misterdownloadmanager/issues/661)).
- Windows: a freeze report whose window thread hardly ran and stopped in the C runtime's memory routines (`VCRUNTIME140.dll`, comparing the first bytes of an icon) now says the computer was too busy to run MDM or slow to load it from disk, with the page faults counted meanwhile, instead of a deadlock; those routines never wait ([#663](https://github.com/anefzaoui/misterdownloadmanager/issues/663)).
- Windows: the notification-area icon's menu (and the window menu of a title bar MDM draws) follows MDM's look: dark in dark mode or with a dark theme, light otherwise, whatever Windows' own mode is. It was always drawn in Windows' light colours, also on Windows 10 ([#682](https://github.com/anefzaoui/misterdownloadmanager/issues/682)).
- Torrents: magnet links without trackers find peers through the DHT again where its first nodes didn't answer. MDM asked only two public routers to join the DHT; where neither answered (one name pointing at an old address, or a network blocking one), the DHT stayed empty and the log said "error in bootstrap" every minute or two. It now asks four together (`dht.libtorrent.org`, `dht.transmissionbt.com`, `router.bittorrent.com`, `router.utorrent.com`), any one that answers is enough, and the others are asked no more once one has. The nodes it knows are kept for the next start, the IPv6 ones too ([#650](https://github.com/anefzaoui/misterdownloadmanager/issues/650)).
- Torrents: with the **Port** box empty in Torrent settings, MDM keeps the port it picked and listens on it again at the next start (another one if a program has taken it), so peers that learned it, and a router's or firewall's rule for it, still reach MDM; it was a new port at each start. MDM's log says where it listens: one socket for IPv4 and IPv6, which Windows' `netstat -p tcp` (IPv4 only) doesn't list ([#650](https://github.com/anefzaoui/misterdownloadmanager/issues/650)).
- macOS: YouTube videos that need yt-dlp's JavaScript runtime load when MDM is opened from Finder or the Dock. MDM found Homebrew's yt-dlp, but the helpers it started didn't get Homebrew's folders on their PATH, so yt-dlp found neither Deno nor ffmpeg ("Requested format is not available"); they now look there too, after the folders of their own PATH ([#659](https://github.com/anefzaoui/misterdownloadmanager/issues/659)).
- A download handed over by the browser whose link the site refuses to MDM before sending anything (FileHippo's links answered **412 Precondition Failed**; a one-time link the browser has already used, or one tied to its session, answers 403, 404 or 410) goes on from the address the browser's own download of it had reached, once; if that is refused too, the link's own answer is the error shown. Its cookies go there only when it is the same site ([#630](https://github.com/anefzaoui/misterdownloadmanager/issues/630)).
- Raising a speed limit, or turning it off, applies at once: to a download's own limit (its progress window's **Speed Limiter** tab), the limit for one file and the limit for all downloads together. A connection already waiting its turn under the old limit sat out the wait the old limit gave it, many seconds at a low limit, before the new speed showed; it now counts what is left at the new limit, and ends its wait as soon as there is no limit ([#621](https://github.com/anefzaoui/misterdownloadmanager/issues/621)).
- **Download video**: a sound-only choice (MP3, Opus, FLAC, the original sound) saves in the Music category's folder and is listed under Music, as a playlist downloaded as sound only does; choosing a video again goes back to the Video folder. A folder you typed or picked in **Save to** stays as it is ([#611](https://github.com/anefzaoui/misterdownloadmanager/issues/611)).
- **Options → Save to**: changing the **General** category's default download directory now moves the default folders of all other categories without their own folder (`Video`, `Music`, `Torrents`, etc.) under it, instead of leaving them under the system's `Downloads` folder ([#556](https://github.com/anefzaoui/misterdownloadmanager/issues/556)). **Add torrent** also starts with the **Torrents** category's folder from the moment it opens (so **Save to** no longer flips when a magnet link's metadata arrives) and remembers a changed folder when **Change folder for this category on last selected** is ticked ([#550](https://github.com/anefzaoui/misterdownloadmanager/issues/550)).
- Windows: dragging a finished file out of **Download complete** (the grip at its bottom right) no longer holds up MDM's windows for as long as the drag lasts, or for as long as the program it is dropped on takes to answer: the drag now runs beside the window, as file dialogs already do ([#561](https://github.com/anefzaoui/misterdownloadmanager/issues/561)).
- Menus: going from an item with a submenu to another item closes the submenu at once, and only the item under the pointer is lit. The submenu stayed open, its item lit as well, until the pointer rested on one item, so as long as it kept moving; going diagonally into the submenu still keeps it open ([#579](https://github.com/anefzaoui/misterdownloadmanager/issues/579)).
- **Download video** without ffmpeg (on macOS and Linux, where MDM doesn't install it): the sound alone comes from a format the site sends as one file when it has one, and a sound or video that came as an HLS stream's pieces is saved as what it is (`.aac`, `.ts`) instead of `.mp4`, which players refused to open ([#569](https://github.com/anefzaoui/misterdownloadmanager/issues/569)).
- A freeze when the disk was slow: MDM looked on the disk for a portable data folder on every tick of the window; it now works the data folder out once ([#565](https://github.com/anefzaoui/misterdownloadmanager/issues/565)).
- macOS 15 and later: Chinese names and text find the PingFang font again, which moved into the system's font assets, with Hiragino Sans GB and STHeiti as fallbacks ([#541](https://github.com/anefzaoui/misterdownloadmanager/issues/541)).
- The video button follows **Options → General → Customize MDM Download panels in browsers** in the pages already open: switched off, or its site listed, it goes within a few seconds, without reloading the page (it stayed until a reload, and on YouTube, which never reloads, for good). The same way, a page opened while the browser couldn't reach MDM (MDM being installed or updated) gets the button within a few seconds once it can, where it stayed off until a reload ([#474](https://github.com/anefzaoui/misterdownloadmanager/issues/474)). A site listed as `example.com` now takes `www.example.com` and its other subdomains too, and a player embedded from another site follows the site in the address bar ([#532](https://github.com/anefzaoui/misterdownloadmanager/issues/532)).
- macOS: file names in Persian, Arabic, Urdu, Hebrew or Armenian show their letters in the download list and windows instead of boxes, whatever the interface language: these scripts now get the font for them that comes with macOS (Arial Unicode), the first time one is needed ([#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533)).
- A download's progress is saved from its first seconds again: since 0.5.2, when the disk was slow to write the resume state as the download started (no data had arrived yet), the first save of its progress waited nine times as long, up to a minute, so a short download cut off by a crash or a power cut in that time started over from the beginning (part of [#518](https://github.com/anefzaoui/misterdownloadmanager/issues/518)).
- macOS: **Download File Info** opens in front of the browser that asked for it ([#484](https://github.com/anefzaoui/misterdownloadmanager/issues/484)).
- macOS: MDM's icon shows in the menu bar, so closing the window keeps MDM running there instead of quitting; the icon was never made, for want of a picture ([#529](https://github.com/anefzaoui/misterdownloadmanager/issues/529)).
- The browser's **video button** over a hover-to-play preview (YouTube's thumbnails) no longer vanishes as the pointer goes to it: it stays where it was while the pointer is on it, a click downloads that video, and it fades once the pointer leaves ([#503](https://github.com/anefzaoui/misterdownloadmanager/issues/503)).
- **Enter new address to download**: an address with a login in **Options → Sites Logins** shows that login under **Use authorization** (ticked, the user name and the password), the one the download uses; it was used but the box stayed empty. A login typed there still goes first ([#511](https://github.com/anefzaoui/misterdownloadmanager/issues/511)).
- The window no longer waits for the disk to write MDM's log or to save the RSS feeds: while Windows held every write on the disk to make a snapshot of it (a restore point or a backup), the window could stop answering until the writes were let go. Both are now written beside the window, as the download list already was (part of [#513](https://github.com/anefzaoui/misterdownloadmanager/issues/513)).
- The window no longer waits for Windows to look up the time zone: the queues' schedules asked for it at every moment, and once Windows took more than 8 seconds to answer, the window stopped answering meanwhile. The time zone is now looked up beside the window, again each minute, so a change of summer time still shows within a minute ([#536](https://github.com/anefzaoui/misterdownloadmanager/issues/536)).
- Windows: video and gallery look-ups no longer fail with "[WinError 448] The path cannot be traversed because it contains an untrusted mount point" when a folder on PATH goes through a junction Windows won't let yt-dlp cross (one an app installed for itself, Codex's for example): yt-dlp, gallery-dl and the ffmpeg they start get that folder's real place instead, and no folder that can't be opened ([#508](https://github.com/anefzaoui/misterdownloadmanager/issues/508)).
- **Download video** from Dailymotion's player embedded in another site: the look-up no longer sends that site as the Referer, which Dailymotion refused with 403, so its formats are found as with the video's own address ([#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359)).
- The video button fades again after a click on the player, in fullscreen too, and stays while you use the keyboard ([#480](https://github.com/anefzaoui/misterdownloadmanager/issues/480)).
- A magnet link or a .torrent handed to MDM while **Add torrent** is already open is no longer dropped without a word: it waits, and Add torrent opens again for it when you close the first ([#507](https://github.com/anefzaoui/misterdownloadmanager/issues/507)).
- The window no longer freezes when a video or a gallery finishes: marking its files as downloaded from the web, which can wait on the disk and the antivirus for a while with a big video, now happens beside the window ([#510](https://github.com/anefzaoui/misterdownloadmanager/issues/510)).
- Linux: **View → Dark Mode support** turns dark on window managers such as Hyprland and sway when only GNOME's setting says so (`gsettings set org.gnome.desktop.interface color-scheme prefer-dark`), as it did when a desktop portal answered ([#324](https://github.com/anefzaoui/misterdownloadmanager/issues/324)).
- Dark mode no longer draws a bright white line under the menu bar, across the whole window, in the Classic and Native themes ([#379](https://github.com/anefzaoui/misterdownloadmanager/issues/379)).
- Windows: a download's progress window with a title of 256 characters or more (a torrent with a long name, or a file name kept whole in a folder too deep to shorten it) has MDM's own title bar again, instead of the system's drawn above it (part of [#540](https://github.com/anefzaoui/misterdownloadmanager/issues/540)).
- Windows: closing the window hides MDM in the notification area again, as it did before 0.5.2, instead of leaving it minimized in the taskbar with its icon right there, among the hidden icons behind ^ (where Windows puts a new icon). Before hiding the window, MDM asked Windows where the icon was, and Windows 10 and some Windows 11 versions give an icon behind ^ no place; MDM took that for no icon at all. It now also asks whether Windows has the icon, wherever it shows it, and keeps the window in the taskbar only when Windows says it has none (Explorer isn't running, or the icon was lost): when Windows doesn't answer, the window hides as usual, and starting MDM again always brings it back. The same goes for **Minimize to the notification area**, and for a start with Windows, which no longer shows the window minimized after half a minute ([#750](https://github.com/anefzaoui/misterdownloadmanager/issues/750), [#718](https://github.com/anefzaoui/misterdownloadmanager/issues/718), [#754](https://github.com/anefzaoui/misterdownloadmanager/issues/754), [#545](https://github.com/anefzaoui/misterdownloadmanager/issues/545), [#534](https://github.com/anefzaoui/misterdownloadmanager/issues/534), [#527](https://github.com/anefzaoui/misterdownloadmanager/issues/527), [#590](https://github.com/anefzaoui/misterdownloadmanager/issues/590), [#779](https://github.com/anefzaoui/misterdownloadmanager/issues/779)).
- Windows: the window no longer freezes while Explorer is slow to answer about MDM's notification-area icon ("freeze in misterdownloadmanager::tray::Tray::visible_now"). The window asked Explorer about the icon every second, and set the icon's tip, on its own thread, and waited as long as Explorer took: 8 seconds in the report. Both now happen beside the window, which goes by Windows' last answer; an answer that is late or missing is never taken for "no icon", so a window in the notification area stays there ([#747](https://github.com/anefzaoui/misterdownloadmanager/issues/747); thanks to [@TG-Twilight](https://github.com/TG-Twilight) for the first version, [#697](https://github.com/anefzaoui/misterdownloadmanager/pull/697)).
- The browser's **video button** over YouTube's thumbnail previews keeps one place, so it can be clicked: it moved to another corner when the preview showed its sound and captions buttons, back when they hid as the pointer reached it, and away again, and it followed the preview as it shrank. Over the same video it now only moves on to a free corner, never back, and it stays put while the pointer is on it. The same held it still over a site's player whose settings button shows while the pointer moves over the player and hides when the pointer stops or reaches the button: 0.5.2 sent it from one side to the other and back each time, so a click had to be timed ([#560](https://github.com/anefzaoui/misterdownloadmanager/issues/560), [#899](https://github.com/anefzaoui/misterdownloadmanager/issues/899)).
- Chrome, Edge, Brave and the other Chromium browsers: a download the extension leaves to the browser keeps the name and folder another extension gives it (Save In's folders, or any extension's own download into a folder). The extension's answer for it counted as a name of its own, an empty one, and when the extension was the newest installed it won: the file landed in the Downloads folder, and the browser could warn of a conflict between the two extensions ([#578](https://github.com/anefzaoui/misterdownloadmanager/issues/578)).
- Windows: fewer freezes ("The window stopped responding") on a system held up by its disk or its memory. Every 2 seconds, and in each frame of an open dialog, MDM asked Windows about its windows' title bar colours, and the window waited with Windows' answer; it now sets a title bar's light or dark once, when the window opens or the theme changes. Looking for its own windows no longer waits on another busy part of MDM either ([#633](https://github.com/anefzaoui/misterdownloadmanager/issues/633), [#583](https://github.com/anefzaoui/misterdownloadmanager/issues/583)).
- Windows: MDM no longer crashes ("crash in ntdll.dll", the heap corrupted) when the clipboard is read in two places at once, as when **Add URL** opened while the clipboard monitor was looking for a copied link. Windows lets the parts of one program open the clipboard together, and their reads spoiled each other's data; MDM's reads and writes of the clipboard, a paste or copy in a text box included, now go one at a time ([#629](https://github.com/anefzaoui/misterdownloadmanager/issues/629)).
- macOS: downloads to a network volume (a folder under `/Volumes` on an SMB, AFP or NFS share) start again. Each one was refused with "Not enough disk space ... 0 bytes free", however much the share had: macOS answers 0 when asked a network volume's space "for important usage" (the free space plus what the system can clear, as Finder counts it on the Mac's own disks), and MDM took that 0. On a network volume, or whenever that answer is 0, MDM now counts the free space the volume reports ([#623](https://github.com/anefzaoui/misterdownloadmanager/issues/623)).
- **Smooth text on Windows, and finer interface sizes:** **View → Text → Smooth** draws letters anti-aliased at their own size instead of as thick, whole-pixel letters that are hard to read on a 1440p screen; **Classic**, the default, keeps the look MDM had. **View → Interface size** now offers 110 %, 115 % and 120 % between 100 % and 125 %.

### Added

- Options → General → **Customize MDM Download panels in browsers**: Export... and Import... for the sites the download button stays off, a plain text file with one site a line ([#424](https://github.com/anefzaoui/misterdownloadmanager/issues/424)).
- **Emerald** among the built-in looks in **View → Theme**: a white canvas with one near-black ink and a ladder of neutral greys for structure, emerald fills that carry near-black text rather than white, 6-point corners and Metro's one-colour icons. Its theme file names a few seeds per side and derives every other colour, so a role added later arrives in this look instead of the classic one ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45)).

### Fixed

- The progress window of a video whose size isn't known says no percentage (it read 0 % in the title and 0.00 % on the Downloaded line), and **File size** stays the file's size at the end (it read 0.1 % too high while yt-dlp finished).
- A **gofile** share link (`gofile.io/d/...`) now says it is a web page to open in your browser, instead of saving that page as if it were the file ([#346](https://github.com/anefzaoui/misterdownloadmanager/issues/346)).
- A download that gets a web page where a file was expected (a server's "The requested URL was rejected", a sign-in page) now stops with "the server sent a web page instead of the file" instead of saving that page under the file's name and calling it complete. MDM knows a file was expected from the type the browser had for it, or from a file's name (.doc, .pdf, .zip, .mp4...); a page the browser saved as one, a `.html` link, and a server's own name for what it sends are downloaded as before (part of [#533](https://github.com/anefzaoui/misterdownloadmanager/issues/533)).
- **Download video**: when a site's bot check (Cloudflare) turns yt-dlp away, the message says so and what to try, instead of yt-dlp's cut-off words ([#317](https://github.com/anefzaoui/misterdownloadmanager/issues/317)).
- The **Download video** window remembers the folder you save into, per **Options → Save to**'s "Change folder for this category on last selected", as **Download File Info** does: it no longer goes back to the Video category's folder each time ([#475](https://github.com/anefzaoui/misterdownloadmanager/issues/475)).
- X (Twitter) videos sent from the browser's media list are named after the tab, not the random name of their file on X's video server ([#493](https://github.com/anefzaoui/misterdownloadmanager/issues/493)).
- Windows: **Open folder** on a download opens the folder with the file manager set up to open folders (Directory Opus, for example), instead of always starting Windows Explorer. With Explorer it still selects the file ([#446](https://github.com/anefzaoui/misterdownloadmanager/issues/446)).
- Left click on the tray icon now toggles the main window: hidden or minimized shows it again, in front hides it to the tray, and behind other windows brings it forward (it only ever showed it, so clicking while the window was visible did nothing). On Windows it goes by what covers the window, so it works with the icon under the taskbar's ^ too.
- A download started in the browser (a video's download button, a link, a magnet or .torrent) now opens only its dialog when the main window is hidden or minimized, instead of always bringing the full window up.


## [0.5.2] - 2026-11-09

A big fixes release: YouTube and video downloads, crashes and freezes, downloads on slow disks, the Mac helper, plus a Safari extension, far more file types and a few new columns and options.

### Added

- **Torrent → Files…**: the list of a torrent's files for any torrent, running, stopped or finished, with a box to choose each (also a Files… button in File Properties).
- **Two columns for torrents in the download list** ([#357](https://github.com/anefzaoui/misterdownloadmanager/issues/357)): **Peers** (those a torrent has connected while it runs, seeders and leechers alike) and **Share ratio** (what it has sent against what it took), after **Transfer rate**. A layout saved before them gets them there.
- The extension's **Media on this page** can name a file after its page: **Prefix media file names with the page title** puts the page's title before the file's own name. Off as installed.
- **Far more file types taken from the browser by default** ([#382](https://github.com/anefzaoui/misterdownloadmanager/issues/382)): AI models and datasets (GGUF, SAFETENSORS, CKPT, PT, ONNX, PARQUET...), 3D and CAD (GLB, FBX, OBJ, STL, BLEND, STEP...), design and camera RAW files, ebooks, Office files, DAT, disk images and split archives. A list you never edited gains them; one you edited stays yours.
- **A Safari extension, inside the Mac app:** a click on a link to a file type MDM takes goes to MDM, and so do the right-click items, the button over videos and **Media on this page**, as in the other browsers. Safari gives extensions no access to its downloads, so a download a page starts by itself stays with Safari, and the browser's proxy is not shared. See [Browser integration](docs/browser-integration.md#safari-macos).
- Help: what to do when an antivirus or VirusTotal flags MDM, and how to check your copy is the real one.

### Changed

- In English, the **Last Try Date** and **Added** columns follow the system's 12- or 24-hour clock: `Oct 28 9:44:12 PM 2026` when Windows uses a 12-hour clock, and **Added** starts wide enough for it ([#392](https://github.com/anefzaoui/misterdownloadmanager/issues/392)).

### Fixed

- With no tray icon to come back from, closing or minimizing the main window now leaves it in the taskbar, with downloads still going, instead of hiding it with no way back. **Options → General → Keep running when closing the window** (on by default) makes closing quit MDM when unticked ([#457](https://github.com/anefzaoui/misterdownloadmanager/pull/457)).
- Crash reports: a crash monitor that can't open its connection no longer makes MDM wait five seconds at start or stays behind as a second process ([#458](https://github.com/anefzaoui/misterdownloadmanager/pull/458)).
- Download File Info no longer stays open but out of sight, with every later download waiting behind it ([#440](https://github.com/anefzaoui/misterdownloadmanager/issues/440)). The next one after answering **Create folder** came up as an invisible window, and on Windows one hidden with a minimized main window is now shown again, as is the duplicate question
- A file name sent by a site in plain UTF-8 now reaches MDM readable from Firefox and browsers built on it, such as Floorp, Waterfox and Zen: an Arabic name was saved as unreadable symbols ([#447](https://github.com/anefzaoui/misterdownloadmanager/issues/447)). A name written with percent signs in the same header is decoded as well. The add-on carries the fix
- A download saved under a numbered name because another file already had its name (`master (1).mp4`) now shows that name in the list and opens its own file, instead of keeping the old name and opening the other file ([#442](https://github.com/anefzaoui/misterdownloadmanager/issues/442)). Two downloads waiting under one name no longer collide when they start: the later one takes the next free name, instead of failing with "already being downloaded" or continuing the other one's data.
- A link sent from the browser no longer fails with 403 on sites that refuse a browser's User-Agent on a connection that isn't a browser's: MDM asks once more with its own, as Add URL does ([#372](https://github.com/anefzaoui/misterdownloadmanager/issues/372)).
- The video download button moves to a free corner instead of covering a control of the page, such as a thumbnail's watch-later button ([#421](https://github.com/anefzaoui/misterdownloadmanager/issues/421)). Dragging it still puts it where you want it.
- Crash reports: no report any more when one of the download engine's background tasks panics while MDM is already closing ([#422](https://github.com/anefzaoui/misterdownloadmanager/issues/422)).
- Crash reports: a freeze whose window had recovered by the time its dump was taken is reported as a freeze, not as a crash in MDM's own watchdog ([#420](https://github.com/anefzaoui/misterdownloadmanager/issues/420)).
- Text in a script outside the interface language (a video title in Bangla, a playlist in Japanese) now shows its letters, instead of boxes: the font for that script is loaded the first time it is needed ([#405](https://github.com/anefzaoui/misterdownloadmanager/issues/405)).
- Options → Proxy / Socks now applies to yt-dlp and gallery-dl too, so video look-ups and downloads no longer go straight out when a proxy is set ([#363](https://github.com/anefzaoui/misterdownloadmanager/issues/363)).
- The window no longer freezes while watching the clipboard when the program holding it hangs, or while a cancelled download's files are removed from a slow disk ([#418](https://github.com/anefzaoui/misterdownloadmanager/issues/418), [#423](https://github.com/anefzaoui/misterdownloadmanager/issues/423)).
- Downloads at full speed no longer stop and go on a slow disk: writing the resume state flushed the file to the disk so often that every connection waited for it, and were then replaced as slow ([#361](https://github.com/anefzaoui/misterdownloadmanager/issues/361)).
- A save folder shown as `Downloads/Video` could be taken literally, asking to create it in the wrong place or failing with "Read-only file system" on macOS.
- macOS: **Download video** no longer stops with a Python error and a "Python.framework is damaged" alert on a copy of MDM downloaded by a browser ([#330](https://github.com/anefzaoui/misterdownloadmanager/issues/330), [#339](https://github.com/anefzaoui/misterdownloadmanager/issues/339)). MDM now installs yt-dlp's folder build, which it unpacks once and checks against yt-dlp's signed list, instead of the single-program build that unpacks Python afresh at every start. The video window then offers to install it in place of a copy that came with an earlier version.
- Browser link import: tighter handling of site data.
- Crash reports: a freeze that was over before its snapshot was taken says so, instead of calling it a deadlock in code that was only waiting for events ([#355](https://github.com/anefzaoui/misterdownloadmanager/issues/355)).
- **Move/Rename** was greyed out with several downloads selected, for example a set just added to a queue. It now asks for a folder and moves them all there ([#333](https://github.com/anefzaoui/misterdownloadmanager/issues/333)).
- The extension no longer asks you to update MDM when only a bug-fix version separates them, which it did even when there was no update to get.
- **Download video** shows a plain file's size, from its server, instead of "size unknown" ([#320](https://github.com/anefzaoui/misterdownloadmanager/issues/320)).
- **Download video** finds the video on pages whose player loads a `.mpd` or `.m3u8` address that answers something else first, as SVT Play does ([#377](https://github.com/anefzaoui/misterdownloadmanager/issues/377)).
- RSS feeds: the boxes on the **Download rules** tab (**Episodes**, **Must contain** and the others) keep the keyboard while you type, instead of losing it after each key ([#353](https://github.com/anefzaoui/misterdownloadmanager/issues/353)).
- A download refused by a site's bot check (Cloudflare's challenge) says so and how to get the file, instead of "the server answered 403 Forbidden" ([#376](https://github.com/anefzaoui/misterdownloadmanager/issues/376)).
- **Open folder** works on a torrent that is still downloading ([#354](https://github.com/anefzaoui/misterdownloadmanager/issues/354)).
- **Alt** and **F10** open the menu bar while a button or the list has the keyboard focus, not only when nothing does ([#328](https://github.com/anefzaoui/misterdownloadmanager/issues/328)).
- **Hide tab** in a download's window takes the Speed Limiter or Options on completion tab off its tab row, instead of only going back to Download status. Right-click the tab row and choose **Show hidden tabs** to bring them back ([#326](https://github.com/anefzaoui/misterdownloadmanager/issues/326)).
- Windows: no more crash on a computer where Direct3D can't start, when a window opened during the switch to OpenGL ([#337](https://github.com/anefzaoui/misterdownloadmanager/issues/337)).
- Windows: no more crash when a screen is unplugged, turned off or rearranged while MDM runs ([#335](https://github.com/anefzaoui/misterdownloadmanager/issues/335)).
- A finished torrent that isn't being shared lets go of its files, so they can be moved or deleted while MDM runs.
- A torrent says **Checking 42%** while its files are read and checked, and a Recheck no longer stops partway and stays there.
- Video downloads: the progress no longer says 100 % while a video is still coming, when the site gives no total size, or between a video and its separate sound ([#344](https://github.com/anefzaoui/misterdownloadmanager/issues/344)). A video yt-dlp only knows by its address is named after the page, not a long hash ([#345](https://github.com/anefzaoui/misterdownloadmanager/issues/345)).
- **A YouTube playlist in Add URL** ([#338](https://github.com/anefzaoui/misterdownloadmanager/issues/338)) opens the video window with its videos, instead of saving the page as a file named "playlist".
- **Dailymotion's embedded player** ([#359](https://github.com/anefzaoui/misterdownloadmanager/issues/359)): its address is looked up as the video's own page, so the download button works on sites that embed one.
- A download that a page starts by script (a thank-you page going to the installer, as obsproject.com's does) gets the file's address, not the page's, so MDM no longer saves that page under the installer's name ([#374](https://github.com/anefzaoui/misterdownloadmanager/issues/374)). A finished download also shows what really came down when the server gives no size.
- **Stop** on a download in a running queue stays stopped, instead of the queue starting it again a moment later; **Stop All** too ([#341](https://github.com/anefzaoui/misterdownloadmanager/issues/341)).
- Removing the last feed in the RSS feeds window no longer crashes the app ([#321](https://github.com/anefzaoui/misterdownloadmanager/issues/321)).
- **YouTube formats with Node.js or Bun installed** ([#360](https://github.com/anefzaoui/misterdownloadmanager/issues/360)): yt-dlp needs a JavaScript runtime for them and looks for Deno alone, so MDM now tells it about Node.js (nvm's too) or Bun when there is no Deno.
- **YouTube's look-up failing with "Requested format is not available"** ([#352](https://github.com/anefzaoui/misterdownloadmanager/issues/352)): signed-in cookies make yt-dlp use only YouTube's players that need a JavaScript runtime, and without one no video is left. The page is now looked up again without the cookies, and lists its qualities.
- **Download video offering only the sound, with no reason given** ([#351](https://github.com/anefzaoui/misterdownloadmanager/issues/351)): when a page's better videos need ffmpeg and it isn't installed, the window now says so.
- Adding a link again after its finished file was moved or deleted starts a new download, instead of asking about a duplicate that isn't there ([#342](https://github.com/anefzaoui/misterdownloadmanager/issues/342)).
- **Sharp, evenly spaced text on Windows:** letters no longer run together or drift apart. English and other Latin, Greek and Cyrillic text is drawn the way Windows draws its own classic controls, with each letter's shape and width fitted together; every other script uses hinting suited to anti-aliased rendering ([#323](https://github.com/anefzaoui/misterdownloadmanager/issues/323), [#334](https://github.com/anefzaoui/misterdownloadmanager/issues/334)).

## [0.5.1] - 2026-11-08

YouTube fixes for the Download video button, and the button in Chrome, Edge and Brave.

### Added

- **The Download video button on YouTube in Chrome, Edge, Brave, Opera and Vivaldi:** the copy of the extension from the Chrome Web Store shows it now, as Firefox's does.

### Fixed

- **The Download video button took the wrong video on YouTube** ([#311](https://github.com/anefzaoui/misterdownloadmanager/issues/311), thanks to [@mayankmohan1992](https://github.com/mayankmohan1992)): YouTube moves from one video to the next without loading a new page, and the browser kept naming the page it first loaded, so MDM got the video watched before. On the home page, a search or a channel it got that whole page instead, and offered about 50 videos. The button now sends the video playing in the player it's on, and a Short as the Short it is.
- **Media on this page says why it's empty**: on a player that streams in pieces there's no file to list, and it says to use the button over the video instead of asking you to play it again.
- A page's own short sounds (a click, a voice-search chime) are no longer listed in **Media on this page**.

## [0.5.0] - 2026-11-08

### Added

#### Themes Support

Now you can change Mister Download Manager's look and feel, and even its icons, with themes, and you can write your own.

- 9 built-in looks in **View → Theme**: Classic, Native, High contrast, Metro, Adwaita, Breeze, Luna, Aero, Aqua.
- Write your own theme as a TOML or JSON file, or import a Base16 scheme or your pywal colours.

#### Remote Control and Headless Mode Support

MDM no longer has to be in front of you: manage your downloads from your phone, or keep it running on a machine with no screen.

- Add links and manage your downloads and queues from your phone, on your own network: **Options → Remote**.
- Run MDM with no window on a server, NAS, Docker or Unraid with `--headless`, and send it downloads from your browser.

#### Browser extension

The extension now installs from your browser's store, updates itself, and catches more of what you download.

- Install it from the Chrome Web Store or Firefox Add-ons.
- See the streams, videos and sounds a page has loaded, with their qualities, and download any of them.
- **VPN and proxy support:** send downloads through the browser's proxy or VPN extension (off until you turn it on).
- See why a download stayed in the browser, and which setting to change.
- **More browsers supported:** it now works in Zen, Helium, LibreWolf, Floorp and Waterfox too.

#### Video and Images

MDM now goes well beyond single videos: whole playlists, subtitles, sound on its own, and picture galleries.

- **Playlist and channel support:** download a whole playlist or channel from a video site: tick the videos you want and pick one quality for all.
- **Subtitle support:** download a video's subtitles, saved beside it or put into it.
- **Audio-only support:** download just a video's sound, as MP3, Opus or FLAC.
- **Signed-in video support:** download videos that need you signed in, using your browser's cookies.
- **Image gallery support:** download whole galleries (Flickr, Imgur, DeviantArt and more) with gallery-dl.

#### Torrents

Torrents grew into a full client: watch while they download, follow feeds, or let your debrid service do the work.

- **Sequential download support:** download a torrent in order, so you can watch it while it downloads.
- **RSS support:** follow feeds and add new torrents automatically, by your own rules.
- **Debrid support:** use your Real-Debrid, AllDebrid or Premiumize account for torrents and file-host links.
- Limit seeding and upload, recheck files, reannounce, add trackers, and block IPs with a blocklist.

#### Downloads

Hand MDM a pile of links or a Google Drive link and let it get on with it.

- Link grabber: paste many links, MDM groups them, checks which are online, and extracts archives when they arrive.
- **Google Drive support:** links now download the actual file instead of Drive's web page.
- Up to 128 connections per download, if you turn it on.
- The Scheduler can wake the computer to run a queue.

#### Network

Decide exactly how MDM reaches the internet, and see where your files come from.

- Send every download through one network adapter, such as your VPN's. If it drops, nothing goes out through your normal connection.
- **Custom DNS support:** choose who looks up addresses: Cloudflare, Google, Quad9, AdGuard, NextDNS or your own, over HTTPS, TLS or plain.
- See each download's server: its country, flag and round trip.

#### Interface

Bigger when you need it, faster to open, and closer to your system.

- Make every window larger, up to 200 %: **View → Interface size**, or Ctrl + and Ctrl −.
- **Direct3D 12, Vulkan and Metal support:** MDM draws with Direct3D 12 on Windows; Vulkan on Linux and Metal on macOS are options.
- Every file name now has its file-type icon, the one your system uses.
- The built-in categories can now be renamed, given other file types, or deleted.
- Hide a download's progress window to the notification area while it keeps going.
- When MDM starts with the computer, it now waits in the notification area instead of opening its window.

#### Other

Smaller additions worth knowing about.

- **System keychain support:** passwords and account sign-ins are now kept in your system's keychain, not in MDM's settings.
- Use your own copies of yt-dlp and ffmpeg instead of MDM's.
- **AppImage tools support:** the AppImage can update through them, downloading only what changed.
- Website: a page for every feature, a Themes gallery, and a hall of fame.

### Changed

MDM got much lighter, and Linux is now supported like Windows.

- Linux is no longer experimental: checked on GNOME, KDE Plasma, Xfce and Cinnamon, and on clean Ubuntu, Debian and Fedora.
- Downloads are smaller: setup.exe 23 → 16 MB, AppImage 36 → 20 MB, .deb 27 → 17 MB.
- Under 10 MB of memory in the notification area on Windows, from about 50 MB.
- Machine translations in all 85 languages were checked again against the English and corrected.
- Downloads from the browser keep the name, size and type the browser saw, so downloads behind a sign-in show correctly right away.
- MDM follows every kind of redirect a browser does, including "your download starts in a moment" pages.
- Freeze reports now say whether MDM was stuck in a loop, waiting on something, or the computer was too busy to run it.

### Removed

No features were removed, only an old label and some bulky files.

- The `-experimental` label on the Linux files (macOS keeps it).
- The 80 MB universal Mac download: there's now one for Apple Silicon and one for Intel. Coming from 0.2.4, update once by hand: **Download** in Check for updates opens the release.
- The separate 28 MB command-line program: now a 0.2 MB launcher, every command works as before.

### Fixed

More than 80 fixes. The main ones:

- Firefox and Waterfox: MDM now takes the download before the browser's file dialog, so the browser doesn't download it too.
- The extension now reaches MDM from Ubuntu's Snap Firefox and from Brave Origin.
- Download File Info and the progress window open in front of the browser.
- No more black band across menus and windows on Windows 11 Insider builds.
- Text is sharper on Windows.
- A slow start no longer makes MDM give up on Direct3D and stay on OpenGL.
- Linux: menus open in the right place and appear at once on Hyprland and similar compositors.
- Linux: dark mode follows the desktop, and a slow keyring or sleep service no longer freezes the window.
- No more crash when a magnet link or browser download arrives while MDM is minimized or in the tray.
- A minimized MDM keeps running its queues and Scheduler.
- Downloads no longer fail with "part file still in use" on a busy computer.
- Links that work only once no longer fail with "403 Forbidden".
- YouTube's "Requested format is not available" no longer stops a download: MDM takes the nearest quality below.
- Stop now also stops yt-dlp, which used to keep downloading in the background.
- Buttons fit their text in every language.
- Screen readers read lists, menus and windows properly.

### Security

This release tightens what MDM accepts from torrents.

- Unsafe file names in torrents are refused.
- When a torrent's files are already in the folder, MDM asks before writing into them.
- Files from torrents are marked as downloaded from the internet, so SmartScreen and Gatekeeper check programs before they first run.
- MDM no longer contacts torrent trackers on this computer or the local network.
- The torrent library, librqbit, is updated to 9.1.0.

### Every change in detail

#### Added

##### Themes and the look

- **Themes: View → Theme** ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45)).
  **Classic** is MDM's look, unchanged and still the default. **Native** takes your system's
  colours: Windows' accent colour and, when it is on, its contrast theme; the desktop's accent
  colour and higher-contrast setting on Linux (GNOME 47 and later, KDE Plasma); the accent colour
  on macOS; it follows them when they change. **High contrast** is black and white with a bright
  accent, every edge drawn and text at 7:1 or more. A theme applies at once, light and dark alike,
  and theme files in the themes folder (**View → Theme → Open themes folder**) are listed too;
  High contrast is itself such a file, in the same format. Six style themes keep the layout and
  bring their own control shapes, icons and, on Windows, a title bar MDM draws (resizing, Snap
  Layouts on Maximize, double-click and Alt+Space work as with the system's):
  - **Metro**, after Windows 8's flat look: square corners, flat buttons, boxes that take an
    accent border when focused, check boxes with a tick, a bordered progress bar, scroll bars with
    their arrows always shown, one-colour icons (Microsoft's Fluent UI System Icons, MIT) and a
    title bar in the theme's colour with the title centred.
  - **Adwaita**, after GNOME's look: round corners, flat grey buttons, a rounded focus ring, boxes
    that take a ring when focused, GNOME's check boxes and radio buttons, GNOME's Adwaita icons
    (LGPL-3.0), in colour for files and categories and one colour in the toolbar, and a header bar
    with the title in bold and round buttons.
  - **Breeze**, after KDE Plasma's look: rounded corners, buttons and boxes whose edge takes the
    accent under the pointer or when focused, KDE's check boxes and radio buttons (the accent's
    edge, a tick in the text's colour), a focus frame, KDE's Breeze icons (LGPL-3.0-or-later) and
    a title bar in KDE's style: the title centred, round buttons that fill under the pointer
    (Close in red).
  - **Luna, Aero and Aqua**, after Windows XP, Windows 7 and Mac OS X (Leopard), each with a light
    and a dark side and icons drawn for MDM in its era's manner. **Luna**: buttons shaded from
    white with a dark blue edge and an orange glow under the pointer, green ticks, square text
    boxes, progress in green blocks and a deep blue title bar with the title in bold and square
    buttons (Close red). **Aero**: glossy buttons in two halves that turn light blue under the
    pointer, quiet check boxes, a glossy green progress bar and a glassy title bar, the title on
    a soft glow, its buttons hung from the top (Close wider and red). **Aqua**: gel buttons round
    at the ends (the default one blue), glossy check boxes, a blue focus ring, a blue gel progress
    bar and a grey title bar with the title centred and small round graphite buttons whose glyphs
    show under the pointer. Every glyph and shade is MDM's own drawing; none of Microsoft's or
    Apple's pictures or fonts are used.
  - **MDM's own icons where Metro, Adwaita and Breeze had none**: Metro's Hide to the
    notification area, Adwaita's PDF, torrent file and Hide to the notification area, and
    Breeze's PDF and Hide to the notification area, drawn for MDM in each one's style (the
    one-colour ones in the theme's text colour).
  - **Theme files**: every colour and size MDM's controls are drawn with (corner radii, borders,
    font sizes, the list's row height, the icons' ink) comes from the theme, by role; the classic
    look is that theme, unchanged pixel for pixel. A theme file (TOML or JSON in the data folder's
    `themes` folder) can change any role, choose each control's style (`[controls]`) and an icon
    set, and a theme can be a folder that brings its own icons. An icon set can mix PNG pictures
    with SVG drawings, and one-colour icons (`-symbolic.svg`, `-symbolic.png`, drawn in the
    theme's text colour) with coloured ones, and hold pictures at 32 and 48 pixels, used on a
    screen scaled to 150 % and more: the style themes' pictures stay sharp there.
  - **Base16 and pywal colour schemes as themes**: a Base16 or Base24 scheme (`.yaml`, the
    classic layout and tinted-theming's `palette:` one) or pywal's `colors.json` (wallust's
    pywal-shaped one too), added with **View → Theme → Add a theme file…** or put in the themes
    folder, is a theme on the classic look: only the colours change. base00 and base01 are the
    backgrounds, base05 and base04 the text, base0D the focus, the selection and the progress bar
    (pywal: its background and foreground, color4); text is kept at 4.5:1 or more against every
    background and the accent at 3:1, moved a little where a scheme falls short. Once `wal` has
    run, **View → Theme** lists **pywal**, read from where pywal writes it
    (`~/.cache/wal/colors.json`, `%USERPROFILE%\.cache\wal\colors.json` on Windows), and MDM
    follows each new wallpaper's colours within two seconds. A theme file can `inherits` a scheme
    to change more than its colours.
- **View → Interface size** ([#262](https://github.com/anefzaoui/misterdownloadmanager/issues/262)):
  every window larger, 100 % to 200 %, on top of the system's display scale; **Ctrl +**,
  **Ctrl −** and **Ctrl 0** in any MDM window, as in a browser. Every open window follows at once
  (dialogs, menus, the drop target) and MDM starts at that size, the crash report window too. A
  window larger than the screen opens as large as the screen allows and scrolls the rest, its
  title bar on the screen.
- **View → Always dark** ([#60](https://github.com/anefzaoui/misterdownloadmanager/issues/60)):
  MDM stays dark whatever the system says, for a desktop that reports no light or dark
  preference (MDM also reads it on Linux now, see Fixed).
- **A file-type icon before every file name or folder**
  ([#122](https://github.com/anefzaoui/misterdownloadmanager/issues/122)): the download list,
  Download File Info, Properties, Download complete (its open folder), Add torrent (the torrent and
  its tree), a torrent's Files tab, the progress window's Save To, the Scheduler's files, Batch
  download's preview, and Download video's qualities and a playlist's videos (the file each one
  gives). Each is your system's own icon for that type, the one its file manager shows: Windows
  Explorer's, macOS Finder's, or your Linux desktop's icon theme (KDE's, GNOME's or GTK's choice,
  then the themes it builds on). It is sharp at every display scale (125, 150, 200 %), taken at
  the size the screen needs. Each type is looked up once, in the background, so a long list never
  waits for it. Where the system has no icon for a type (a Linux desktop without an icon theme),
  MDM draws one of its own, from the Tango icons (public domain).
- **Windows: MDM draws with Direct3D 12** ([#89](https://github.com/anefzaoui/misterdownloadmanager/issues/89)).
  Its windows open faster (about a quarter of a second sooner on our test PC), and Direct3D may
  avoid the stalls some graphics drivers' OpenGL has
  ([#82](https://github.com/anefzaoui/misterdownloadmanager/issues/82)). **View → Draw with**
  goes back to OpenGL, as before, from the next start; a choice of OpenGL is kept. The window
  always opens: when Direct3D can't start, its first frame doesn't come within 10 seconds, or a
  start with it ends before its first frame, MDM draws with OpenGL until Direct3D is chosen
  again (the menu says so, and why); without a usable OpenGL either, it draws with Microsoft's
  software renderer (Remote Desktop, virtual machines without a graphics driver). When the
  graphics card is reset while MDM runs (a driver update), MDM starts again by itself. Freeze and
  crash reports say what was drawing. Linux and macOS keep OpenGL by default.
- **Linux: Vulkan, and macOS: Metal, in View → Draw with** ([#206](https://github.com/anefzaoui/misterdownloadmanager/issues/206)).
  Linux can draw its windows with Vulkan (on Wayland and X11), and macOS with Metal, the way
  Windows draws with Direct3D. OpenGL stays the default there until reports from real graphics
  cards are in: try it and tell us how it went. The same safety net as on Windows: when Vulkan
  or Metal can't start (no driver for it), its first frame doesn't come within 10 seconds, or a
  start with it ends before its first frame, MDM draws with OpenGL until it is chosen again, and
  the menu says why. A graphics card reset while MDM runs starts MDM again.
- **Hide to the notification area** ([#63](https://github.com/anefzaoui/misterdownloadmanager/issues/63)):
  a download's progress window has a button that hides it in the tray while the download goes
  on: on Windows in its title bar, left of Minimize (the progress window draws its own Windows 11
  title bar now, keeping the system's snapping, shaking, menu and shadow), and in its Alt+Space
  menu; on Linux and macOS at the end of its tab row. The tray menu's new **Downloads in
  progress** lists the running downloads to bring one's window back (double-clicking it in the
  list does too). With **Options → General → Minimize to the notification area** (on by
  default), Minimize hides the main window and progress windows there as well. Only where a tray
  shows MDM's icon.
- **Started with the computer, MDM waits in the notification area**
  ([#226](https://github.com/anefzaoui/misterdownloadmanager/issues/226)): with **Options →
  General → Launch Mister Download Manager on startup**, MDM no longer opens its window in front
  at every sign-in. It starts hidden, its icon in the notification area, with its downloads,
  queues, Scheduler and browser hand-overs running; its icon, or starting MDM again, shows the
  window as before. The same on Linux and macOS; where no notification area shows MDM's icon
  (some Linux desktops), it starts minimized, and the option's tooltip says so (it waits for
  one up to 30 seconds after its icon is made, its window hidden: a desktop's can come up after
  MDM at sign-in). The startup entry now passes `/startup` (new on the
  [command line](docs/command-line.md)); an entry written by an earlier version or its installer
  is written again at the next start, and the installer writes the new one.
- **Drop-down lists for screen readers**: a screen reader or UI Automation can now set a
  drop-down list's choice by its text, and open or close the list.

##### Downloads

- **Downloads through one network adapter, with a kill switch**
  ([#35](https://github.com/anefzaoui/misterdownloadmanager/issues/35)): **Options → Proxy /
  Socks → Network adapter**. Pick one, usually your VPN's, and every connection MDM opens goes out
  through it or not at all: downloads, FTP, the DNS you chose, MDM's own downloads and the update
  check. When the adapter disconnects, nothing goes out through your normal connection. Running
  downloads stop and start again when it's back. The command line's `get` takes `--adapter`. PAC
  scripts' `myIpAddress` no longer opens a network socket on Windows.
  - **Video pages and image galleries** go through it too, on Windows, Linux and macOS: video
    downloads (yt-dlp and the ffmpeg it starts) and galleries (gallery-dl) go through a proxy MDM
    runs on this computer, which opens their connections through the adapter as MDM opens its
    own, with the DNS you chose, IPv4 and IPv6. The same kill switch: when the adapter
    disconnects, their connections close at once and the downloads stop, then start again when
    it is back. Formats over RTMP, RTSP, MMS or FTP, which can't go through it, are refused. The
    command line's `video-get` and `gallery-get` take `--adapter`.
  - **Torrents** go through it on Windows too
    ([#143](https://github.com/anefzaoui/misterdownloadmanager/issues/143)): every torrent
    connection goes out from the adapter's address, with the same kill switch as downloads:
    peers, trackers, the DHT and the listening port. When the adapter disconnects, torrents stop,
    nothing goes out another way, and they start again when it is back.
  - Only `ftps://` (FTP with implicit TLS) can't be held to an adapter yet, so it doesn't start
    while one is chosen.
- **Choose the DNS** ([#36](https://github.com/anefzaoui/misterdownloadmanager/issues/36)):
  **Options → Connection → DNS** picks who turns names into addresses for downloads, FTP, MDM's
  own downloads and the update check. The system's (the default), Cloudflare, Google, Quad9,
  AdGuard, NextDNS (with your configuration ID) or your own, asked over HTTPS (the default), TLS,
  or plain over TCP. **Test** shows how long it takes to answer. When it can't be reached, the
  download stops with a message naming it, unless you allow falling back to the system's DNS;
  your hosts file and local names stay with the system's. The command line's `get` takes
  `--dns`. Torrent trackers still use the system's DNS, and so do video pages and galleries
  unless a network adapter is chosen.
- **Where a download's server is** ([#140](https://github.com/anefzaoui/misterdownloadmanager/issues/140)):
  **Download File Info**, the progress window and **Properties** show the server's country, with
  its flag, and its round trip, such as **about 24 ms (near)**, in green, amber or red. The country
  comes from a table that ships with MDM ([IP to Country Lite by DB-IP](https://db-ip.com),
  CC BY 4.0, refreshed with each release), looked up on the computer: no address is sent
  anywhere. The time is measured from the download's own connections, with nothing extra sent.
  Through a proxy MDM says so instead of guessing, and a CDN's server (Cloudflare, CloudFront,
  Fastly, Akamai and others) is said to be the CDN's, not the site owner's. Properties keeps what
  the last run saw. MDM is about 770 KB larger (the table 674 KB, the flags 95 KB).
- **More than 32 connections per download, if you want them** ([#12](https://github.com/anefzaoui/misterdownloadmanager/issues/12)):
  **Options → Connection** now takes any number from 1 to 32, typed or set with the arrows,
  and **Allow more than 32 connections** raises that to 128, for server exceptions too. Servers
  that allow fewer are still noticed and asked for fewer. The command line's `-c` takes 1 to 128.
- **Google Drive links** ([#34](https://github.com/anefzaoui/misterdownloadmanager/issues/34)):
  paste a Drive file's link (`drive.google.com/file/d/…/view`, `…/uc?id=…`, `…/open?id=…`) and
  MDM downloads the file itself, as it was uploaded, over several connections and resumable,
  with its real name and size in Download File Info. Large files get past Drive's "can't scan
  this file for viruses" page as **Download anyway** does. Google Docs, Sheets and Slides come
  as Word, Excel and PowerPoint files. A private file downloads with the sign-in the extension
  passes from the browser. When Drive won't give a file (too many downloads lately, sign-in
  needed, no access), MDM says so in your language instead of saving Drive's page. The video
  button over a Drive video downloads the original file. Folders aren't downloaded yet: MDM
  says the link is a folder. See [Google Drive links](docs/adding-downloads.md#google-drive-links).
- **Link grabber, and archives extracted when they are downloaded**
  ([#37](https://github.com/anefzaoui/misterdownloadmanager/issues/37)). The window that took
  the browser's "Download all links", Tasks → Import and dropped lists grew into Link grabber
  (also Tasks → Link grabber..., where pasted links go). Links come in packages: a split
  archive's volumes together, the other links of a page together, the rest in "Various files".
  Each link is looked up for its name, size and whether the server has it (Offline links come
  unticked), and each package has its folder, an archive password, "Extract when done" and
  "Delete archives after extracting". **Start** or **Add to queue** (IDM's import, unchanged:
  an `.ef2` file still brings each download's referrer and cookies). MDM extracts ZIP (AES and
  Deflate64 too), TAR, TAR.GZ, TAR.XZ, GZ and XZ itself; 7z and RAR need 7-Zip (split `.7z.001`
  and `.partN.rar` too), and a window says so when it isn't installed. It extracts when a package
  says so, for any download with **Options → Archives → Extract archives when their download
  completes** (off by default), or with right-click → **Extract**. A split archive is extracted
  once its last volume is in. Extracted files are marked as downloaded from the internet, as the
  archive is; names that would leave the folder, links, and archives that would fill the disk are
  refused; an existing file is never overwritten. The archive goes to the Recycle Bin (the Trash)
  only when you ask. Options has a new **Archives** page.
- **Image galleries** ([#58](https://github.com/anefzaoui/misterdownloadmanager/issues/58)): a
  gallery's address in Add URL (a Wikimedia Commons category, a Flickr album, a DeviantArt or
  Imgur gallery and more) opens **Download a gallery**, which downloads its pictures with
  gallery-dl into a folder of their own, all of them or the first N. Started again, a gallery
  fetches only its new pictures. gallery-dl is an optional helper like yt-dlp: installed from its
  releases only when signed by its author's key, or your own copy (Helpers). On the command line:
  `install-gallery-dl` and `gallery-get`.
- **The built-in categories are yours to change** ([#93](https://github.com/anefzaoui/misterdownloadmanager/issues/93)):
  Compressed, Documents, Music, Programs and Video can be renamed, given other file types and
  deleted, like your own categories (right-click → Properties or Delete). A renamed category keeps
  its folder. Deleting one asks what happens to its downloads: move them to General, or take them
  off the list; their files stay either way. **Options → Save to → Restore the built-in
  categories** brings them back as they came. General and Torrents still change only their folder.

##### Videos

- **Playlists and channels** ([#69](https://github.com/anefzaoui/misterdownloadmanager/issues/69)):
  Download video from a page lists a playlist's or a channel's videos, with their lengths, all
  ticked, with Select all and Select none. One quality applies to all of them (the best, up to a
  height, the original sound, MP3, Opus or FLAC). Each video gets its own download, in a folder
  named after the list, and Download Now starts the main queue, which runs as many at once as its
  settings say. A playlist is listed as its videos come in; the listing can be stopped and taken
  up again (List the rest), and one the site breaks off says after how many videos and why. A
  channel is listed 50 videos at a time, with Load more. Lists of thousands stay quick, and adding
  more than 500 downloads asks first. On the command line: `playlist-info` and `playlist-get`.
- **Video subtitles** ([#68](https://github.com/anefzaoui/misterdownloadmanager/issues/68)):
  Download video from a page lists the video's subtitle languages, the uploader's first, then the
  site's automatic captions (in the language spoken, English and yours). They're saved beside the
  video as `<name>.<language>.srt`, or put into the video with ffmpeg (MP4, MKV), and the choice is
  offered again for the next video. Subtitles a site refuses (YouTube limits how often they may
  be fetched) never cost the video: MDM asks once more after a pause, then says which are missing.
  On the command line: `video-get --subs`, `--auto-subs`, `--embed-subs`; `video-info` lists them.
  A finished video and its subtitles are now marked as downloaded from the internet, as other
  downloads are.
- **A video's sound as MP3, Opus or FLAC**
  ([#32](https://github.com/anefzaoui/misterdownloadmanager/issues/32),
  [#70](https://github.com/anefzaoui/misterdownloadmanager/issues/70)): **Download video** lists
  **Audio only MP3** at its best variable bitrate (VBR) and at 320, 256, 192 and 128 kbps, after
  the sound in the site's own format, now named **Audio only, original**. ffmpeg converts it (on
  Windows MDM offers to install it, as for the best video qualities; on Linux and macOS it uses
  the system's), and the browser's **Download video** button offers the same. Under the MP3 ones,
  **Audio only OPUS** (YouTube's Opus is only moved into an `.opus` file, not converted) and
  **Audio only FLAC** (lossless, with an honest note in the window: it can't add quality the
  site's lossy sound doesn't have, the file is only bigger). The command line takes
  `video-get --format mp3:192` (`mp3:0` for VBR), `-f opus:` and `-f flac:`.
- **Cookies for video sites**
  ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229)): **Options → Sites
  Logins** can let yt-dlp read a browser's cookies on this computer (Firefox, Chrome, Edge, Brave,
  Opera, Vivaldi, Chromium, Whale; Safari on macOS), with a profile by name or folder, so a pasted
  link to a video that needs you signed in (YouTube's "Sign in to confirm you're not a bot")
  works. Off by default. When the browser's extension passes the page's own cookies, those are
  used instead. When the browser's cookies can't be read (Chrome, Edge and Brave lock them on
  Windows; a locked keyring on Linux), the Download video window says so in plain words.
- **Your own yt-dlp and ffmpeg** ([#51](https://github.com/anefzaoui/misterdownloadmanager/issues/51)):
  **Download video from a page → Helpers...** shows where each comes from, and **Use another
  program...** takes a copy you already have instead of MDM's own (checked once, then used by
  the window and the command line alike; MDM leaves its updates to you). **Use MDM's own copy**
  goes back.

##### Torrents

- **Download in order** ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39)): to
  watch a video while it downloads, tick **Download in order** in **Add torrent**, or choose
  **Torrent → Download in order** in the list's right-click menu, even while it runs. A file's
  start comes first, and with **First and last pieces first** (on by default) its end comes early
  too, which most video files need before they play. Slower peers take pieces a little further
  on, so the start isn't waiting for them. Other torrents now download the rarest pieces first,
  as BitTorrent programs usually do: on MDM's test swarms they finished just as fast, and they
  finished where a scarce piece's only holder left. Torrents already in the list carry on, with
  nothing checked again. MDM also honours a peer's request to cancel a piece it no longer needs,
  so its upload isn't spent on data nobody wants.
- **RSS feeds** ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39)): **Tasks →
  RSS feeds...** watches RSS and Atom feeds of torrents, lists their articles (download one by
  hand), and its download rules add the new ones whose titles they match: words or regular
  expressions, words to skip, an episode filter (`1x2;1x5-8;2x1-`) that takes each episode once, a
  folder per rule. A feed's backlog is never downloaded by itself.
- **Torrent commands** ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39)): a
  torrent's right-click menu has **Torrent → Recheck files**, **Reannounce**, **Add trackers...**
  and **Copy magnet link**. Its Files tab can tick and untick files while it runs. A `.torrent`
  file's web address, in Add URL or from the browser, opens Add torrent with what the file holds.
  TORRENT is in Options → File types' defaults, so the browser hands `.torrent` files over; a
  list saved before keeps what it had.
- **Seeding and upload limits, and a share ratio that lasts**
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39)): under "Keep sharing
  torrents after they finish", Torrent settings can stop sharing a finished torrent at a share
  ratio, or after it has been shared for some minutes; both off by default. A torrent stopped so
  says it in its event log and isn't shared again at the next start. Torrent settings can limit
  what all torrents together send, and a torrent's progress window can limit its own, kept with
  it. Its General tab now counts what it sent and how long it shared over all its runs, so the
  share ratio no longer starts again at 0 after a pause or a restart.
- **Torrent settings: the network is yours to choose**
  ([#40](https://github.com/anefzaoui/misterdownloadmanager/issues/40)): accept incoming
  connections or not, map the port with UPnP or not, a fixed port, the DHT, finding peers on the
  local network, a SOCKS5 proxy from Proxy / Socks (which turns off what would go around it), an
  IP blocklist (P2P or eMule format, fetched again every 7 days), and stopping the torrent engine
  when no torrent runs. A new help page, [Torrents and your security](docs/torrent-security.md),
  says what others can see and what each switch does.
- **Torrents through your debrid account**
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44)): with **Send torrents and
  magnet links to the first account instead of to peers** ticked (**Options → Accounts**), a
  magnet link or a `.torrent` file goes to the account. Real-Debrid, AllDebrid and Premiumize
  download the torrent on their servers, and the list shows how far they are (**45% at
  Real-Debrid**). MDM then downloads its files over HTTP, with all its connections and resume,
  and removes the torrent from the account's list. Your computer never connects to the torrent's
  peers. Experimental, as the accounts are.

##### Debrid accounts, experimental

- **Real-Debrid, AllDebrid and Premiumize** ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44)):
  **Options → Accounts** keeps your debrid service accounts. Real-Debrid signs in by code in the
  browser, or with your API token; AllDebrid by a PIN in the browser or with an API key, and MDM
  waits while it prepares a slow link; Premiumize with its API key. A link from a file host your
  service supports is unlocked through your account, then downloaded with all of MDM's
  connections, pause and resume, from Add URL, the browser's link menu and batch downloads. When
  the direct link expires, MDM asks the service for a new one. The direct link is never shown or
  copied, and the sign-in is kept like your other passwords (the system's keychain, below). Each
  service is asked well under its request limit. See [Debrid service accounts](docs/debrid.md).
  Experimental: built from the services' public API documentation, not yet confirmed with a real
  paid account.
- **One link directly, and the command line**
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44)): **Download File Info**
  says which account a link goes through (**Download through Real-Debrid (your account)**);
  untick it to download that link directly. **File Properties** has the same choice for a
  download in the list, and MDM keeps it after a restart. The command line's `get` and `resume`
  go through the accounts of **Options → Accounts** too, and `--direct` skips them. A download
  the browser had already started stays direct after a restart, and when one service doesn't
  answer, the other accounts still work.

##### The browser extension

- **In the stores**: the Firefox add-on is on addons.mozilla.org,
  [Mister Download Manager](https://addons.mozilla.org/firefox/addon/mister-download-manager/),
  installed with one click and kept up to date by Firefox (and by Zen and LibreWolf); the
  Chromium one is in the Chrome Web Store,
  [Mister Download Manager](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp),
  for Chrome, Edge, Brave, Opera and Vivaldi, installed with one click and kept up to date.
  Chrome offers it by itself once MDM is installed, and the website, the help and the README
  offer **Add to Firefox** and **Add to Chrome** first. The store's copy needs this release of
  MDM: the helper of MDM 0.2.x serves only the copy in MDM's own folder, and the browser refuses
  the store's ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)). The
  store's copy shows no Download video button on YouTube, whose downloads the store's policy
  forbids; **Tasks → Download video from a page** still takes any page.
- **MDM offers the browser extension**: until a browser's extension has reached MDM, each start
  shows **Add Mister Download Manager to your browser**, with a button for every browser
  installed, which opens its store page (**Install it by hand** stays there for a browser that
  can't use its store). The window says when the extension is connected; **Don't ask again**
  stops it.
- **The extension's toolbar button** says whether the browser reaches MDM. When it doesn't, a
  red **!** and **How to fix it** lead to the new help page, [The browser and MDM aren't
  connected](https://misterdownloadmanager.com/help/browser-help.html), at the section for the
  reason. Its popup also pauses taking downloads, leaves the current site to the browser for
  good, and opens MDM or its Options.
- **Options → General → Test browser integration** checks MDM's side in order (integration on,
  the installed copy, the browsers can find MDM, which browsers' extension connected) and
  offers **Set it up** when the browsers can't find MDM.
- **The extension lists the media a page has loaded**
  ([#38](https://github.com/anefzaoui/misterdownloadmanager/issues/38)). The toolbar button's
  window has **Media on this page**: the HLS and DASH streams the page's players loaded (each
  once, with its qualities and length) and the video and sound files it played, with their size.
  **Download** sends a stream to the Download video window and a file to the list, with the
  page's cookies, the page as the referrer and the browser's User-Agent, so a stream a page won't
  give up can still be saved. Copy-protected (DRM) streams are listed as **Protected** and never
  sent. Nothing new to allow: the extension already watched the tab's responses.
- **Downloads from a browser behind a VPN or proxy extension go through it too**
  ([#42](https://github.com/anefzaoui/misterdownloadmanager/issues/42)): the browser extension
  passes the proxy the browser uses for a download (an extension's, or its settings' manual proxy
  or auto-config script) with it. MDM downloads through the same proxy while **Options → Proxy /
  Socks → Use proxy/socks from a browser's request** is on, which is the default. A proxy that
  needs the VPN extension's own login can't be shared: the download says so rather than going
  without it. It is an opt-in, off as installed: **Send downloads through this browser's proxy or
  VPN extension** in the extension's popup. In Firefox the switch asks for the browser's *proxy*
  permission (to read the browser's proxy, never to change it). Chrome, Edge, Brave, Opera and
  Vivaldi can't make that permission optional, so their extension has it from the install; Chrome
  counts it under the access to every site the extension already has, so the update asks for
  nothing new. There the extension reads the browser's proxy settings: the server for the
  download's scheme (honouring their bypass list), or their auto-config script. Addresses on
  this computer (localhost, 127.0.0.1) go straight to it, as the browser sends them.
- **Zen, Helium and other browsers built on Firefox or Chromium** ([#41](https://github.com/anefzaoui/misterdownloadmanager/issues/41)):
  the extension reaches MDM in them, also when one is the only browser on Linux or macOS (Zen
  reads Firefox's folder, which MDM now creates; Helium its own). **Add Mister Download Manager
  to your browser** lists them, Zen, LibreWolf, Floorp and Waterfox by their own names (Options
  too), and sends Firefox's family to the right page. On Windows MDM lists the browsers Windows
  itself lists, so Helium is no longer taken for Chrome. Flatpak browsers can't start MDM's
  helper, which the help page now says.
- **MDM says why the browser kept a download**
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)): every download left
  to the browser by Options is a line in MDM's log naming the setting that decided (its type
  isn't in **File types**, its site or address is excluded, it's under the minimum size, the
  browser is unticked, browser integration is off, or the extension may not read the site), and
  the extension's button shows the last one, with the file's name. A download from a private
  window is logged without its name or site.
- **More file types taken from the browser by default**
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)): today's installers and
  packages (DMG and PKG for macOS, DEB, RPM and APPIMAGE for Linux, MSIX, APPX, APPXBUNDLE and CAB
  for Windows, JAR), archives (XZ, ZST, TGZ, TXZ) and WEBM, FLAC and OPUS join **Options → File
  types**. A list never edited gains them on the next start; one you edited stays as it is.
  **Options → File types → Don't take files smaller than** leaves small files (a receipt, an
  icon) to the browser, and **Edit list ...** for the addresses left to the browser works.
- **PDFs, music and videos the browser would open itself are taken too**, when their type is in
  **Options → File types** (**Options → General → Keys...** turns it off). Hold **Alt** while
  clicking a link to let the browser have it.
- **Download selected links** and **Download all links** on the browser's right-click menu: the
  page's links go to Import, to tick there.
- **Downloads from forms** (a button that posts and gets a file back) are posted again by MDM,
  with the form's data, instead of being lost. A form that sends a file (multipart) stays with
  the browser.

##### Remote control, servers and scripts

- **Control MDM from your phone** ([#141](https://github.com/anefzaoui/misterdownloadmanager/issues/141)):
  **Options → Remote**, off by default. A phone or another computer on the local network, paired
  with a code (or a QR code) MDM shows, opens a page MDM serves over HTTPS: it adds links, magnet
  links and `.torrent` files, shows the list with its progress, pauses, starts and removes
  downloads, and starts and stops the queues, in the phone's language. Only paired devices are
  answered, each removable in Options, and only on the computer's private network addresses.
  [Guide](docs/remote-control.md).
- **Run MDM on a server** ([#157](https://github.com/anefzaoui/misterdownloadmanager/issues/157)):
  `misterdownloadmanager --headless` runs MDM without a window (a home server, a NAS, Docker,
  Unraid), its folders and port set by flags or the environment; a `Dockerfile`, a Compose file
  and an Unraid template come with it. Phones pair with the code it prints (or
  `misterdownloadmanager-cli pair`), and the browser extension can **send downloads to another
  MDM**. [Guide](docs/server.md).
- The automation API's list also gives the queues, the categories and the free space; it starts
  and stops a queue by name, and an added address can choose its category and queue.

##### Accounts, the Scheduler and support

- **Sign-ins in the system's keychain**
  ([#44](https://github.com/anefzaoui/misterdownloadmanager/issues/44)): debrid accounts, Sites
  Logins' passwords and the proxy's password are kept in Windows Credential Manager, in the
  system keyring on Linux (Secret Service: GNOME Keyring, KeePassXC and others) and in the
  macOS keychain, and the settings keep only a reference. Those already saved move there on
  the next start. A locked keyring is never unlocked and no keyring is ever asked for a
  password: without one, they stay in MDM's settings as before, sealed for your Windows account
  on Windows (the proxy's password now too, like Sites Logins' passwords). **Options →
  Accounts** says where they are. A portable copy keeps them in its own folder, and uninstalling
  with **remove the download list and settings** removes them from Credential Manager too.
- **The Scheduler wakes the computer to run a queue**
  ([#142](https://github.com/anefzaoui/misterdownloadmanager/issues/142)): tick **Wake the
  computer to run this queue** under the start time, and a sleeping computer is woken a minute
  before the queue starts and kept awake while it runs; the line under it says when. On Windows
  it needs **Allow wake timers** in the power plan: when that is off the Scheduler says so, with a
  link to the setting. On Linux an administrator has to allow it once (the guide shows how); on
  macOS only an administrator can schedule a wake, so the option says why it's greyed. MDM never
  asks for an administrator's password. The Scheduler is 40 pixels taller for it.
- **Support MDM**: **Help → Support MDM** and a line in **About** open the creator's
  [GitHub Sponsors](https://github.com/sponsors/anefzaoui) page in the browser; the website, the
  README and the browser extension's popup link to it too. Only when asked: MDM stays free, with
  no reminders, and nothing changes or unlocks.

##### Linux

- **The AppImage updates through AppImage tools too** ([#24](https://github.com/anefzaoui/misterdownloadmanager/issues/24)):
  it carries the address of its updates, and each release has its `.AppImage.zsync`, so
  AppImageLauncher, Gear Lever and AppImageUpdate download only the parts that changed.
  **Update now** works as before.
- **The Linux files are named without `-experimental`**: the AppImage, .deb, .rpm and .tar.gz
  are `MisterDownloadManager-<version>-linux-x86_64.<ext>`, and the .deb's and .rpm's
  descriptions no longer call them an experimental build. Update now, the AppImage tools and the
  website find them by either name. The macOS .dmg files keep `-experimental`.

##### The website

- **A page for every feature, and a Themes page**
  ([#260](https://github.com/anefzaoui/misterdownloadmanager/issues/260)): each feature has a page
  of its own, with the real window, where to find it in the app, since which version, and what it
  can't do yet; the home page lists them all, by group. The Themes page shows every built-in theme
  in its light and dark side, and how to make your own theme file, try it without building MDM,
  and share it, with a prompt to give an AI assistant. What isn't in a release yet says so.
- **Made possible by these amazing humans**: the home page's first view thanks everyone who sent
  a pull request, opened an issue or joined a conversation on GitHub, with their round photos; the new
  [Contributors](https://misterdownloadmanager.com/contributors.html) page shows what each one did;
  each release, on the News page and in its notes on GitHub, ends with the people who helped since
  the one before; and the README thanks everyone too.

#### Changed

##### Downloads and the browser

- A download from the browser keeps the browser's name, size and type: Download File Info no
  longer asks the server again, so a link's own name (`download="…"`), a Unicode name and a
  download behind a sign-in show right away, and correctly. When it must ask (an address typed
  in), it asks with the cookies, referrer and User-Agent the download will use.
- The address the browser was given is kept along with where its redirects led, so an expired
  signed link (a GitHub release, a cloud drive) is renewed when resumed, and a site excluded in
  Options is left alone even when it redirects to another.
- **MDM follows every kind of redirect a browser does** ([#28](https://github.com/anefzaoui/misterdownloadmanager/issues/28)):
  any HTTP redirect (301, 302, 303, 307, 308), up to 20 in a row instead of 10, and the pages
  download sites put in between ("your download starts in a moment"), which lead on with a
  `Refresh` header or a `<meta http-equiv="refresh">`: their wait is kept (up to 10 seconds), and
  the cookies they set go along while the download runs (not into a resume later). A page that
  leads on only through its own script still stops there.
- The download button over videos passes the page's referrer, cookies and the browser's
  User-Agent to yt-dlp, so videos that need a sign-in or play only where they're embedded work.
- Files downloaded from the web are marked as downloaded from the internet, as the browser marks
  its own: Windows' Mark of the Web (SmartScreen, Office's Protected View), macOS' quarantine.
- Downloads from a private window are kept out of the saved list.
- **Torrents behind a proxy open no UDP socket toward the network**: UDP trackers were already
  left out there; now their client stays on this computer too.
- **Proxy auto-config scripts (PAC) run in QuickJS**, a much smaller JavaScript engine, with
  limits on memory and running time: a script that loops or grows forever now fails, as a broken
  script does (the download goes on with the default proxy settings), instead of holding it.
- The extension is called **Mister Download Manager**.

##### Smaller and lighter

- **MDM's programs and downloads are much smaller** ([#25](https://github.com/anefzaoui/misterdownloadmanager/issues/25)):
  on Windows, the programs went from 93 MB to 41 MB (`misterdownloadmanager.exe` from 61 MB to
  38 MB, the command line from 28 MB to a 0.2 MB launcher), and setup.exe from 23 MB to 16 MB.
  On Linux the AppImage went from 36 MB to 20 MB, the .deb from 27 MB to 17 MB.
  - The command line runs in the app's own program, so the download engine ships once. Every
    command, `--json`, Ctrl+C and exit code work as before.
  - The translations (85 languages besides English) are packed, 12 MB of text in under 1 MB
    without the translators' notes, and unpacked once at start.
  - Release signatures are checked by MDM's own small verifier instead of a full OpenPGP
    library.
  - The code that draws the windows is optimised for size (the engine stays optimised for
    speed), and the programs are optimised as a whole.
  - Crash reports name functions from the release's symbol files only, without the debug-info
    reader a build from source can add.
  - The installer packs tighter, and MDM's own code no longer carries the build machine's
    folder paths (a bundled C library still keeps a few of its own).
  - Later, the program is built for size
    ([#158](https://github.com/anefzaoui/misterdownloadmanager/issues/158)), which takes the
    installer from 16.1 to 13.6 MB and the zips from 19.3 to 15.6 MB, with the same features and
    the same download speed.
- **Two smaller macOS downloads** ([#25](https://github.com/anefzaoui/misterdownloadmanager/issues/25)):
  a .dmg for Apple Silicon (`-macos-arm64`, about 14 MB) and one for Intel (`-macos-x86_64`,
  about 16 MB) instead of one universal .dmg of 80 MB. From 0.2.4 or older, update once by hand:
  **Download** in Check for updates opens the release; pick your Mac's .dmg. **Update now**
  works again after that.
- **Less memory in the notification area** ([#72](https://github.com/anefzaoui/misterdownloadmanager/issues/72)):
  closed to the tray, or minimized there (on by default), MDM gives the memory it isn't using back
  to Windows. Task Manager's **Memory** drops from about 50 MB to under 10 MB (with 500 downloads
  in the list, from 58 to about 14 MB). It gives it back again 2 minutes after hiding, when the
  clock's ticks and a one-time allocation have brought some back, then every 10 minutes while MDM
  stays there. After the language list (which needs a font for every script) closes, those fonts
  are let go: about 70 MB less than before. On Linux the heap's free memory goes back to the
  system the same way. [MDM uses a lot of memory](docs/troubleshooting.md#mdm-uses-a-lot-of-memory)
  explains Task Manager's figures.

##### Reports, translations and help

- **Freeze reports say what kind of freeze it was** ([#57](https://github.com/anefzaoui/misterdownloadmanager/issues/57)):
  MDM measures how much processor time the window got while it didn't answer. The report then
  says whether it ran all along (a loop), waited (a deadlock, or the disk or the network), or
  hardly ran at all, which means a computer too busy to run MDM or slow to load it from disk,
  most likely not a bug in MDM. Those last ones share one signature, so they find one issue
  instead of many.
- **Better translations in every language.** Every machine-translated text of the program and
  of the website was read again against the English, with a style guide for each language (how
  it writes commands, questions and labels) and the terms native reviewers settled, and fixed
  where it was wrong or unnatural. Texts a person reviewed (all of Italian and Arabic) were left
  as they are. New texts are translated the same way from now on.
- **Translators: a reviewed translation is never replaced by machine translation.** A row whose
  `#, machine` line a person deleted is kept by `tools/i18n/translate.py` in every case (`--redo`
  included; a check it fails is printed for the person to decide), and CI fails a pull request
  that drops such a row or marks it `#, machine` again (`tools/i18n/reviewed_guard.py`). The first
  reviewed catalog is Italian ([#150](https://github.com/anefzaoui/misterdownloadmanager/pull/150)).
- **The help shows what it describes** ([#228](https://github.com/anefzaoui/misterdownloadmanager/issues/228)):
  every guide and the main help pages now have pictures of the windows they explain, cropped to
  what matters, numbered where a step points at one control, and step strips for the usual flows
  (a download from the browser, a first download, a video, a torrent, a queue overnight). On the
  website they follow its light or dark theme, keep their place while they load, and open larger
  with a click; their descriptions and captions are translated with the page. The pictures are
  made by a script from one list (`tools/shots/`), so they can all be taken again in one go.

##### For developers

- **A Linux test machine, scripted** (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)):
  `tools/lab/linux-vm/` sets up an Ubuntu 24.04 desktop in VirtualBox from a Windows host
  (`vm.ps1`: hardware, headless start, linked clones; `setup.sh`: packages, Rust, Node, the
  browsers, UI automation, an unlocked keyring), builds any branch or pull request there with a
  quick test profile (`build.sh`), and checks pull requests on it: windows shot and compared
  pixel by pixel (`shots.sh`, `shot-host.sh`, `pixdiff.py`), HLS and DASH hand-overs from the Snap
  Firefox and Google Chrome (`streams.mjs`, `streams-chrome.mjs`), the extension's proxy switch
  (`proxy-optin.mjs`), hand-overs and the Scheduler while minimized, torrents without a network,
  and MDM's start-up time (`measure.sh`). Every change is now checked on Linux as on Windows. The
  machine also has copies on KDE Plasma, Cinnamon and XFCE (`desktop.sh`), which get a build
  copied over instead of making one (`vm.ps1 deploy`); a window harness shoots many windows from
  one running MDM in any language, mode and theme, with real clicks and keys inside the machine
  (`win.sh`, `win-host.sh`); and a store package is checked before it ships (`store-check.mjs`,
  `chrome-menu.mjs`).
- **Linux builds and window shots in minutes, on WSL2** (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)): `tools/lab/wsl/` sets up
  `mdm-lab`, an Ubuntu 24.04 WSL2 distribution of its own, which builds any branch or pull request
  for Linux at the computer's full speed (`build.sh`: about 5 minutes from scratch, under a minute
  after a change, against 15 to 60 in the VirtualBox machine), shoots MDM's windows in any language
  and mode on a virtual screen that never shows on Windows (`shots.sh`: about a second a window,
  against 40), compares OpenGL and Vulkan pixel by pixel (`renderers.sh`), and hands a build to a
  VirtualBox machine for what needs a real desktop (`deploy.ps1`).
- **The engine tests no longer fail on a busy computer**
  ([#123](https://github.com/anefzaoui/misterdownloadmanager/issues/123),
  [#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136),
  [#201](https://github.com/anefzaoui/misterdownloadmanager/issues/201)):
  - the pause-and-resume and event-log tests no longer depend on how fast the machine is: the lab
    server can hold a byte back until a test lets it go (`stall_at`), and a test can wait until
    every request it made is in the server's log (#123);
  - three end-to-end tests depended on timing: a refused connection coming back, the learned
    connection limit, and a dead address (a server without ranges was fixed earlier). Each now
    waits for the state it checks. The lab server can hold a connection slot for a test
    (`hold_slot`), and `settled()` now also means every slot is free (#136);
  - nine more tests checked a time, a speed or a byte count that a starved CPU or a busy CI runner
    can't keep. Each now checks the state itself, or times from the event it is about: a slow
    request's own duration, a stop measured from Stop against a wait of minutes, the first bytes
    before a pause, a speed limit against a download the server paces at the same time (and every
    byte paid for once), parts asked again instead of a deadline, links that expire after a number
    of requests instead of milliseconds, range attempts a server without ranges answers slowly
    enough not to fill the sockets, and the bytes a sparse file takes on the drive. The
    slow-connection test now fails without shedding (it passed without it before). The lab server
    can hand out such links (`/redirect?uses=N`) and pace answers to range requests on their own
    (`range_rate`) (#201).
- **The host connection limit test tells a reconnect from a connection too many**
  ([#224](https://github.com/anefzaoui/misterdownloadmanager/issues/224)). A server sees that a
  client dropped an answer only when it next writes to it, so on a busy computer a connection
  that reconnects can be refused once while the server still counts its old answer. The lab
  server now logs, with each refusal, how far the answers holding the client's slots had been
  sent; the test fails only for a refusal none of them explains.
- **Tests no longer make Windows Firewall ask about them.** The torrent tests and test tools
  listen on 127.0.0.1 only (a new setting of the torrent engine, its listen address), so each new
  build of a test no longer opens sockets on every address.
- **`/show menu:<n>` opens the menu bar's menu even with downloads in the list**; it opened the
  first download's right-click menu instead.

#### Fixed

- **The progress window's Download status tab is balanced**: its rows sat 10 px below the
  tab's top edge but touched its bottom edge; there are now 7 px above and below.
- **Where a download's server is says "this computer" or "the local network"**, each with its own
  icon in the flag's place, instead of "this computer or the local network" with nothing; the dot
  between the place and the time is round, where the font's thin "·" read as an apostrophe.
- **Windows: a slow start no longer turns Direct3D off** (part of
  [#280](https://github.com/anefzaoui/misterdownloadmanager/issues/280)): MDM gives Direct3D 10
  seconds to show its first frame, and goes back to OpenGL when it doesn't. Those 10 seconds
  counted MDM's own loading too (the list, the fonts, the icons), which can take that long by
  itself on a busy computer just after it starts, so Direct3D was taken for failing and OpenGL drew
  from then on. Only Direct3D's own time counts now. Crash and freeze reports say what drew, on
  which graphics card and driver (OpenGL's too), and why, near their top, where a report cut to fit
  still has it.
- **Linux: torrents work on a system without CA certificates too** (part of
  [#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264)): the torrent engine built
  its own connection for the trackers, which can't be made without them, so no torrent or magnet
  link started there. It now uses MDM's: peers, the DHT and `http://` trackers work, and an
  `https://` tracker fails saying there are no certificates to check it with.
- **Linux: uninstalling the .deb or .rpm leaves no browser files behind**
  ([#266](https://github.com/anefzaoui/misterdownloadmanager/issues/266)): MDM wrote the files
  that tell each browser where its helper is in every user's home folder, and `apt remove` or
  `dnf remove` left them there, naming a program that was gone. The packages now install them for
  everyone, in each browser's own folder (Chrome and Brave, Chromium and Vivaldi, Edge, Firefox,
  the Snap Firefox included), and remove them with the package; MDM writes none in the home
  folder for those browsers, and takes away the ones an earlier version wrote. The AppImage and
  the tar.gz, and Opera, Helium, Thorium and Firefox's forks (which read them only in the home
  folder), still get them there.
##### Windows, menus and dialogs

- **Sharper text on Windows** ([#262](https://github.com/anefzaoui/misterdownloadmanager/issues/262)):
  Segoe UI and Tahoma are grid-fitted across as well as up and down, and each letter starts on a
  whole pixel, as Windows draws its own grey text: upright strokes were two grey pixels wide at
  100 % and 125 %, which read as blurred.
- Menus opened near the screen's right or bottom edge flip inside it at every display scale and
  interface size: the screen's edge was measured in pixels and the menu in points.
- **A menu shut with Escape opens again at the first click on its title**: a menu opened from
  the menu bar and shut with Escape, the pointer still on its title, was left marked open. On
  Linux the next click on that title only "shut" it, and a second click was needed; on Windows
  Escape seemed to do nothing (the menu came back at once). Escape, or MDM losing the focus, now
  shuts the menu whatever the pointer rests on; a press on a title still shuts its own menu or
  opens another.
- **Menus and the drop target without a black band across their top**
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)): on a Windows 11
  Insider build (22635), every menu and submenu showed a black band a title bar tall across its
  top and a thin one down its left side, its items moved down by as much and its last ones cut
  off; so did the drop target (Tasks → Show drop target), its icon moved down. Each of these
  windows had a title bar's style, hidden by MDM, and something on that build placed the window's
  picture where the title bar would have ended. Menus and the drop target are now plain pop-up
  windows, as Windows' own menus are, with nothing above their contents to take for a title bar;
  every window of MDM without a title bar is made one the same way.
- **The progress window, and the windows with a style theme's title bar, without a black band
  across their top** ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)): on
  the same Windows 11 Insider build, the progress window showed a black band a title bar tall
  above its own title bar and a thin one down its left side, its contents moved down and cut off
  at the bottom; so could every dialog and the main window with Metro, Luna, Aero or Aqua. MDM
  draws these title bars itself, in windows that kept the system's title bar style, hidden.
  On Windows 11 they now have no title bar style at all: the progress window and the dialogs are
  plain windows, all theirs; the main window keeps the system's resizing edges, so it still
  resizes, snaps and maximizes as before, with a thin strip of its title bar's colour above it.
  Moving them by their title bar, their taskbar button, Alt+Tab and the window menu (Alt+Space,
  or a right-click on the title bar) work as before. Windows 10 keeps the frames it had.
- **Windows: Download File Info and the progress window open in front of the browser**
  ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)): a download handed over
  by the browser could open its window behind the browser, and stay there. Windows lets a
  program in the background take the foreground only with the browser's say-so, which the user's
  next click or key ends, and a window refused it opens under the program in front. Every dialog
  and progress window of MDM that opens without the foreground now goes above the other
  programs' windows, without taking the keyboard from the one being typed in: a click on it does,
  as with Download complete.
- **A window closed with its title bar's × no longer turns black first**
  ([#207](https://github.com/anefzaoui/misterdownloadmanager/issues/207)): every dialog and the
  progress window turned black for a moment when closed with the × or Alt+F4, rather than with
  Esc or one of its buttons: one to several seconds on Linux, a few hundredths of a second on
  Windows. The window now keeps its contents until it goes.
- **Dark mode: grey notes and error texts are easier to read**
  ([#45](https://github.com/anefzaoui/misterdownloadmanager/issues/45)): they now have the
  contrast WCAG asks for (4.5:1) on every dark surface, Options' failed checks no longer use light
  mode's dark red on the dark dialog, and an editable drop-down's arrow is tinted dark under the
  pointer.
- **Download complete shows above other programs' windows**, as IDM's does, so the end of a
  download isn't missed ([#77](https://github.com/anefzaoui/misterdownloadmanager/issues/77)):
  it doesn't take the keyboard from the program being typed in, and once clicked it is a window
  like any other. **Options opened from the extension's Button settings... comes to the front**
  ([#77](https://github.com/anefzaoui/misterdownloadmanager/issues/77)), instead of opening
  behind the browser on Windows.
- **Dragging the file out of Download complete works in every language**
  ([#78](https://github.com/anefzaoui/misterdownloadmanager/issues/78)): the handle looked for the
  English window title, so outside English the drag did nothing. The handle now says what it does
  on hover. It is Windows only, and no longer shown on Linux and macOS, where it did nothing.
- **Help opens the website's help pages, in your language**
  ([#227](https://github.com/anefzaoui/misterdownloadmanager/issues/227)): **Help contents**,
  **Tutorials**, **Scheduler and queues**, **Video downloads**, the **Help** buttons of Options and
  the Scheduler, and the browser extension window's tutorial and help links opened the docs on
  GitHub (the Options and Scheduler buttons a page that wasn't there). They open
  misterdownloadmanager.com now, at the same section, in the interface language when the website
  has it (else English), and **Home Page** opens the website too. The command line's and the MCP
  server's help name the website's guide. Problem reports, releases and the source stay on GitHub.

##### Every language

- **Buttons fit their text in every language, in every window**
  ([#192](https://github.com/anefzaoui/misterdownloadmanager/issues/192)): many windows gave
  their buttons a width made for English, so a longer translation was cut with "..." (German
  **Später herunterla...** in Download File Info, **Hilfsprogramme...** in Download a gallery,
  Russian **Открыть с помощью...** in Download complete, French **Nouvelle file d’attente** in the
  Scheduler). Each button is now as wide as its translated text, and the buttons beside it move
  along with the same gaps, the row keeping its end; a box or caption beside a widened button
  ends before it. Where a window has no room left, the buttons wider than their text give some
  back first. English is unchanged, and in Arabic and the other right-to-left languages the
  buttons widen the mirrored way.
  - **Three more controls** (after #212): in Russian and a few other languages, Download File
    Info's **Preview**, Options → Proxy's **Get System** and the Scheduler's drop-down after
    **Turn off computer when done** still ended in "...". Download File Info grows by what
    Preview's label needs (Russian, Ukrainian, Telugu, Belarusian and a few more), the Proxy
    page's title moves left to give Get System its room, and the Scheduler's drop-down widens
    towards its caption.
  - **The last languages** (after #247): Options → Proxy's title takes two lines where one leaves
    Get System too little room (Bulgarian, Armenian, Tamil), and where it ran over the page's icon
    or past the window's left edge (Greek, Finnish, Catalan, Estonian, Latvian, Lithuanian,
    Malayalam, Serbian, Afrikaans). In Greek and Malayalam, the Scheduler's "Turn off computer
    when done" and its choices have shorter machine translations, so the drop-down fits beside
    the caption; in Belarusian, Welsh, French, Russian, Tamil and Ukrainian, the longer ones the
    translation review brought (such as Russian's Hibernate) are back to their earlier, shorter
    wording.
- **Download video: the buttons fit their text in every language**
  ([#172](https://github.com/anefzaoui/misterdownloadmanager/issues/172)): in German **ffmpeg
  installieren** showed as "ffmpeg installie...", and in Russian **Helpers...** and **Install
  ffmpeg** were cut too. Each button is now as wide as its translated text (English is unchanged),
  and the page's title stops before the Install button instead of running under it. In Arabic
  and the other right-to-left languages, a widened **Hide tab** in a download's window now grows
  into the room beside it instead of towards the window's edge.
- **"Hide tab" was cut in longer languages** (German "Registerkarte au..."): the button grows to
  fit its label where there is room, through a shared helper other buttons can use.
- **Add torrent: the labels fit in every language**: in German, **Speichern unter** ran under
  its field.
- **A long file name ran out of the progress window** ([#62](https://github.com/anefzaoui/misterdownloadmanager/issues/62)):
  on the **Options on completion** tab, a longer translation of "Save To:" (Turkish, German…)
  ran into the path, and the path ran past the window's edge. The label now takes the room its
  language needs, and the path is shortened in the middle, keeping the file's name in sight,
  and shown whole on hover; right-to-left languages included.
- **The crash report window in other languages**
  ([#204](https://github.com/anefzaoui/misterdownloadmanager/issues/204)): in any language but
  English, the report in its box ran together on a few long lines, as its line breaks were taken
  for a translation's and flowed again. The report now keeps its own lines in every language, and
  in Arabic and Persian it reads left to right, from the box's left edge, as the English data it
  is (the window around it stays right to left). The release notes in **Check for updates** keep
  their lines the same way. **End and restart MDM** showed cut in German, French, Russian and
  Arabic: the buttons are now as wide as their text, the others giving back room they don't need.
- **The crash report window's status line stays inside the window**
  ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)): the line above the
  check box ("The symbols couldn't be fetched: …", an issue's title, a translation) ran past the
  window's right edge, or its left edge in Persian. It is now cut with "…" at the margin and shown
  whole under the pointer. Its title ("Mister Download Manager crash report") is now in MDM's
  language too; it was always in the system's. On Windows that also gives its title bar MDM's
  light or dark again in every language: outside English it kept Windows' own mode.
- **The extension's popup: Open Mister Download Manager shows whole**
  ([#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)): on Linux, whose fonts
  are wider, and in longer languages, the button was cut to "Open Mister Do…". Each button is now
  as wide as its text, and Options moves below it when both don't fit on one line.
- **Numbers are written the interface language's way** ([#128](https://github.com/anefzaoui/misterdownloadmanager/issues/128)):
  counts, sizes, speeds, percentages and the list's columns group and point their digits as the
  chosen language does (German `10.200` and `1,50 MB`, French `10 200`, Hindi `1,00,000`),
  whatever the Windows region; a region in the same language keeps its own way (Swiss German
  `10’200`). English is unchanged.
- **Download complete's Open with... stayed in English** in every other language
  ([#80](https://github.com/anefzaoui/misterdownloadmanager/issues/80)). The tool that collects
  MDM's texts for translation now also finds labels given through a window's own helpers, and
  checks in CI that it keeps finding them.
- **Download File Info's errors are worded in the interface language**, as the list's are.
- Bangla shows Bangladesh's flag instead of India's, in the language picker, on the website and
  in the README ([#27](https://github.com/anefzaoui/misterdownloadmanager/issues/27)).

##### Linux

- **Ubuntu's Firefox reaches MDM** (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)):
  the Snap Firefox never makes the folder MDM looked for, so browser integration wasn't set up
  for it and its extension couldn't find MDM. It is now, and the Snap goes through the desktop,
  which asks once (**Allow**); the help says how to be asked again after **Don't allow**.
  Chromium as a Snap ran MDM's helper inside its sandbox, away from MDM (it started a second,
  sandboxed MDM): MDM no longer offers it, and the help says why.
- **Brave Origin connects, and the AppImage's helper is always there**
  ([#253](https://github.com/anefzaoui/misterdownloadmanager/issues/253)): Brave Origin keeps
  its settings in a folder of its own (`~/.config/BraveSoftware/Brave-Origin`), which MDM didn't
  know, so its extension said MDM wasn't installed. MDM now sets up Brave Origin, the Beta, Dev
  and Nightly versions of Brave, Chrome and Edge, and Vivaldi's snapshot; on macOS, Brave reads
  Chrome's folder, which MDM now writes for it. From the AppImage, MDM copies the helper program
  the browsers start out of the AppImage under a new name and swaps it in, so a browser still
  running the old copy no longer keeps the new one out, and puts it back at each start if it went
  missing. A copy or setup that fails is written to MDM's log, and **Test browser integration**
  says when the helper program is missing, when no browser was found, or why setting it up
  failed.
- **Dark Mode support follows the desktop, on every desktop**
  ([#156](https://github.com/anefzaoui/misterdownloadmanager/issues/156)): **View → Dark Mode
  support** did nothing on Linux: MDM asked the window system for the desktop's light or dark
  setting, and X11 never says. It now asks the desktop itself (the freedesktop portal, which KDE,
  GNOME, Cinnamon and most others answer) and follows it as it changes. Where the portal doesn't
  answer, as on XFCE and MATE, MDM stayed light under a dark theme: it now reads the theme the
  desktop gives its programs (a name such as Adwaita-dark, Mint-Y-Dark or Greybird-dark is dark),
  GNOME's own dark style setting when there is no portal, and GTK's settings with no desktop at
  all, and follows changes as they happen. The portal still comes first. A change now shows at
  once too: the window waited for its next theme check, up to 20 s on a busy machine, on every
  desktop.
- **Menus open where they belong**: in a Wayland session they opened in the middle of the screen
  ([#50](https://github.com/anefzaoui/misterdownloadmanager/issues/50)), as Wayland lets no
  program place its windows, and MDM's menus and drop-down lists are windows it places; MDM now
  runs through XWayland when it is there (`MDM_WAYLAND=1` keeps Wayland). Under tiling
  compositors such as sway ([#267](https://github.com/anefzaoui/misterdownloadmanager/issues/267)),
  through XWayland, the menu bar's menus, their submenus, the right-click menus and the drop-down
  lists still opened in the middle of the screen, as the compositor placed them like windows. They
  are now menu windows no window manager places, as X11 menus are, so they open where MDM puts
  them, on every window manager.
- **Linux: menus appear at once, as menus, on compositors that animate windows** (Hyprland,
  picom with fading, [#277](https://github.com/anefzaoui/misterdownloadmanager/issues/277)).
  Before the fix above, Hyprland gave MDM's menus a window's border and slid each one down from
  the top of the screen. And moving from title to title along the menu bar closed one menu
  window and opened another, so such a compositor faded each menu out and the next one in at
  every switch. The menu bar's menus now share one window that moves from title to title, as a
  submenu's window already did from item to item, and each menu window says which menu it is (a
  drop-down menu, a right-click menu, a drop-down list), as GTK's and Qt's do.
- **Leaving a submenu no longer closes the whole menu**
  ([#250](https://github.com/anefzaoui/misterdownloadmanager/issues/250)): with a submenu open
  (Tasks → Import, Downloads → Speed Limiter), moving the pointer back onto its menu, to another
  item, closed the menu itself as well. Some window managers (KWin, sway) give a submenu's window
  the focus; when the submenu closed, its window handed the focus on, and for a moment no window
  of MDM had it, which counted as MDM losing the focus. A menu now closes for that only when the
  focus stays away (a click in another program). A submenu also closes when the pointer rests on
  another item without moving, instead of waiting for the next move.
- **The window couldn't be closed** on desktops without a tray, such as MangoWC
  ([#59](https://github.com/anefzaoui/misterdownloadmanager/issues/59)): closing hid it to a tray
  that wasn't there. Closing now hides to the tray only when a tray shows MDM's icon, and quits
  otherwise. (The menus and dark mode there: see #50 and #60.)
- **The first window opens seconds sooner, and no longer freezes on a busy computer**
  (part of [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)): MDM asked
  fontconfig for its fonts one at a time (one program for each of some thirty scripts, more each
  time the language list opened) while the window waited, 2 s on an idle computer and long
  enough on a busy one for a freeze report on the very first start. It now asks once, all
  together, while the rest of MDM starts.
- **Open folder no longer freezes the window**
  ([#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222)): Download complete's
  **Open folder** asks the desktop's file manager (over D-Bus) to show the file, and MDM waited
  for its answer on the window's own thread. A file manager slow to start, or one that never
  answered (seen on Pop!_OS), froze the window until the freeze report caught it. MDM now asks in
  the background and, when the file manager hasn't answered within 5 seconds, opens the folder
  instead. Every other program MDM starts from a window goes the same way, on every system: Test
  browser integration's **Set it up**, moving an extracted archive to the Trash (on a Mac the
  Finder may ask first), putting an update in place (**Installing the update...**, with Cancel
  greyed while it runs), opening pages and folders, reading the icon theme on Linux and, on a
  Mac, the accent colour (every 2 seconds with the Native theme) and the system's version for a
  bug report.
- **The window no longer freezes when the system's sleep or keychain service is slow**
  (follows [#222](https://github.com/anefzaoui/misterdownloadmanager/issues/222)): a queue set to
  **Wake the computer to run this queue** asks the system to keep the computer awake while it
  runs. On Linux that is logind, which can be slow to answer just after the computer wakes,
  exactly when such a queue starts, and MDM waited for it on the window's own thread (asking
  again every 5 seconds while it refused). It is now asked in the background, on every system;
  the queue's downloads, retries and log lines are the same. A keyring that doesn't answer at
  start (a stopped GNOME Keyring) is now waited for once, at most 5 seconds, instead of once per
  saved password, and saving the list no longer asks it at all. On Linux the check for a tray to
  hide windows in, and in Download File Info the check that **Save As** isn't a folder (slow on a
  network share that doesn't answer), no longer run on the window's thread on every frame.
- **The .deb and .rpm bring what MDM's window needs** (part of
  [#164](https://github.com/anefzaoui/misterdownloadmanager/issues/164)): on a system without
  libxkbcommon-x11 (a minimal install, a container), MDM installed from them stopped at start
  under X11 and XWayland, and without the CA certificates (Debian's minimal install) no download
  started. They now require both, and EGL for Wayland windows. They also require the glibc the
  programs were built against, read from the programs themselves, instead of a fixed version: a
  package built on a newer system no longer installs on an older one where it can't start.
- **A missing window library is named, with how to install it**
  ([#265](https://github.com/anefzaoui/misterdownloadmanager/issues/265)): from the tar.gz or the
  AppImage, on a system without libxkbcommon-x11 (or another library X11 windows need), MDM
  stopped at start with a crash. It now says which library is missing and the command that
  installs it (apt, dnf, zypper, pacman or apk, by the distribution), on the terminal and in a
  message box (zenity or kdialog, when there is one). In a Wayland session it opens its window
  on Wayland instead, saying so.
- **Plain HTTP downloads work on a system without CA certificates**
  ([#264](https://github.com/anefzaoui/misterdownloadmanager/issues/264)): on a minimal install
  without the `ca-certificates` package (the tar.gz, the AppImage or a source build), no download
  started, `http://` ones included ("No CA certificates were loaded from the system"). Plain HTTP
  and FTP now work there, and each HTTPS or FTPS download fails at once, without retrying, saying
  that the computer has no certificates to check the server with and to install
  `ca-certificates`.
- **Galleries work with the gallery-dl a Linux distribution ships**
  ([#236](https://github.com/anefzaoui/misterdownloadmanager/issues/236)): with Ubuntu 24.04's
  gallery-dl 1.26.9 chosen in Helpers, every gallery failed at once ("unrecognized arguments:
  --no-input"): MDM passed options that only newer releases know. MDM now reads the chosen
  gallery-dl's version and passes only what that release accepts, from 1.20 (Ubuntu 22.04) to
  today's; checked against every release from 1.17 to 1.32. An older one (Debian 11's 1.17) is
  refused when chosen, saying it is too old and which version MDM needs, and one chosen before is
  passed over for MDM's own copy, which Helpers says. gallery-dl 1.30.0, whose version starts
  with an empty line, is no longer taken for another program.
- **Update now asks the package manager which package MDM is**: a copy in /usr/bin was taken
  for the .deb on a Debian-like system and for the .rpm anywhere else, so a copy installed with
  another package manager (pacman on Arch) would have fetched and opened an .rpm. MDM now asks
  dpkg, rpm and pacman which one owns its program; the .deb and .rpm update as before, and a copy
  of another package manager is left to the system's updates: Check for updates shows the new
  version's notes and says to update it there.
- **Update now finds the Linux and macOS files by either name**: a release's AppImage, .deb,
  .rpm, .tar.gz and .dmg were named `…-experimental`; MDM now looks for the name without it
  first and still takes the older one, so Update now keeps working whichever a release uses.

##### Downloads

- **A minimized MDM keeps working**: with the main window minimized (not to the notification
  area), queues didn't start their next file, the Scheduler's starts and stops didn't fire,
  download limits, the network adapter's kill switch, the clipboard monitor, the update check, the
  taskbar's progress and saving the list all waited until the window was restored. They run in
  every window state now, and their windows (Download complete, a limit's notice) open at once.
- **A download or magnet link sent while the window is minimized opens at once**
  ([#162](https://github.com/anefzaoui/misterdownloadmanager/issues/162)): with the main window
  minimized (not to the notification area), a download from the browser or a magnet link was
  accepted but its window waited until MDM was restored. Download File Info and Add torrent now
  open right away, as they do with the window open or in the notification area.
- MDM closed with an internal error when a window opened while the main window was minimized
  or hidden in the notification area, for example Download File Info for a download from the
  browser, or Add torrent for a magnet link opened in the browser
  ([#22](https://github.com/anefzaoui/misterdownloadmanager/issues/22),
  [#145](https://github.com/anefzaoui/misterdownloadmanager/issues/145)). The cause was in egui,
  the toolkit MDM is drawn with ([emilk/egui#8470](https://github.com/emilk/egui/issues/8470)),
  and is fixed in MDM's copy of it.
- **A finished download no longer fails with "part file still in use" on a busy computer**
  ([#201](https://github.com/anefzaoui/misterdownloadmanager/issues/201)): when the last bytes
  came in, the connections still running were cut off. One cut off while it read or wrote the
  part file (checking the bytes before a reconnect, for example) left that read or write to finish
  on its own, with the file still open, and renaming the finished file failed. The connections are
  now told to stop and waited for, when a download completes and when it is stopped or fails, so
  none of them outlives it. A stopped download's last writes are also in its saved progress now.
- **A download's progress file could be written again just after it stopped**
  ([#106](https://github.com/anefzaoui/misterdownloadmanager/issues/106)), and **a download could
  fail as it finished, with "part file still in use"**
  ([#116](https://github.com/anefzaoui/misterdownloadmanager/issues/116)). MDM saves a
  download's progress every few seconds, in a file beside it. A download paused or finished
  during one of those saves could stop before the save was done; that save then wrote an old
  progress file a moment later, or, when the last bytes arrived during it, still held the file. A
  download now stops only once its save is done.
- **A download too big for the drive is refused on a busy computer too**
  ([#224](https://github.com/anefzaoui/misterdownloadmanager/issues/224)): before a download
  starts, MDM checks the drive has room for it. It gave up checking after 5 seconds (a network
  drive that no longer answers can hang), counted from when it asked; on a very busy computer
  the check could wait longer than that just to start, and a download too big for its drive
  started anyway. The 5 seconds now count from when the check runs, and it asks only the
  folder's own drive instead of listing every drive, which takes well under a millisecond. A
  shared network folder (`\\server\share`) is checked too, which it wasn't before, and a
  folder that doesn't exist yet is checked on the drive it will be made on.
- **A server without ranges no longer sends the file twice**
  ([#136](https://github.com/anefzaoui/misterdownloadmanager/issues/136)): when the extra
  connections' answers came late (a busy computer), the download could finish and then start over
  on one connection, fetching the whole file again. MDM now stops its other connections once every
  byte is in, and a server that never said it takes ranges is known for one at its first full
  answer (a CDN that says it does and misses a range now and then still gets several connections).
- **Stopping a download under a low speed limit took seconds**: each connection first waited
  for its turn under the limit. Stop now ends those waits at once.
- **Links that work only once** (or hosts that allow one download at a time) failed with "403
  Forbidden" after Download File Info had looked them up
  ([#28](https://github.com/anefzaoui/misterdownloadmanager/issues/28)): the window's look and the
  download each asked the server. The download now continues from the answer the window got,
  and asks again only if that connection has closed meanwhile.
- A download whose address ends in a long token (file hosts such as AnkerGames' mirrors) failed
  with "the path is too long for Windows" before it started
  ([#28](https://github.com/anefzaoui/misterdownloadmanager/issues/28)). Every new download's name
  is now shortened to fit its folder (keeping the extension), and when nobody named the file
  (no name from the browser, no answer to the look-up, not edited), it gets the name the server
  gives, as in a browser: `Call-of-Duty-Infinite-Warfare-AnkerGames.zip` rather than the
  address's token.
- **Resuming after a waiting page**: when a download's address had to be asked again (an expired
  link), MDM now goes through the same waiting pages and redirects as the first time, instead
  of giving up on them.
- **A Google Drive link saved Drive's web page** (a file named `view` or `download`) instead of
  the file ([#34](https://github.com/anefzaoui/misterdownloadmanager/issues/34)).
- File names can no longer hide a right-to-left mark (`invoice‮fdp.exe`), and a server's
  Unicode `filename*` name wins over its plain fallback.
- **A freeze while MDM handles a command is reported again**
  ([#202](https://github.com/anefzaoui/misterdownloadmanager/issues/202)): since the clock and
  what comes in from outside (a second launch, the browser, the automation API) run in every
  window state, a freeze there opened no crash report window and wrote no freeze report, minimized
  or not. The watchdog now times that part of each frame too.

##### The browser

- **Waterfox (and Firefox set to ask what to do with each file): the download stayed in the
  browser** ([#208](https://github.com/anefzaoui/misterdownloadmanager/issues/208)). Firefox's
  family hands an extension the download only after its *What should … do with this file?*
  dialog, by which time it has already downloaded the file behind the dialog. MDM now takes the
  download as the server's answer arrives, before the dialog: for a file MDM takes, the dialog
  never shows and the browser downloads nothing. Other files open the dialog as before.
- A second browser download arriving while Download File Info was open replaced the first, which
  was lost (the browser had already given it up). They now wait their turn, and those still
  waiting when MDM quits are kept in the main queue (Download Later).
- When Firefox started MDM for a download, closing its helper could close MDM too.
- A download with very large cookies made the browser's helper start a second MDM and wait 5
  seconds; MDM only starts now when it isn't running, and takes what the browser sends.
- MDM listens for the browser before its window is up, and the browser's helper waits 20 seconds
  for a slow first start instead of 5.
- Browser downloads are no longer taken when the extension may not read the site (its access set
  to **On click**): without the site's cookies they would fail.
- The extension's right-click menu, and its translation, are set up again cleanly after an
  update.
- **The browser extension's Download video menu: an item's highlight ran past the menu's
  edge** ([#81](https://github.com/anefzaoui/misterdownloadmanager/issues/81)).

##### Videos

- **HLS and DASH videos from the browser, as their stream**
  ([#155](https://github.com/anefzaoui/misterdownloadmanager/issues/155)): a video played as a
  stream (most sites' long videos) failed with yt-dlp's "Unsupported URL" when the extension sent
  the page instead of the stream. **Download with Mister Download Manager** in the right-click
  menu on such a video now sends the stream its player loaded, as the button over videos and
  **Media on this page** do. In Firefox it did nothing at all. The cookies sent are those of the
  stream's own server, not the page's.
- **Streams served as plain text, from addresses without .m3u8 or .mpd**
  ([#171](https://github.com/anefzaoui/misterdownloadmanager/issues/171)): some players load
  their stream from such an address, and the extension didn't know it as one. The button over the
  video, the right-click menu and **Media on this page** then sent the page alone, which yt-dlp
  answered "Unsupported URL". Now, when one of them is used and no stream is known, the extension
  reads the start of what the player fetched (a few kilobytes, never on sight) and recognises an
  HLS playlist or a DASH manifest.
- **Videos that need a sign-in** ([#229](https://github.com/anefzaoui/misterdownloadmanager/issues/229)):
  - **from the button over them**: the button's cookies reached the Download video window's
    look-up but not the download's own, which then failed with "Sign in to confirm you're not a
    bot";
  - **from YouTube Music and YouTube's mobile site**: the button over a video on
    music.youtube.com or m.youtube.com passed the page's cookies for that host only, while yt-dlp
    asks www.youtube.com, so the sign-in didn't reach it. The extension now sends each cookie with
    the domain the browser keeps it for, and MDM gives yt-dlp each one for that domain:
    youtube.com's sign-in reaches every YouTube host, and a host's own cookie stays with that
    host. With an older extension, a YouTube page's cookies (or a site's mobile page's, m. and
    mobile.) go to the whole site.
- **A video download could fail in MDM while yt-dlp went on and finished it**
  ([#79](https://github.com/anefzaoui/misterdownloadmanager/issues/79)). A line yt-dlp printed in
  the system's code page rather than UTF-8 (a title in Turkish or Russian on some Windows) ended
  MDM's reading there, and the finished file was then "not said". yt-dlp now always writes
  UTF-8, every line is read to the end, and the saved file is also found by its name. Whenever
  MDM gives a video download up, it ends yt-dlp and everything yt-dlp started, so nothing goes
  on downloading unseen.
- **Stopping a video download left yt-dlp downloading in the background** on Windows
  ([#61](https://github.com/anefzaoui/misterdownloadmanager/issues/61)): `yt-dlp.exe` starts a
  second process that does the work, and only the first was ended. Resuming then failed ("yt-dlp
  did not provide information") and a new try crawled, sharing the line with the old one. Each
  helper now runs in a job of its own that Stop ends whole.
- **YouTube: "Requested format is not available" after MDM moved a video to another server**
  ([#94](https://github.com/anefzaoui/misterdownloadmanager/issues/94)). When the servers this
  computer can reach don't serve the chosen quality, MDM downloads the nearest quality below it
  (always a video, never the sound alone) and says so in the download's log. With no video left
  at all, it tries the page's own servers. A server yt-dlp couldn't resolve or reach is avoided on
  the next look-up. The log also names the qualities a move left out.
- **ffmpeg could fail to install** with "the release has no an LGPL Windows build": BtbN, whose
  builds MDM installs, publishes a dated build each day and then remakes its `latest` release,
  and meanwhile GitHub's "latest" is the dated one, whose files are named differently. MDM now
  reads BtbN's newest releases, either naming, and takes the newest complete build. Errors while
  installing yt-dlp or ffmpeg are also worded in the interface language now.

##### Torrents

- **A magnet link could wait minutes for its file list** ([#92](https://github.com/anefzaoui/misterdownloadmanager/issues/92)):
  in 0.2.4 on Windows, the DHT (how MDM finds peers without trackers) could stop for the rest
  of the session once a computer it asked was gone, so a magnet its trackers didn't serve never
  got its file list. The updated torrent library keeps going, and a test keeps it that way.
  **Add torrent** now shows the wait: the peers found, from the trackers, the DHT or the local
  network, and how many answered. After 30 seconds without an answer, it says what may be in
  the way.
- **Speed limits now work as they do for other downloads**
  ([#39](https://github.com/anefzaoui/misterdownloadmanager/issues/39)): a limit set in a
  torrent's progress window changed every torrent's limit, a remembered limit was ignored, the
  Speed Limiter's "for one file" was shared by all torrents together, and "Limit all downloads
  together" left torrents out. Each torrent now has a limit of its own, a remembered one applies
  again when it resumes, and torrents and other downloads share the global cap evenly: each
  side gets at least half of it, and the rest when the other side needs less. A limit under
  16 KB/s, which made a torrent drop its peers and stall, gives it 16 KB/s. A torrent started
  again while its files were still being checked no longer stays paused, and of two torrents
  started at the same moment, the second no longer fails with "the torrent is no longer there".
- **A finished torrent's progress window lost its tabs while it was still shared**: Files, Peers,
  Trackers, Pieces and General (with the share ratio) said "Shown while the torrent is
  downloading." They now stay for as long as the torrent is shared.

##### Screen readers

- **Every row of a list, not only those in view**
  ([#111](https://github.com/anefzaoui/misterdownloadmanager/issues/111)): a list's rows out of
  view (the last qualities of Download video, a long playlist, the downloads) are there too,
  named, with their place ("12 of 40"), and can be scrolled into view or selected. A list of
  thousands offers 300 rows around the view, so it stays as fast.
- **Lists read as blank items** ([#95](https://github.com/anefzaoui/misterdownloadmanager/issues/95)):
  the rows of Download video's qualities, Add torrent's files, the language picker, Options
  (browsers, sounds, the file-type and site lists), the Scheduler and Import now have their names
  (a format and its size, a file's name and size), their check boxes theirs, ticked or not, and
  their selected row; Download video's list is called Quality, Add torrent's Files and the
  sounds' Sounds. Choosing a row from a screen reader (UI Automation's Select) now shows the new
  choice at once: it used to wait for the next mouse move.
- **List rows sit inside their list** ([#102](https://github.com/anefzaoui/misterdownloadmanager/issues/102)):
  they used to be listed beside it. A screen reader now says which row of how many ("4 of 12"),
  choosing from the list itself works (UI Automation's Selection), and a tree's check box belongs
  to its file's row. The language picker's list and the Options browser list and the Scheduler's
  two lists have names now too.
- **The Categories pane and the menus** ([#113](https://github.com/anefzaoui/misterdownloadmanager/issues/113)):
  the pane reads as a tree (each category named with its count, open or shut, selected), the menu
  bar and every right-click menu as menus whose items can be run, and **Shift+F10** or the
  **Menu** key opens the selected category's menu. A menu opened from the keyboard no longer stays
  invisible until the mouse moves.
- **A window's contents the first time they read it**
  ([#131](https://github.com/anefzaoui/misterdownloadmanager/issues/131)): a window read for the
  first time (Link grabber, Options, About...) showed only its title bar until it was drawn
  again. It now answers with what it shows at once, as soon as a screen reader is running.
- **The progress window's Speed Limiter** took a speed set with a screen reader or UI Automation
  only after the focus moved on, and its "Remember Speed Limiter settings for this file" check box
  had no name. A speed set that way applies at once, and the box is named.

#### Security

- **Torrents: file names are checked more strictly** ([#40](https://github.com/anefzaoui/misterdownloadmanager/issues/40)):
  a torrent with names the system can't write safely is refused, saying which name, and so are
  torrents kept from an earlier run.
- The torrent library is updated (librqbit 9.1.0).
- **No silent overwrite**: when some of a torrent's files are already in the folder, MDM asks
  before writing into them (**No** is the default). The command line's `torrent-get` stops and
  names them, unless given `--use-existing`.
- **Files from torrents are marked as downloaded from the internet**, as web downloads are:
  SmartScreen checks a program before it first runs, Office opens documents in Protected View,
  and Gatekeeper checks apps on a Mac.
- Torrents' trackers on this computer or the local network aren't contacted, and a `.torrent`
  file over 10 MB isn't read (it isn't one). Peers see MDM's major version only.

## [0.2.4] - 2026-11-01

A fix for 0.2.3, whose toolbar buttons did nothing (only their drop-down arrows worked), and for
dark mode, which drew everything a little bigger and in other places than day mode.

### Added

- **The video helpers are offered on the first start.** A window explains what yt-dlp and
  ffmpeg do and downloads both on **Yes, download them (recommended)**; **Not now** leaves them
  for the video window. Upgrading? It asks once too, if one of them is missing.

### Changed

- **MDM's own downloads use its own downloader**: yt-dlp, ffmpeg, MDM's updates and a crash
  report's symbols now come down with several connections, resume after a cut connection and go
  through your proxy settings. Each file is still checked against its publisher's signed
  checksums before use.

### Fixed

- **The toolbar's buttons work again**: Add URL, Add Torrent, Delete Completed, Options,
  Scheduler, Start Queue and the others did nothing when clicked in 0.2.3; their drop-down arrows
  did.
- **Dark mode has day mode's sizes and places**: the menu bar, the toolbar, the panes, the list's
  lines and selection, buttons and the progress window's table measure the same in both modes.
  Only the colours change.
- **Texts that were always in English** are translated in every language: the empty list's
  message, buttons of the crash report window, and why OK or Verify is greyed in some windows.
- **Menus fit their texts**: they were wider than needed, so short commands looked lost in them.
- **The language list scrolls with the mouse wheel.**
- **A cut file name's full text shows at the pointer**, not under the list.
- **Toolbar buttons** have as much room under their label as above their icon when the pointer is
  over them.

## [0.2.3] - 2026-11-01

A fix for 0.2.2, which froze on every system as soon as Add URL opened, and crash reports, so the
next problem like it is caught and fixed sooner.

### Added

- **Crash reports.** If MDM closes unexpectedly or stops responding, a report window opens. It
  shows what happened and where in MDM's code, lets you read the whole report, and **Send on
  GitHub** opens a crash report form in your browser, filled in. Nothing is sent by itself. When
  the same problem was already reported, it takes you to that issue instead. A frozen MDM can be
  ended and restarted from there. Reports leave out your download addresses and file names. See
  [Crash reports](docs/crash-reports.md).
- **A log**, kept in MDM's data folder (`logs`), so a report can show what led to a problem.
- **MDM checks for updates every time it starts** (it was once a week), and once a day while it
  stays open; a new version's window opens in front. **Check for updates at every start** in
  that window turns it off.

### Changed

- **One look for every kind of control**, in both themes: text boxes, read-only fields, check
  boxes, buttons, drop-downs, tabs, lists and their headers, popups and progress bars are each
  drawn one way everywhere (dark mode drew some windows' controls in an older style, with bright
  borders). A number and its arrows are one box now, as Windows 11's are; text is centred in
  controls of any height; one blue for selection and focus.
- **A tour of the features** on the website instead of a wall of screenshots.

### Fixed

- **The window froze for seconds while a big download was writing**: MDM saved its list on the
  window's thread, and the save waited for the disk. It saves on a thread of its own now (the
  new crash reports caught it).
- **Add URL froze MDM** (Tasks → Add new download, the toolbar's Add URL, Ctrl+N), and so did
  Download File Info: their address and Save As boxes took the keyboard focus and MDM waited on
  itself. Everything clicked after that seemed to do nothing. A test now opens Add URL the way
  you do, and a stricter check looks for this kind of hang in the whole app.

## [0.2.2] - 2026-11-01

A sweep through every window for the small things that make a program feel finished: how each
control looks and answers the mouse and the keyboard, dark mode everywhere, screen readers,
and a long list of fixes found by reviewing all of it (issue #7 and more).

### Added

- **A status strip** under the list: how many downloads it shows and how many are selected,
  how many are running and how fast in all. The tray icon's tip says the same.
- **Drag and drop on the main window**: drop a link to add it, several links or a list
  (.txt, .ef2) to choose them in Import, a .torrent file to add it.
- **MDM comes back as you left it**: the window's place and size, the Categories pane, its
  open branches and the node shown.
- **Download counts** in the Categories tree, **an empty list that says why** (with Clear
  filters or Add new download), **a right-click menu on the list's blank space**, a
  double-click on a column border to fit it, and **Reset columns**.
- **Screen readers** (Narrator, NVDA): every window, dialogs and menus included, now tells them
  what each control is, its name and its state; the download list reads each download.
- **Keyboard everywhere**: arrow keys in radio groups and tab strips, Ctrl+Tab between tabs,
  type a name to jump to it in any list or drop-down, Up/Down through Add URL's and Save As'
  history, Home/End in menus, Tab between the Categories tree and the list (the tree answers
  the arrows), F2 to rename, held arrows repeat, the date picker's calendar works from the
  keyboard and has Today.

### Changed

- **Windows 11's look in light mode**: rounded text boxes and drop-downs (no more black corner
  dots), pressed states on every button (issue #7), focus rings only when you use the
  keyboard, tooltips with Windows 11's corners and padding, and dark mode in every window.
- **Every list in the dialogs** scrolls, highlights and takes the keys the same way, and the
  Scheduler's list of a queue's files now scrolls at all.
- **Numbers** are typed in one kind of box everywhere: arrows step them, a value out of range is
  outlined in red and set right when you leave the box (some were changed silently before).
- **Things that fail say why**: opening a file that was moved, a file that can't be deleted or
  moved, the list that can't be saved, a computer that won't shut down, a check closed early.
- **System file dialogs no longer freeze MDM's other windows** while they are open.
- **The mouse wheel scrolls as many lines a notch as Windows' mouse settings say**, in every
  list, tree and text box; Page Up / Page Down in the download list go to the edge of what's
  shown first, as in Explorer; text boxes of several lines have the lists' scroll bar.
- The toolbar's right-click menu is gone: it offered button sizes and styles that don't exist.

### Fixed

- **Options' OK undid changes made meanwhile** (the toolbar's Speed Limiter, Torrent
  settings…), and the Scheduler's Close undid downloads added to or removed from a queue.
- **Shift+Delete's answer was remembered**: after one Shift+Delete, a plain Delete with "Don't
  show again" erased files from disk. Shift+Delete now works on macOS and Linux too.
- **Sleep when done hibernated** the computer on Windows.
- **Enter did two things**: with Cancel or a check box focused, Enter also pressed OK.
- **Download File Info** ignored an edited address and accepted an empty Save As.
- **Find** could select a download hidden by the column filters; the selection vanished when
  the list lost focus; Delete now selects the next download.
- **The keyboard focus got lost** when a button greyed itself or Escape closed a drop-down:
  Tab started over at the top of the window. It now moves on, or comes back, as in Windows.
- **Help → Report a problem** filled the form's System and Installed with only for a moment;
  they stay filled now.
- A torrent's progress window: its tabs touched the buttons above them, and Resume
  capability's Yes / No stood apart from the other values. A long list of file types in
  Options ran out under its box.
- Deleting a queue asks first; a running queue stops when deleted; the toolbar's Start / Stop
  Queue arrows always work.
- Many more: the checksum dialog, the progress window's completion options and speed limit,
  the torrent and video windows (Stop a look-up, stale video choices), Batch's letter ranges,
  addresses without http://, duplicate links arriving together, imports that doubled
  downloads, menu titles that shared access letters in French and other languages, and more.

## [0.2.1] - 2026-10-31

### Added

- **Update now** in Help → Check for updates: MDM downloads the new version, checks it, puts it
  in place and restarts, on Windows (setup, portable and plain zip), Mac and Linux (AppImage,
  .tar.gz; a .deb or .rpm opens in the system's installer). It installs only files listed in a
  checksum list signed by MDM's release key, which every release now carries
  (`SHA256SUMS.txt.sig`). The first update this way will be from 0.2.1 to the next version.

### Fixed

- **Ghost windows while a menu is open** (issue #4): faded copies of the main window titled
  "egui window" opened and closed with every redraw of an open menu, and the toolbar's hover
  flickered under them. On a Mac in full screen they opened a space of their own and froze MDM
  (issue #5). They came with 0.2.0's menu keyboard support, which asked about a submenu's window
  that wasn't open, and that made one.
- **Check for updates**: the release notes scroll (the wheel, a trackpad, their scroll bar, and
  the arrow, Page and Home/End keys); their lines no longer break mid-sentence, and links show
  as their text (issue #5).
- **Controls that didn't react to the mouse**: the ▾ of Address and Save As (Add URL, Download
  File Info), the drop-down lists, check boxes, radio buttons, the Categories pane's ×, the
  Scheduler's date button and calendar, scroll bars and the list's column headers now show
  when the pointer is over them and when they're pressed, as Windows' own controls do; an
  arrow stays pressed while its list is open.

### Changed

- **Help → Report a problem** opens GitHub's new bug report form with your MDM version, your
  system and its version, how MDM was installed and its language filled in. Every report now
  says which system and version it comes from.

## [0.2.0] - 2026-10-31

Mac fixes from the first round of real use, found on every system and fixed there too; the
menus and text boxes work as in any desktop program; and MDM works with AI assistants.

### Fixed

- **Menus on macOS and Linux**: clicking a menu item did nothing (the click moved the focus to
  the menu's own window, which closed it first). The list's right-click menu and the toolbar's
  drop-downs had the same fault.
- **Paste** from other programs into any text box (Ctrl+V, ⌘V): MDM kept a clipboard of its own.
- **The app icon on macOS**: the Dock showed a small, blurry icon while MDM ran; it keeps the
  app's own now. Elsewhere windows take their icon from a large size (sharp at any scaling).
- **The download list's file icons** were drawn over the Categories pane when the list was
  scrolled sideways (the Categories tree and the Scheduler had the same fault).
- **The download progress window** could be stretched on macOS and Linux without its contents
  following; it keeps its size, as on Windows.
- **Questions** ("Create this folder?", delete, a duplicate download, the turn-off countdown)
  could open behind the window that asked them; they stay in front now.
- **Video downloads' progress** went back and forth (a stream in fragments re-estimates its size,
  and picture and sound arrive one after the other); it only moves forward now, from the size
  the video window showed.

### Added

- The menus show each command's keys (**Ctrl+N**, on a Mac **⌘N**) and work from the keyboard:
  **F10** or **Alt** (on a Mac **Ctrl+F2**), the underlined letters, the arrows, **Enter**, **Esc**.
- Right-click any text box for **Cut**, **Copy**, **Paste**, **Delete** and **Select all**.
- **Address** (Add URL) and **Save As** (Download File Info) list the last 20 addresses and folders
  from their arrows; **...** beside Save As chooses a folder, **+** beside Category makes one.
- Help → Keyboard shortcuts shows the Mac's keys on a Mac.
- **AI assistants**: `misterdownloadmanager-mcp`, an MCP server that comes with MDM, lets Claude,
  Cursor, VS Code and other MCP assistants list, add, pause, resume and remove the running
  app's downloads and start its queue, through the automation API
  ([how to set it up](docs/ai-assistants.md)).
- **`--json`** for `misterdownloadmanager-cli`: every command prints JSON for scripts and AI
  agents, downloads as one `progress` line per half second and then `done` or `paused`
  ([the formats](docs/command-line.md#json-for-scripts-and-programs)).
- **The automation API's JSON Schema**, [automation-api.schema.json](docs/automation-api.schema.json),
  made from the code the app answers with. Each download in its list now says why it failed
  (`error`), and the "it is off" answer carries `"code": "api-off"` for programs to check.
- The website has `llms.txt` and `llms-full.txt`, and every help page as Markdown, for AI tools.

### Changed

- `misterdownloadmanager-cli status`, `pause`, `start`, `remove`, `delete` and `api` exit with
  code 1 when the app answers with an error (the API is off, an unknown action), as they do when
  the app isn't running.

## [0.1.0] - 2026-10-30

The first release: a free download manager for Windows, laid out the way Internet Download
Manager users expect, with experimental builds for macOS and Linux.

- **The download engine**: up to 32 connections per file, re-splitting the slowest part until
  the last byte, and crash-safe resume after a restart or a dropped connection. HTTP, HTTPS,
  **FTP and FTPS** (`ftp://`, `ftpes://` explicit TLS, `ftps://` implicit TLS), proxies, SOCKS and
  **proxy auto-config (PAC)** scripts.
- **The main window** you know: categories, the download list, the progress window with every
  connection's share, queues and the **Scheduler**, the speed limiter, Sites Logins, and
  **Options** where you expect them, every control working. Day and dark mode.
- **Browser integration** for Chrome, Edge, Brave, Opera and Vivaldi (Firefox in testing):
  downloads hand over to MDM with their cookies and referrer, right-click **Download with Mister
  Download Manager**, and a **Download video** button over videos on any page, which can be
  dragged along the video's edge, hidden on a video or a site, or turned off in Options.
- **Copy a link, get the download window**: copying a link to a file type on the **File types**
  list opens **Download File Info** with it, ready to start, also without the browser extension
  (**Options → General → Offer to download links copied to the clipboard**, on by default).
- **Videos from web pages** through yt-dlp (installed and kept current from MDM, with ffmpeg for
  the qualities that need merging), and **BitTorrent and magnet links** through librqbit: choose
  the files, see peers, trackers and pieces, keep sharing if you like.
- **86 interface languages**, every language Windows 11 offers, chosen by flag on the first start
  or in **View → Language...**; right to left for Arabic, Hebrew, Persian, Urdu and Uyghur; fonts
  for every script from the system itself. The installer speaks the same languages, and the
  browser extension speaks your browser's (50 languages).
- **Options → Regional**: dates and times as your region writes them, or your own way (day, month
  or year first, 12- or 24-hour clock, the week's first day); the list and the Scheduler follow.
- **[Command line and automation](docs/command-line.md)**: IDM's switches (`/d /p /f /a /n /s`),
  `misterdownloadmanager-cli` (files, videos, torrents) and a local, token-protected automation
  API that lists, pauses, starts, removes and deletes downloads.
- **Downloads**: the Windows installer, a portable zip and a plain zip, the browser extension on
  its own (a zip for Chromium browsers, an `.xpi` for Firefox, always the app's version), and
  **macOS and Linux to try**: a universal `.dmg` and an AppImage, `.deb`, `.rpm` and `.tar.gz`,
  marked `-experimental` because they are untested by us. There, MDM connects itself to Chrome,
  Edge, Brave, Vivaldi and Firefox, has its tray icon, follows the system's light or dark look and
  uses the system's fonts and ffmpeg. Tell us how they work for you.
- **Help** opens the [user guide and tutorials](docs/README.md).
