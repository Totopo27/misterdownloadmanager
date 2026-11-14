---
title: Privacy policy
description: What Mister Download Manager, its browser extension and this website do with your data: the app and extension collect nothing; the website counts visits with Statcounter.
lede: Short version: the Mister Download Manager app and its browser extension don't collect anything about you. No accounts, no analytics, no telemetry, no ads. This website counts its visits with Statcounter.
fine: Last updated {{TODAY}} · applies to the app, the browser extension and misterdownloadmanager.com
priority: 0.4
---

<div class="summary-box" markdown="1">

## In short

- The app and the extension **send nothing about you to us**. We don't run servers that could receive it.
- This **website** counts visits with **Statcounter**, which sets a cookie and sees your IP address; it runs only on the website, never in the app.
- The app talks to **the servers you download from**, to **GitHub** to check for updates when it starts (you can turn that off), and to GitHub again for **yt-dlp and ffmpeg** if you download videos.
- **Torrents** are peer to peer: other people in a torrent's swarm see your IP address, as with every torrent client.
- The **browser extension** talks only to Mister Download Manager on your own computer.
- Your download list and settings stay **on your computer**.

</div>

Mister Download Manager ("MDM") is free, open-source software made by Ahmed Nefzaoui and contributors. Because its whole source code is public at [github.com/anefzaoui/misterdownloadmanager]({{REPO}}), you don't have to take this page's word for it: every network connection described here can be checked in the code.

## The app

### What stays on your computer

Your download list, settings, logins you save in **Sites Logins**, the history of each download and the files you download are stored only on your computer: in `%APPDATA%\misterdownloadmanager` on Windows (or the `data` folder of the portable copy), and in the usual application-data folder on macOS and Linux. Nothing is uploaded anywhere. Uninstalling offers to delete them. From version 0.5.0, the passwords of Sites Logins, the proxy and debrid accounts go into the system's keychain instead (Windows Credential Manager, the keyring on Linux, the macOS keychain), still on your computer, when one is there and unlocked.

### Who the app talks to, and why

- **The servers you download from.** To download a file, MDM connects to the server that holds it and sends what any browser would: the request, and the cookies, referrer and browser identification the extension passed along, so downloads that need you to be signed in keep working. Those servers see your IP address, as they would if your browser downloaded the file.
- **Your proxy**, if you set one in **Options → Proxy / Socks**, and your network's proxy auto-config script, if you point MDM at one.
- **GitHub, to check for updates.** Each time it starts, and once a day while it stays open, MDM asks GitHub's public API for the latest release of MDM (`api.github.com/repos/anefzaoui/misterdownloadmanager/releases/latest`). The request carries MDM's version in its User-Agent and nothing else. Turn the check off in **Help → Check for updates** (**Check for updates at every start**). If you choose **Update now**, MDM downloads the new version's file and its signed checksum list from the same GitHub release. GitHub's own privacy statement applies to these requests.
- **GitHub, for video helpers.** If you use **Download video from a page**, MDM downloads yt-dlp (and ffmpeg, if you want merged qualities) from their official GitHub releases, checks them, and once per session asks GitHub whether yt-dlp has a newer release. yt-dlp then talks to the video site you asked for, as your browser would.
- **Torrent peers and trackers.** When you download a torrent, MDM connects to the trackers listed in the torrent, to the BitTorrent DHT network and to other peers, who see your IP address and which pieces you have. To accept connections it may ask your router to open a port (UPnP). This is how BitTorrent works; it ends when the torrent stops.
- **Your AI assistant, only if you connect one.** MDM's MCP server (`misterdownloadmanager-mcp`) talks only to the MDM on your computer. But if you add it to an AI assistant (Claude, Cursor, VS Code…) and turn on the automation API, the assistant can read your download list, and what it reads (file names, addresses, folders) goes to that assistant's AI service, under its privacy terms. Nothing of this happens unless you set it up; see [AI assistants](/help/ai-assistants.html).

From version 0.5.0, only if you choose them:

- **The DNS you choose.** With a DNS other than the system's in **Options → Connection → DNS** (Cloudflare, Google, Quad9, AdGuard, NextDNS or your own), MDM asks it, over HTTPS, TLS or TCP, for the addresses of the servers it downloads from. That service sees those names and your IP address, under its own privacy terms.
- **Your debrid accounts.** A link or torrent you send through a Real-Debrid, AllDebrid, Premiumize or Mega-Debrid account in **Options → Accounts** goes to that service, which then serves the file. Its sign-in stays on your computer, with the passwords above.
- **Codeberg, for gallery-dl.** If you download an image gallery, MDM installs gallery-dl from its official releases on Codeberg and checks its author's signature; gallery-dl then talks to the site you asked for.
- **Link sites, for Click'n'Load.** Off until you turn it on in **Options → Archives and Link grabber**: then MDM listens on port 9666 of this computer only. The first time a site sends links MDM asks whether to allow it; what an allowed site sends opens in Link grabber, where nothing is looked up or downloaded until you add it. Links to this computer or your own network are left out.
- **A torrent IP blocklist**, if you give one: MDM fetches it from the address you entered, again every 7 days.
- **Your own devices, for the remote control.** Off until you turn it on in **Options → Remote**: then MDM answers, over HTTPS on your local network, only the phones and computers you paired with its code.

The server's country that **Download File Info** and the progress window show comes from a table inside MDM: no address is sent anywhere to look it up.

MDM has **no analytics, no automatic crash reporting, no telemetry and no advertising** (a crash report goes only where you send it yourself). It never sends us your download list, the addresses you download, or anything else.

