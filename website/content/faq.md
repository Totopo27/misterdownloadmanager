# Questions and answers

Each question becomes its own page (faq/<id>.html) and an entry of faq/index.html; groups are the
level-two headings. Answers link to the full help page. Built by tools/website/build.py.

## About Mister Download Manager

### Is Mister Download Manager really free? {#is-it-free}

Yes, completely. Mister Download Manager (MDM) is free and open-source software under the GNU General Public License, version 3. There is no trial period, no paid edition, no registration key, no nag screen and no ads, and there never will be. You may use it at home or at work, copy it and give it to others.

### Is it a copy or a crack of Internet Download Manager? {#is-it-idm}

No. MDM is an independent program written from scratch in Rust, with no connection to Tonec Inc., the makers of Internet Download Manager (IDM). It contains none of IDM's code or artwork. Its windows are laid out the way IDM users expect, so you can switch without learning anything new — and because MDM is free, there is nothing to crack. Please don't download "cracked" download managers: they are a common way to spread malware.

### How is MDM different from IDM? {#mdm-vs-idm}

The layout is familiar on purpose: the toolbar, the categories, the download list, the progress window with every connection, queues and the scheduler. The differences: MDM is free and open source, downloads BitTorrent and magnet links in the same list, finished faster in [our benchmark](/benchmark.html), speaks 86 languages and has builds for Linux and macOS. IDM is the more mature product, refined over many years. See the full [comparison](/idm-alternative.html).

### Why does MDM show a larger memory figure than IDM? {#memory-vs-idm}

