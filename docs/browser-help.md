# The browser and MDM aren't connected

[← Back to docs index](README.md)

When the browser can't hand a download to MDM, it keeps the download itself, so nothing is lost. The extension's toolbar button then shows a red **!**. Click it to see what's wrong, then **How to fix it** brings you to the matching section below.

To check MDM's side, open **Options → General** in MDM and click **Test browser integration...**. It checks that browser integration is on, that this is the installed copy, that the browsers can find MDM, and which browsers' extension has connected; **How to fix it** opens the section for the first that fails.

## MDM isn't installed, or browser integration isn't set up {#not-installed}

The browser can't find the small helper program it talks to. It is registered when MDM is installed.

1. Install MDM with the setup from [the download page](https://misterdownloadmanager.com/download.html), and keep **Set up browser integration** ticked.
2. Already installed? Open MDM, then **Options → General → Test browser integration**, and click **Set it up** if it offers to.
3. The portable copy can't be connected: it writes nothing outside its own folder. Use the installed copy for browser integration. See [Portable mode](portable.md).
4. On Linux and macOS, MDM sets browser integration up itself each time it starts: start MDM once. **Test browser integration** says what went wrong if it couldn't, such as **The helper program the browsers start is missing**: **Set it up** puts it back.
5. Restart the browser after setting it up.
6. **Zen, Helium, Brave Origin or another browser built on Firefox or Chromium, as your only browser on Linux or macOS?** Older versions of MDM set browser integration up only for the browsers they knew; the current one also knows Brave Origin and the Beta, Dev and Nightly versions of Brave, Chrome and Edge. Update MDM and start it once.
7. **The AppImage on Linux:** the AppImage's files are gone once it quits, so MDM keeps a copy of its helper program in its data folder (`~/.local/share/misterdownloadmanager/browser-host`) and points the browsers there. Start MDM once after moving or renaming the AppImage, or after an AppImage manager (Gear Lever, AppImageLauncher and the like) moved it, so the helper knows where to start it from.
8. **Zen on Windows:** Zen 1.21.15b and later have a bug of their own that keeps add-ons from reaching their helper program ([zen-browser/desktop#15432](https://github.com/zen-browser/desktop/issues/15432)), with any add-on that uses one. Until Zen fixes it, Zen 1.21.14b works.

## MDM didn't start in time {#not-running}

When MDM isn't running, the browser starts it for each download and waits for it. It waited about 20 seconds this time and gave up.

- Start MDM yourself and try the download again.
- A first start, a slow disk, or an antivirus scanning MDM can take that long. Next time it is usually quick.
- With a VPN or proxy program, allow connections to `127.0.0.1` (your own computer): the browser's helper reaches MDM through one.
- If MDM never opens, see [The setup or MDM won't start](troubleshooting.md).

## The connection to MDM failed {#host-failed}

The browser started MDM's helper, but it stopped before answering. Security software is the usual cause.

- Allow `misterdownloadmanager-nmhost.exe` (in MDM's folder) in your antivirus or anti-malware program.
- Reinstall MDM to restore its files.
- On Linux, with Firefox or Chromium installed as a Snap, see [Flatpak and Snap browsers](#linux-sandbox).
- Uninstalled MDM on Linux? The .deb and .rpm take their browser files with them; the AppImage's and the tar.gz's stay in your home folder: [remove them](getting-started.md#uninstall-linux), and the extension too.

## MDM doesn't accept this copy of the extension {#forbidden}

MDM's helper serves only its own extension. The browser says this copy isn't one it knows.

- **MDM 0.2.x with the extension from the Chrome Web Store?** The store's copy needs a newer MDM:
  0.2.x knows only the copy in its own folder. Update MDM with **Help → Check for updates...**.
  Until then, remove the store's copy
  and load the one in MDM's folder instead (`extension\chromium`, see
  [Browser integration](browser-integration.md#chrome-edge-brave-opera-and-vivaldi)).
- **Two copies installed?** Open the browser's extensions page (`chrome://extensions`, `edge://extensions`, `brave://extensions`) and keep only one **Mister Download Manager**.
- **A copy from elsewhere?** Remove it and add the extension as MDM's **Add Mister Download Manager to your browser** window shows (see [Browser integration](browser-integration.md)).
- **An older MDM?** Update MDM: a newer extension may have an ID an old helper doesn't know.

## The extension is newer than MDM {#update-app}

Downloads still work, but some of the extension's features need a newer MDM. Update it with **Help → Check for updates...** in MDM.

If MDM says it's up to date, there's nothing for you to do. The extension updates by itself through its store and can get ahead of the app for a while. Everything keeps working, and the message goes away with the next version of MDM.

## A download's details were too large {#too-large}

A download carries its site's cookies (and a form's data) to MDM. A few sites set so many that together they exceed what MDM accepts from the browser (1 MB). The browser kept this one. Signing out and back in to the site usually trims them.

## Downloads stay with the browser although MDM is connected {#options}

The extension is connected, but MDM's own settings leave this download to the browser. Check, in MDM:

1. **Options → General → Use advanced browser integration** is on, and your browser is ticked under **Capture downloads from the following browsers**.
2. The file's type is listed in **Options → File types**.
3. The site isn't in **Don't start downloading automatically from the following sites**, nor the address in **Don't start downloading automatically from the following addresses**.
4. The file isn't smaller than **Don't take files smaller than** in **Options → File types**.

Also check the extension's toolbar button: with **Take downloads from this browser** off, everything stays with the browser.

## The browser opens PDFs and music instead of sending them {#opened}

MDM also takes the file types the browser would show or play itself (a PDF, an MP3), when they're listed in **Options → File types** and **Also take files the browser would open** is ticked in **Options → General → Keys...**. This needs a Chromium browser (Chrome, Edge, Brave, Opera, Vivaldi) version 128 or later, or Firefox. Sites in MDM's excluded sites keep opening them in the browser. Hold **Alt** (**Option** on a Mac) while clicking a link to leave it to the browser: Firefox then follows it as a plain click does, while Chromium browsers and Safari download it themselves (it is their own "download link").

## No Download video button on YouTube in Chrome, Edge or Brave {#store-youtube}

The copy of the extension from the Chrome Web Store showed no **Download video** button on YouTube before version 0.5.1. From 0.5.1 it shows it. Your browser updates extensions by itself; to get it sooner, open its extensions page and update them there.

You can always save a video without the button: in MDM, open **Tasks → Download video from a page** and paste the video's address.

## The extension can't read this site {#site-access}

In Chrome, Edge and Brave you can limit an extension to some sites (right-click its toolbar button → **This can read and change site data**). On a site it may not read, the extension can't pass your sign-in on, so it leaves the download to the browser. Choose **On all sites** to let MDM take downloads everywhere.

## Firefox forgets the extension after a restart {#firefox-temporary}

An add-on loaded with **Load Temporary Add-on** (from `about:debugging`) is removed when Firefox closes. Remove it and install the add-on from [its page on addons.mozilla.org](https://addons.mozilla.org/firefox/addon/mister-download-manager/) instead: Firefox keeps that one, and updates it.

## Linux: Flatpak and Snap browsers {#linux-sandbox}

Browsers installed as a Flatpak or a Snap run in a sandbox, which decides whether they can reach MDM.

- **Firefox as a Snap** (Ubuntu's own Firefox) reaches MDM through the desktop, which reads the browser files in your home folder and those the .deb or .rpm installs for everyone. The first time the extension calls MDM, the desktop asks **Allow Firefox to start WebExtension backend?** for Mister Download Manager: choose **Allow**. After **Don't allow**, the extension says the connection failed and the desktop doesn't ask again; to be asked again, run this in a terminal, then use the extension again:

  `gdbus call --session --dest org.freedesktop.impl.portal.PermissionStore --object-path /org/freedesktop/impl/portal/PermissionStore --method org.freedesktop.impl.portal.PermissionStore.Delete webextensions com.misterdownloadmanager.host`
- **Chromium as a Snap** (Ubuntu's Chromium) starts MDM's helper inside its sandbox, where it can't reach MDM, so MDM doesn't list it in **Add Mister Download Manager to your browser**. Use Firefox, or Google Chrome from its own `.deb`.
- **Flatpak browsers** (such as Zen's `app.zen_browser.zen`) can't start MDM's helper, so MDM doesn't list them either. Install the browser from your distribution's packages (or the browser maker's own `.deb`, `.rpm`, tarball or AppImage) to use browser integration.

## Next steps

- [Browser integration →](browser-integration.md)
- [Troubleshooting →](troubleshooting.md)
- [Back to the docs index →](README.md)