### Reporting a problem

**Help → Report a problem** opens GitHub's bug report form in your browser with MDM's version, your system and its version, how MDM was installed and your language filled in. Nothing is sent until you choose to submit it on GitHub, where it becomes public; read it before you post.

### The automation API

The automation API is off by default. When you turn it on, it listens only on `127.0.0.1` (your own computer) and answers only programs that present the secret token from MDM's data folder.

## The browser extension

The **Mister Download Manager** extension for Chrome, Edge, Brave, Opera, Vivaldi and Firefox:

- When you start a download that MDM should take, it passes **the download's address (and where its redirects led), the page it came from, the site's cookies for that address, your browser's User-Agent, and the file's name, size and type** to MDM on your computer, through the browser's native-messaging channel. Nothing goes over the internet.
- Once you switch on **Send downloads through this browser's proxy or VPN extension** in its popup (Firefox asks you first), when the browser uses a proxy for a download (set by a VPN or proxy extension, or in its settings), the extension passes **the proxy's address**, or its **proxy auto-config script**, so MDM downloads through the same proxy. It never reads or passes the proxy's login. It notes only that a proxy asked for one, so MDM can tell you it can't use that proxy. Switched off (as installed), it reads nothing of the browser's proxy.
- When a download comes from a form you submitted, it passes **the form's data** too, so MDM can submit it again. It notes the data of forms you submit (in memory, used for two minutes at most, never stored) only for that.
- It notes the addresses of video streams (HLS and DASH playlists) a page's player loads, in memory for that tab, so the **Download video** button can pass the stream on. Its button reads the page only to find video and sound players and place itself. Clicking it passes the page's address, the video's address, the page's title, the site's cookies, the page that embeds the player and your browser's User-Agent to MDM.
- **Download selected links** and **Download all links** read the links of the page (or of your selection) when you choose them, and pass them to MDM with the page's address and its site's cookies.
- It notes where page loads' redirects started and ended (in memory, for two minutes), so MDM gets the address you clicked, not only where it led.
- A small script in every page notes only the links you click while holding **Alt**, so those downloads stay with the browser; the addresses are kept in memory for a few seconds.
- Every message to MDM also says which browser it comes from (its name, as the browser tells it) and the extension's version.
- It stores its own settings (paused or not, MDM's file types and excluded sites, its connection status, where you dragged the video button on each of the last 200 sites; nothing from private windows) in the browser's extension storage, on your computer.
- It has **no remote code, no analytics and no ads**, and sends nothing to us or anyone else.

Its permissions, and why: *downloads* to see and hand over downloads; *cookies* so signed-in downloads work in MDM; *nativeMessaging* to talk to MDM; *contextMenus* for the right-click items; *storage* for its settings; *webRequest* to see forms submitted and video streams loaded (it changes nothing); *declarativeNetRequestWithHostAccess* (Chrome, Edge and other Chromium browsers) to have the files the browser would open (a PDF) downloaded instead, for the types MDM takes; in Firefox, *webRequestBlocking* does that; *scripting* to read a page's links when you ask, and to add the video button to pages already open when the extension is installed; *proxy* to read which proxy the browser uses, so downloads taken from a browser behind a VPN or proxy extension go through it too, only while you have that switched on in the popup (it never changes the browser's proxy; in Firefox it is optional, asked for when you switch it on; Chrome and the other Chromium browsers can't make it optional, and count it under the access to all sites); access to all sites because downloads and videos can come from any site.

## This website

misterdownloadmanager.com is a static site. Three companies take part in showing it to you:

- **GitHub Pages** hosts it. GitHub receives your IP address and browser details when you visit and may keep them in its server logs for security; see the [GitHub Privacy Statement](https://docs.github.com/en/site-policy/privacy-policies/github-general-privacy-statement). Download links lead to GitHub Releases.
- **Cloudflare** sits in front of the site: every request passes through its network, which sees your IP address and browser details, protects the site from attacks and may set a cookie needed for that. See [Cloudflare's privacy policy](https://www.cloudflare.com/privacypolicy/).
- **Statcounter** counts visits so we know which pages people read and where they come from. Its script records the page you open, the page that linked to it, your browser, system and screen size, and your approximate location from your IP address, and sets a cookie to tell new visitors from returning ones. We see these as statistics in Statcounter's dashboard; we don't use them to identify you, and we don't sell or share them. See [Statcounter's privacy policy](https://statcounter.com/about/legal/#privacy). Blocking cookies or scripts for statcounter.com in your browser, or a content blocker, keeps you out of the count; the site works the same.

The site has no ads and no other trackers. Its fonts and scripts are served from the site itself. Your choice of light or dark mode is remembered in your browser's local storage and never sent anywhere.

## Children

The app and the extension collect no personal data from anyone, children included. The website's visit counter doesn't know who you are or how old you are.

## Your rights

The app and the extension hold no personal data about you, so there is nothing for us to show, correct or delete there. The website's visit statistics are held by Statcounter; to have records of your visits removed, open an issue with the date and time of your visit, or ask Statcounter directly. Data that you send to GitHub (an issue, for example) is handled by GitHub and can be edited or deleted there.

## Changes

If MDM or this website ever needs to use a new service, this page will say so first, and the change will be listed in [What's new](/news.html). Past versions of this page are in the project's history on GitHub.

## Questions

Open an [issue on GitHub]({{REPO}}/issues). For anything sensitive, use GitHub's private [security advisory form]({{REPO}}/security/advisories/new).