Windows reports two different memory numbers. Memory in use (Task Manager's "Memory" column, the private working set) is what a program is holding in RAM right now. Committed memory (Process Explorer's "Private Bytes", Resource Monitor's "Commit") is memory Windows has set aside for the program; most of it can be paged out, so it doesn't mean the program is using that much RAM.

On one test machine (Windows 11, MDM 0.5.2, IDM 6.42, release builds), memory in use in the tray was 6–9 MB for MDM and about 6 MB for IDM. With its window open, MDM uses about 70 MB right after it starts, and about 10 MB once the window has been hidden in the tray, which gives memory back to Windows. Committed memory was 130–160 MB for MDM and about 10 MB for IDM. While MDM downloads fast, its memory in use can rise to a few hundred MB for a moment (it holds data in memory while the disk catches up); it drops back within seconds of the download ending, and in the tray it hands the rest back to Windows within about ten minutes.

### Is it safe? Does it contain adware or spyware? {#is-it-safe}

MDM contains no adware, spyware, bundled offers or tracking. Its whole source code is public, so anyone can check what it does, and every release is built from that code by GitHub's servers, where the build can be inspected. The helper programs it can fetch for videos are verified before they run: yt-dlp against its developers' signing key, ffmpeg against its published checksums. Download MDM only from [this site](/download.html) or the project's GitHub releases.

### Does MDM collect my data? {#privacy}

No. MDM has no accounts, no analytics and no telemetry. It connects to the servers you download from and, each time it starts and once a day while it runs, asks GitHub whether a newer version exists (you can turn that off in Help → Check for updates). The browser extension talks only to MDM on your own computer. The details are in the [privacy policy](/privacy.html).

### Who makes Mister Download Manager? {#who-makes-it}

MDM is made by Ahmed Nefzaoui, an independent developer, and is open to contributions from anyone on [GitHub]({{REPO}}). Bug reports, translations and code are all welcome.

## Installing

### Which systems does it run on? {#systems}

Windows 10 and 11, 64-bit, is the main platform. Builds for Linux (AppImage, .deb, .rpm and .tar.gz) and macOS (.dmg) come with every release too; today's are marked experimental. From version 0.5.0, Linux isn't experimental any more: it is checked on GNOME, KDE Plasma, Xfce and Cinnamon, in Wayland sessions too, and its packages in clean Ubuntu, Debian and Fedora systems. Still to come there: the Flatpak Firefox isn't checked yet, and screen readers (Orca) don't read MDM's windows yet. macOS stays experimental. See [Windows, Mac and Linux](/features/systems.html) and [Download](/download.html).

### Does it run on Windows 7 or 8? {#windows-7}

No. MDM needs Windows 10 or 11, 64-bit. The Rust toolchain it is built with no longer supports Windows 7 and 8, and neither does Microsoft.

### Can I use it without installing it? {#portable}

Yes. The portable download is a zip: unzip it anywhere, a USB stick included, and run it. Your downloads list and settings stay in its own `data` folder, so nothing is written to the computer. See [Portable mode](/help/portable.html).

### Can I bring my download list over from IDM? {#import-from-idm}

Yes. In IDM, export your list with **Tasks → Export → To IDM export file**. In MDM, choose **Tasks → Import → From IDM export file** and pick that `.ef2` file: every download comes over with its address, referrer and cookies. A plain text file of links works too. See [Adding downloads](/help/adding-downloads.html).

### How do I uninstall it? {#uninstall}

Open Windows **Settings → Apps**, find Mister Download Manager and choose Uninstall. The uninstaller asks whether to delete your download list and settings as well. Files you downloaded are never touched. For the portable copy, delete its folder. On Linux, remove the package with your package manager, or delete the AppImage; [Uninstall on Linux](/help/getting-started.html#uninstall-linux) says what stays in your home folder and how to remove it.

## Browsers

### Which browsers does it work with? {#browsers}

Firefox (from [addons.mozilla.org](https://addons.mozilla.org/firefox/addon/mister-download-manager/)), Google Chrome, Microsoft Edge, Brave, Opera and Vivaldi (from [the Chrome Web Store](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp)), through the MDM browser extension, and browsers built on them: Zen and LibreWolf, Helium. The extension hands downloads to MDM with their cookies and referrer, so files behind a sign-in still work, and it adds **Download with Mister Download Manager** to the right-click menu and a **Download video** button over videos. See [Browser integration](/help/browser-integration.html).

### How do I install the browser extension? {#install-extension}

Install MDM with the setup and keep **Set up browser integration** ticked. In Firefox, then open [the add-on's page on addons.mozilla.org](https://addons.mozilla.org/firefox/addon/mister-download-manager/) and click **Add to Firefox**. In Chrome, Edge, Brave, Opera and Vivaldi, open [its page in the Chrome Web Store](https://chromewebstore.google.com/detail/mister-download-manager/ejoleppdfgjdndddcjoehopknmlglojp) and click **Add to Chrome** (Edge shows **Get**, and first asks to allow extensions from other stores). Can't use a store? Open the browser's extensions page, turn on Developer mode, choose **Load unpacked** and pick the `extension\chromium` folder in MDM's folder. The [tutorial](/help/guide-catch-browser-downloads.html) walks through it.

### Why doesn't MDM take over my download? {#not-catching}

Check four things: the extension is installed and turned on; **Options → General → Use advanced browser integration** is on and your browser is ticked; the file's type is in **Options → File types**; and the site isn't in the list of sites MDM leaves alone. Right-click a link and choose **Download with Mister Download Manager** to send any file whatever its type. More in [Troubleshooting](/help/troubleshooting.html).

### Can I download without the browser extension? {#without-extension}

Yes. Copy a link to a file and MDM offers to download it at once (**Options → General → Offer to download links copied to the clipboard**, on by default). You can also paste links into **Add URL**, drag them onto the drop target, add a whole batch, or use the command line. See [Adding downloads](/help/adding-downloads.html).

## Downloading

### Why is MDM faster than a browser? {#why-faster}

A browser fetches a file over one connection. MDM asks the server for several parts of the file at once, up to 32 connections (128 if you allow it), and keeps re-splitting: when a connection finishes its part, it takes over half of the biggest part still left, so no slow connection holds up the end. It also remembers servers that refuse extra connections and stops asking them. See [Connections and speed](/help/connections-and-speed.html).

### How many connections does it use? {#connections}

Eight per download by default, and up to 32 if you choose in **Options → Connection**, or 128 after ticking **Allow more than 32 connections** there. MDM uses fewer when a server allows fewer, and only one when a server can't send parts of a file at all.

### Can it resume broken downloads? {#resume}

Yes. MDM writes its progress as it goes, so after a lost connection, a crash or a restart it carries on where it stopped. Before resuming it checks that the file on the server hasn't changed. When a download address has expired, **Refresh download address** lets you give it a fresh one and keep the data. See [Resume a broken download](/help/guide-resume-broken-download.html).

### Can I limit the download speed? {#speed-limit}

Yes: for one download from its progress window, or for everything with **Downloads → Speed Limiter**. See [Keep some speed for everything else](/help/guide-keep-speed-for-everything-else.html).

### Can it download overnight or on a schedule? {#schedule}

Yes. Put downloads in a queue and let the Scheduler start it at a set time, stop it at another, and even shut down or put the computer to sleep when the queue is done. See [Download overnight](/help/guide-download-overnight.html).

### Does it support FTP? {#ftp}

Yes: `ftp://`, and FTP over TLS both ways (`ftpes://` and `ftps://`), with several connections at once, resume, and logins from the address or from Sites Logins. See [Adding downloads](/help/adding-downloads.html#ftp-and-ftps).

### Does it support proxies? {#proxy}

Yes: HTTP and SOCKS proxies, and proxy auto-config (PAC) scripts, in **Options → Proxy / Socks**. See [Options](/help/options.html). To keep every download on a VPN, see [Network adapter](/features/network-adapter.html).

## Video and torrents

### How do I download a video from YouTube or another site? {#download-video}

Point at the video in your browser and click the **Download video** button that appears over it, or paste the page's address into **Tasks → Download video from a page**. MDM lists every quality the site offers, with sizes; pick one and it downloads like any file. It uses yt-dlp, which supports thousands of sites. Download only videos you have the right to. See [Download a video from a page](/help/guide-video-from-page.html).

### Can it download a video's subtitles? {#subtitles}

Yes. When a video has subtitles, the **Subtitles** list in the video window offers them: the uploader's languages and the site's automatic captions. They're saved beside the video as an `.srt` file, or put inside it with **Put them into the video**. See [Subtitles](/help/video.html#subtitles).

### Can it download from Netflix or other paid streaming services? {#drm}

No. MDM doesn't break copy protection (DRM), by design. Videos that are protected stay protected. See also [what MDM is made for](/intended-use.html).

### Does it download torrents? {#torrents}

Yes, built in: open a `.torrent` file or paste a magnet link, tick the files you want in the folder tree, and the torrent downloads in the same list as everything else, with its peers, trackers and pieces one tab away. See [Torrents](/help/torrents.html).

### Does it keep uploading after a torrent finishes? {#seeding}

No, unless you want it to. While a torrent downloads it trades pieces with other people, as every torrent client does. When it finishes, it stops, like any other download. Turn on **Keep sharing torrents after they finish** in Options if you'd like to give back.

## Help and more

### Is MDM available in my language? {#languages}

Very likely: MDM speaks 86 languages, every display language Windows 11 offers, chosen on the first start or any time in **View → Language**. Arabic, Hebrew, Persian, Urdu and Uyghur are laid out right to left. See [all the languages](/languages.html) and how to help improve yours.

### Can other programs or scripts control MDM? {#automation}

Yes. MDM accepts IDM's familiar command-line switches, comes with `misterdownloadmanager-cli` for downloads from a terminal, and has a local, token-protected automation API that can list, pause, start and remove downloads. The API is off until you turn it on. See [Command line and automation](/help/command-line.html).

### Can my AI assistant use MDM? {#ai}

Yes. MDM comes with an MCP server, `misterdownloadmanager-mcp`: add it to Claude Desktop, Claude Code, Cursor, VS Code or another assistant that supports MCP, and you can ask it in your own words to list, add, pause, resume or remove downloads. It works with the MDM on your computer only, and needs the automation API on. See [AI assistants](/help/ai-assistants.html).

### How do I report a bug or ask for a feature? {#report-bug}

Use **Help → Report a problem** in MDM: it opens the bug report form on GitHub with your version, system and language filled in. Or [open the form]({{REPO}}/issues/new?template=bug.yml) yourself. Security problems go privately through the [security page](/security.html).
