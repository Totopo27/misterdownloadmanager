// The whole browser integration in real browsers, with nothing on the screen: each browser runs
// headless with its own new profile and loads the extension (target/extension, from pack.ps1);
// the native messaging host (target/release) hands what it sends to a separate Mister Download
// Manager instance (its own data folder, MDM_DATA_DIR; off the screens, MDM_OFFSCREEN). Each case
// checks what MDM received (the lab server's request log, the files in MDM's folders, its list
// through the automation API, its Download video window through UI Automation) and that the
// browser kept nothing.
//
//   cargo build --release -p mdm-app -p mdm-nmhost -p mdm-labserver; tools\extension\pack.ps1
//   node tools/extension/e2e.mjs [chrome] [brave] [edge] [helium] [firefox] [waterfox] [--only=case,case]
//
// Waterfox runs with Firefox's driver (E2E_WATERFOX: its program, when not installed in the usual
// place). E2E_FIREFOX_ASK=1: Firefox's family asks what to do with each new type in every case
// (Waterfox's default), not only in the ask case.
//
// The cases (per browser): folders (another extension's download in a folder of its own keeps
// it, #578), link (a .zip link, throttled and not), ask (Firefox set to ask what to
// do with each new type, as Waterfox is: caught before its dialog, #208), cookie (cookies and referrer
// forwarded), names (Content-Disposition), redirect, leadon (a page going to the file by script, #374), form (POST, 303), alt (Alt+click leaves it
// to the browser), opened (a PDF or MP3 the browser would show or play), spot (the video button
// clear of the page's controls, #421), feeds (a video in a feed sends its post, its own file, or
// nothing and says so, never the feed: sample pages served at LinkedIn's, Facebook's, X's,
// Instagram's, Reddit's, TikTok's, Vimeo's, Dailymotion's addresses, #689), preview (the video button kept over a hover-to-play
// preview the page stopped as the pointer went to it, #503), steady (the video button keeping one place over
// a preview that grows and shows its own buttons while hovered, #560), player (one place over a
// player whose settings button comes and goes as the pointer moves, #899), csp (the video button
// under a strict CSP), hls (the stream a player loaded), streams (HLS and DASH players handed over
// by the button, the right-click menu and the media list, and read by yt-dlp; #155; also with no
// .m3u8 or .mpd, served as text/plain or octet-stream, #171), sniffed (#171 without yt-dlp: such
// streams found only when asked, by the button and the media list, a text decoy never), media (the
// toolbar button's list of the media a tab loaded: streams, files, a protected stream; #38),
// scripted (a video a server's program sends, named after its page with its real type: the list,
// the button, the Download video window; #695, #800), guarded (a stream another site's server
// serves to its page's player only: looked up and downloaded with the Origin and Referer the
// browser sent, #900),
// qualities (the button's menu: a stream's qualities looked up by MDM, with their sizes; the one
// picked downloads with its progress window only, MDM's main window hidden), page (the menu on a
// real video page, E2E_VIDEO_PAGE, and the line E2E_VIDEO_PICK downloaded: off by default), video (shown,
// clicked, dragged, kept; "never on the site" in the last browser; the button's clicks go through
// its menu's "Open in…" to the Download video window, as in csp, hls, streams, sniffed and
// inplace), panelOptions (the button follows Options in an open tab: off, on, the site listed;
// #532), videoFocus (mouse idle/fullscreen and keyboard focus, #480), reconnect (a tab opened while
// the host couldn't start gets the button once it can, no reload; then MDM started by a click, and
// reached after it quits and starts again, #474), start (MDM not running: the host starts it),
// remote (Send downloads to → Another MDM, #157: paired from the popup, a link downloaded by a
// headless MDM; E2E_REMOTE_URL for one running elsewhere, such as the Docker image: remoteServer).
//
// Screenshots for a pull request (Chromium only): E2E_LANG=de (or ar, ...) starts the browser in that
// language, so the extension shows its _locales text of it; with E2E_SHOTS=<folder> the `qualities`
// case saves <lang>-looking.png, <lang>-video-tab.png and <lang>-audio-tab.png there (the player and
// the menu), before the pick, and the `scripted` case <lang>-media-list.png (the popup's list) and
// <lang>-video-window.png. Neither changes anything when unset. E2E_APP_LANG sets MDM's own language
// (the words it sends the menu).
//
// Windows: the host is registered for the current user while it runs (nmhost --register, as the
// installer does) and unregistered at the end. Every registry key that touches (the browsers'
// host entries and Chrome's external extension, per user and for everyone) is exported first
// and checked byte for byte at the end; one that differs is put back from its copy. An installed
// copy's registration for everyone (HKLM) is only read. With E2E_HOST_NAME, the host is registered
// under that test name instead, for the run's browsers only (testHost). Your own browser
// profiles are never used, nor the clipboard. Every process started here is stopped at the end.
//
// Linux (the lab's VM, tools/lab/linux-vm): the host's manifests (files) are written instead, and
// put back byte for byte at the end (E2E_REPLACE_HOST=1 lets it replace a copy already
// registered there); the cases that read the app's windows (UI Automation) are skipped.
//
// The programs: target/release, or CARGO_TARGET_DIR's release folder when it is set.
import { spawn, spawnSync } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
export const release = path.join(process.env.CARGO_TARGET_DIR ? path.resolve(process.env.CARGO_TARGET_DIR) : path.join(root, "target"), "release");
export const exe = (name) => path.join(release, process.platform === "win32" ? `${name}.exe` : name);
// E2E_WORK: the run's own folder instead (its profiles, the instance's data and downloads).
const work = process.env.E2E_WORK ? path.resolve(process.env.E2E_WORK) : path.join(root, "target", "e2e");
const PORT = 8101;
const LAB = `http://127.0.0.1:${PORT}`;
// The proxy case (#42): a proxy on 127.0.0.1 that alone knows the name lab.mdm.test (the lab
// server), standing for a VPN extension's proxy.
const PROXY_PORT = 8102;
const PROXY_HOST = "lab.mdm.test";
const proxyLog = [];
// The same proxy as a PAC script (Chromium's case: inline, or by its address on that proxy).
const PAC = `function FindProxyForURL(url, host) { return host === "${PROXY_HOST}" ? "PROXY 127.0.0.1:${PROXY_PORT}" : "DIRECT"; }`;
// The extension's own address in Firefox (moz-extension://UUID/), fixed in the test profile so
// its pages (the popup) can be opened in a tab.
const FIREFOX_ID = "integration@misterdownloadmanager.com";
// The extensions the browsers load: pack.ps1's, or their copies for E2E_HOST_NAME (testHost).
let chromiumExtension = path.join(root, "target", "extension", "chromium");
let firefoxExtension = { type: "archivePath", path: path.join(root, "target", "extension", "mdm-firefox.xpi") };
const FIREFOX_UUID = "5d0c3b8e-7a51-4c1e-9f0a-38e2c4a1d7b6";

const pf = process.env.ProgramFiles || "C:\\Program Files";
const pf86 = process.env["ProgramFiles(x86)"] || "C:\\Program Files (x86)";
const local = process.env.LOCALAPPDATA || "";
const WIN = process.platform === "win32";
const BROWSERS = WIN
  ? {
      chrome: ["chromium", [`${pf}\\Google\\Chrome\\Application\\chrome.exe`, `${local}\\Google\\Chrome\\Application\\chrome.exe`]],
      brave: ["chromium", [`${pf}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe`, `${local}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe`]],
      edge: ["chromium", [`${pf86}\\Microsoft\\Edge\\Application\\msedge.exe`, `${pf}\\Microsoft\\Edge\\Application\\msedge.exe`]],
      // Helium (#685): installed for the user by its setup, or its zip unpacked anywhere (E2E_HELIUM).
      helium: ["chromium", [process.env.E2E_HELIUM || "", `${local}\\imput\\Helium\\Application\\chrome.exe`].filter(Boolean)],
      firefox: ["firefox", [`${pf}\\Mozilla Firefox\\firefox.exe`, `${pf86}\\Mozilla Firefox\\firefox.exe`]],
      // Firefox's family: the same driver (E2E_WATERFOX: a copy unpacked elsewhere).
      waterfox: ["firefox", [process.env.E2E_WATERFOX || "", `${pf}\\Waterfox\\waterfox.exe`].filter(Boolean)],
    }
  : // Linux (the lab's VM: tools/lab/linux-vm).
    {
      chrome: ["chromium", ["/usr/bin/google-chrome", "/opt/google/chrome/chrome"]],
      brave: ["chromium", ["/usr/bin/brave-browser"]],
      edge: ["chromium", ["/usr/bin/microsoft-edge"]],
      // Helium's .deb (helium-bin), else its tarball or AppImage unpacked anywhere (E2E_HELIUM).
      helium: ["chromium", [process.env.E2E_HELIUM || "", "/usr/bin/helium", "/opt/helium/helium"].filter(Boolean)],
      // A .deb or tarball Firefox; else Ubuntu's Snap.
      firefox: ["firefox", ["/usr/lib/firefox/firefox", "/opt/firefox/firefox", "/snap/bin/firefox"]],
      waterfox: ["firefox", [process.env.E2E_WATERFOX || "", "/usr/lib/waterfox/waterfox", "/opt/waterfox/waterfox"].filter(Boolean)],
    };

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export async function until(what, ms, test) {
  const end = Date.now() + ms;
  for (;;) {
    const v = await test();
    if (v) return v;
    if (Date.now() > end) throw new Error(`timed out: ${what}`);
    await sleep(250);
  }
}
// A process and its children, stopped.
export function killPid(pid) {
  if (WIN) return spawnSync("taskkill", ["/T", "/F", "/PID", String(pid)]);
  spawnSync("pkill", ["-KILL", "-P", String(pid)]);
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // Gone already.
  }
}
export const killTree = (p) => p && p.exitCode === null && killPid(p.pid);
// Every process using a test profile: a browser's launcher may exit and leave the browser (Firefox),
// so its tree is not enough. Only processes naming that folder are stopped.
export function killProfile(profile) {
  if (!WIN) return spawnSync("pkill", ["-KILL", "-f", profile]);
  const folder = profile.replace(/'/g, "''");
  spawnSync("powershell", [
    "-NoProfile",
    "-Command",
    `Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine.Contains('${folder}') } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`,
  ]);
}

// Chromium's DevTools protocol over --remote-debugging-pipe (needed to load an extension).
export class Pipe {
  constructor(proc) {
    this.out = proc.stdio[3];
    this.next = 1;
    this.waiting = new Map();
    // Event listeners (`on`): each gets every event {method, params, sessionId}.
    this.listeners = [];
    let buf = "";
    proc.stdio[4].on("data", (d) => {
      buf += d.toString("utf8");
      let i;
      while ((i = buf.indexOf("\0")) >= 0) {
        const m = JSON.parse(buf.slice(0, i));
        buf = buf.slice(i + 1);
        if (process.env.E2E_DEBUG && m.method) console.error("cdp", m.method, JSON.stringify(m.params).slice(0, 300));
        const w = m.id && this.waiting.get(m.id);
        if (w) {
          this.waiting.delete(m.id);
          m.error ? w.reject(new Error(`${w.method}: ${m.error.message}`)) : w.resolve(m.result);
        } else if (m.method) {
          for (const f of this.listeners) f(m);
        }
      }
    });
  }
  on(f) {
    this.listeners.push(f);
  }
  send(method, params = {}, sessionId) {
    const id = this.next++;
    this.out.write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + "\0");
    return new Promise((resolve, reject) => this.waiting.set(id, { resolve, reject, method }));
  }
}

// Firefox's WebDriver BiDi over its remote-debugging WebSocket.
class Bidi {
  constructor(ws) {
    this.ws = ws;
    this.next = 1;
    this.waiting = new Map();
    // Event listeners (`on`): each gets every event {method, params}.
    this.listeners = [];
    ws.addEventListener("message", (e) => {
      const m = JSON.parse(e.data);
      const w = m.id && this.waiting.get(m.id);
      if (w) {
        this.waiting.delete(m.id);
        m.type === "error" ? w.reject(new Error(`${w.method}: ${m.error} ${m.message}`)) : w.resolve(m.result);
      } else if (m.type === "event") {
        for (const f of this.listeners) f(m);
      }
    });
  }
  on(f) {
    this.listeners.push(f);
  }
  send(method, params = {}) {
    const id = this.next++;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolve, reject) => this.waiting.set(id, { resolve, reject, method }));
  }
}

// What the button's pill looks like (in its closed shadow root, which only the browser's own
// protocol reaches): its size and background.
const PILL_STYLE = `function () {
  const p = this.querySelector ? this.querySelector(".pill") : this;
  if (!p) return null;
  const r = p.getBoundingClientRect();
  const s = getComputedStyle(p);
  return JSON.stringify({ h: r.height, w: r.width, image: s.backgroundImage, color: s.backgroundColor });
}`;

// The qualities' menu the button's click opens (video.js), in the same closed shadow root: null
// while closed; else whether it is still looking, its message, its tabs (none when the video has one
// kind of quality only: name, selected), its rows (name, size; the selected tab's) and "Open
// in…", each with its middle on the page (where a click lands). `reveal` ("row": the first row,
// "open": "Open in…") scrolls that into the menu's view first (the menu scrolls past 300 px).
const PICKS = (reveal = "") => `function () {
  const m = this.querySelector ? this.querySelector(".menu") : null;
  if (!m || !m.classList.contains("open")) return null;
  const open = m.querySelector('[data-act="window"]');
  const shown = ${JSON.stringify(reveal)} === "open" ? open : ${JSON.stringify(reveal)} === "row" ? m.querySelector(".rows button[data-choice]") : null;
  if (shown) shown.scrollIntoView({ block: "nearest" });
  const at = (e) => { const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
  const note = m.querySelector(".note");
  const box = m.getBoundingClientRect();
  return JSON.stringify({
    box: { l: box.left, t: box.top, r: box.right, b: box.bottom },
    busy: Boolean(m.querySelector(".rows .skrow")),
    note: note.hidden ? "" : note.textContent,
    caps: [...m.querySelectorAll(".rows .cap")].map((c) => c.textContent),
    // Each line: "Download best" first (top), then the numbered ones under VIDEO and AUDIO.
    rows: (() => { const out = []; let kind = "top";
      for (const e of m.querySelectorAll(".rows > *")) {
        if (e.classList.contains("cap")) { kind = e.dataset.kind; continue; }
        if (e.localName !== "button") continue;
        const f = e.querySelector(".f");
        out.push({ kind: e.classList.contains("top") ? "top" : kind, name: [(e.querySelector(".q") || e.querySelector(".lead")).textContent, f && f.firstChild ? f.firstChild.textContent : ""].filter(Boolean).join(" "),
          size: (e.querySelector(".z") || {}).textContent || "", choice: e.dataset.choice ?? null, ...at(e) });
      }
      return out; })(),
    open: { text: open.textContent, ...at(open) },
  });
}`;
// Where the button's arrow (its menu) is.
const MORE_AT = `function () {
  const m = this.querySelector(".more");
  const r = m && m.getBoundingClientRect();
  return JSON.stringify(r && r.width ? { x: r.left + r.width / 2, y: r.top + r.height / 2 } : null);
}`;
// A click on the button's arrow, as a person makes it: the pointer onto it first, a moment (the tab
// shows its grip as the pointer comes, and keeps its end where it was), then the click where the
// arrow is now.
async function clickArrow(session) {
  const first = await session.pill(MORE_AT);
  if (!first) return null;
  await session.mouse(first.x, first.y);
  await sleep(300);
  const at = (await session.pill(MORE_AT)) || first;
  await session.mouse(at.x, at.y, "click");
  return at;
}
// The button's state (ready, looking, sending, sent, warn, err, off, drm), its words and its
// buttons ("Show", "Retry"…), each with where a click lands.
const PILL_STATE = `function () {
  const p = this.querySelector(".pill");
  const at = (e) => { const r = e.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
  return JSON.stringify(p && {
    state: p.dataset.state,
    text: this.querySelector(".label").textContent,
    chip: this.querySelector(".chip").textContent,
    compact: p.classList.contains("compact"),
    acts: [...this.querySelectorAll(".act")].map((b) => ({ act: b.dataset.act, text: b.textContent, ...at(b) })),
  });
}`;

// Each driver opens `page` in the browser with the extension loaded and returns `evaluate`
// (an expression in the page, run as a user's action), `mouse` (move or click at page
// coordinates, Alt held with `{alt: true}`), `navigate` (`{download: true}`: the address
// becomes a download, the page stays), `key`, `pill`, `picks` (PICKS), `major` (the browser's
// version) and `close`. `ready` resolves once the extension reached the native host.
async function chromium(bin, profile, dl, page, ready) {
  // The browser's own download folder, without asking, set as a user would: the profile's
  // preferences. (DevTools' Browser.setDownloadBehavior would take downloads past extensions.)
  fs.mkdirSync(path.join(profile, "Default"), { recursive: true });
  fs.writeFileSync(
    path.join(profile, "Default", "Preferences"),
    JSON.stringify({
      download: { default_directory: dl, prompt_for_download: false, directory_upgrade: true },
      ...(process.env.E2E_LANG ? { intl: { accept_languages: process.env.E2E_LANG } } : {}),
    }),
  );
  const proc = spawn(
    bin,
    [
      "--headless",
      "--remote-debugging-pipe",
      "--enable-unsafe-extension-debugging",
      `--user-data-dir=${profile}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--window-size=1280,900",
      ...(process.env.E2E_LANG ? [`--lang=${process.env.E2E_LANG}`] : []),
      // The remote case's server has its own certificate (in real use the popup's "Open it" lets
      // the person accept it once); the lab's pages are plain http.
      "--ignore-certificate-errors",
      // Linux: no keyring prompt for the test profile.
      ...(WIN ? [] : ["--password-store=basic"]),
      // E2E_CHROMIUM_ARGS: more switches, space separated (screenshots: --lang=de, --force-dark-mode).
      ...(process.env.E2E_CHROMIUM_ARGS || "").split(" ").filter(Boolean),
      "about:blank",
    ],
    { env: process.env, stdio: ["ignore", "ignore", "ignore", "pipe", "pipe"], windowsHide: true },
  );
  const cdp = new Pipe(proc);
  try {
    const { product } = await cdp.send("Browser.getVersion");
    const major = Number((/\/(\d+)\./.exec(product) || [])[1]) || 0;
    const { id } = await cdp.send("Extensions.loadUnpacked", { path: chromiumExtension });
    // Its service worker runs, and has asked the host for the rules.
    const sw = await until("the extension's service worker", 15000, async () =>
      (await cdp.send("Target.getTargets")).targetInfos.find((t) => t.type === "service_worker" && t.url.includes(id)),
    );
    await ready();
    let trace = async () => "";
    if (process.env.E2E_DEBUG) {
      // Every downloads / native call the extension makes, with its result (E2E_DEBUG).
      const { sessionId: s } = await cdp.send("Target.attachToTarget", { targetId: sw.targetId, flatten: true });
      await cdp.send("Runtime.evaluate", { expression: `
        globalThis.trace = []; const t0 = Date.now();
        for (const [obj, names] of [[chrome.downloads, ["pause", "resume", "cancel", "erase"]], [chrome.runtime, ["sendNativeMessage"]]]) {
          for (const n of names) {
            const orig = obj[n].bind(obj);
            obj[n] = (...a) => { trace.push([Date.now() - t0, n, JSON.stringify(a).slice(0, 300)]);
              return orig(...a).then(r => { trace.push([Date.now() - t0, n + " ok", JSON.stringify(r).slice(0, 80)]); return r; },
                                     e => { trace.push([Date.now() - t0, n + " err", e.message]); throw e; }); };
          }
        }
        chrome.downloads.onCreated.addListener(d => trace.push([Date.now() - t0, "created", d.url, d.filename]));
        chrome.downloads.onChanged.addListener(d => trace.push([Date.now() - t0, "changed", JSON.stringify(d).slice(0, 140)]));
        "ok"` }, s);
      // With the browser's downloads, the session rules and what the extension stored.
      trace = async () =>
        (
          await cdp.send(
            "Runtime.evaluate",
            {
              expression: `(async () => JSON.stringify({ trace, downloads: (await chrome.downloads.search({})).map(d => [d.url, d.filename, d.state, d.error, d.mime]),
                rules: await chrome.declarativeNetRequest.getSessionRules(), stored: await chrome.storage.local.get(null) }))()`,
              awaitPromise: true,
            },
            s,
          )
        ).result.value;
    }
    const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
    const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
    await cdp.send("Page.enable", {}, sessionId);
    const evaluate = async (expression) => {
      const r = await cdp.send("Runtime.evaluate", { expression, userGesture: true, awaitPromise: true, returnByValue: true }, sessionId);
      if (r.exceptionDetails) throw new Error(`evaluate: ${r.exceptionDetails.text} ${expression}`);
      return r.result.value;
    };
    const navigate = async (url, { download = false } = {}) => {
      await cdp.send("Page.navigate", { url }, sessionId);
      // A download leaves the page that was there; else the new page, loaded.
      const want = new URL(url).href;
      await until(`the page ${url}`, 15000, () =>
        evaluate("[location.href, document.readyState]")
          .then(([at, state]) => state === "complete" && (download || at === want))
          .catch(() => false),
      );
    };
    await navigate(page);
    // `act`: "move" (the default), "click", or "down" / "up" for a drag.
    let held = false;
    const mouse = async (x, y, act = "move", { alt = false } = {}) => {
      const at = { x, y, button: "left", buttons: held ? 1 : 0, modifiers: alt ? 1 : 0 };
      await cdp.send("Input.dispatchMouseEvent", { type: "mouseMoved", ...at }, sessionId);
      if (act === "click" || act === "down") {
        held = true;
        await cdp.send("Input.dispatchMouseEvent", { type: "mousePressed", ...at, buttons: 1, clickCount: 1 }, sessionId);
      }
      if (act === "click" || act === "up") {
        held = false;
        await cdp.send("Input.dispatchMouseEvent", { type: "mouseReleased", ...at, buttons: 0, clickCount: 1 }, sessionId);
      }
    };
    // A PNG of a page rectangle (CSS pixels, the device's own pixel ratio) in `file`.
    const shot = async (file, box) => {
      const x = Math.max(0, Math.floor(box.l));
      const y = Math.max(0, Math.floor(box.t));
      const clip = { x, y, width: Math.ceil(box.r) - x, height: Math.ceil(box.b) - y, scale: 1 };
      const { data } = await cdp.send("Page.captureScreenshot", { format: "png", clip }, sessionId);
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(file, Buffer.from(data, "base64"));
    };
    const KEYS = { ArrowDown: 40, ArrowUp: 38, Enter: 13, Tab: 9, End: 35, Home: 36 };
    const key = async (k) => {
      for (const type of ["keyDown", "keyUp"]) {
        // Enter carries its text, as a real key press does.
        const text = type === "keyDown" && k === "Enter" ? { text: "\r" } : {};
        await cdp.send("Input.dispatchKeyEvent", { type, key: k, code: k, windowsVirtualKeyCode: KEYS[k], ...text }, sessionId);
      }
    };
    // A function (PILL_STYLE, PICKS) run on the button's closed shadow root (DevTools sees
    // through it): its JSON answer, parsed; null without the button.
    const inShadow = async (fn) => {
      const { root: doc } = await cdp.send("DOM.getDocument", { depth: -1, pierce: true }, sessionId);
      const find = (n) => {
        if (n.nodeName === "MDM-VIDEO-BUTTON") return n;
        for (const c of [...(n.children || []), ...(n.shadowRoots || []), ...(n.contentDocument ? [n.contentDocument] : [])]) {
          const f = find(c);
          if (f) return f;
        }
        return null;
      };
      const host = find(doc);
      const shadow = host && (host.shadowRoots || [])[0];
      if (!shadow) return null;
      const { object } = await cdp.send("DOM.resolveNode", { nodeId: shadow.nodeId }, sessionId);
      const r = await cdp.send("Runtime.callFunctionOn", { objectId: object.objectId, functionDeclaration: fn, returnByValue: true }, sessionId);
      return r.result.value ? JSON.parse(r.result.value) : null;
    };
    const pill = (fn = PILL_STYLE) => inShadow(fn);
    const picks = (reveal) => inShadow(PICKS(reveal));
    // Another tab at `url` (the extension's own pages too): its `evaluate` and `close`.
    const tab = async (url) => {
      const { targetId: tid } = await cdp.send("Target.createTarget", { url });
      const { sessionId: s } = await cdp.send("Target.attachToTarget", { targetId: tid, flatten: true });
      const ev = async (expression) => {
        const r = await cdp.send("Runtime.evaluate", { expression, userGesture: true, awaitPromise: true, returnByValue: true }, s);
        if (r.exceptionDetails) throw new Error(`evaluate: ${r.exceptionDetails.text} ${expression}`);
        return r.result.value;
      };
      // A picture of its page, cut to the body (the popup's 300 px), for a PR's body; in
      // E2E_COLOR_SCHEME's colours (light or dark) when set, else the system's.
      const shot = async (file) => {
        const scheme = process.env.E2E_COLOR_SCHEME;
        if (scheme) {
          await cdp.send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: scheme }] }, s);
          await sleep(300);
        }
        const r = JSON.parse(await ev("JSON.stringify(document.body.getBoundingClientRect())"));
        // Its margin as much on each side as above it (a right-to-left page's body is at the right).
        const clip = { x: Math.max(0, r.left - r.top), y: 0, width: Math.ceil(r.width + 2 * r.top), height: Math.ceil(r.bottom + r.top), scale: 2 };
        const { data } = await cdp.send("Page.captureScreenshot", { format: "png", clip }, s);
        fs.writeFileSync(file, Buffer.from(data, "base64"));
      };
      return { evaluate: ev, shot, close: () => cdp.send("Target.closeTarget", { targetId: tid }).catch(() => {}) };
    };
    // The tab's requests to `hosts` (http and https) answered here, never by the network:
    // `respond(url)` gives { status, type, body }, or null for the network's own answer.
    let respond = () => null;
    const serve = async (hosts, answer) => {
      respond = answer;
      cdp.on((m) => {
        if (m.sessionId !== sessionId || m.method !== "Fetch.requestPaused") return;
        const r = respond(m.params.request.url);
        const { requestId } = m.params;
        if (!r) return cdp.send("Fetch.continueRequest", { requestId }, sessionId).catch(() => {});
        const headers = [{ name: "Content-Type", value: r.type }];
        cdp.send("Fetch.fulfillRequest", { requestId, responseCode: r.status, responseHeaders: headers, body: Buffer.from(r.body).toString("base64") }, sessionId).catch(() => {});
      });
      const patterns = hosts.flatMap((h) => [`http://${h}/*`, `https://${h}/*`]).map((urlPattern) => ({ urlPattern, requestStage: "Request" }));
      await cdp.send("Fetch.enable", { patterns }, sessionId);
    };
    return {
      extension: id,
      base: `chrome-extension://${id}/`,
      serve,
      major,
      evaluate,
      mouse,
      screenshot: async (file) => {
        const { data } = await cdp.send("Page.captureScreenshot", { format: "png" }, sessionId);
        fs.writeFileSync(file, Buffer.from(data, "base64"));
      },
      navigate,
      key,
      shot,
      pill,
      picks,
      tab,
      trace,
      // The errors in the extension's service worker's console so far (#42: none), Runtime.enable
      // replaying those logged before it.
      workerErrors: async () => {
        const s = (await cdp.send("Target.attachToTarget", { targetId: sw.targetId, flatten: true })).sessionId;
        const seen = [];
        cdp.on((m) => {
          if (m.sessionId !== s) return;
          if (m.method === "Runtime.exceptionThrown") seen.push(`exception: ${m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text}`);
          if (m.method === "Runtime.consoleAPICalled" && (m.params.type === "error" || m.params.type === "warning")) {
            seen.push(`${m.params.type}: ${m.params.args.map((a) => a.value ?? a.description).join(" ")}`);
          }
          if (m.method === "Log.entryAdded" && m.params.entry.level === "error") seen.push(`log: ${m.params.entry.text}`);
        });
        await cdp.send("Runtime.enable", {}, s);
        await cdp.send("Log.enable", {}, s);
        await sleep(500);
        return seen;
      },
      // Run `expression` in the extension's service worker.
      inExtension: async (expression) => {
        const s = (await cdp.send("Target.attachToTarget", { targetId: sw.targetId, flatten: true })).sessionId;
        const r = await cdp.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }, s);
        if (r.exceptionDetails) throw new Error(`in the extension: ${r.exceptionDetails.text} ${expression}`);
        return r.result.value;
      },
      // Another extension (the folder `dir`), loaded beside ours, and its page `page` open in a tab:
      // that tab's `evaluate` and `close` (the proxy case's stand-in VPN extension sets the
      // browser's proxy there, as one does). A page, not its service worker: the browser may
      // stop an idle worker, a page keeps its extension API while open.
      loadExtension: async (dir, page, api = "proxy") => {
        const { id: other } = await cdp.send("Extensions.loadUnpacked", { path: dir });
        const t = await tab(`chrome-extension://${other}/${page}`);
        await until("the other extension's page", 10000, () => t.evaluate(`document.readyState === 'complete' && typeof chrome === 'object' && Boolean(chrome.${api})`).catch(() => false));
        return t;
      },
      close: () => {
        killTree(proc);
        killProfile(profile);
      },
    };
  } catch (e) {
    killTree(proc);
    killProfile(profile);
    throw e;
  }
}

async function firefox(bin, profile, dl, page, ready) {
  const port = 9300 + Math.floor(Math.random() * 500);
  const pref = (k, v) => `user_pref(${JSON.stringify(k)}, ${JSON.stringify(v)});`;
  fs.writeFileSync(
    path.join(profile, "user.js"),
    [
      pref("browser.download.dir", dl),
      pref("browser.download.folderList", 2),
      pref("browser.download.useDownloadDir", true),
      // E2E_FIREFOX_ASK=1: every case with the browser asking what to do with each new type
      // (Waterfox's default, #208); else it doesn't (Firefox's).
      pref("browser.download.always_ask_before_handling_new_types", process.env.E2E_FIREFOX_ASK === "1"),
      pref("browser.shell.checkDefaultBrowser", false),
      pref("datareporting.policy.dataSubmissionEnabled", false),
      pref("browser.aboutwelcome.enabled", false),
      // An extension's host permissions are granted without asking (MV3 asks the user).
      pref("extensions.originControls.grantByDefault", true),
      // Alt+click saves the link, as in Chromium (Firefox's default ignores an Alt+click on a
      // link: then nothing is downloaded at all, by the browser or MDM).
      pref("browser.altClickSave", true),
      // The extension's address, known in advance (its popup opened in a tab).
      pref("extensions.webextensions.uuids", JSON.stringify({ [FIREFOX_ID]: FIREFOX_UUID })),
    ].join("\n"),
  );
  const proc = spawn(bin, ["-headless", "-no-remote", "-profile", profile, "--remote-debugging-port", String(port), "--width=1280", "--height=900"], {
    // Headless even where its launcher restarts it without the flag. System access (this test
    // profile only, its protocol on 127.0.0.1): to open the extension's own pages in a tab.
    env: { ...process.env, MOZ_HEADLESS: "1", MOZ_REMOTE_ALLOW_SYSTEM_ACCESS: "1" },
    stdio: "ignore",
    windowsHide: true,
  });
  try {
    const ws = await until("Firefox's remote protocol", 30000, () =>
      new Promise((resolve) => {
        const s = new WebSocket(`ws://127.0.0.1:${port}/session`);
        s.addEventListener("open", () => resolve(s));
        s.addEventListener("error", () => resolve(null));
      }),
    );
    const bidi = new Bidi(ws);
    // The remote case's server certificate, as in Chromium (above).
    const { capabilities } = await bidi.send("session.new", { capabilities: { alwaysMatch: { acceptInsecureCerts: true } } });
    const major = Number(String(capabilities.browserVersion || "").split(".")[0]) || 0;
    // A temporary add-on (unsigned is fine), gone with the profile.
    const { extension } = await bidi.send("webExtension.install", { extensionData: firefoxExtension });
    await ready();
    const { contexts } = await bidi.send("browsingContext.getTree", {});
    const context = contexts[0].context;
    // BiDi returns objects in its own form: the value goes through JSON instead.
    const evaluate = async (expression) => {
      const json = `(async () => JSON.stringify(await (${expression})))()`;
      const r = await bidi.send("script.evaluate", { expression: json, target: { context }, awaitPromise: true, userActivation: true });
      if (r.type === "exception") throw new Error(`evaluate: ${r.exceptionDetails.text} ${expression}`);
      return r.result.value === undefined ? undefined : JSON.parse(r.result.value);
    };
    const navigate = async (url, { download = false } = {}) => {
      if (!download) return bidi.send("browsingContext.navigate", { context, url, wait: "complete" });
      // The navigation ends in a download (an error for BiDi): the page stays.
      await bidi.send("browsingContext.navigate", { context, url, wait: "none" }).catch(() => {});
      await sleep(500);
      await until("the page", 15000, () => evaluate("document.readyState === 'complete'").catch(() => false));
    };
    await navigate(page);
    // Alt held: a key source pressing it around the pointer's actions, tick for tick.
    const mouse = async (x, y, act = "move", { alt = false } = {}) => {
      const actions = [{ type: "pointerMove", x: Math.round(x), y: Math.round(y) }];
      if (act === "click" || act === "down") actions.push({ type: "pointerDown", button: 0 });
      if (act === "click" || act === "up") actions.push({ type: "pointerUp", button: 0 });
      const sources = [{ type: "pointer", id: "mouse", actions }];
      if (alt) {
        const ALT = "\uE00A";
        sources.push({
          type: "key",
          id: "keyboard",
          actions: [{ type: "keyDown", value: ALT }, ...actions.slice(1).map(() => ({ type: "pause" })), { type: "keyUp", value: ALT }],
        });
        actions.push({ type: "pause" });
      }
      await bidi.send("input.performActions", { context, actions: sources });
    };
    // WebDriver key codes.
    const KEYS = { ArrowDown: "\uE015", ArrowUp: "\uE013", Enter: "\uE007", Tab: "\uE004", End: "\uE010", Home: "\uE011" };
    const key = (k) =>
      bidi.send("input.performActions", {
        context,
        actions: [{ type: "key", id: "keyboard", actions: [{ type: "keyDown", value: KEYS[k] }, { type: "keyUp", value: KEYS[k] }] }],
      });
    // A function (PILL_STYLE, PICKS) run on the button's closed shadow root, which BiDi
    // serializes with includeShadowTree "all": its JSON answer, parsed; null without the button.
    const inShadow = async (fn) => {
      const r = await bidi.send("script.evaluate", {
        expression: `document.querySelector("mdm-video-button")`,
        target: { context },
        awaitPromise: false,
        resultOwnership: "root",
        serializationOptions: { includeShadowTree: "all", maxDomDepth: 1 },
      });
      const shadow = r.result && r.result.value && r.result.value.shadowRoot;
      if (!shadow || !shadow.sharedId) return null;
      const c = await bidi.send("script.callFunction", {
        functionDeclaration: fn,
        this: { sharedId: shadow.sharedId },
        target: { context },
        awaitPromise: false,
      });
      return c.result && c.result.value ? JSON.parse(c.result.value) : null;
    };
    const pill = (fn = PILL_STYLE) => inShadow(fn);
    const picks = (reveal) => inShadow(PICKS(reveal));
    // Another tab at `url` (the extension's own pages too): its `evaluate` and `close`.
    // A web page can't be sent to an extension's page: the browser's own window (its chrome
    // scope, MOZ_REMOTE_ALLOW_SYSTEM_ACCESS) opens that tab instead.
    const tab = async (url) => {
      let c;
      if (url.startsWith("moz-extension:")) {
        const { contexts: windows } = await bidi.send("browsingContext.getTree", { "moz:scope": "chrome" });
        const open = `gBrowser.addTab(${JSON.stringify(url)}, { triggeringPrincipal: Services.scriptSecurityManager.getSystemPrincipal() }) && "opened"`;
        await bidi.send("script.evaluate", { expression: open, target: { context: windows[0].context }, awaitPromise: false, "moz:scope": "chrome" });
        c = await until("the extension's page in a tab", 10000, async () => {
          const { contexts: tabs } = await bidi.send("browsingContext.getTree", {});
          return (tabs.find((x) => x.url === url) || {}).context;
        });
      } else {
        c = (await bidi.send("browsingContext.create", { type: "tab", background: true })).context;
        await bidi.send("browsingContext.navigate", { context: c, url, wait: "complete" });
      }
      const ev = async (expression) => {
        const json = `(async () => JSON.stringify(await (${expression})))()`;
        const r = await bidi.send("script.evaluate", { expression: json, target: { context: c }, awaitPromise: true, userActivation: true });
        if (r.type === "exception") throw new Error(`evaluate: ${r.exceptionDetails.text} ${expression}`);
        return r.result.value === undefined ? undefined : JSON.parse(r.result.value);
      };
      // A click at the centre of `selector`, as a user's (the tab brought to the front first):
      // what a browser lets an extension's page ask a permission from.
      const click = async (selector) => {
        // An extension's page is privileged: BiDi won't bring it to the front, the browser's
        // window does.
        await inBrowser(`(() => { const t = [...gBrowser.tabs].find((x) => x.linkedBrowser.currentURI.spec === ${JSON.stringify(url)}); if (t) gBrowser.selectedTab = t; return Boolean(t); })()`);
        const r = await ev(`(() => { const b = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; })()`);
        await bidi.send("input.performActions", {
          context: c,
          actions: [{ type: "pointer", id: "mouse", actions: [{ type: "pointerMove", x: Math.round(r[0]), y: Math.round(r[1]) }, { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 }] }],
        });
      };
      return { evaluate: ev, click, close: () => bidi.send("browsingContext.close", { context: c }).catch(() => {}) };
    };
    // An expression in the browser's own window (its chrome scope): its preferences, its console,
    // its permission prompts.
    const inBrowser = async (expression) => {
      const { contexts: windows } = await bidi.send("browsingContext.getTree", { "moz:scope": "chrome" });
      const json = `(async () => JSON.stringify(await (${expression})))()`;
      const r = await bidi.send("script.evaluate", { expression: json, target: { context: windows[0].context }, awaitPromise: true, "moz:scope": "chrome" });
      if (r.type === "exception") throw new Error(`in the browser: ${r.exceptionDetails.text} ${expression}`);
      return r.result.value === undefined ? undefined : JSON.parse(r.result.value);
    };
    // The page's right-click menu at (x, y), and its item `label` chosen (#155): a right-click,
    // then the item activated in the browser's own window (its chrome scope). "chosen", or the
    // items there were.
    const menu = async (x, y, label) => {
      await bidi.send("input.performActions", {
        context,
        actions: [{ type: "pointer", id: "mouse", actions: [{ type: "pointerMove", x: Math.round(x), y: Math.round(y) }, { type: "pointerDown", button: 2 }, { type: "pointerUp", button: 2 }] }],
      });
      const { contexts: windows } = await bidi.send("browsingContext.getTree", { "moz:scope": "chrome" });
      const pick = `(async () => {
        const menu = document.getElementById("contentAreaContextMenu");
        const shown = () => [...menu.querySelectorAll("menuitem")].filter((m) => !m.hidden);
        for (let i = 0; i < 50 && !shown().some((m) => m.label === ${JSON.stringify(label)}); i++) await new Promise((r) => setTimeout(r, 100));
        const item = shown().find((m) => m.label === ${JSON.stringify(label)});
        if (!item) { const all = shown().map((m) => m.label).join(" | "); menu.hidePopup(); return "not there: " + all; }
        const at = gContextMenu ? { onVideo: gContextMenu.onVideo, mediaURL: gContextMenu.mediaURL, linkURL: gContextMenu.linkURL } : null;
        menu.activateItem(item);
        return "chosen " + JSON.stringify(at);
      })()`;
      const r = await bidi.send("script.evaluate", { expression: pick, target: { context: windows[0].context }, awaitPromise: true, "moz:scope": "chrome" });
      if (r.type === "exception") throw new Error(`the menu: ${r.exceptionDetails.text}`);
      return r.result.value;
    };
    // The tab's requests to `hosts` answered here (as in Chromium, above).
    let respond = () => null;
    const serve = async (hosts, answer) => {
      respond = answer;
      bidi.on((m) => {
        if (m.method !== "network.beforeRequestSent" || !m.params.isBlocked) return;
        const request = m.params.request.request;
        const r = respond(m.params.request.url);
        if (!r) return bidi.send("network.continueRequest", { request }).catch(() => {});
        bidi
          .send("network.provideResponse", {
            request,
            statusCode: r.status,
            reasonPhrase: r.status === 200 ? "OK" : "Not Found",
            headers: [{ name: "Content-Type", value: { type: "string", value: r.type } }],
            body: { type: "string", value: r.body },
          })
          .catch(() => {});
      });
      await bidi.send("session.subscribe", { events: ["network.beforeRequestSent"], contexts: [context] });
      const urlPatterns = hosts.flatMap((hostname) => ["http", "https"].map((protocol) => ({ type: "pattern", protocol, hostname })));
      await bidi.send("network.addIntercept", { phases: ["beforeRequestSent"], urlPatterns, contexts: [context] });
    };
    return {
      extension,
      serve,
      // Another add-on (the folder `dir`), temporary beside ours: its ID; and removed again.
      install: async (dir) => (await bidi.send("webExtension.install", { extensionData: { type: "path", path: dir } })).extension,
      uninstall: (id) => bidi.send("webExtension.uninstall", { extension: id }),
      inBrowser,
      base: `moz-extension://${FIREFOX_UUID}/`,
      major,
      evaluate,
      mouse,
      menu,
      navigate,
      key,
      pill,
      picks,
      tab,
      close: () => {
        ws.close();
        killTree(proc);
        killProfile(profile);
      },
    };
  } catch (e) {
    killTree(proc);
    killProfile(profile);
    throw e;
  }
}

// ---- the test instance of Mister Download Manager ----------------------------------------------

let data = "";
let downloads = "";
// The yt-dlp the test instance uses (the streams case), or "" (none found).
let ytdlp = "";
// The ffmpeg the qualities case needs (E2E_FFMPEG, else one on PATH).
let ffmpeg = "";
const appExe = exe("misterdownloadmanager");

// One command to the instance over its channel (ipc.json), its one-line answer.
function ipc(cmd, dir = data) {
  return new Promise((resolve, reject) => {
    let ep;
    try {
      ep = JSON.parse(fs.readFileSync(path.join(dir, "ipc.json"), "utf8"));
    } catch (e) {
      return reject(e);
    }
    const s = net.connect(ep.port, "127.0.0.1", () => s.write(JSON.stringify({ token: ep.token, ...cmd }) + "\n"));
    let answer = "";
    s.setTimeout(5000, () => s.destroy(new Error("no answer")));
    s.on("data", (d) => (answer += d));
    s.on("end", () => resolve(answer.trim()));
    s.on("error", reject);
  });
}

// The instance's list (the automation API, on in the test settings).
async function mdmList() {
  const a = JSON.parse(await ipc({ query: "list" }));
  if (!a.downloads) throw new Error(`the automation API: ${JSON.stringify(a)}`);
  return a.downloads;
}

// The running copies of target/release's app (each with its crash monitor, the same program with
// --crash-monitor): [pid, is the monitor]. A headless one (the remote case's server) isn't one.
function appProcesses() {
  if (!WIN) {
    // /proc: each process's program and arguments.
    const found = [];
    for (const pid of fs.readdirSync("/proc").filter((d) => /^\d+$/.test(d))) {
      try {
        if (fs.readlinkSync(`/proc/${pid}/exe`).replace(/ \(deleted\)$/, "") !== appExe) continue;
        const args = fs.readFileSync(`/proc/${pid}/cmdline`, "utf8").split("\0");
        if (!args.includes("--headless")) found.push([Number(pid), args.includes("--crash-monitor")]);
      } catch {
        // Gone, or another user's.
      }
    }
    return found;
  }
  const p = appExe.replace(/'/g, "''");
  const out = spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `Get-CimInstance Win32_Process -Filter "Name='misterdownloadmanager.exe'" | Where-Object { $_.ExecutablePath -eq '${p}' -and $_.CommandLine -notlike '*--headless*' } | ForEach-Object { "$($_.ProcessId) $($_.CommandLine -like '*--crash-monitor*')" }`,
    ],
    { encoding: "utf8" },
  ).stdout;
  return out
    .split(/\r?\n/)
    .filter(Boolean)
    .map((l) => l.trim().split(" "))
    .map(([pid, monitor]) => [Number(pid), monitor === "True"]);
}
// Copies that ran before the test (never touched), and so ours.
let before = new Set();
const ourPids = () => appProcesses().map(([pid]) => pid).filter((p) => !before.has(p));
// Our app itself, without its monitor.
const ourApps = () => appProcesses().filter(([pid, monitor]) => !monitor && !before.has(pid)).map(([pid]) => pid);

async function startApp() {
  if (ourPids().length) return;
  fs.rmSync(path.join(data, "ipc.json"), { force: true });
  spawn(appExe, [], { env: process.env, stdio: "ignore", detached: false });
  await until("the test instance", 20000, () => mdmList().then(() => true, () => false));
}

// Saved and quit through its channel; stopped if it doesn't.
async function stopApp() {
  if (!ourPids().length) return;
  await ipc({ exit: true }).catch(() => {});
  try {
    await until("the test instance to quit", 15000, () => !ourPids().length);
  } catch {
    for (const pid of ourPids()) killPid(pid);
  }
}

// The download named `file` in MDM's `sub` folder (under the test's Downloads), complete in its
// list and on disk with `size` bytes; its list entry.
async function arrived(file, size, sub = "Compressed", ms = 45000) {
  const want = path.join(downloads, sub, file);
  let last = null;
  try {
    return await until(`${file} in Mister Download Manager`, ms, async () => {
      const item = (await mdmList().catch(() => [])).find((d) => d.file === file);
      last = item || last;
      if (item && item.status === "Error") throw new Error(`${file}: MDM's download failed: ${item.error}`);
      if (!item || item.status !== "Complete") return null;
      if (path.resolve(item.path) !== path.resolve(want)) throw new Error(`${file} saved as ${item.path}, not ${want}`);
      return fs.existsSync(want) && fs.statSync(want).size === size && item;
    });
  } catch (e) {
    if (last && !/MDM's|saved as/.test(e.message)) e.message += ` (MDM: ${last.status} ${last.downloaded}/${last.size} ${last.path})`;
    if (!last) {
      const list = await mdmList().catch(() => []);
      e.message += ` (MDM's latest: ${list.slice(-2).map((d) => `${d.file} ${d.status} ${d.path}`).join("; ") || "none"})`;
    }
    throw e;
  }
}

// The lab server's request log for `p` (a path), oldest first.
async function requests(p) {
  const log = await (await fetch(`${LAB}/_log`)).json();
  return log.filter((e) => e.path === p).sort((a, b) => a.req - b.req);
}
const clearLog = () => fetch(`${LAB}/_log`, { method: "DELETE" });
const brief = (e) => `${e.method} ${e.status} ${e.outcome} ${e.range || "-"}`;

// ---- another Mister Download Manager (the remote case, #157) -------------------------------------

// The server the remote case sends downloads to: E2E_REMOTE_URL names one already running (the
// Docker image: E2E_REMOTE_PAIR is a command printing a pairing code, `docker exec mdm
// misterdownloadmanager-cli pair`, and E2E_REMOTE_DOWNLOADS the folder its downloads land in, as
// seen from here); else target/release's app runs headless here, on 127.0.0.1, with its own data
// and download folders under target/e2e.
const REMOTE_PORT = 18463;
let remote = null;
async function remoteServer() {
  if (remote) return remote;
  if (process.env.E2E_REMOTE_URL) {
    const cmd = process.env.E2E_REMOTE_PAIR;
    const downloads = process.env.E2E_REMOTE_DOWNLOADS;
    if (!cmd || !downloads) throw new Error("E2E_REMOTE_URL needs E2E_REMOTE_PAIR and E2E_REMOTE_DOWNLOADS");
    const code = async () => {
      const out = spawnSync(cmd, { shell: true, encoding: "utf8" });
      const m = [...String(out.stdout).matchAll(/code (\d{4}) (\d{4})/g)].pop();
      if (!m) throw new Error(`no pairing code from ${cmd}: ${out.stdout}${out.stderr}`);
      return m[1] + m[2];
    };
    remote = { url: process.env.E2E_REMOTE_URL, downloads, code };
    return remote;
  }
  const dir = path.join(work, "remote");
  const rdata = path.join(dir, "data");
  const downloads = path.join(dir, "Downloads");
  fs.mkdirSync(rdata, { recursive: true });
  const args = ["--headless", "--data", rdata, "--downloads", downloads, "--listen", "127.0.0.1", "--port", String(REMOTE_PORT)];
  const proc = spawn(appExe, args, { env: { ...process.env, MDM_DATA_DIR: rdata }, stdio: "ignore", windowsHide: true });
  const code = async () => {
    const a = JSON.parse(await ipc({ pair: true }, rdata));
    if (!a.code) throw new Error(`no pairing code: ${JSON.stringify(a)}`);
    return a.code;
  };
  remote = { url: `https://127.0.0.1:${REMOTE_PORT}/`, downloads, code, proc, data: rdata };
  await until("the headless MDM", 20000, () => code().then(() => true, () => false));
  return remote;
}
async function stopRemote() {
  if (!remote || !remote.proc) return;
  await ipc({ exit: true }, remote.data).catch(() => {});
  await until("the headless MDM to quit", 15000, () => remote.proc.exitCode !== null).catch(() => killTree(remote.proc));
}
// The file `name` of `size` bytes somewhere in `dir` (a category's folder): its path.
function findFile(dir, name, size) {
  let entries = [];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    return null;
  }
  for (const e of entries) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      const f = findFile(p, name, size);
      if (f) return f;
    } else if (e.name === name && fs.statSync(p).size === size) return p;
  }
  return null;
}
const sizeOf = async (url) => Number((await fetch(url, { method: "HEAD" })).headers.get("content-length"));

// The browser kept nothing: its download folder is empty (no .part, no .crdownload, no
// "Unconfirmed", no placeholder). Chromium deletes its working file (`<uuid>.tmp`) on its own
// after the extension cancels its copy, sometimes a second or two later on a busy computer or
// with an older app's host (seen past 5 s while other builds ran): a file still there after 10 s is kept.
async function keptNothing(dl) {
  await sleep(1000);
  let left = fs.readdirSync(dl);
  for (const end = Date.now() + 9000; left.length && Date.now() < end; left = fs.readdirSync(dl)) await sleep(250);
  if (left.length) throw new Error(`the browser kept: ${left.join(", ")}`);
}

// ---- the test instance's windows (Windows) ----------------------------------------------------

// Elsewhere they are not read (Win32 and UI Automation): the cases that need them are skipped.
const NO_UIA = "the app's windows are read through UI Automation (Windows only)";

// Its windows: titles, closing the one titled `close`, and those that show on a screen. Collected
// in C#: what a PowerShell callback prints is lost.
function appWindows(pid, close = "", onScreen = false) {
  if (!WIN) throw new Skip(NO_UIA);
  const script = `
Add-Type @"
using System; using System.Text; using System.Collections.Generic; using System.Runtime.InteropServices;
public static class W {
  public delegate bool P(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
  [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] static extern int GetSystemMetrics(int i);
  public static List<string> Of(uint pid, string close, bool onScreen) {
    var r = new List<string>();
    int sx = GetSystemMetrics(76), sy = GetSystemMetrics(77), sw = GetSystemMetrics(78), sh = GetSystemMetrics(79);
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
      if (p == pid && IsWindowVisible(h)) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256);
        var t = sb.ToString(); R rc; GetWindowRect(h, out rc);
        bool seen = rc.Rt > sx && rc.L < sx + sw && rc.B > sy && rc.T < sy + sh && rc.Rt > rc.L && rc.B > rc.T;
        if (t.Length > 0 && (!onScreen || seen)) { r.Add(t); if (t == close) PostMessage(h, 0x10, IntPtr.Zero, IntPtr.Zero); } }
      return true; }, IntPtr.Zero);
    return r;
  }
}
"@
[W]::Of(${pid}, '${close.replace(/'/g, "''")}', $${onScreen})`;
  return spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" }).stdout.split(/\r?\n/).filter(Boolean);
}

// The address in the Download video window's "Page address" box, read through UI Automation
// (the app's windows have AccessKit adapters, also off the screens); null while it isn't open.
function videoAddress(pid) {
  if (!WIN) throw new Skip(NO_UIA);
  const script = `
Add-Type -AssemblyName UIAutomationClient; Add-Type -AssemblyName UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$pid_ = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, ${pid})
$edit = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)),
  (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Page address')))
foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $pid_)) {
  $e = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $edit)
  if ($e) { 'ADDRESS ' + $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value; break }
}`;
  const out = spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" }).stdout;
  const m = /^ADDRESS (.*)$/m.exec(out);
  return m ? m[1].trim() : null;
}

const VIDEO_WINDOW = "Download video from a page";

// The Download video window's "Download Now", pressed through UI Automation (its Invoke, as a
// screen reader does: no pointer, nothing on the screen). Whether it was there to press.
function videoDownloadNow(pid) {
  if (!WIN) throw new Skip(NO_UIA);
  const script = `
Add-Type -AssemblyName UIAutomationClient; Add-Type -AssemblyName UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$pid_ = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, ${pid})
$now = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)),
  (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Download Now')))
$edit = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)),
  (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Page address')))
foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $pid_)) {
  if (-not $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $edit)) { continue }
  $b = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $now)
  if ($b -and $b.Current.IsEnabled) { $b.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); 'PRESSED'; break }
}`;
  return /PRESSED/.test(spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" }).stdout);
}

// Whether the Download video window's "Download Now" is enabled: yt-dlp looked the address up
// and offered something to download (#155). null while the window isn't open.
function videoReady(pid) {
  if (!WIN) throw new Skip(NO_UIA);
  const script = `
Add-Type -AssemblyName UIAutomationClient; Add-Type -AssemblyName UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]
$pid_ = New-Object System.Windows.Automation.PropertyCondition($A::ProcessIdProperty, ${pid})
$now = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Button)),
  (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Download Now')))
$edit = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition($A::ControlTypeProperty, [System.Windows.Automation.ControlType]::Edit)),
  (New-Object System.Windows.Automation.PropertyCondition($A::NameProperty, 'Page address')))
# The window's accessible root is named after the app, not its title: the one with "Page address".
foreach ($w in $A::RootElement.FindAll([System.Windows.Automation.TreeScope]::Children, $pid_)) {
  if (-not $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $edit)) { continue }
  $b = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $now)
  if ($b) { 'READY ' + $b.Current.IsEnabled; break }
}`;
  const out = spawnSync("powershell", ["-NoProfile", "-Command", script], { encoding: "utf8" }).stdout;
  const m = /^READY (\w+)$/m.exec(out);
  return m ? m[1] === "True" : null;
}

// #155: a stream handed over by `trigger` (the button over the player, the right-click menu, the
// media list) reaches yt-dlp as its manifest, with the page's cookie and the page as referrer,
// and yt-dlp reads it (Download Now enabled). Before, a player playing from a blob: sent the page
// itself, and yt-dlp, not knowing the site, answered "Unsupported URL". The window's address,
// and what yt-dlp asked the lab for.
async function streamLookedUp(trigger, manifest, page, cookie) {
  const pid = appPid();
  appWindows(pid, VIDEO_WINDOW);
  await until("the Download video window closed", 5000, () => !appWindows(pid).includes(VIDEO_WINDOW));
  await clearLog();
  await trigger();
  await until("the Download video window", 15000, () => appWindows(pid).includes(VIDEO_WINDOW)).catch(async (e) => {
    const asked = (await (await fetch(`${LAB}/_log`)).json()).map((x) => `${x.path} ${x.status}`);
    throw new Error(`${e.message} (the lab saw: ${asked.join(", ") || "nothing"})`);
  });
  const address = await until("the Download video window's address", 20000, () => videoAddress(pid)).catch((e) => {
    // What the window showed, for the report (PrintWindow: off the screens too).
    const shot = path.join(work, `video-window-${Date.now()}.png`);
    spawnSync("powershell", ["-NoProfile", "-File", path.join(root, "tools", "lab", "cap-own.ps1"), "-ProcessId", String(pid), "-Title", VIDEO_WINDOW, "-Out", shot]);
    e.message += fs.existsSync(shot) ? ` (the window: ${shot})` : " (no capture)";
    throw e;
  });
  // yt-dlp's requests carry the page as referrer (the extension's own fetch of a manifest, for
  // the media list's qualities, doesn't).
  const asked = async () => (await (await fetch(`${LAB}/_log`)).json()).filter((e) => e.referer === page);
  let ready = false;
  try {
    ready = await until("yt-dlp's lookup (Download Now enabled)", 45000, () => videoReady(pid));
  } catch (e) {
    const got = await asked();
    throw new Error(`${e.message}; the window got ${address}; yt-dlp asked for ${got.map((x) => `${x.path} ${x.status}`).join(", ") || "nothing"}`);
  } finally {
    appWindows(pid, VIDEO_WINDOW);
  }
  const got = await asked();
  // The address with its query: #171's master and variants share one path.
  const full = (x) => `${x.path}${x.query ? `?${x.query}` : ""}`;
  const first = got.find((x) => full(x) === manifest);
  if (!first) throw new Error(`yt-dlp didn't ask for ${manifest}: ${got.map(full).join(", ")}`);
  if (got.some((x) => x.path === "/page")) throw new Error("yt-dlp was given the page too");
  if (!(first.cookie || "").includes(cookie)) throw new Error(`${manifest} without the cookie ${cookie}: ${first.cookie}`);
  return `${address.replace(LAB, "")} read by yt-dlp${ready ? "" : "?"} (cookie, referrer)`;
}
const appPid = () => {
  const [pid] = ourApps();
  if (!pid) throw new Error("the test instance isn't running");
  return pid;
};

// ---- the download button over videos ----------------------------------------------------------

const near = (a, b, what) => {
  if (Math.abs(a - b) > 6) throw new Error(`${what}: ${Math.round(a)} instead of ${Math.round(b)}`);
};

// An element's rectangle (the button's only while shown).
const rect = (session, sel) =>
  session.evaluate(`(() => { const e = document.querySelector(${JSON.stringify(sel)});
    if (!e || (e.localName === "mdm-video-button" && e.style.display !== "block")) return null;
    const r = e.getBoundingClientRect(); return { l: r.left, t: r.top, r: r.right, b: r.bottom }; })()`);

// #480: a mouse-focused player must still fade, including after entering fullscreen. Run
// against a local player so YouTube's network, sign-in and changing layout aren't involved.
export async function videoFocus(session) {
  await session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
  await session.evaluate(`(() => {
    const v = document.querySelector("#video");
    const player = document.createElement("div");
    player.id = "focus-player";
    player.tabIndex = 0;
    player.style.cssText = "width:640px;height:360px;background:#18212d";
    v.before(player);
    player.append(v);
    v.controls = false;
    v.tabIndex = -1;
    v.style.cssText = "width:100%;height:100%;pointer-events:none";
    const after = document.createElement("button");
    after.id = "after-player";
    after.textContent = "After the player";
    player.after(after);
  })()`);
  const focused = (id) => session.evaluate(`document.activeElement?.id === ${JSON.stringify(id)}`);
  const keyboard = () => session.evaluate('document.querySelector("#focus-player").matches(":focus-visible")');
  const shown = () => rect(session, "mdm-video-button");
  const idle = () => sleep(3100); // 2600 ms idle + the 180 ms fade, with room for a slow frame.
  const v = await rect(session, "#video");
  await session.mouse((v.l + v.r) / 2, (v.t + v.b) / 2, "click");
  if (!(await focused("focus-player")) || await keyboard()) throw new Error("player did not receive mouse focus");
  await until("the video button", 5000, shown);
  await idle();
  if (await shown()) throw new Error("#480: mouse focus keeps the video button visible after idle");

  await session.evaluate('document.querySelector("#focus-player").requestFullscreen()');
  if (!(await session.evaluate('document.fullscreenElement?.id === "focus-player"'))) throw new Error("player did not enter fullscreen");
  const fs = await rect(session, "#video");
  await session.mouse((fs.l + fs.r) / 2, (fs.t + fs.b) / 2);
  await until("the fullscreen video button", 5000, shown);
  if (!(await session.evaluate('document.querySelector("mdm-video-button").parentNode === document.fullscreenElement'))) throw new Error("button is outside fullscreen player");
  await idle();
  if (await shown()) throw new Error("#480: mouse focus keeps the fullscreen video button visible after idle");
  await session.evaluate("document.exitFullscreen()");

  // Tab into the player from a preceding control, without a pointer over the video.
  await session.evaluate(`(() => {
    const before = document.createElement("button");
    before.textContent = "Before the player";
    document.querySelector("#focus-player").before(before);
    before.focus();
  })()`);
  await session.mouse(1, 1);
  await session.key("Tab");
  if (!(await focused("focus-player")) || !(await keyboard())) throw new Error("Tab did not give the player keyboard focus");
  await until("the keyboard video button", 5000, shown);
  await idle();
  if (!(await shown())) throw new Error("keyboard-focused player lost its video button");
  await session.key("Tab");
  if (!(await session.evaluate('document.activeElement?.localName === "mdm-video-button"'))) throw new Error("Tab skipped the video button");
  await idle();
  if (!(await shown())) throw new Error("keyboard-focused video button faded");
  await session.key("Tab"); // Back through the player to the next page control.
  if (!(await focused("after-player"))) throw new Error("Tab could not leave the video button");
  await until("the video button hidden after focus leaves", 5000, async () => !(await shown()));

  // Return to the player by mouse after using the keyboard.
  await session.mouse((v.l + v.r) / 2, (v.t + v.b) / 2, "click");
  if (await keyboard()) throw new Error("mouse click did not clear keyboard focus");
  await until("the player button again", 5000, shown);
  await idle();
  if (await shown()) throw new Error("switching to the mouse kept the button visible");
  // Hover shows it again; Tab from a clicked player must still reach the button.
  await session.mouse((v.l + v.r) / 2 + 1, (v.t + v.b) / 2);
  await until("the hovered video button", 5000, shown);
  await session.key("Tab");
  if (!(await session.evaluate('document.activeElement?.localName === "mdm-video-button"'))) throw new Error("Tab from a mouse-focused player skipped the video button");
  return "mouse focus fades in normal view and fullscreen; keyboard focus stays, Tab enters and leaves, switching to the mouse fades again";
}

// Point at the video (rectangle `v`) until the button shows; its rectangle, or null.
async function hover(session, v) {
  for (let i = 0; i < 40; i++) {
    await session.mouse((v.l + v.r) / 2 + (i % 2), (v.t + v.b) / 2);
    const b = await rect(session, "mdm-video-button");
    if (b) return b;
    await sleep(200);
  }
  return null;
}

// The button's menu opened with its arrow, and its "All qualities and subtitles…" clicked, as a
// user does, which opens the app's Download video window (#703: what the button's click did
// before). Once the video's look-up ended (its rows came): then "All qualities…" stays where it
// is. "menu" once the window opened.
async function openInMdm(session, pid, b = null) {
  // The button again first, when it faded meanwhile (1.5 s after the pointer rests): the pointer
  // moves over its video, just under it.
  const more = await until("the button's arrow", 8000, async () => {
    const at = await session.pill(MORE_AT);
    if (!at && b) await session.mouse((b.l + b.r) / 2 + (Date.now() % 2), b.b + 24);
    return at;
  });
  await clickArrow(session);
  for (let i = 0; i < 5; i += 1) {
    // Its look-up ended (the lines below "Download best" came), or, while it still runs (a site
    // yt-dlp takes long over), as it is: "All qualities…" stays where it is until it ends.
    const m = await until("the menu, its look-up ended", 30000, async () => {
      const p = await session.picks("open");
      return p && !p.busy ? p : null;
    }).catch(() => session.picks("open"));
    if (!m) throw new Error("the menu didn't open");
    await session.mouse(m.open.x, m.open.y, "click");
    const opened = await until("the Download video window", 3000, () => appWindows(pid).includes(VIDEO_WINDOW)).catch(() => false);
    if (opened) return "menu";
    if (!(await session.picks())) await clickArrow(session);
  }
  throw new Error(`"All qualities and subtitles…" opened no window`);
}

// Where the button's main part is (its .go, read through its closed shadow root: at its left
// end, at its right end in a right-to-left language); its left end when it can't be read.
const GO_AT = `function () {
  const g = this.querySelector(".go");
  const r = g && g.getBoundingClientRect();
  return JSON.stringify(r ? { x: r.left + Math.min(14, r.width / 2), w: r.width } : null);
}`;
const mainPart = async (session, b) => {
  const go = session.pill ? await session.pill(GO_AT).catch(() => null) : null;
  return go && go.w ? go.x : b.l + 14;
};

// The button's menu, then "All qualities and subtitles…" in it (openInMdm): the app's Download
// video window opens; the address it shows.
// `shot`: a picture of that window first (PrintWindow: off the screens too), for a PR's body.
// `open`: the window is left open (closed otherwise).
async function clickButton(session, b, shot = "", open = false) {
  const pid = appPid();
  appWindows(pid, VIDEO_WINDOW);
  await until("the Download video window closed", 5000, () => !appWindows(pid).includes(VIDEO_WINDOW));
  await openInMdm(session, pid, b);
  await until("the Download video window", 15000, () => appWindows(pid).includes(VIDEO_WINDOW));
  const address = await until("the Download video window's address", 10000, () => videoAddress(pid));
  if (shot) {
    await sleep(1500);
    spawnSync("powershell", ["-NoProfile", "-File", path.join(root, "tools", "lab", "cap-own.ps1"), "-ProcessId", String(pid), "-Title", VIDEO_WINDOW, "-Out", shot]);
  }
  if (!open) appWindows(pid, VIDEO_WINDOW);
  return address;
}

// Point at a video, see the button at its top right, click it (its qualities' menu, then "Open
// in…": the app's video window opens), drag it to the bottom left (it stays there after a
// reload; a drag opens neither); `never`: its menu's "Never on this site" by keyboard, then it no
// longer shows and the app lists the site.
async function videoButton(session, never) {
  const page = `${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`;
  await session.navigate(page);
  const v = await rect(session, "#video");
  let b = await hover(session, v);
  if (!b) throw new Error("no button over the video");
  near(b.r, v.r - 8, "button's right edge");
  near(b.t, v.t + 8, "button's top");
  // #703: a click on its main part downloads the video's own plain file at once (no look-up, no
  // window), then says so in green, with "Show" (MDM's window, the download selected). An earlier
  // browser's download of it off the list first: a duplicate would be asked about.
  const earlier = (await mdmList()).filter((d) => d.url === `${LAB}/gen/1M/clip.mp4`).map((d) => d.id);
  if (earlier.length) await ipc({ action: "delete", ids: earlier });
  const before = new Set((await mdmList()).map((d) => d.id));
  await session.mouse(await mainPart(session, b), (b.t + b.b) / 2, "click");
  const added = await until("the video's file in MDM's list", 15000, async () => (await mdmList()).find((d) => !before.has(d.id))).catch(async (e) => {
    throw new Error(`${e.message} (the button: ${JSON.stringify(await session.pill(PILL_STATE))})`);
  });
  if (added.url !== `${LAB}/gen/1M/clip.mp4`) throw new Error(`MDM downloads ${added.url}`);
  const sent = await until("the button's green state", 5000, async () => {
    const p = await session.pill(PILL_STATE);
    return p && p.state === "sent" ? p : null;
  });
  const show = sent.acts.find((a) => a.act === "show");
  if (!show) throw new Error(`no "Show" on the button: ${JSON.stringify(sent)}`);
  if (appWindows(appPid()).includes(VIDEO_WINDOW)) throw new Error("the click opened the Download video window");
  await session.mouse(show.x, show.y, "click");
  await until("MDM's main window, shown", 10000, () => appWindows(appPid()).some((x) => /^Mister Download Manager/.test(x)));
  await session.mouse(5, 5);
  await sleep(300);
  b = await hover(session, v);
  // Its menu's "All qualities and subtitles…": the app's Download video window for the file.
  const address = await clickButton(session, b);
  if (address !== `${LAB}/gen/1M/clip.mp4`) throw new Error(`the video window got ${address}`);
  // Drag it to the bottom left: it snaps to that corner. (Shown again first: it fades 1.5 s after
  // the pointer rests, #741, and the window's steps take longer.)
  b = await hover(session, v);
  await session.mouse(b.l + 14, (b.t + b.b) / 2, "down");
  await session.mouse(b.l + 30, (b.t + b.b) / 2 + 20);
  await session.mouse(v.l + 40, v.b - 30);
  await session.mouse(v.l + 40, v.b - 30, "up");
  await sleep(300);
  b = await rect(session, "mdm-video-button");
  near(b.l, v.l + 8, "dragged: left edge");
  near(b.b, v.b - 8, "dragged: bottom");
  if (appWindows(appPid()).includes(VIDEO_WINDOW)) throw new Error("a drag opened the video window");
  if (await session.picks()) throw new Error("a drag opened the qualities' menu");
  // Remembered for the site.
  await session.mouse(5, 5);
  await session.navigate(page);
  b = await hover(session, v);
  if (!b) throw new Error("no button after a reload");
  near(b.l, v.l + 8, "after a reload: left edge");
  near(b.b, v.b - 8, "after a reload: bottom");
  if (!never) return "shown, clicked (its file downloaded, Show), its menu's window, dragged, kept";
  const listed = () => {
    try {
      const s = JSON.parse(fs.readFileSync(path.join(data, "downloads.json"), "utf8").replace(/^\uFEFF/, "")).settings;
      return (s.video_panel_sites || "").split(" ").includes("127.0.0.1");
    } catch {
      return false;
    }
  };
  // Its menu by keyboard: open (its arrow), to "Never on 127.0.0.1" (the last but one), Enter.
  const neverHere = async () => {
    await clickArrow(session);
    await sleep(200);
    await session.key("End");
    await session.key("ArrowUp");
    await session.key("Enter");
    // Grey "Off on this site", with "Turn on".
    return until("the button's off state", 5000, async () => {
      const p = await session.pill(PILL_STATE);
      return p && p.state === "off" ? p : null;
    });
  };
  let off = await neverHere();
  await until("the site in the app's list", 15000, listed);
  // "Turn on": the site off the app's list again.
  const on = off.acts.find((a) => a.act === "on");
  if (!on) throw new Error(`no "Turn on": ${JSON.stringify(off)}`);
  await session.mouse(on.x, on.y, "click");
  await until("the site off the app's list", 15000, () => !listed());
  b = await hover(session, v);
  if (!b) throw new Error("no button after Turn on");
  off = await neverHere();
  await until("the site in the app's list", 15000, listed);
  // Its state shown a moment, then gone once the pointer leaves.
  await session.mouse(5, 5);
  await until("the button gone", 10000, async () => !(await rect(session, "mdm-video-button")));
  await session.navigate(page);
  if (await hover(session, v)) throw new Error("the button still shows on a site it should never show on");
  return "shown, clicked (its file downloaded, Show), its menu's window, dragged, kept, never on the site, turned on, never again";
}

// ---- #689: a video in a feed sends its post ----------------------------------------------------

// The sample feeds (our own pages, reduced to what names a post on each site), served at their
// sites' own addresses by the browser's protocol (nothing reaches those sites).
const FEEDS = path.join(root, "tools", "extension", "feeds");
// What the button says when nothing names the video, in the browser's language (English, or
// Chromium's --lang in E2E_CHROMIUM_ARGS).
const OPEN_POST = (t) => {
  const lang = (t.kind === "chromium" && /--lang=([\w-]+)/.exec(process.env.E2E_CHROMIUM_ARGS || "")) || [];
  const file = (l) => path.join(root, "extension", "_locales", l, "messages.json");
  const at = lang[1] && fs.existsSync(file(lang[1].replace("-", "_"))) ? file(lang[1].replace("-", "_")) : file("en");
  return JSON.parse(fs.readFileSync(at, "utf8")).video_open_post.message;
};
// The page's videos in the sample pages' order (test.mjs's): into open shadow roots too.
const VIDEOS = `(() => { const all = []; const walk = (n) => { for (const c of n.children) { if (c.localName === "video") all.push(c);
  if (c.shadowRoot) walk(c.shadowRoot); walk(c); } }; walk(document); return all; })()`;
// What the button says now (its label, and whether that is an error), in its closed shadow root.
const PILL_LABEL = `function () {
  const p = this.querySelector(".pill");
  const l = this.querySelector(".label");
  return JSON.stringify({ text: l ? l.textContent : "", err: Boolean(p && ["warn", "err", "off"].includes(p.dataset.state)) });
}`;

// Point at the video (rectangle `v`) until the button shows over it (not over the last one); its
// rectangle, or null.
async function buttonOver(session, v) {
  for (let i = 0; i < 40; i++) {
    await session.mouse((v.l + v.r) / 2 + (i % 2), (v.t + v.b) / 2);
    const b = await rect(session, "mdm-video-button");
    if (b && b.t >= v.t - 1 && b.b <= v.b + 1 && b.l >= v.l - 1 && b.r <= v.r + 1) {
      // Settled there (it fades in and follows the video).
      await sleep(400);
      const again = await rect(session, "mdm-video-button");
      if (again && Math.abs(again.l - b.l) < 2 && Math.abs(again.t - b.t) < 2) return again;
    }
    await sleep(200);
  }
  return null;
}

// Each video of each sample page hovered and its button clicked: the app's Download video window
// opens on what the page names for it (its post, or the page when it is the video's own), or on
// its own plain file when nothing names it; with neither, no window, and the button says to open
// the post. Never the feed.
async function feedVideos(t) {
  const s = t.session;
  // E2E_FEEDS: only these sample pages (comma separated names, "linkedin,x").
  const only = (process.env.E2E_FEEDS || "").split(",").filter(Boolean);
  const pages = fs
    .readdirSync(FEEDS)
    .filter((f) => f.endsWith(".html") && (!only.length || only.includes(f.replace(/\.html$/, ""))))
    .map((f) => {
      const html = fs.readFileSync(path.join(FEEDS, f), "utf8");
      return { f, html, page: /<meta name="mdm-page" content="([^"]+)"/.exec(html)[1].replaceAll("&amp;", "&") };
    });
  const css = fs.readFileSync(path.join(FEEDS, "feeds.css"), "utf8");
  // media.example.test: a video's own plain file (social.html), never fetched from a network
  // either (another download manager there would take it).
  await s.serve([...new Set(pages.map((p) => new URL(p.page).hostname)), "media.example.test"], (url) => {
    const u = new URL(url);
    if (u.pathname.endsWith("/feeds.css")) return { status: 200, type: "text/css", body: css };
    if (u.hostname === "media.example.test") return { status: 200, type: "video/mp4", body: "" };
    const p = pages.find((x) => new URL(x.page).host === u.host && new URL(x.page).pathname === u.pathname);
    return p ? { status: 200, type: "text/html; charset=utf-8", body: p.html } : { status: 404, type: "text/plain", body: "not a sample page" };
  });
  const pid = appPid();
  const openPost = OPEN_POST(t);
  const done = [];
  let again = 0;
  // E2E_FEEDS_SHOT: a folder for a PR's pictures, from the LinkedIn sample: the app's window for
  // its first video, the button's answer for one nothing names.
  const shots = process.env.E2E_FEEDS_SHOT || "";
  const shot = (f, i, what) => (shots && f === "linkedin.html" && (what === "told" || i === 0) ? path.join(shots, `${t.name}-${what}.png`) : "");
  if (shots) fs.mkdirSync(shots, { recursive: true });
  for (const { f, page } of pages) {
    await s.navigate(page);
    const count = await s.evaluate(`${VIDEOS}.length`);
    if (!count) throw new Error(`${f}: no video on the page (served?)`);
    let posts = 0;
    let files = 0;
    let told = 0;
    for (let i = 0; i < count; i++) {
      const want = await s.evaluate(`(() => { const v = ${VIDEOS}[${i}]; v.scrollIntoView({ block: "center" });
        const e = v.getAttribute("data-expect");
        return e === "page" ? location.href : e === "none" ? (/^https?:/.test(v.currentSrc) ? v.currentSrc : null) : e.replaceAll("{origin}", location.origin); })()`);
      await sleep(150);
      const v = await s.evaluate(`(() => { const r = ${VIDEOS}[${i}].getBoundingClientRect(); return { l: r.left, t: r.top, r: r.right, b: r.bottom }; })()`);
      let b = await buttonOver(s, v);
      if (!b) throw new Error(`${f}: no button over video ${i + 1}`);
      if (want) {
        // What the button says meanwhile, for the report when no window opens.
        // A click the button never took (it said nothing: headless Firefox now and then drops
        // one) is made once more, and counted; one it answered without a window fails.
        let got = "";
        for (let attempt = 0; !got; attempt++) {
          const said = new Set();
          const end = Date.now() + 5000;
          const watching = (async () => {
            while (Date.now() < end) {
              const l = await s.pill(PILL_LABEL).catch(() => null);
              if (l) said.add(l.text);
              await sleep(100);
            }
          })();
          got = await clickButton(s, b, shot(f, i, "window")).catch(async (e) => {
            await watching;
            if (attempt === 0 && said.size <= 1) return "";
            throw new Error(`${f}: video ${i + 1} (${want}): ${e.message} (the button said: ${[...said].join(" | ")})`);
          });
          if (!got) {
            again += 1;
            await s.mouse(2, 2);
            b = await buttonOver(s, v);
            if (!b) throw new Error(`${f}: no button over video ${i + 1} again`);
          }
        }
        if (got !== want) throw new Error(`${f}: video ${i + 1} sent ${got}, not ${want}`);
        /^http:\/\/media\.example\.test\//.test(got) ? (files += 1) : (posts += 1);
      } else {
        appWindows(pid, VIDEO_WINDOW);
        await s.mouse(await mainPart(s, b), (b.t + b.b) / 2, "click");
        const said = await until(`${f}: video ${i + 1}'s answer on the button`, 10000, async () => {
          const l = await s.pill(PILL_LABEL);
          return l && l.err ? l.text : "";
        });
        if (shot(f, i, "told") && s.screenshot) await s.screenshot(shot(f, i, "told"));
        if (said !== openPost) throw new Error(`${f}: video ${i + 1} said "${said}"`);
        await sleep(1000);
        if (appWindows(pid).includes(VIDEO_WINDOW)) throw new Error(`${f}: video ${i + 1} opened the video window`);
        told += 1;
        // Its answer shown to the end (video.js flash), as a person would see it.
        await sleep(1500);
      }
      await s.mouse(2, 2);
    }
    done.push(`${f.replace(/\.html$/, "")} ${[posts && `${posts} named`, files && `${files} file`, told && `${told} told`].filter(Boolean).join("/")}`);
  }
  return `each video's own address, its file, or "${openPost}": ${done.join(", ")}${again ? ` (${again} click(s) the button never took, made again)` : ""}`;
}

// How long the page and the extension keep the app's "may it show here" (video.js ASK_MS,
// background.js PANEL_KEEP_MS), with room for the question.
const PANEL_KEPT_MS = 5000 + 1500;

// Options → General → Download panels, changed as its OK does: the settings saved in
// downloads.json, which the host reads at each question. Changed with the app stopped, so its own
// next save doesn't put them back.
async function panelSettings(change) {
  await stopApp();
  const file = path.join(data, "downloads.json");
  const saved = JSON.parse(fs.readFileSync(file, "utf8").replace(/^﻿/, ""));
  Object.assign(saved.settings, change);
  fs.writeFileSync(file, JSON.stringify(saved));
  await startApp();
}

// #532: the button follows Options in a tab already open (no reload: YouTube never reloads),
// switched off, on again, and off on the site; and after a reload.
async function videoButtonFollowsOptions(session) {
  const page = `${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`;
  await session.navigate(page);
  const v = await rect(session, "#video");
  const away = async () => {
    await session.mouse(5, 5);
    await sleep(PANEL_KEPT_MS);
  };
  const done = [];
  try {
    if (!(await hover(session, v))) throw new Error("no button over the video");
    await panelSettings({ video_panel: false });
    await away();
    if (await hover(session, v)) throw new Error("switched off in Options: the button still shows in the open tab");
    done.push("off: gone in the open tab");
    await session.mouse(5, 5);
    await session.navigate(page);
    if (await hover(session, v)) throw new Error("switched off in Options: the button shows after a reload");
    done.push("and after a reload");
    await panelSettings({ video_panel: true });
    await away();
    if (!(await hover(session, v))) throw new Error("switched on again: no button in the open tab");
    done.push("on again: back in the open tab");
    await panelSettings({ video_panel_sites: "127.0.0.1" });
    await away();
    if (await hover(session, v)) throw new Error("the site listed in Options: the button still shows in the open tab");
    done.push("the site listed: gone in the open tab");
  } finally {
    await session.mouse(5, 5);
    await panelSettings({ video_panel: true, video_panel_sites: "" });
  }
  return done.join("; ");
}

// ---- the cases ---------------------------------------------------------------------------------

class Skip extends Error {}

// A forward proxy on 127.0.0.1 (#42): plain requests in absolute form and CONNECT tunnels, the
// name lab.mdm.test sent to the lab server, every request logged ({method, target}).
function startProxy() {
  const to = (host, port) => (host === PROXY_HOST ? ["127.0.0.1", PORT] : [host, Number(port) || 80]);
  const server = http.createServer((req, res) => {
    // Its PAC script, as a VPN extension may set one by its address (Chromium's pac_script.url).
    if (req.url === "/mdm.pac") {
      res.writeHead(200, { "content-type": "application/x-ns-proxy-autoconfig" }).end(PAC);
      return;
    }
    proxyLog.push({ method: req.method, target: req.url });
    let u;
    try {
      u = new URL(req.url);
    } catch {
      res.writeHead(400).end();
      return;
    }
    const [host, port] = to(u.hostname, u.port);
    const up = http.request({ host, port, method: req.method, path: u.pathname + u.search, headers: req.headers }, (r) => {
      res.writeHead(r.statusCode || 502, r.headers);
      r.pipe(res);
    });
    up.on("error", () => (res.headersSent ? res.destroy() : res.writeHead(502).end()));
    req.pipe(up);
  });
  server.on("connect", (req, client, head) => {
    proxyLog.push({ method: "CONNECT", target: req.url });
    const [h, p] = req.url.split(":");
    const [host, port] = to(h, p);
    const up = net.connect(port, host, () => {
      client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
      up.write(head);
      up.pipe(client);
      client.pipe(up);
    });
    up.on("error", () => client.destroy());
    client.on("error", () => up.destroy());
  });
  server.listen(PROXY_PORT, "127.0.0.1");
  return server;
}

// A link on a page, clicked as a user does.
async function clickLink(t, link) {
  await t.session.navigate(`${LAB}/page?link=${encodeURIComponent(link)}`);
  await t.session.evaluate("document.getElementById('link').click()");
}

// The requests MDM made for `p`: all but the browser's own first one.
async function mdmRequests(p) {
  const all = await requests(p);
  if (all.length < 2) throw new Error(`${p}: only the browser's request reached the server (${all.map(brief).join("; ")})`);
  return all.slice(1);
}

// One file through the lab's host (lab.mdm.test, known to the test proxy only): how many of its
// requests went through the proxy (the browser's own first; MDM's when it got the proxy).
async function throughProxy(t, label, wantMdm) {
  proxyLog.length = 0;
  const file = `e2e-proxy-${t.name}-${label}.xz`;
  await clickLink(t, `http://${PROXY_HOST}:${PORT}/gen/1M/${file}`);
  if (wantMdm) await arrived(file, 1 << 20, "");
  else await sleep(8000);
  await keptNothing(t.dl);
  return proxyLog.filter((e) => e.target.includes(file)).length;
}

// With the switch on and a proxy set: an ordinary .zip link on the lab's own address (127.0.0.1,
// which the browser itself never sends to a proxy) still reaches MDM. How many of its requests
// went through the proxy.
async function plainWhileProxied(t) {
  proxyLog.length = 0;
  const file = `e2e-proxyon-${t.name}.zip`;
  await clickLink(t, `/gen/1M/${file}`);
  await arrived(file, 1 << 20);
  await keptNothing(t.dl);
  return proxyLog.filter((e) => e.target.includes(file)).length;
}

// A stand-in VPN extension: it has the "proxy" permission and sets the browser's proxy
// (chrome.proxy.settings.set, from its page), as real VPN and proxy extensions do.
function vpnExtension() {
  const dir = path.join(work, "vpn-extension");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({ manifest_version: 3, name: "MDM e2e stand-in VPN", version: "1.0", permissions: ["proxy"] }),
  );
  // Its page, driven over DevTools by e2e.mjs.
  fs.writeFileSync(path.join(dir, "page.html"), "<!doctype html><title>stand-in VPN</title>");
  return dir;
}

// #578's stand-in for an extension that saves files in folders of its own (Save In): it starts
// a download with `downloads.download` and a `filename` in a folder. Its page, driven over DevTools.
function folderExtension() {
  const dir = path.join(work, "folder-extension");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({ manifest_version: 3, name: "MDM e2e stand-in Save In", version: "1.0", permissions: ["downloads"], host_permissions: ["<all_urls>"] }),
  );
  fs.writeFileSync(path.join(dir, "page.html"), "<!doctype html><title>stand-in Save In</title>");
  return dir;
}

// Firefox's stand-in VPN add-on: it sends the lab's host (lab.mdm.test) through the test proxy
// per request (proxy.onRequest), leaving the browser's proxy settings alone.
function firefoxVpnExtension() {
  const dir = path.join(work, "vpn-addon");
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, "manifest.json"),
    JSON.stringify({
      manifest_version: 2,
      name: "MDM e2e stand-in VPN",
      version: "1.0",
      permissions: ["proxy", "<all_urls>"],
      background: { scripts: ["background.js"] },
      browser_specific_settings: { gecko: { id: "vpn-e2e@misterdownloadmanager.com" } },
    }),
  );
  fs.writeFileSync(
    path.join(dir, "background.js"),
    `browser.proxy.onRequest.addListener((d) => (new URL(d.url).hostname === ${JSON.stringify(PROXY_HOST)} ? { type: "http", host: "127.0.0.1", port: ${PROXY_PORT} } : { type: "direct" }), { urls: ["<all_urls>"] });`,
  );
  return dir;
}

// #42 in Chrome and the other Chromium browsers: "proxy" is required there (Chrome can't make it
// optional), and the popup's switch is the extension's own setting, off as installed. A stand-in
// VPN extension sets the browser's proxy: a fixed server, then a PAC script (inline, then by its
// address). Off, MDM gets nothing of it (only the browser's request goes through the proxy); on,
// MDM downloads through the same proxy. Also: what Chrome warns about on install and update is
// the same with and without "proxy" (Chrome counts it as access to every site, which the
// extension already has), so the update doesn't switch the extension off to ask.
async function proxyInChromium(t) {
  const s = t.session;
  const done = [];
  const ext = JSON.parse(
    await s.inExtension(`(async () => {
      const m = chrome.runtime.getManifest();
      // As a manifest on its own: its name in plain text, no locale folder to read.
      const plain = (perms, hosts = m.host_permissions) => JSON.stringify({ manifest_version: 3, name: "MDM", version: m.version, permissions: perms, host_permissions: hosts });
      const warn = (...a) => new Promise((r) => chrome.management.getPermissionWarningsByManifest(plain(...a), r));
      const without = m.permissions.filter((p) => p !== "proxy");
      return JSON.stringify({ api: typeof chrome.proxy, perms: m.permissions, optional: m.optional_permissions || [], with: await warn(m.permissions), without: await warn(without), alone: await warn(["proxy"], []) });
    })()`),
  );
  if (ext.api !== "object" || !ext.perms.includes("proxy") || ext.optional.includes("proxy")) throw new Error(`Chrome's proxy permission: ${JSON.stringify(ext)}`);
  if (JSON.stringify(ext.with) !== JSON.stringify(ext.without)) throw new Error(`"proxy" adds a warning: ${JSON.stringify(ext.with)} vs ${JSON.stringify(ext.without)}`);
  // The same check is no proof if Chrome said nothing for "proxy" at all: on its own, it warns.
  if (!ext.alone.length || !ext.with.includes(ext.alone[0])) throw new Error(`"proxy" alone: ${JSON.stringify(ext.alone)}`);
  done.push(`"proxy" required; Chrome's warnings with it ${JSON.stringify(ext.with)}, the same as without it ("proxy" alone: ${JSON.stringify(ext.alone)})`);

  const vpnPage = await s.loadExtension(vpnExtension(), "page.html");
  const vpn = vpnPage.evaluate;
  const setProxy = async (value) => {
    await vpn(`chrome.proxy.settings.set({ value: ${JSON.stringify(value)}, scope: "regular" }).then(() => "set")`);
    await sleep(500);
    // What our extension then reads: the other extension's.
    const seen = JSON.parse(await s.inExtension(`chrome.proxy.settings.get({}).then((d) => JSON.stringify(d))`));
    if (seen.levelOfControl !== "controlled_by_other_extensions" || seen.value.mode !== value.mode) throw new Error(`the browser's proxy as read: ${JSON.stringify(seen)}`);
    return seen;
  };
  const fixed = { mode: "fixed_servers", rules: { singleProxy: { scheme: "http", host: "127.0.0.1", port: PROXY_PORT } } };
  const popup = await s.tab(`${s.base}popup.html`);
  try {
    await setProxy(fixed);
    await until("the popup", 10000, () =>
      popup.evaluate(`location.href.startsWith(${JSON.stringify(s.base)}) && document.readyState === "complete" && Boolean(document.getElementById("proxy-row"))`).catch(() => false),
    );
    await sleep(300);
    // As installed: the switch shown, off; nothing handed over.
    const shown = await popup.evaluate("[!document.getElementById('proxy-row').hidden, document.getElementById('proxy').checked, document.getElementById('proxy-row').title]");
    if (!shown[0] || shown[1]) throw new Error(`the switch as installed: ${JSON.stringify(shown)} (want shown, off)`);
    const off = await throughProxy(t, "off", false);
    if (off !== 1) throw new Error(`as installed: ${off} requests through the proxy (want the browser's only)`);
    done.push(`as installed: the switch shown, off; a VPN extension's proxy (controlled_by_other_extensions): the browser's request only`);

    // On, as a user clicks it: the setting stored, no permission asked.
    await popup.evaluate("document.getElementById('proxy').click()");
    await until("the switch on", 5000, async () => (await popup.evaluate("chrome.storage.local.get('proxyOn').then((v) => v.proxyOn === true)")));
    const on = await throughProxy(t, "on", true);
    if (on < 2) throw new Error(`on: only ${on} request(s) through the proxy`);
    done.push(`on, a fixed server: ${on} requests through it (the browser's, then MDM's)`);
    const local = await plainWhileProxied(t);
    if (local) throw new Error(`on: ${local} request(s) for 127.0.0.1 through the proxy (Chromium sends none there)`);
    done.push("on: an ordinary link on 127.0.0.1 taken, straight (the proxy bypassed, as the browser does)");

    await setProxy({ mode: "pac_script", pacScript: { data: PAC } });
    const pacData = await throughProxy(t, "pac-data", true);
    if (pacData < 2) throw new Error(`a PAC script (inline): only ${pacData} request(s) through the proxy`);
    done.push(`an inline PAC script: ${pacData} requests through its proxy`);

    await setProxy({ mode: "pac_script", pacScript: { url: `http://127.0.0.1:${PROXY_PORT}/mdm.pac` } });
    const pacUrl = await throughProxy(t, "pac-url", true);
    if (pacUrl < 2) throw new Error(`a PAC script (by its address): only ${pacUrl} request(s) through the proxy`);
    done.push(`a PAC script by its address: ${pacUrl} requests through its proxy`);

    // Off again: nothing more of it goes.
    await setProxy(fixed);
    await popup.evaluate("document.getElementById('proxy').click()");
    await until("the switch off", 5000, async () => (await popup.evaluate("chrome.storage.local.get('proxyOn').then((v) => v.proxyOn === false)")));
    const after = await throughProxy(t, "off-again", false);
    if (after !== 1) throw new Error(`switched off: ${after} requests through the proxy`);
    done.push("switched off: the browser's request only");
  } finally {
    await popup.close();
    await vpn(`chrome.proxy.settings.clear({ scope: "regular" }).then(() => "cleared")`).catch(() => {});
    await vpnPage.close();
  }
  // An ordinary download goes as before, with no proxy set.
  const file = `e2e-noproxy-${t.name}.zip`;
  await clickLink(t, `/gen/1M/${file}`);
  await arrived(file, 1 << 20);
  await keptNothing(t.dl);
  const errors = await s.workerErrors();
  if (errors.length) throw new Error(`the service worker's console: ${errors.join(" | ")}`);
  done.push("no proxy: a download as before; no error in the service worker's console");
  return done.join("; ");
}

const CASES = {
  // #578: a download another extension starts in a folder of its own, of a type MDM doesn't
  // take, lands in that folder. Chromium only (Firefox has no onDeterminingFilename): MDM's
  // listener answers for every download, and an answer that names nothing must not empty the
  // other extension's name.
  async folders(t) {
    if (t.kind !== "chromium") throw new Skip("Chromium's onDeterminingFilename only");
    const page = await t.session.loadExtension(folderExtension(), "page.html", "downloads");
    try {
      const file = `e2e-folders-${t.name}.png`;
      const sub = path.join("Pic", "OthersW");
      const id = await page.evaluate(`chrome.downloads.download({ url: ${JSON.stringify(`${LAB}/gen/64K/${file}`)}, filename: ${JSON.stringify(`Pic/OthersW/${file}`)} })`);
      const want = path.join(t.dl, sub, file);
      const where = () => {
        const found = [];
        const walk = (d) => fs.readdirSync(d, { withFileTypes: true }).forEach((e) => (e.isDirectory() ? walk(path.join(d, e.name)) : found.push(path.relative(t.dl, path.join(d, e.name)))));
        walk(t.dl);
        return found;
      };
      try {
        await until(`${file} in the stand-in's folder`, 20000, () => fs.existsSync(want) && fs.statSync(want).size === 64 << 10);
      } catch {
        throw new Error(`the browser saved it as ${JSON.stringify(where())} (want ${path.join(sub, file)})`);
      }
      const mdm = (await mdmList().catch(() => [])).filter((d) => d.file === file);
      if (mdm.length) throw new Error(`MDM took it too: ${JSON.stringify(mdm)}`);
      const got = (await requests(`/gen/64K/${file}`)).map(brief);
      for (const f of fs.readdirSync(t.dl)) fs.rmSync(path.join(t.dl, f), { force: true, recursive: true });
      return `download ${id} saved in ${path.join(sub, file)}, MDM left it (the lab's log: ${got.join("; ")})`;
    } finally {
      await page.close();
    }
  },
  // #42: the browser behind a proxy (a VPN or proxy extension's). The file's host (lab.mdm.test) is
  // known to that proxy only, so MDM can download it only through the same proxy, which the
  // extension hands over with the download, once the user switched that on: the popup's switch
  // asks for the optional "proxy" permission (Firefox). Chrome can't make it optional, so its
  // extension has it from the install and the switch is its own setting (proxyInChromium).
  async proxy(t) {
    if (!t.session.inBrowser) return proxyInChromium(t);
    const s = t.session;
    const pref = (k, v) => s.inBrowser(`(Services.prefs.${typeof v === "number" ? "setIntPref" : typeof v === "boolean" ? "setBoolPref" : "setStringPref"}(${JSON.stringify(k)}, ${JSON.stringify(v)}), "set")`);
    // The browser's proxy, as a VPN extension sets it (Firefox's own setting: 127.0.0.1 itself
    // stays direct).
    await pref("network.proxy.http", "127.0.0.1");
    await pref("network.proxy.http_port", PROXY_PORT);
    await pref("network.proxy.type", 1);
    const popupUrl = `${s.base}popup.html`;
    const granted = async (popup) => popup.evaluate("browser.permissions.contains({ permissions: ['proxy'] })");
    // One file through the lab's host: how many of its requests went through the proxy (the
    // browser's own first; MDM's when it got the proxy) after `wait` ms.
    const fetchFile = async (label, wantMdm) => {
      proxyLog.length = 0;
      const file = `e2e-proxy-${t.name}-${label}.xz`;
      await clickLink(t, `http://${PROXY_HOST}:${PORT}/gen/1M/${file}`);
      if (wantMdm) await arrived(file, 1 << 20, "");
      else await sleep(8000);
      await keptNothing(t.dl);
      return proxyLog.filter((e) => e.target.includes(file)).length;
    };
    const done = [];
    let perRequest = "";
    try {
      const popup = await s.tab(popupUrl);
      try {
        await until("the popup", 10000, () => popup.evaluate(`document.readyState === "complete" && Boolean(document.getElementById("proxy-row"))`).catch(() => false));
        await sleep(300);
        // As installed: the switch shown, off; nothing handed over.
        const shown = await popup.evaluate("[!document.getElementById('proxy-row').hidden, document.getElementById('proxy').checked]");
        if (!shown[0] || shown[1]) throw new Error(`the switch as installed: ${JSON.stringify(shown)} (want shown, off)`);
        if (await granted(popup)) throw new Error("the permission is granted as installed");
        const off = await fetchFile("off", false);
        if (off !== 1) throw new Error(`as installed: ${off} requests through the proxy (want the browser's only)`);
        done.push("as installed: off, the browser's request only");

        // Refused: the browser asks (its prompt), the user says no.
        await pref("extensions.webextOptionalPermissionPrompts", true);
        await popup.click("#proxy");
        const ids = await until("the browser's permission prompt", 10000, () =>
          s.inBrowser(`PopupNotifications._currentNotifications.map((n) => n.id)`).then((l) => l.length && l),
        );
        const refused = await s.inBrowser(`(() => {
          const n = PopupNotifications._currentNotifications.find((x) => /permissions/.test(x.id));
          if (!n) return "no prompt";
          n.secondaryActions[0].callback();
          PopupNotifications.remove(n);
          return "refused";
        })()`);
        if (refused !== "refused") throw new Error(`the prompt (${JSON.stringify(ids)}): ${refused}`);
        await until("the switch back off", 5000, async () => !(await popup.evaluate("document.getElementById('proxy').checked")));
        if (await granted(popup)) throw new Error("refused, yet granted");
        const no = await fetchFile("refused", false);
        if (no !== 1) throw new Error(`refused: ${no} requests through the proxy`);
        done.push(`refused at the prompt (${ids.join(", ")}): off, the browser's request only`);

        // Granted: the switch on, MDM downloads through the same proxy.
        await pref("extensions.webextOptionalPermissionPrompts", false);
        await popup.click("#proxy");
        await until("the permission granted", 10000, () => granted(popup));
        await until("the switch on", 5000, () => popup.evaluate("document.getElementById('proxy').checked"));
        const on = await fetchFile("on", true);
        if (on < 2) throw new Error(`granted: only ${on} request(s) through the proxy`);
        done.push(`granted: ${on} requests through the browser's proxy (the browser's, then MDM's)`);
        // An ordinary link on 127.0.0.1 is still taken (Firefox itself never proxies it; MDM
        // may, the test proxy passes it on: said, not failed).
        const local = await plainWhileProxied(t);
        done.push(`granted: an ordinary link on 127.0.0.1 taken (${local} of its requests through the proxy)`);

        // Granted, the proxy set per request by a VPN extension (proxy.onRequest, as Firefox's
        // proxy and VPN add-ons do) rather than in the browser's settings: MDM gets that one too.
        await pref("network.proxy.type", 0);
        // A failure here is said at the end, after the other steps.
        const vpn = await s.install(firefoxVpnExtension());
        try {
          await sleep(1000);
          const per = await fetchFile("onrequest", true);
          if (per < 2) throw new Error(`only ${per} request(s) through it`);
          done.push(`granted, a VPN add-on's per-request proxy (proxy.onRequest): ${per} requests through it`);
        } catch (e) {
          perRequest = `a VPN add-on's per-request proxy (proxy.onRequest): ${e.message}`;
          for (const f of fs.readdirSync(t.dl)) fs.rmSync(path.join(t.dl, f), { force: true, recursive: true });
        } finally {
          await s.uninstall(vpn).catch(() => {});
          await pref("network.proxy.type", 1);
        }

        // Revoked: the switch off again.
        await popup.click("#proxy");
        await until("the permission removed", 10000, async () => !(await granted(popup)));
        const after = await fetchFile("revoked", false);
        if (after !== 1) throw new Error(`revoked: ${after} requests through the proxy`);
        done.push("switched off: removed, the browser's request only");
      } finally {
        await popup.close();
      }
      // Nothing in the browser's console from the extension.
      const errors = await s.inBrowser(`Services.console.getMessageArray()
        .filter((m) => m instanceof Ci.nsIScriptError && !(m.flags & Ci.nsIScriptError.warningFlag) && String(m.sourceName).startsWith(${JSON.stringify(s.base)}))
        .map((m) => m.errorMessage)`);
      if (errors.length) throw new Error(`the extension's console: ${errors.join(" | ")}`);
      done.push("no error in the console");
      if (perRequest) throw new Error(`${perRequest} (the rest ok: ${done.join("; ")})`);
      return done.join("; ");
    } finally {
      await pref("network.proxy.type", 0).catch(() => {});
      await pref("extensions.webextOptionalPermissionPrompts", true).catch(() => {});
    }
  },

  // A .zip link, throttled (the browser's download is held while MDM is asked: Firefox pauses it)
  // and at full speed.
  async link(t) {
    const done = [];
    for (const [file, query] of [[`e2e-${t.name}-slow.zip`, "?rate=256K"], [`e2e-${t.name}.zip`, ""]]) {
      const url = `/gen/2M/${file}${query}`;
      const size = await sizeOf(LAB + url);
      await clickLink(t, url);
      await arrived(file, size);
      await keptNothing(t.dl);
      done.push(query ? "throttled" : "full speed");
    }
    return done.join(", ");
  },

  // #208: Firefox set to ask what to do with each new type (Waterfox's default): the download is
  // caught from the answer before the browser's "What should … do with this file?" dialog, which
  // never opens, and the browser doesn't download the file behind it. Firefox's family only.
  async ask(t) {
    if (!t.session.inBrowser) throw new Skip("Firefox's family only");
    const s = t.session;
    const always = JSON.stringify("browser.download.always_ask_before_handling_new_types");
    // As the run set it (E2E_FIREFOX_ASK), put back after.
    const was = await s.inBrowser(`Services.prefs.getBoolPref(${always}, false)`);
    await s.inBrowser(`(Services.prefs.setBoolPref(${always}, true), "set")`);
    try {
      const file = `e2e-ask-${t.name}.zip`;
      const url = `/gen/2M/${file}?rate=512K`;
      const size = await sizeOf(LAB + url);
      await clickLink(t, url);
      await arrived(file, size);
      const dialog = await s.inBrowser(
        `[...Services.wm.getEnumerator(null)].some((w) => Boolean(w.document.getElementById("unknownContentTypeWindow")))`,
      );
      if (dialog) throw new Error("the browser's dialog opened");
      await keptNothing(t.dl);
      // The browser's own request (the first) was stopped before it had the file.
      const [own] = (await requests(`/gen/2M/${file}`)).filter((e) => e.method === "GET");
      if (!own || own.bytes_sent >= size) throw new Error(`the browser downloaded the file itself: ${own ? brief(own) : "no request"}`);
      return `no dialog, MDM has it, the browser's request stopped after ${own.bytes_sent} bytes`;
    } finally {
      await s.inBrowser(`(Services.prefs.setBoolPref(${always}, ${was === true}), "set")`).catch(() => {});
    }
  },

  // Cookies and referrer: a file behind a session cookie and hotlink protection.
  async cookie(t) {
    await t.session.navigate(`${LAB}/cookie?set=sid=1`);
    const file = `e2e-c-${t.name}.zip`;
    const prefix = `${LAB}/page`;
    const link = `/gen/1M/${file}?require_cookie=sid%3D1&require_referer=${encodeURIComponent(prefix)}`;
    await clickLink(t, link);
    await arrived(file, 1 << 20);
    const mdm = await mdmRequests(`/gen/1M/${file}`);
    for (const e of mdm) {
      if (!/(^|;\s*)sid=1(;|$)/.test(e.cookie || "")) throw new Error(`MDM's request without the cookie: ${brief(e)} cookie=${e.cookie}`);
      if (!(e.referer || "").startsWith(prefix)) throw new Error(`MDM's request without the referrer: ${brief(e)} referer=${e.referer}`);
      if (e.user_agent !== t.ua) throw new Error(`MDM's User-Agent ${e.user_agent} is not the browser's ${t.ua}`);
      if (e.status >= 400) throw new Error(`MDM's request refused: ${brief(e)}`);
    }
    await keptNothing(t.dl);
    return `${mdm.length} request(s) with sid=1, the page as referrer, the browser's User-Agent`;
  },

  // Names from Content-Disposition: filename= and filename*= (UTF-8).
  async names(t) {
    const plain = `e2e-report-${t.name}.zip`;
    const star = `报告-e2e-${t.name}.zip`;
    await clickLink(t, `/gen/1M/download.php?cd=${encodeURIComponent(plain)}`);
    await arrived(plain, 1 << 20);
    await keptNothing(t.dl);
    await clickLink(t, `/gen/1M/get?cdstar=${encodeURIComponent(star)}`);
    await arrived(star, 1 << 20);
    await keptNothing(t.dl);
    // #447: raw UTF-8 in filename=, no filename* (Firefox gives extensions its bytes).
    const raw = `تقرير-e2e-${t.name}.zip`;
    await clickLink(t, `/gen/1M/raw.php?cd=${encodeURIComponent(raw)}`);
    await arrived(raw, 1 << 20);
    await keptNothing(t.dl);
    const wrong = fs.readdirSync(path.join(downloads, "Compressed")).filter((f) => /^(download\.php|get|fallback-name\.zip)/.test(f));
    if (wrong.length) throw new Error(`saved under the address's or the fallback name too: ${wrong.join(", ")}`);
    return `${plain}, ${star}, ${raw}`;
  },

  // A redirect: MDM keeps the address clicked (it asks it again to renew the link).
  async redirect(t) {
    const file = `e2e-r-${t.name}.zip`;
    const link = `/redirect?to=${encodeURIComponent(`/gen/2M/${file}`)}`;
    await clickLink(t, link);
    const item = await arrived(file, 2 << 20);
    if (item.url !== LAB + link) throw new Error(`MDM's address is ${item.url}, not the one clicked (${LAB + link})`);
    await mdmRequests(`/gen/2M/${file}`);
    await keptNothing(t.dl);
    return "the address clicked kept, the file from its target";
  },

  // A page that goes to the file by script (#374): MDM gets the file's address, not the page's.
  async leadon(t) {
    const file = `e2e-leadon-${t.name}.zip`;
    const page = `/leadon?to=${encodeURIComponent(`/gen/2M/${file}`)}`;
    await t.session.navigate(`${LAB}${page}`);
    const item = await arrived(file, 2 << 20);
    if (item.url !== `${LAB}/gen/2M/${file}`) throw new Error(`MDM's address is ${item.url}, not the file's`);
    const asked = await requests(page.split("?")[0]);
    if (asked.length !== 1) throw new Error(`the page was asked ${asked.length} times (only the browser's expected)`);
    await keptNothing(t.dl);
    return "the file's address, the page asked by the browser only";
  },

  // A form's download: MDM posts it again with the form's body; one answered 303 too.
  async form(t) {
    const name = `e2e-form-${t.name}.zip`;
    await t.session.navigate(`${LAB}/formpage?size=1M&name=${encodeURIComponent(name)}`);
    await t.session.evaluate("document.getElementById('submit-form').click()");
    await arrived(name, 1 << 20);
    const posts = await mdmRequests(`/form/1M/${name}`);
    for (const e of posts) {
      if (e.method !== "POST") throw new Error(`MDM didn't post the form again: ${brief(e)}`);
      if (!/(^|&)token=abc(&|$)/.test(e.req_body_head || "")) throw new Error(`MDM's post without token=abc: ${e.req_body_head}`);
      if (e.status >= 400) throw new Error(`MDM's post refused: ${brief(e)}`);
    }
    await keptNothing(t.dl);
    // Posted, answered "303 See Other" with the file's address.
    const other = `see-other-${name}`;
    await t.session.navigate(`${LAB}/formpage?size=1M&name=${encodeURIComponent(name)}`);
    await t.session.evaluate("document.getElementById('submit-see-other').click()");
    await arrived(other, 1 << 20);
    const seen = [...(await requests(`/form/1M/${other}`)), ...(await requests(`/gen/1M/${other}`))].sort((a, b) => a.req - b.req);
    await keptNothing(t.dl);
    return `posted again with token=abc (${posts.length} request(s)); 303: ${seen.map((e) => `${e.method} ${e.path.split("/")[1]} ${e.status}`).join(", ")}`;
  },

  // Alt+click: the browser keeps the download, MDM gets nothing.
  async alt(t) {
    const file = `e2e-alt-${t.name}.zip`;
    const p = `/gen/1M/${file}`;
    await t.session.navigate(`${LAB}/page?link=${encodeURIComponent(p)}`);
    const a = await rect(t.session, "#link");
    await t.session.mouse((a.l + a.r) / 2, (a.t + a.b) / 2, "click", { alt: true });
    const kept = path.join(t.dl, file);
    try {
      await until(`${file} in the browser's folder`, 20000, () => {
        const names = fs.readdirSync(t.dl);
        return names.length === 1 && names[0] === file && fs.statSync(kept).size === 1 << 20;
      });
    } catch (e) {
      const item = (await mdmList()).find((d) => d.file === file);
      const tab = await t.session.evaluate("location.href").catch((x) => x.message);
      e.message += ` (MDM: ${item ? `${item.status} ${item.path}` : "nothing"}; the tab: ${tab}; requests: ${(await requests(p)).map(brief).join("; ") || "none"}; the browser's folder: ${fs.readdirSync(t.dl).join(", ") || "empty"})`;
      throw e;
    }
    await sleep(1500);
    const item = (await mdmList()).find((d) => d.file === file);
    if (item) throw new Error(`MDM got it too: ${item.status} ${item.path}`);
    const all = await requests(p);
    if (all.length !== 1) throw new Error(`${all.length} requests for it (only the browser's expected): ${all.map(brief).join("; ")}`);
    fs.rmSync(kept);
    return "the browser's own download, MDM untouched";
  },

  // Files the browser would show or play, made downloads, MDM's (Chromium 128+, Firefox): typed
  // into the address bar, the tab stays on its page. A PDF sent with "Content-Disposition:
  // inline; filename=..." (as many servers do), one sent without it, an MP3 sent inline.
  async opened(t) {
    if (t.kind === "chromium" && t.session.major < 128) throw new Skip(`${t.name} ${t.session.major}: needs 128 (header conditions)`);
    const failed = [];
    const done = [];
    const left = [];
    for (const [file, query, sub, size] of [
      [`e2e-doc-${t.name}.pdf`, "inline=1", "Documents", "1M"],
      [`e2e-bare-${t.name}.pdf`, "inline=bare", "Documents", "1M"],
      [`e2e-song-${t.name}.mp3`, "inline=1", "Music", "512K"],
    ]) {
      if (idmRunning && file.endsWith(".pdf") && query === "inline=1") {
        left.push(`pdf ${query} (left out: IDM runs and takes it)`);
        continue;
      }
      const p = `/gen/${size}/${file}`;
      await t.session.navigate(`${LAB}/page?link=none`);
      await t.session.navigate(`${LAB}${p}?${query}`, { download: true });
      try {
        await arrived(file, await sizeOf(LAB + p), sub, 20000);
        const [at, type] = await t.session.evaluate("[location.pathname, document.contentType]");
        if (at !== "/page" || type !== "text/html") throw new Error(`the tab shows ${at} (${type}), not the page it was on`);
        await keptNothing(t.dl);
        done.push(`${file.split(".").pop()} ${query}`);
      } catch (e) {
        const tab = await t.session.evaluate("[location.href, document.contentType]").catch((x) => x.message);
        const log = (await requests(p)).map(brief).join("; ");
        failed.push(`${file} (${query}): ${e.message} [the tab: ${JSON.stringify(tab)}; requests: ${log}; the browser's folder: ${fs.readdirSync(t.dl).join(", ") || "empty"}]`);
        for (const f of fs.readdirSync(t.dl)) fs.rmSync(path.join(t.dl, f), { force: true, recursive: true });
      }
    }
    if (failed.length) throw new Error(`${failed.join(" | ")}${done.length ? ` (taken: ${done.join(", ")})` : ""}`);
    return `taken, the tab stayed: ${[...done, ...left].join(", ")}`;
  },

  // The video button on a page whose Content-Security-Policy allows no inline style.
  async csp(t) {
    await t.session.navigate(`${LAB}/page?csp=strict`);
    const v = await rect(t.session, "#video");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the video under csp=strict");
    near(b.b - b.t, 30, "the button's height");
    const pill = await t.session.pill();
    if (!pill) throw new Error("the button's pill wasn't found");
    if (Math.abs(pill.h - 30) > 2 || !/linear-gradient/.test(pill.image)) throw new Error(`the tab isn't styled: ${JSON.stringify(pill)}`);
    const address = await clickButton(t.session, b);
    const want = `${LAB}/gen/1M/clip.mp4?inline=1`;
    if (address !== want) throw new Error(`the video window got ${address}, not ${want}`);
    return `styled (${Math.round(pill.h)}px, ${pill.image.slice(0, 15)}...), sent ${address.replace(LAB, "")}`;
  },

  // The button doesn't sit on a control of the page (#421: a thumbnail's watch-later button at
  // the top right): it moves to a free corner.
  async spot(t) {
    // E2E_SPOT_SHOT: a screenshot of the page with the button shown, for a PR's body.
    const shoot = async () => {
      if (process.env.E2E_SPOT_SHOT && t.session.screenshot) await t.session.screenshot(process.env.E2E_SPOT_SHOT);
    };
    await t.session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
    await t.session.evaluate(`(() => {
      const r = document.querySelector("#video").getBoundingClientRect();
      const c = document.createElement("button");
      c.id = "page-control";
      c.textContent = "x";
      c.style.cssText = "position:fixed;left:" + (r.right - 44) + "px;top:" + (r.top + 4) + "px;width:40px;height:40px;z-index:9";
      document.body.appendChild(c);
      return true;
    })()`);
    const v = await rect(t.session, "#video");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the video");
    await shoot();
    const c = await rect(t.session, "#page-control");
    const overlaps = b.l < c.r && b.r > c.l && b.t < c.b && b.b > c.t;
    if (overlaps) throw new Error(`the button covers the page's control: ${JSON.stringify({ b, c })}`);
    return `at ${Math.round(b.l - v.l)},${Math.round(b.t - v.t)}, clear of the page's control`;
  },

  // #503: a hover-to-play preview (YouTube's thumbnails) stops as soon as the pointer leaves the
  // video for the button, the page seeing the pointer go: hidden, or taken out of the page. The
  // button stays where it was while the pointer is on it, a click sends the video it was over,
  // and it fades once the pointer leaves.
  async preview(t) {
    // E2E_PREVIEW_SHOT: screenshots of the page with the preview stopped, for a PR's body.
    const shoot = async (name) => {
      if (process.env.E2E_PREVIEW_SHOT && t.session.screenshot) await t.session.screenshot(`${process.env.E2E_PREVIEW_SHOT}-${name}.png`);
    };
    const done = [];
    for (const stop of ["hidden", "removed"]) {
      await t.session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
      await t.session.evaluate(`(() => {
        const v = document.querySelector("#video");
        v.addEventListener("mouseleave", () => {
          if (${JSON.stringify(stop)} === "hidden") v.style.display = "none"; else v.remove();
          document.body.dataset.preview = "stopped";
        });
        return true;
      })()`);
      const v = await rect(t.session, "#video");
      const b = await hover(t.session, v);
      if (!b) throw new Error(`${stop}: no button over the video`);
      // Onto the button: the page stops its preview.
      await t.session.mouse(b.l + 14, (b.t + b.b) / 2);
      await until(`${stop}: the page's preview stopped`, 3000, () => t.session.evaluate(`document.body.dataset.preview === "stopped"`));
      await sleep(500);
      await shoot(stop);
      const kept = await rect(t.session, "mdm-video-button");
      if (!kept) throw new Error(`${stop}: the button went with the preview`);
      near(kept.l, b.l, `${stop}: kept, left edge`);
      near(kept.t, b.t, `${stop}: kept, top`);
      // Still there with the pointer resting on it, past the time the pointer is given to reach it.
      await sleep(2000);
      if (!(await rect(t.session, "mdm-video-button"))) throw new Error(`${stop}: the button went while the pointer rested on it`);
      const address = await clickButton(t.session, kept);
      if (address !== `${LAB}/gen/1M/clip.mp4`) throw new Error(`${stop}: the video window got ${address}`);
      // The pointer leaves: it fades, as over any video.
      await t.session.mouse(5, 5);
      await until(`${stop}: the button gone after the pointer left`, 3000, async () => !(await rect(t.session, "mdm-video-button")));
      done.push(`${stop}: kept, sent ${address.replace(LAB, "")}, gone after`);
    }
    return done.join("; ");
  },

  // #560: YouTube's thumbnail previews grow while the pointer is over them and show their own
  // sound button at the top right, and shrink back and hide it once the pointer leaves for the
  // button. The button keeps one place as the pointer goes to it and rests on it, and a click
  // sends the video.
  async steady(t) {
    await t.session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
    await t.session.evaluate(`(() => {
      const v = document.querySelector("#video");
      v.style.transition = "transform 150ms linear";
      let timer = 0;
      v.addEventListener("mouseenter", () => {
        v.style.transform = "scale(1.08)";
        clearTimeout(timer);
        timer = setTimeout(() => {
          const r = v.getBoundingClientRect();
          const c = document.createElement("button");
          c.id = "preview-sound";
          c.textContent = "sound";
          c.style.cssText = "position:fixed;left:" + (r.right - 48) + "px;top:" + (r.top + 6) + "px;width:40px;height:40px;z-index:9";
          document.body.appendChild(c);
        }, 300);
      });
      v.addEventListener("mouseleave", () => {
        clearTimeout(timer);
        v.style.transform = "";
        document.getElementById("preview-sound")?.remove();
        document.body.dataset.left = String(Number(document.body.dataset.left || 0) + 1);
      });
      return true;
    })()`);
    const v0 = await rect(t.session, "#video");
    const first = await hover(t.session, v0);
    if (!first) throw new Error("no button over the preview");
    // The preview grown, its sound button shown: the button takes a free corner.
    await until("the preview's sound button", 3000, () => t.session.evaluate(`Boolean(document.getElementById("preview-sound"))`));
    await sleep(900);
    const c = await rect(t.session, "#preview-sound");
    const b = await rect(t.session, "mdm-video-button");
    if (!b) throw new Error("the button went as the preview started");
    if (b.l < c.r && b.r > c.l && b.t < c.b && b.b > c.t) throw new Error(`the button covers the preview's sound button: ${JSON.stringify({ b, c })}`);
    // The pointer goes to it in steps, then rests on it: the page shrinks the preview and hides
    // its sound button. Where the button is, every step and every 100 ms while it rests.
    const v = await rect(t.session, "#video");
    const from = [(v.l + v.r) / 2, (v.t + v.b) / 2];
    const to = [b.l + 14, (b.t + b.b) / 2];
    const seen = [];
    const at = async (what) => {
      const r = await rect(t.session, "mdm-video-button");
      seen.push(r ? `${what} ${Math.round(r.l)},${Math.round(r.t)}` : `${what} gone`);
      return r;
    };
    for (let i = 1; i <= 8; i++) {
      await t.session.mouse(from[0] + ((to[0] - from[0]) * i) / 8, from[1] + ((to[1] - from[1]) * i) / 8);
      await sleep(60);
      await at(`step ${i}`);
    }
    await until("the page saw the pointer leave the preview", 3000, () => t.session.evaluate(`Number(document.body.dataset.left || 0) > 0`));
    const landed = await at("on it");
    for (let i = 0; i < 25; i++) {
      await sleep(100);
      await at(`rest ${i}`);
    }
    // E2E_STEADY_SHOT: a screenshot after the pointer rested on it, for a PR's body.
    if (process.env.E2E_STEADY_SHOT && t.session.screenshot) await t.session.screenshot(process.env.E2E_STEADY_SHOT);
    const moved = seen.filter((x) => {
      const m = /(-?\d+),(-?\d+)$/.exec(x);
      return !m || !landed || Math.abs(m[1] - landed.l) > 2 || Math.abs(m[2] - landed.t) > 2;
    });
    if (!landed || moved.length) throw new Error(`the button moved: ${seen.join("; ")} (landed at ${landed ? `${Math.round(landed.l)},${Math.round(landed.t)}` : "none"})`);
    const address = await clickButton(t.session, landed);
    if (address !== `${LAB}/gen/1M/clip.mp4`) throw new Error(`the video window got ${address}`);
    await t.session.mouse(5, 5);
    return `${Math.abs(first.l - b.l) > 2 || Math.abs(first.t - b.t) > 2 ? "moved" : "stayed"} off the preview's sound button, at ${Math.round(b.l - v.l)},${Math.round(b.t - v.t)}; then still for ${seen.length} looks (8 steps there, 2.5 s on it while the preview shrank and hid its button), sent ${address.replace(LAB, "")}`;
  },

  // #899: a site's player shows its own settings button at the top right while the pointer moves
  // over the player, and hides it a moment after the pointer stops or leaves the player (for the
  // button, which is no part of it). The button takes a free corner once, and stays there as the
  // pointer goes to it, rests on it, goes back over the video and stops: it never hops back to
  // the corner the settings button hid from (0.5.2 did, every time it came and went).
  async player(t) {
    await t.session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
    await t.session.evaluate(`(() => {
      const v = document.querySelector("#video");
      const box = document.createElement("div");
      box.id = "player-box";
      box.style.cssText = "position:relative;display:inline-block;background:#111";
      v.replaceWith(box);
      box.appendChild(v);
      const gear = document.createElement("button");
      gear.id = "player-settings";
      gear.textContent = "settings";
      gear.style.cssText = "position:absolute;right:8px;top:8px;width:44px;height:36px;z-index:2;display:none";
      box.appendChild(gear);
      let idle = 0;
      const hide = (ms) => {
        clearTimeout(idle);
        idle = setTimeout(() => { gear.style.display = "none"; document.body.dataset.hidden = String(Number(document.body.dataset.hidden || 0) + 1); }, ms);
      };
      box.addEventListener("mousemove", () => { gear.style.display = "block"; hide(1200); });
      box.addEventListener("mouseleave", () => hide(250));
      return true;
    })()`);
    const v = await rect(t.session, "#video");
    const mid = [(v.l + v.r) / 2, (v.t + v.b) / 2];
    const seen = [];
    const side = (r) => (r ? ((r.l + r.r) / 2 > mid[0] ? "right" : "left") : "gone");
    const look = async (what) => {
      const r = await rect(t.session, "mdm-video-button");
      seen.push(`${what} ${side(r)}${r ? ` ${Math.round(r.l)},${Math.round(r.t)}` : ""}`);
      return r;
    };
    const glide = async (from, to, steps, what) => {
      for (let i = 1; i <= steps; i++) {
        await t.session.mouse(from[0] + ((to[0] - from[0]) * i) / steps, from[1] + ((to[1] - from[1]) * i) / steps);
        await sleep(60);
        await look(`${what} ${i}`);
      }
    };
    const rest = async (ms, what) => {
      for (let n = 0; n < ms / 100; n++) {
        await sleep(100);
        await look(`${what} ${n}`);
      }
    };
    // Over the player: its settings button shows, the button shows clear of it.
    const first = await hover(t.session, v);
    if (!first) throw new Error("no button over the player");
    await until("the player's settings button", 3000, () => t.session.evaluate(`document.getElementById("player-settings").style.display === "block"`));
    await sleep(900);
    const b = await look("shown");
    const gear = await rect(t.session, "#player-settings");
    if (!b) throw new Error("the button went as the player's controls showed");
    if (gear && b.l < gear.r && b.r > gear.l && b.t < gear.b && b.b > gear.t) throw new Error(`the button covers the player's settings button: ${JSON.stringify({ b, gear })}`);
    // To the button, and resting on it: the player hides its settings button.
    const on = [b.l + 14, (b.t + b.b) / 2];
    await glide(mid, on, 8, "to it");
    await rest(2000, "on it");
    // Back over the video, still there while the player's controls show and hide again.
    await glide(on, mid, 8, "back");
    await rest(2500, "still");
    await t.session.mouse(mid[0] + 20, mid[1] + 10);
    await rest(800, "moved");
    // And to it again.
    await glide([mid[0] + 20, mid[1] + 10], on, 8, "again");
    await rest(1500, "on it again");
    const hid = await t.session.evaluate("Number(document.body.dataset.hidden || 0)");
    if (hid < 2) throw new Error(`the player's settings button hid ${hid} time(s): the test's player didn't behave as the issue's`);
    const sides = seen.map((x) => x.split(" ").find((w) => ["left", "right", "gone"].includes(w)));
    const flips = sides.filter((x, i) => i > 0 && x !== "gone" && sides[i - 1] !== "gone" && x !== sides[i - 1]).length;
    // Gone a moment is fine: it fades while the pointer rests still over the video (#480).
    const kept = seen.filter((x) => !x.includes(side(b)) && !x.endsWith(" gone"));
    if (flips || kept.length) throw new Error(`the button hopped (${flips} times): ${seen.join("; ")}`);
    await t.session.mouse(5, 5);
    return `took the ${side(b)} corner clear of the player's settings button, then kept it for ${seen.length} looks (to it, on it, back over the video, still, moved, on it again) while the player hid its settings button ${hid} times`;
  },

  // A player playing a stream (a blob: in the page): the button sends the manifest it loaded.
  async hls(t) {
    await t.session.navigate(`${LAB}/page?hls=1`);
    const state = await until("the stream player", 15000, () => t.session.evaluate("document.body.dataset.stream || ''"));
    if (state !== "done") throw new Error(`the page's player: ${state}`);
    const v = await rect(t.session, "#player");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the stream's player");
    const address = await clickButton(t.session, b);
    const want = `${LAB}/hls/master.m3u8`;
    if (address !== want) throw new Error(`the video window got ${address}, not ${want}`);
    return `sent ${address.replace(LAB, "")}`;
  },

  // #155: an HLS or DASH player (playing from a blob:) handed to MDM, by the button over it, the
  // right-click menu and the toolbar button's media list: yt-dlp gets the stream's manifest, with
  // the cookie and the page as referrer, and reads it. Needs a yt-dlp (E2E_YTDLP, else one on
  // PATH): the test instance uses it as the person's own copy (Helpers).
  async streams(t) {
    if (!ytdlp) throw new Skip("no yt-dlp (E2E_YTDLP or PATH)");
    const cookie = `sid155=${t.name}`;
    await t.session.navigate(`${LAB}/cookie?set=${encodeURIComponent(cookie)}`);
    const done = [];
    // Each path's failure names it.
    const named = async (what, f) => {
      try {
        return `${what}: ${await f()}`;
      } catch (e) {
        e.message = `${what}: ${e.message}${done.length ? ` (passed: ${done.join("; ")})` : ""}`;
        throw e;
      }
    };
    // #171: the same with no .m3u8 or .mpd and served as text/plain or octet-stream, after a
    // text answer that is no manifest (the player's settings, never sent).
    const kinds = [
      ["hls", "/hls/master.m3u8"],
      ["dash", "/dash/manifest.mpd"],
      ["hls2", "/hls2/playlist?id=lab"],
      ["dash2", "/dash2/manifest?id=lab"],
    ];
    for (const [kind, manifest] of kinds) {
      await t.session.navigate(`${LAB}/page?${kind}=1`);
      const state = await until("the stream player", 15000, () => t.session.evaluate("document.body.dataset.stream || ''"));
      if (state !== "done") throw new Error(`the page's ${kind} player: ${state}`);
      const page = await t.session.evaluate("location.href");
      const v = await rect(t.session, "#player");
      // The button over the player: its menu, then "All qualities and subtitles…" (openInMdm).
      done.push(await named(`${kind} button`, () => streamLookedUp(async () => {
        const b = await hover(t.session, v);
        if (!b) throw new Error("no button over the stream's player");
        await openInMdm(t.session, appPid());
      }, manifest, page, cookie)));
      // The right-click menu on the player (Firefox: its menu can be driven headless).
      if (t.session.menu) {
        done.push(await named(`${kind} menu`, () => streamLookedUp(async () => {
          const r = await t.session.menu((v.l + v.r) / 2, (v.t + v.b) / 2, "Download with Mister Download Manager");
          if (!String(r).startsWith("chosen")) throw new Error(`the right-click menu: ${r}`);
          if (process.env.E2E_DEBUG) console.error("menu:", r);
        }, manifest, page, cookie)));
      }
      // The toolbar button's media list.
      const popup = await t.session.tab(`${t.session.base}popup.html?page=${encodeURIComponent(page)}`);
      try {
        const name = manifest.split("?")[0].split("/").pop();
        await until("the popup's media list", 15000, () =>
          popup.evaluate(`[...document.querySelectorAll("#media-list li")].some((li) => li.querySelector(".name").textContent === ${JSON.stringify(name)} && li.querySelector("button"))`),
        );
        done.push(await named(`${kind} list`, () => streamLookedUp(
          () => popup.evaluate(`(() => { const li = [...document.querySelectorAll("#media-list li")].find((x) => x.querySelector(".name").textContent === ${JSON.stringify(name)}); li.querySelector("button").click(); return true; })()`),
          manifest,
          page,
          cookie,
        )));
      } finally {
        await popup.close();
      }
    }
    if (!t.session.menu) done.push("right-click menu: not drivable in Chromium (tools/extension/test.mjs)");
    return done.join("; ");
  },

  // #171: a page whose players load streams with no .m3u8 or .mpd in any address, served as
  // text/plain (HLS) and application/octet-stream (DASH), each after a text answer that is no
  // manifest. Nothing is read while the page plays; the button over the player sends the HLS
  // master; the media list shows the two streams with their qualities, and nothing else.
  async sniffed(t) {
    const play = async (q) => {
      await clearLog();
      await t.session.navigate(`${LAB}/page?${q}`);
      const state = await until("the stream players", 20000, () => t.session.evaluate("document.body.dataset.stream || ''"));
      if (state !== "done") throw new Error(`the page's players: ${state}`);
      return t.session.evaluate("location.href");
    };
    const masters = async () => (await (await fetch(`${LAB}/_log`)).json()).filter((x) => x.path === "/hls2/playlist" && x.query === "id=lab");
    // The HLS player alone (two players race: either manifest could come first).
    await play("hls2=1");
    await sleep(500);
    const before = (await masters()).length;
    if (before !== 1) throw new Error(`the master was asked for ${before} times while the page played (the player's once)`);
    // The button over the HLS player: no stream known, the frame's possible manifests are read.
    const v = await rect(t.session, "#player");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the stream's player");
    const address = await clickButton(t.session, b);
    const want = `${LAB}/hls2/playlist?id=lab`;
    if (address !== want) throw new Error(`the video window got ${address}, not ${want}`);
    const read = (await masters()).length - before;
    // The media list, on the page played again (nothing known yet).
    const page = await play("hls2=1&dash2=1");
    const popup = await t.session.tab(`${t.session.base}popup.html?page=${encodeURIComponent(page)}`);
    try {
      const rows = () =>
        popup.evaluate(`[...document.querySelectorAll("#media-list li")].map((li) => ({
          tag: li.querySelector(".tag").textContent, name: li.querySelector(".name").textContent,
          meta: li.querySelector(".meta").textContent, get: Boolean(li.querySelector("button")) }))`);
      await until("the popup's media list", 15000, async () => (await rows().catch(() => [])).length >= 2);
      await sleep(500);
      const all = await rows();
      const want = [
        ["HLS", "playlist", "720p · 360p"],
        ["DASH", "manifest", "720p · 360p · 0:12"],
      ];
      for (const [tag, name, meta] of want) {
        const r = all.find((x) => x.tag === tag && x.name === name);
        if (!r) throw new Error(`not listed: ${tag} ${name} (the list: ${JSON.stringify(all)})`);
        if (!r.meta.startsWith(meta) || !r.get) throw new Error(`${name}: ${JSON.stringify(r)}, not "${meta}" with a Download button`);
      }
      if (all.length !== want.length) throw new Error(`${all.length} rows, not ${want.length}: ${JSON.stringify(all)}`);
      await keptNothing(t.dl);
      return `nothing read while playing; the button sent ${address.replace(LAB, "")} (then the master was asked for ${read} more times, the extension's read among them); listed ${all.map((r) => `${r.tag} ${r.name} (${r.meta})`).join(", ")}, not the settings`;
    } finally {
      await popup.close();
    }
  },

  // #38: the toolbar button's list of what the tab loaded. A page plays a video file (behind a
  // cookie and hotlink protection), a sound, an HLS stream, a DASH stream and a DRM-protected
  // one; the popup (its page, in a tab of its own) lists the five, each stream once with its
  // qualities, the protected one without a Download button. The file downloads in MDM with the
  // page's cookies and referrer; each stream opens the Download video window for its manifest.
  async media(t) {
    await t.session.navigate(`${LAB}/cookie?set=msid=7`);
    const file = `e2e-media-${t.name}.mp4`;
    const guarded = `/gen/1M/${file}?inline=1&require_cookie=msid%3D7&require_referer=${encodeURIComponent(`${LAB}/page`)}`;
    await t.session.navigate(`${LAB}/page?video=${encodeURIComponent(guarded)}&audio=1&hls=1&dash=1&drm=1`);
    const state = await until("the stream players", 20000, () => t.session.evaluate("document.body.dataset.stream || ''"));
    if (state !== "done") throw new Error(`the page's players: ${state}`);
    const page = await t.session.evaluate("location.href");
    const popup = await t.session.tab(`${t.session.base}popup.html?page=${encodeURIComponent(page)}`);
    try {
      const rows = () =>
        popup.evaluate(`[...document.querySelectorAll("#media-list li")].map((li) => ({
          tag: li.querySelector(".tag").textContent, name: li.querySelector(".name").textContent,
          meta: li.querySelector(".meta").textContent, get: Boolean(li.querySelector("button")),
          drm: Boolean(li.querySelector(".drm")) }))`);
      const list = await until("the popup's media list", 15000, async () => {
        const r = await rows().catch(() => []);
        return r.length >= 5 && r;
      });
      await sleep(500);
      const all = await rows();
      const want = [
        ["HLS", "master.m3u8", "720p · 360p", true],
        ["DASH", "manifest.mpd", "720p · 360p · 0:12", true],
        ["DASH", "protected.mpd", "", false],
        ["MP4", file, "", true],
        ["MP3", "sound.mp3", "", true],
      ];
      const found = (tag, name) => all.find((r) => r.tag === tag && r.name === name);
      for (const [tag, name, meta, get] of want) {
        const r = found(tag, name);
        if (!r) throw new Error(`not listed: ${tag} ${name} (the list: ${JSON.stringify(all)})`);
        if (meta && !r.meta.startsWith(meta)) throw new Error(`${name}: "${r.meta}", not "${meta}"`);
        if (r.get !== get || r.drm === get) throw new Error(`${name}: ${get ? "no Download button" : "not shown as protected"} (${JSON.stringify(r)})`);
      }
      if (all.length !== want.length) throw new Error(`${all.length} rows, not ${want.length}: ${JSON.stringify(all)}`);
      void list;
      const click = (name) =>
        popup.evaluate(`(() => { const li = [...document.querySelectorAll("#media-list li")].find((x) => x.querySelector(".name").textContent === ${JSON.stringify(name)});
          li.querySelector("button").click(); return true; })()`);
      // The file: MDM downloads it with the page's cookie and referrer and the browser's UA.
      await click(file);
      await arrived(file, 1 << 20, "Video");
      const mdm = (await requests(`/gen/1M/${file}`)).filter((e) => e.user_agent === t.ua && e.status < 400);
      const theirs = (await requests(`/gen/1M/${file}`)).filter((e) => e.status >= 400);
      if (theirs.length) throw new Error(`refused: ${theirs.map(brief).join("; ")}`);
      if (!mdm.length) throw new Error("no request for the file");
      const said = await until("the popup's word", 5000, () => popup.evaluate(`document.getElementById("media-said").textContent`));
      if (!/Sent to Mister Download Manager/.test(said)) throw new Error(`the popup said "${said}"`);
      // Each stream: the Download video window, for its manifest.
      const pid = appPid();
      const opened = [];
      for (const name of ["master.m3u8", "manifest.mpd"]) {
        appWindows(pid, VIDEO_WINDOW);
        await until("the Download video window closed", 5000, () => !appWindows(pid).includes(VIDEO_WINDOW));
        await click(name);
        await until("the Download video window", 15000, () => appWindows(pid).includes(VIDEO_WINDOW));
        const address = await until("the Download video window's address", 10000, () => videoAddress(pid));
        appWindows(pid, VIDEO_WINDOW);
        const wantAddress = `${LAB}/${name === "master.m3u8" ? "hls" : "dash"}/${name}`;
        if (address !== wantAddress) throw new Error(`the video window got ${address}, not ${wantAddress}`);
        opened.push(address.replace(LAB, ""));
      }
      await keptNothing(t.dl);
      return `listed ${all.map((r) => `${r.tag} ${r.name}${r.drm ? " (protected)" : ""}`).join(", ")}; ${file} downloaded with the cookie and referrer; video window: ${opened.join(", ")}`;
    } finally {
      await popup.close();
    }
  },

  // #695, #800: a video a server's program sends (/remote_control.php, /stream.php as video/mp4,
  // no Content-Disposition), on a page titled "Lab lecture: week N". The toolbar button's list
  // names it after the page with its type ("Lab lecture week 1.mp4 · MP4", never
  // "remote_control.php · PHP"), and MDM saves it so from there; the button over its player (a
  // plain file) saves "Lab lecture week 2.mp4"; with yt-dlp, the Download video window lists a
  // `stream.php?file=` video as MP4 and saves an .mp4 (yt-dlp refused a .php). E2E_SHOTS:
  // <lang>-media-list.png (the popup's list) and <lang>-video-window.png (that window, Windows).
  async scripted(t) {
    const shots = process.env.E2E_SHOTS ? path.resolve(process.env.E2E_SHOTS) : "";
    const lang = process.env.E2E_LANG || "en";
    const page = (week, src) => `${LAB}/page?title=${encodeURIComponent(`Lab lecture: week ${week}`)}&video=${encodeURIComponent(src)}`;
    const said = [];
    // An earlier browser's files of the same names off the list and the disk: they'd be numbered.
    const earlier = (await mdmList()).filter((d) => /^Lab lecture/.test(d.file)).map((d) => d.id);
    if (earlier.length) await ipc({ action: "delete", ids: earlier });
    // The list: named after the page, typed by its server.
    await t.session.navigate(page(1, `/remote_control.php?id=${t.name}&seed=11`));
    const at = await t.session.evaluate("location.href");
    const popup = await t.session.tab(`${t.session.base}popup.html?page=${encodeURIComponent(at)}`);
    const first = "Lab lecture week 1.mp4";
    try {
      const rows = () =>
        popup.evaluate(`[...document.querySelectorAll("#media-list li")].map((li) => ({
          tag: li.querySelector(".tag").textContent, name: li.querySelector(".name").textContent }))`);
      const listed = await until("the popup's media list", 15000, async () => {
        const r = await rows().catch(() => []);
        return r.length >= 1 && r;
      });
      await sleep(500);
      if (shots && popup.shot) await popup.shot(path.join(shots, `${lang}-media-list.png`));
      if (listed.length !== 1 || listed[0].tag !== "MP4" || listed[0].name !== first) {
        throw new Error(`the list shows ${JSON.stringify(listed)}, not MP4 ${first}`);
      }
      await popup.evaluate(`(() => { document.querySelector("#media-list li button").click(); return true; })()`);
      await arrived(first, 1 << 20, "Video");
      said.push(`list: MP4 ${first}, saved as it`);
    } finally {
      await popup.close();
    }
    // The button over its player: the file at once, named the same way.
    await t.session.navigate(page(2, `/stream.php?id=${t.name}&seed=12`));
    const v = await rect(t.session, "#video");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the video");
    await t.session.mouse(await mainPart(t.session, b), (b.t + b.b) / 2, "click");
    await arrived("Lab lecture week 2.mp4", 1 << 20, "Video");
    said.push("button: Lab lecture week 2.mp4");
    // The Download video window (#695): yt-dlp's look-up of a `file=` address, typed MP4.
    if (!WIN || !ytdlp) return `${said.join("; ")} (no Download video window: ${WIN ? "no yt-dlp" : NO_UIA})`;
    const file = `/stream.php?file=e2e-${t.name}.mp4&token=x&seed=13`;
    await t.session.navigate(page(3, file));
    const v3 = await rect(t.session, "#video");
    const b3 = await hover(t.session, v3);
    if (!b3) throw new Error("no button over the third video");
    const address = await clickButton(t.session, b3, "", true);
    if (address !== `${LAB}${file}`) throw new Error(`the video window got ${address}`);
    const pid = appPid();
    await until("yt-dlp's lookup (Download Now enabled)", 60000, () => videoReady(pid));
    await sleep(1000);
    if (shots) spawnSync("powershell", ["-NoProfile", "-File", path.join(root, "tools", "lab", "cap-own.ps1"), "-ProcessId", String(pid), "-Title", VIDEO_WINDOW, "-Out", path.join(shots, `${lang}-video-window.png`)]);
    const before = new Set((await mdmList()).map((d) => d.id));
    if (!videoDownloadNow(pid)) throw new Error("no Download Now to press");
    const done = await until("the Download video window's download", 60000, async () => {
      const d = (await mdmList()).find((x) => !before.has(x.id));
      if (d && d.status === "Error") throw new Error(`it failed: ${d.error}`);
      return d && d.status === "Complete" ? d : null;
    });
    if (!/\.mp4$/.test(done.file) || path.dirname(path.resolve(done.path)) !== path.join(downloads, "Video")) {
      throw new Error(`the window saved ${done.path}`);
    }
    said.push(`window: ${done.file}`);
    return said.join("; ");
  },

  // A page that moves in place (history.pushState, as YouTube does between videos): the button
  // hands the app the address it is at now, not the one the document loaded at.
  async inplace(t) {
    await t.session.navigate(`${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`);
    // A blob: source, as a site's own player has (there's no file of its own to send).
    await t.session.evaluate(`(() => { const v = document.querySelector("#video"); v.src = URL.createObjectURL(new MediaSource()); history.pushState({}, "", "/page?video=moved"); return true; })()`);
    const v = await rect(t.session, "#video");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the player");
    const address = await clickButton(t.session, b);
    const want = `${LAB}/page?video=moved`;
    if (address !== want) throw new Error(`the video window got ${address}, not ${want}`);
    return `sent ${address.replace(LAB, "")}`;
  },

  // The button's qualities' menu on a stream's player: the click has MDM look the stream up
  // (yt-dlp), the menu lists its qualities with their sizes (estimated from the variants' bitrate
  // and the stream's length), and the one picked downloads with only its progress window: MDM,
  // started in the notification area (as with the session), keeps its main window hidden, and no
  // Download video window opens. Needs a yt-dlp, as streams does, and an ffmpeg: the lab's stream
  // has its sound apart (EXT-X-MEDIA), and without ffmpeg to put them together only the sound is
  // offered, as the Download video window does.
  async qualities(t) {
    if (!ytdlp) throw new Skip("no yt-dlp (E2E_YTDLP or PATH)");
    if (!ffmpeg) throw new Skip("no ffmpeg (E2E_FFMPEG or PATH)");
    if (!WIN) throw new Skip(NO_UIA);
    await stopApp();
    fs.rmSync(path.join(data, "ipc.json"), { force: true });
    spawn(appExe, ["/startup"], { env: process.env, stdio: "ignore", detached: false });
    try {
      await until("the test instance, in the notification area", 20000, () => mdmList().then(() => true, () => false));
      const pid = appPid();
      const main = (titles) => titles.filter((x) => /^Mister Download Manager \d/.test(x));
      const progress = (titles) => titles.find((x) => /^\d+% /.test(x));
      const at = appWindows(pid);
      if (main(at).length) throw new Error(`started in the notification area, its main window shows (${at.join(", ")})`);
      await t.session.navigate(`${LAB}/page?hls=1`);
      const state = await until("the stream player", 15000, () => t.session.evaluate("document.body.dataset.stream || ''"));
      if (state !== "done") throw new Error(`the page's player: ${state}`);
      const v = await rect(t.session, "#player");
      const b = await hover(t.session, v);
      if (!b) throw new Error("no button over the stream's player");
      // An earlier browser's downloads of this stream off the list (with their files): the same
      // quality again would be a duplicate, which Options' default asks about.
      const earlier = (await mdmList()).filter((d) => d.url.startsWith(`${LAB}/hls/`)).map((d) => d.id);
      if (earlier.length) await ipc({ action: "delete", ids: earlier });
      const before = new Set((await mdmList()).map((d) => d.id));
      // What the page's own shortcuts would see (a player's Space and arrows).
      await t.session.evaluate('(() => { window.__mdmKeys = 0; document.addEventListener("keydown", () => { window.__mdmKeys += 1; }); return 0; })()');
      // #703: the look-up starts as the button shows; its chip then names the best quality.
      const chipped = await until("the best quality on the button's chip", 60000, async () => {
        const p = await t.session.pill(PILL_STATE);
        return p && p.state === "ready" && p.chip ? p : null;
      });
      // Its menu: the arrow.
      await clickArrow(t.session);
      const clicked = Date.now();
      // Pictures for a pull request (E2E_SHOTS, see the top): the player and the menu together.
      const shots = process.env.E2E_SHOTS && t.session.shot ? path.resolve(process.env.E2E_SHOTS) : "";
      const lang = process.env.E2E_LANG || "en";
      const snap = (name, p) => {
        const box = { l: Math.min(v.l, p.box.l) - 10, t: Math.min(v.t, p.box.t) - 10, r: Math.max(v.r, p.box.r) + 10, b: Math.max(v.b, p.box.b) + 10 };
        return t.session.shot(path.join(shots, `${lang}-${name}.png`), box);
      };
      let shotLooking = false;
      // The menu, looking, then the qualities MDM found.
      let looked = false;
      const m = await until("the qualities in the menu", 60000, async () => {
        if (appWindows(pid).includes(VIDEO_WINDOW)) throw new Error("the arrow opened the Download video window, not the menu");
        const p = await t.session.picks("row");
        looked ||= Boolean(p && p.busy);
        if (shots && p && p.busy && !shotLooking) {
          shotLooking = true;
          await snap("looking", p);
        }
        if (p && !p.busy && !p.rows.length) throw new Error(`the menu shows no qualities: "${p.note}"`);
        return p && p.rows.length ? p : null;
      });
      const rowsMs = Date.now() - clicked;
      if (!m.rows.some((r) => r.size)) throw new Error(`no row has a size: ${JSON.stringify(m.rows.map((r) => [r.name, r.size]))}`);
      if (!m.open.text) throw new Error("no \"All qualities and subtitles…\" under the qualities");
      // The arrows move through the menu, and the page's shortcuts don't hear them.
      await t.session.key("ArrowDown");
      await t.session.key("ArrowUp");
      const heard = await t.session.evaluate("window.__mdmKeys");
      if (heard !== 0) throw new Error(`the page heard ${heard} keys the menu used`);
      // "Download best" first, then the picture's qualities under VIDEO and the sound's under AUDIO
      // (the ffmpeg gives MP3, Opus and FLAC beside the picture's), each line its own kind.
      if (m.caps.length !== 2) throw new Error(`the captions are ${JSON.stringify(m.caps)}, not VIDEO and AUDIO`);
      if (m.rows[0].kind !== "top" || !m.rows[0].name) throw new Error(`the first line isn't "Download best": ${JSON.stringify(m.rows[0])}`);
      const picture = /\b\d{3,4}p\b/;
      const pictures = { rows: m.rows.filter((r) => r.kind === "video") };
      const sounds = { rows: m.rows.filter((r) => r.kind === "audio") };
      if (!pictures.rows.length || pictures.rows.some((r) => !picture.test(r.name))) throw new Error(`VIDEO lists a sound: ${JSON.stringify(pictures.rows.map((r) => r.name))}`);
      if (!sounds.rows.length || sounds.rows.some((r) => picture.test(r.name))) throw new Error(`AUDIO lists a picture: ${JSON.stringify(sounds.rows.map((r) => r.name))}`);
      // #755: each picture's line says its codec (the lab's variants are H.264); #801: a stream's
      // sizes come from its bitrate, so each is marked as about ("~").
      if (pictures.rows.some((r) => !/\bH\.264\b/.test(r.name))) throw new Error(`a picture's line without its codec: ${JSON.stringify(pictures.rows.map((r) => r.name))}`);
      if (pictures.rows.some((r) => !r.size.startsWith("~"))) throw new Error(`a stream's estimate not marked as about: ${JSON.stringify(pictures.rows.map((r) => r.size))}`);
      await sleep(500);
      if (shots) await snap("list", await t.session.picks());
      // 360p from VIDEO (the click later takes the best, 720p: not the same download).
      const row = pictures.rows.find((r) => /\b360p\b/.test(r.name));
      if (!row) throw new Error(`no 360p in the Video tab: ${JSON.stringify(pictures.rows.map((r) => r.name))}`);
      const picked = Date.now();
      await t.session.mouse(row.x, row.y, "click");
      // MDM downloads the stream's manifest, with only the download's progress window.
      const item = await until("the quality picked, in MDM's list", 15000, async () => (await mdmList()).find((d) => !before.has(d.id)));
      if (item.url !== `${LAB}/hls/master.m3u8`) throw new Error(`MDM downloads ${item.url}, not the stream's manifest`);
      const opened = await until("the download's progress window", 15000, () => progress(appWindows(pid)));
      const startMs = Date.now() - picked;
      if (await t.session.picks()) throw new Error("the menu stayed open after the pick");
      // A moment more: nothing else comes up after it.
      await sleep(1500);
      const after = appWindows(pid);
      if (main(after).length) throw new Error(`the main window showed (${after.join(", ")})`);
      if (after.includes(VIDEO_WINDOW)) throw new Error("the Download video window opened too");
      // The main part's click: the best quality (the chip's), started at once, green with "Show".
      await sleep(7000);
      const b2 = await hover(t.session, v);
      if (!b2) throw new Error("no button for the second click");
      const beforeBest = new Set((await mdmList()).map((d) => d.id));
      await t.session.mouse(await mainPart(t.session, b2), (b2.t + b2.b) / 2, "click");
      const best = await until("the best quality, in MDM's list", 60000, async () => (await mdmList()).find((d) => !beforeBest.has(d.id)));
      const green = await until("the button's green state", 10000, async () => {
        const p = await t.session.pill(PILL_STATE);
        return p && p.state === "sent" ? p : null;
      });
      if (!green.acts.some((a) => a.act === "show")) throw new Error(`no "Show": ${JSON.stringify(green)}`);
      if (appWindows(pid).includes(VIDEO_WINDOW)) throw new Error("the click opened the Download video window");
      const list = (rs) => rs.map((r) => `${r.name}${r.size ? ` ${r.size}` : ""}`).join(", ");
      return `chip "${chipped.chip}"; the click: ${best.file} (${green.text} · Show); ${looked ? "looking, then " : ""}VIDEO ${pictures.rows.length} (${list(pictures.rows)}), AUDIO ${sounds.rows.length} (${list(sounds.rows)}); "${row.name}" picked: ${item.file} (${item.status}) with its progress window "${opened}" only, the main window hidden; click to rows ${rowsMs} ms, pick to first progress ${startMs} ms`;
    } finally {
      // The next case starts MDM as usual.
      await stopApp();
    }
  },

  // #900: a stream on another site's server that serves it only to its page's player: every
  // request, its playlists' and each segment's, must carry that page's Origin and a Referer on it
  // (the lab's /hlsg/, as a site's video server checks; 403 otherwise). The page is the lab's on
  // localhost, the stream on 127.0.0.1. The button's look-up finds its qualities, and one picked
  // in its menu downloads through MDM's engine to its end, every request of MDM's with what the
  // browser sent (E2E_GUARDED_SHOT: a screenshot of the button's menu then, for a PR's body).
  async guarded(t) {
    if (!ytdlp) throw new Skip("no yt-dlp (E2E_YTDLP or PATH)");
    if (!ffmpeg) throw new Skip("no ffmpeg (E2E_FFMPEG or PATH)");
    const page = `http://localhost:${PORT}`;
    const site = `localhost:${PORT}`;
    await clearLog();
    await t.session.navigate(`${page}/page?hlsg=127.0.0.1:${PORT}&title=${encodeURIComponent("A guarded stream")}`);
    const state = await until("the stream player", 15000, () => t.session.evaluate("document.body.dataset.stream || ''"));
    if (state !== "done") throw new Error(`the page's player: ${state} ${await t.session.evaluate("document.body.dataset.streamError || ''")}`);
    const guarded = (x) => x.path.startsWith(`/hlsg/${site}/`);
    const browser = (await (await fetch(`${LAB}/_log`)).json()).filter(guarded);
    if (!browser.length || browser.some((x) => x.status !== 200)) throw new Error(`the browser's own requests: ${browser.map((x) => `${x.path} ${x.status}`).join(", ")}`);
    // Without the page's Origin, the stream's server refuses it, as the issue's did.
    const bare = await fetch(`${LAB}/hlsg/${site}/master.m3u8`, { headers: { Referer: `${page}/page` } });
    if (bare.status !== 403) throw new Error(`the guarded stream answered ${bare.status} without the page's Origin`);
    const earlier = (await mdmList()).filter((d) => d.url.startsWith(`${LAB}/hlsg/`)).map((d) => d.id);
    if (earlier.length) await ipc({ action: "delete", ids: earlier });
    await clearLog();
    const v = await rect(t.session, "#player");
    const b = await hover(t.session, v);
    if (!b) throw new Error("no button over the stream's player");
    const before = new Set((await mdmList()).map((d) => d.id));
    const mdm = async () => (await (await fetch(`${LAB}/_log`)).json()).filter(guarded);
    const refused = async () => (await mdm()).filter((x) => x.status !== 200).map((x) => `${x.path} ${x.status} (origin ${x.origin || "none"}, referer ${x.referer || "none"})`);
    // The look-up: its chip names the best quality (a failed one names none).
    let last = null;
    const pill = await until("the best quality on the button's chip", 60000, async () => {
      last = await t.session.pill(PILL_STATE);
      return last && last.state === "ready" && last.chip ? last : null;
    }).catch(async () => {
      throw new Error(`no quality found (the button: ${last && last.state} "${last && last.text}"); the stream's server refused: ${(await refused()).join(", ") || "nothing"}`);
    });
    // Its menu: the sound's "original" line, one stream, downloaded by MDM's engine to its end
    // (no merge: the lab's tracks carry sound only, which yt-dlp's merge with some ffmpeg
    // builds refuses, whatever the request).
    await clickArrow(t.session);
    const m = await until("the qualities in the menu", 30000, async () => {
      const p = await t.session.picks("row");
      return p && p.rows.length ? p : null;
    });
    if (process.env.E2E_GUARDED_SHOT && t.session.shot) {
      await sleep(400);
      await t.session.shot(process.env.E2E_GUARDED_SHOT, { l: Math.min(v.l, m.box.l) - 10, t: Math.min(v.t, m.box.t) - 10, r: Math.max(v.r, m.box.r) + 10, b: Math.max(v.b, m.box.b) + 10 });
    }
    const row = m.rows.find((r) => r.kind === "audio" && /original/i.test(r.name));
    if (!row) throw new Error(`no original sound in the menu: ${JSON.stringify(m.rows.map((r) => r.name))}`);
    await t.session.mouse(row.x, row.y, "click");
    const item = await until("the stream, in MDM's list", 60000, async () => (await mdmList()).find((d) => !before.has(d.id)));
    const done = await until("the download's end", 120000, async () => {
      const d = (await mdmList()).find((x) => x.id === item.id);
      if (d && d.status === "Error") throw new Error(`the download failed: ${d.error}; refused: ${(await refused()).join(", ")}`);
      return d && d.status === "Complete" ? d : null;
    });
    // Every request of MDM's (yt-dlp's look-up and the engine's playlists and segments): 200,
    // with the page's Origin and Referer.
    const asked = await mdm();
    const segments = asked.filter((x) => x.path.endsWith(".ts"));
    const bad = await refused();
    if (bad.length) throw new Error(`refused: ${bad.join(", ")}`);
    const lacking = asked.filter((x) => x.origin !== page || !(x.referer || "").startsWith(`${page}/`));
    if (lacking.length) throw new Error(`without the page's Origin and Referer: ${lacking.map((x) => `${x.path} (${x.origin}, ${x.referer})`).join(", ")}`);
    if (segments.length < 3) throw new Error(`only ${segments.length} segments asked for: ${asked.map((x) => x.path).join(", ")}`);
    await t.session.mouse(5, 5);
    return `chip "${pill.chip}"; "${row.name}" picked: ${done.file} (${fs.statSync(done.path).size} bytes); MDM's ${asked.length} requests (${asked.length - segments.length} playlists, ${segments.length} segments) all 200 with Origin ${page} and its Referer`;
  },

  // E2E_VIDEO_PAGE=<a video's page>: the button's qualities' menu on a real site (a pull
  // request's screenshots and its real check; the network's, so never by default): the lines the
  // menu shows, then, with E2E_VIDEO_PICK=<a pattern of a line's words>, that line picked and
  // downloaded to its end, its file's size beside the line's (#801: the size shown is the size
  // that comes). E2E_SHOTS as in `qualities`: <lang>-page-list.png (the menu), page-pick.png
  // (the line about to be picked, scrolled to).
  async page(t) {
    const address = process.env.E2E_VIDEO_PAGE;
    if (!address) throw new Skip("no E2E_VIDEO_PAGE");
    if (!ytdlp) throw new Skip("no yt-dlp (E2E_YTDLP or PATH)");
    if (!ffmpeg) throw new Skip("no ffmpeg (E2E_FFMPEG or PATH)");
    if (!WIN) throw new Skip(NO_UIA);
    await stopApp();
    fs.rmSync(path.join(data, "ipc.json"), { force: true });
    spawn(appExe, ["/startup"], { env: process.env, stdio: "ignore", detached: false });
    try {
      await until("the test instance, in the notification area", 20000, () => mdmList().then(() => true, () => false));
      await t.session.navigate(address);
      const v = await until("the page's video", 30000, () => rect(t.session, "video"));
      const b = await hover(t.session, v);
      if (!b) throw new Error("no button over the page's video");
      await until("the best quality on the button's chip", 120000, async () => {
        const p = await t.session.pill(PILL_STATE);
        return p && p.state === "ready" && p.chip ? p : null;
      });
      await clickArrow(t.session);
      const m = await until("the qualities in the menu", 300000, async () => {
        const p = await t.session.picks("row");
        if (p && !p.busy && !p.rows.length) throw new Error(`the menu shows no qualities: "${p.note}"`);
        return p && p.rows.length ? p : null;
      });
      const shots = process.env.E2E_SHOTS && t.session.shot ? path.resolve(process.env.E2E_SHOTS) : "";
      const lang = process.env.E2E_LANG || "en";
      const snap = (name, p) => t.session.shot(path.join(shots, `${lang}-${name}.png`), { l: p.box.l - 10, t: p.box.t - 10, r: p.box.r + 10, b: p.box.b + 10 });
      await sleep(500);
      if (shots) await snap("page-list", await t.session.picks());
      const lines = m.rows.map((r) => `${r.name} = ${r.size}`);
      // #852: what the menu says above them (YouTube without a JavaScript runtime).
      if (m.note) lines.unshift(`note: ${m.note}`);
      const pick = process.env.E2E_VIDEO_PICK;
      if (!pick) return lines.join("; ");
      const wanted = new RegExp(pick);
      const row = m.rows.find((r) => r.kind !== "top" && wanted.test(r.name));
      if (!row) throw new Error(`no line like ${pick}: ${lines.join("; ")}`);
      // Where it is now (the first lines are in the menu's view).
      const now = await t.session.picks();
      const seen = now.rows.find((r) => r.choice === row.choice) || row;
      if (shots) await snap("page-pick", now);
      const before = new Set((await mdmList()).map((d) => d.id));
      await t.session.mouse(seen.x, seen.y, "click");
      const item = await until("the line picked, in MDM's list", 30000, async () => (await mdmList()).find((d) => !before.has(d.id)));
      const done = await until("the download's end", 20 * 60000, async () => {
        const d = (await mdmList()).find((x) => x.id === item.id);
        if (d && d.status === "Error") throw new Error(`the download failed: ${d.error}`);
        return d && d.status === "Complete" ? d : null;
      });
      const bytes = fs.statSync(done.path).size;
      return `${lines.join("; ")}; picked "${seen.name}" (${seen.size}): ${done.file}, ${bytes} bytes`;
    } finally {
      await stopApp();
    }
  },

  // Before `video`, which ends with "Never on this site" for the lab in the last browser.
  async panelOptions(t) {
    return videoButtonFollowsOptions(t.session);
  },

  // Before `video`: its last step turns the button off for the lab's site in the last browser.
  async videoFocus(t) {
    // Headless Firefox refuses the player's fullscreen request and Tab's focus on it here.
    if (t.kind === "firefox") throw new Skip("headless Firefox denies fullscreen and keyboard focus to the test page");
    return videoFocus(t.session);
  },

  // #689: the sample feeds at their sites' addresses (served by the browser's protocol).
  async feeds(t) {
    if (!WIN) throw new Skip(NO_UIA);
    return feedVideos(t);
  },

  async video(t) {
    return videoButton(t.session, t.last);
  },

  // #474: a tab opened while the browser can't reach MDM's host (MDM being installed or updated,
  // the host failing to start) gets the video button within seconds once the host is back, with
  // no reload; its click then starts MDM, and after MDM quits and starts again under the open
  // tab a click still reaches it. The host's program is moved away while the page loads (the
  // browser can't start it), then put back.
  async reconnect(t) {
    const s = t.session;
    const nmhost = exe("misterdownloadmanager-nmhost");
    const away = `${nmhost}.e2e-away`;
    const page = `${LAB}/page?video=${encodeURIComponent("/gen/1M/clip.mp4")}`;
    const clip = `${LAB}/gen/1M/clip.mp4`;
    const status = async () => {
      const popup = await s.tab(`${s.base}popup.html`);
      try {
        // The popup's page may still be loading: asked again until it answers.
        const said = await until("the extension's status", 10000, () =>
          popup.evaluate(`(globalThis.browser ?? chrome).storage.local.get({ status: { why: null } }).then((v) => JSON.stringify(v.status.why))`).catch(() => ""),
        );
        return JSON.parse(said);
      } finally {
        await popup.close();
      }
    };
    const click = async (v, label) => {
      const b = await hover(s, v);
      if (!b) throw new Error(`${label}: no button over the video`);
      await s.mouse(await mainPart(s, b), (b.t + b.b) / 2, "click");
    };
    const done = [];
    await stopApp();
    try {
      fs.renameSync(nmhost, away);
      await s.navigate(page);
      const v = await rect(s, "#video");
      if (await hover(s, v)) throw new Error("the host can't start: a button showed all the same");
      const why = await status();
      if (!why) throw new Error("the host can't start: the toolbar button says connected");
      done.push(`host away: no button, the toolbar button says ${why}`);
      fs.renameSync(away, nmhost);
      // The page asks again (video.js ASK_MS), with no reload.
      await s.mouse(5, 5);
      await sleep(PANEL_KEPT_MS);
      if (!(await hover(s, v))) throw new Error("the host back: no button in the open tab without a reload");
      done.push("host back: the button in the open tab, no reload");
      // MDM isn't running: the click starts it, which takes the video. (Its earlier copy of the
      // file first off its list: a duplicate would be asked about.)
      await startApp();
      const earlier = (await mdmList()).filter((d) => d.url === clip).map((d) => d.id);
      if (earlier.length) await ipc({ action: "delete", ids: earlier });
      await stopApp();
      await click(v, "MDM not running");
      const first = await until("the video's file in MDM's list (MDM started by the host)", 45000, async () =>
        (await mdmList().catch(() => [])).find((d) => d.url === clip),
      );
      if (ourApps().length !== 1) throw new Error(`${ourApps().length} instances run after the click`);
      const said = await status();
      if (said && said !== "update-app") throw new Error(`MDM reached, the toolbar button still says ${said}`);
      done.push(`click: MDM started, ${first.file} in its list, the toolbar button connected`);
      // MDM quits and starts again under the open tab: the next click reaches the new one.
      await ipc({ action: "delete", ids: [first.id] });
      await stopApp();
      await startApp();
      const before = new Set((await mdmList()).map((d) => d.id));
      // The first click's green "Added to MDM" over (6 s): a click on it isn't a new one.
      await s.mouse(5, 5);
      await until("the first click's state over", 10000, async () => (await s.pill(PILL_STATE).catch(() => null))?.state !== "sent");
      await click(v, "MDM started again");
      const again = await until("the video's file in the restarted MDM's list", 15000, async () => (await mdmList()).find((d) => !before.has(d.id))).catch(async (e) => {
        throw new Error(`${e.message} (the button: ${JSON.stringify(await s.pill(PILL_STATE))})`);
      });
      if (again.url !== clip) throw new Error(`MDM downloads ${again.url}`);
      done.push(`MDM quit and started again: the next click reached it (${again.file})`);
    } finally {
      if (fs.existsSync(away)) fs.renameSync(away, nmhost);
      await s.mouse(5, 5);
    }
    return done.join("; ");
  },

  // MDM not running: the host starts it (off the screens, with the test's data folder: both
  // reach it through the browser's environment) and hands the download over.
  async start(t) {
    await stopApp();
    const file = `e2e-start-${t.name}.zip`;
    await clickLink(t, `/gen/1M/${file}`);
    await arrived(file, 1 << 20, "Compressed", 60000);
    const pids = ourApps();
    if (pids.length !== 1) throw new Error(`${pids.length} instances run after the download`);
    // Off the screens: read on Windows only (NO_UIA).
    const shown = WIN ? pids.flatMap((p) => appWindows(p, "", true)) : [];
    await keptNothing(t.dl);
    await stopApp();
    if (shown.length) throw new Error(`the instance the host started shows on the screen: ${shown.join(", ")}`);
    return `started by the host (pid ${pids[0]})${WIN ? ", off the screens" : ""}, stopped`;
  },

  // #157: Send downloads to → Another Mister Download Manager. The popup pairs with a server (its
  // address and a code); a link clicked then downloads there, into the server's folders, the
  // browser's cookies not sent: neither the browser nor this computer's MDM keeps it. The popup
  // goes back to This computer at the end.
  async remote(t) {
    const r = await remoteServer();
    const s = t.session;
    const file = `e2e-remote-${t.name}.zip`;
    const popup = await s.tab(`${s.base}popup.html`);
    try {
      await until("the popup", 10000, () => popup.evaluate(`document.readyState === "complete" && Boolean(document.getElementById("to-remote"))`).catch(() => false));
      await sleep(300);
      // Another MDM chosen: its address and code boxes show (the popup's script may still be
      // starting: chosen again until they do).
      await until("the address and code boxes", 10000, () =>
        popup.evaluate(`(document.getElementById("remote-form").hidden && document.getElementById("to-remote").click(), !document.getElementById("remote-form").hidden)`),
      );
      const code = await r.code();
      await popup.evaluate(`(() => {
        document.getElementById("remote-url").value = ${JSON.stringify(r.url)};
        document.getElementById("remote-code").value = ${JSON.stringify(code)};
        document.getElementById("remote-pair").click();
        return true; })()`);
      // Paired: the boxes close and the status names the server.
      const host = new URL(r.url).host;
      const said = await until("paired", 15000, () =>
        popup.evaluate(`(() => {
          const bad = document.getElementById("remote-said");
          if (!bad.hidden && bad.textContent) return "refused: " + bad.textContent;
          const text = document.getElementById("status-text").textContent;
          const ok = document.getElementById("remote-form").hidden && document.getElementById("status").classList.contains("ok");
          return ok && text.includes(${JSON.stringify(host)}) ? text : ""; })()`),
      ).catch(async (e) => {
        e.message += ` (the popup: ${await popup.evaluate(`document.getElementById("status-text").textContent`).catch(() => "?")})`;
        throw e;
      });
      if (said.startsWith("refused")) throw new Error(`pairing ${said}`);
      await clickLink(t, `/gen/1M/${file}`);
      const at = await until(`${file} in the server's folder`, 45000, () => findFile(r.downloads, file, 1 << 20));
      // The server's requests (not the browser's own first one, which it cancels): none with the
      // browser's cookies.
      const theirs = (await requests(`/gen/1M/${file}`)).filter((e) => e.user_agent !== t.ua);
      if (!theirs.length) throw new Error("no request from the server");
      if (theirs.some((e) => (e.cookie || "").length)) throw new Error(`a cookie went to the server's download: ${theirs.map((e) => e.cookie).join("; ")}`);
      if ((await mdmList().catch(() => [])).some((d) => d.file === file)) throw new Error("this computer's MDM got it too");
      await keptNothing(t.dl);
      return `paired (${said}); ${file} downloaded by the server into ${path.relative(r.downloads, at)} (${theirs.length} request(s), no cookie), not by this computer`;
    } finally {
      await popup.evaluate(`(document.getElementById("to-here").click(), true)`).catch(() => {});
      await sleep(500);
      await popup.close();
    }
  },
};

// Known product bugs: a case listed here still runs, and its failure is reported as BUG (with
// what was found) instead of FAILED. Key: "case" or "browser:case".
// A product failure known and not fixed yet, by case ("browser:case" or "case"): listed as BUG
// (still failing the run) rather than FAILED.
const KNOWN_BUGS = {};

// The registry keys `nmhost --register` and `--unregister` touch (crates/nmhost REGISTRY_KEYS,
// FIREFOX_KEY, CHROME_EXTERNAL_KEY): each browser's host entry and Chrome's external extension,
// per user and for everyone (the latter only read: an installed copy's). [key, 32-bit view].
const HOST = "com.misterdownloadmanager.host";
const REG_KEYS = ["HKCU", "HKLM"].flatMap((root) => [
  ...[
    "Software\\Google\\Chrome\\NativeMessagingHosts",
    "Software\\Chromium\\NativeMessagingHosts",
    "Software\\Microsoft\\Edge\\NativeMessagingHosts",
    "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts",
    "Software\\Mozilla\\NativeMessagingHosts",
  ].map((k) => [`${root}\\${k}\\${HOST}`, false]),
  [`${root}\\Software\\Google\\Chrome\\Extensions`, true],
]);

// Linux and macOS: the host's manifests (`<HOST>.json`) in the home folder's browser folders,
// found by a walk (each browser family has its own places: crates/ipc browsers.rs host_dirs),
// as {key: path, bytes}. The walk skips build and package folders.
function manifestFiles() {
  const home = process.env.HOME || "";
  const skip = new Set(["target", "node_modules", ".cargo", ".rustup", ".git", ".cache"]);
  const found = [];
  const walk = (dir, depth) => {
    let entries = [];
    try {
      entries = fs.readdirSync(dir, { withFileTypes: true });
    } catch {
      return;
    }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory() && !e.isSymbolicLink() && depth < 6 && !skip.has(e.name)) walk(p, depth + 1);
      else if (e.name === `${HOST}.json`) found.push(p);
    }
  };
  walk(home, 0);
  return found;
}
function unixSnapshot(extra = []) {
  return [...new Set([...manifestFiles(), ...extra])].map((key) => ({ key, bytes: fs.existsSync(key) ? fs.readFileSync(key) : null }));
}
// The manifests as they were before the run: each one written since is put back or removed.
function unixRestore(before, written) {
  const was = new Map(before.map((b) => [b.key, b.bytes]));
  const changed = [];
  for (const a of unixSnapshot(written)) {
    const b = was.get(a.key) ?? null;
    if ((a.bytes === null && b === null) || (a.bytes && b && a.bytes.equals(b))) continue;
    changed.push(a.key);
    if (b) fs.writeFileSync(a.key, b);
    else fs.rmSync(a.key, { force: true });
  }
  const again = new Map(unixSnapshot(written).map((a) => [a.key, a.bytes]));
  const differ = [...new Set([...was.keys(), ...again.keys()])].filter((k) => {
    const [x, y] = [was.get(k) ?? null, again.get(k) ?? null];
    return !((x === null && y === null) || (x && y && x.equals(y)));
  });
  return { changed, differ };
}

// Each key's export (reg export: its values, byte for byte), or null when it isn't there.
export function regSnapshot(dir) {
  if (!WIN) return unixSnapshot();
  fs.mkdirSync(dir, { recursive: true });
  return REG_KEYS.map(([key, wow32], i) => {
    const file = path.join(dir, `key${i}.reg`);
    fs.rmSync(file, { force: true });
    const view = wow32 ? ["/reg:32"] : [];
    const r = spawnSync("reg", ["export", key, file, "/y", ...view], { stdio: "ignore" });
    return { key, view, file, bytes: r.status === 0 ? fs.readFileSync(file) : null };
  });
}

// The keys as they were before the run: compared byte for byte; one that differs (per user only,
// never HKLM) is removed and imported again from its copy. What was found, for the summary.
export function regRestore(before, dir, written = []) {
  if (!WIN) return unixRestore(before, written);
  const after = regSnapshot(dir);
  const changed = [];
  for (const [i, b] of before.entries()) {
    const a = after[i];
    const same = (a.bytes === null && b.bytes === null) || (a.bytes && b.bytes && a.bytes.equals(b.bytes));
    if (same) continue;
    changed.push(b.key);
    if (!b.key.startsWith("HKCU")) continue;
    spawnSync("reg", ["delete", b.key, "/f", ...b.view], { stdio: "ignore" });
    if (b.bytes) spawnSync("reg", ["import", b.file, ...b.view], { stdio: "ignore" });
  }
  const again = changed.length ? regSnapshot(path.join(dir, "again")) : after;
  const differ = before.filter((b, i) => !((again[i].bytes === null && b.bytes === null) || (again[i].bytes && b.bytes && again[i].bytes.equals(b.bytes))));
  return { changed, differ: differ.map((b) => b.key) };
}

// E2E_HOST_NAME=com.mdm.sometest (Windows): the host is registered under that test name only, for
// this run's browsers only (per user), instead of `nmhost --register`'s every browser under the
// real name: a browser you have open never reaches the test's host. The extensions loaded are
// pack.ps1's with that name in background.js (Firefox's family: its folder, as a temporary
// add-on, with the same add-on ID). Its keys are removed at the end.
const TEST_HOST_KEYS = {
  chrome: "Software\\Google\\Chrome\\NativeMessagingHosts",
  brave: "Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts",
  edge: "Software\\Microsoft\\Edge\\NativeMessagingHosts",
  // Helium is built with Chromium's branding: it reads Chromium's key, then Chrome's (#685).
  helium: "Software\\Chromium\\NativeMessagingHosts",
  // Firefox's forks read Mozilla's key too.
  firefox: "Software\\Mozilla\\NativeMessagingHosts",
  waterfox: "Software\\Mozilla\\NativeMessagingHosts",
};
function testHost(name, browsers, nmhost) {
  if (!WIN) throw new Error("E2E_HOST_NAME: Windows only");
  if (!/^[a-z0-9_]+(\.[a-z0-9_]+)+$/.test(name) || name === HOST) throw new Error(`E2E_HOST_NAME: ${name} isn't a test host name`);
  const other = browsers.filter((b) => !TEST_HOST_KEYS[b]);
  if (other.length) throw new Error(`E2E_HOST_NAME: not for ${other.join(", ")}`);
  // pack.ps1's folder for `kind`, copied with the test host's name in its background.js.
  const renamedCopy = (kind) => {
    const dir = path.join(work, `extension-${kind}`);
    fs.cpSync(path.join(root, "target", "extension", kind), dir, { recursive: true });
    const bg = path.join(dir, "background.js");
    const code = fs.readFileSync(bg, "utf8");
    const renamed = code.replace(`const HOST = "${HOST}";`, `const HOST = ${JSON.stringify(name)};`);
    if (renamed === code) throw new Error("background.js: its host's name wasn't found");
    fs.writeFileSync(bg, renamed);
    return dir;
  };
  const kinds = new Set(browsers.map((b) => BROWSERS[b][0]));
  const manifests = {};
  const write = (kind, more) => {
    const file = path.join(work, "host", `${name}.${kind}.json`);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, JSON.stringify({ name, description: "Mister Download Manager (test)", path: nmhost, type: "stdio", ...more }, null, 2));
    manifests[kind] = file;
  };
  if (kinds.has("chromium")) {
    const dir = renamedCopy("chromium");
    // The unpacked extension's ID, from its manifest's key (as Chromium makes it).
    const key = JSON.parse(fs.readFileSync(path.join(dir, "manifest.json"), "utf8")).key;
    const hex = crypto.createHash("sha256").update(Buffer.from(key, "base64")).digest("hex").slice(0, 32);
    const id = [...hex].map((c) => String.fromCharCode(97 + parseInt(c, 16))).join("");
    write("chromium", { allowed_origins: [`chrome-extension://${id}/`] });
    chromiumExtension = dir;
  }
  if (kinds.has("firefox")) {
    const dir = renamedCopy("firefox");
    const id = JSON.parse(fs.readFileSync(path.join(dir, "manifest.json"), "utf8")).browser_specific_settings.gecko.id;
    if (id !== FIREFOX_ID) throw new Error(`the Firefox add-on's ID is ${id}, not ${FIREFOX_ID}`);
    write("firefox", { allowed_extensions: [id] });
    firefoxExtension = { type: "path", path: dir };
  }
  // One key per browser family's registry place (Firefox and its forks share one).
  const entries = [...new Map(browsers.map((b) => [`HKCU\\${TEST_HOST_KEYS[b]}\\${name}`, manifests[BROWSERS[b][0]]])).entries()];
  const keys = entries.map(([k]) => k);
  const there = (k) => spawnSync("reg", ["query", k], { stdio: "ignore" }).status === 0;
  for (const k of keys) if (there(k)) throw new Error(`${k} is there already: not touching it`);
  for (const [k, file] of entries) {
    if (spawnSync("reg", ["add", k, "/ve", "/t", "REG_SZ", "/d", file, "/f"], { stdio: "ignore" }).status !== 0) {
      for (const [added] of entries) spawnSync("reg", ["delete", added, "/f"], { stdio: "ignore" });
      throw new Error(`reg add ${k}`);
    }
  }
  return {
    keys,
    // Removed; the ones still there after (none).
    remove: () => {
      for (const k of keys) spawnSync("reg", ["delete", k, "/f"], { stdio: "ignore" });
      return keys.filter(there);
    },
  };
}

// Is Internet Download Manager running (E2E_IDM_OK=1)? Even with its 127.0.0.1 exception it takes
// a PDF sent "inline; filename=..." at the network level (the browser gets "204 No Content",
// IDM downloads the file itself), so that case is left out while it runs.
let idmRunning = false;

async function run() {
  const args = process.argv.slice(2);
  const only = (args.find((a) => a.startsWith("--only=")) || "").slice(7).split(",").filter(Boolean);
  const wanted = args.filter((a) => !a.startsWith("--"));
  const names = (wanted.length ? wanted : Object.keys(BROWSERS)).filter((n) => {
    if (!BROWSERS[n]) throw new Error(`unknown browser ${n}`);
    return wanted.length || BROWSERS[n][1].some((p) => fs.existsSync(p));
  });
  for (const c of only) if (!CASES[c]) throw new Error(`unknown case ${c} (${Object.keys(CASES).join(", ")})`);
  // IDM's browser integration takes browsers' downloads at the network level (the browser gets
  // "204 No Content"): it would take the test's files and show its own windows. E2E_IDM_OK=1 when
  // IDM leaves 127.0.0.1 alone (an exception for it in its options).
  if (process.platform === "win32") {
    const idm = spawnSync("tasklist", ["/FI", "IMAGENAME eq IDMan.exe", "/NH"], { encoding: "utf8" }).stdout;
    idmRunning = /IDMan\.exe/i.test(idm);
    if (idmRunning && process.env.E2E_IDM_OK !== "1") throw new Error("Internet Download Manager is running: quit it first (it takes the browsers' downloads), or set E2E_IDM_OK=1 if it leaves 127.0.0.1 alone");
  }
  for (const p of [appExe, exe("misterdownloadmanager-nmhost"), exe("labserver")]) {
    if (!fs.existsSync(p)) throw new Error(`build it first: ${p}`);
  }
  const t0 = Date.now();
  fs.rmSync(work, { recursive: true, force: true });
  data = path.join(work, "data");
  downloads = path.join(work, "Downloads");
  fs.mkdirSync(data, { recursive: true });
  // Captured downloads start at once (no Download File Info) into folders under target/e2e; the
  // automation API answers what the list holds.
  const dirs = ["General", "Compressed", "Documents", "Music", "Programs", "Video", "Torrents"].map((c) => [
    c,
    c === "General" ? downloads : path.join(downloads, c),
  ]);
  fs.writeFileSync(
    path.join(data, "downloads.json"),
    JSON.stringify({
      next_id: 1,
      items: [],
      version: 2,
      settings: {
        // E2E_APP_LANG: MDM's own language for a pull request's pictures (what the app sends
        // the menu, #852, is in it), English otherwise.
        language: process.env.E2E_APP_LANG || "en",
        language_asked: true,
        extension_never_ask: true,
        show_start_dialog: false,
        automation_api: true,
        // The default types plus XZ, for the proxy case (a type other download managers leave alone).
        auto_types: "3GP 7Z AAC ACE AIF APK ARJ ASF AVI BIN BZ2 EXE GZ GZIP IMG ISO LZH M4A M4V MKV MOV MP3 MP4 MPA MPE MPEG MPG MSI MSU OGG OGV PDF PLJ PPS PPT QT R0* R1* RA RAR RM RMVB SEA SIT SITX TAR TIF TIFF WAV WMA WMV XZ Z ZIP",
        category_dirs: dirs,
      },
    }),
  );
  // A yt-dlp for the streams case (#155): E2E_YTDLP, else one on PATH, chosen as the person's
  // own copy (Helpers: <data>/tools/programs.json), so nothing is downloaded.
  const found = process.env.E2E_YTDLP || spawnSync(process.platform === "win32" ? "where" : "which", ["yt-dlp"], { encoding: "utf8" }).stdout.split(/\r?\n/)[0];
  const version = found && fs.existsSync(found) ? spawnSync(found, ["--version"], { encoding: "utf8" }).stdout.trim() : "";
  // An ffmpeg for the qualities case (streams with their sound apart are merged): E2E_FFMPEG,
  // else one on PATH, chosen the same way.
  const where = (name) => spawnSync(process.platform === "win32" ? "where" : "which", [name], { encoding: "utf8" }).stdout.split(/\r?\n/)[0];
  const ffFound = process.env.E2E_FFMPEG || where("ffmpeg");
  const ffVersion = ffFound && fs.existsSync(ffFound) ? (/ffmpeg version (\S+)/.exec(spawnSync(ffFound, ["-version"], { encoding: "utf8" }).stdout) || [])[1] || "" : "";
  const programs = {};
  if (version) {
    ytdlp = found;
    programs["yt-dlp"] = { version, exe: found };
  }
  if (ffVersion) {
    ffmpeg = ffFound;
    programs.ffmpeg = { version: ffVersion, exe: ffFound };
  }
  if (Object.keys(programs).length) {
    fs.mkdirSync(path.join(data, "tools"), { recursive: true });
    fs.writeFileSync(path.join(data, "tools", "programs.json"), JSON.stringify(programs));
  }
  // The browsers pass these on to the host, and the host to an app it starts.
  process.env.MDM_DATA_DIR = data;
  process.env.MDM_OFFSCREEN = "1";

  before = new Set(appProcesses().map(([pid]) => pid));
  const server = spawn(exe("labserver"), ["--bind", `127.0.0.1:${PORT}`], { stdio: "ignore", windowsHide: true });
  const proxy = startProxy();
  const nmhost = exe("misterdownloadmanager-nmhost");
  const results = [];
  let regBefore = null;
  let regCheck = null;
  let written = [];
  // This run ran `nmhost --register` under the real name: only then is it unregistered at the end
  // (a run stopped before, e.g. by an installed copy's registration, must leave that one alone).
  let registered = false;
  // E2E_HOST_NAME's registration (testHost), and what was left of it at the end.
  let test = null;
  let testLeft = [];
  try {
    await until("the lab server", 10000, () => fetch(`${LAB}/`).then((r) => r.ok, () => false));
    // An installed Mister Download Manager's registration is never replaced or removed.
    // Linux and macOS: its manifests (files) are put back byte for byte at the end, so a lab
    // machine's (E2E_REPLACE_HOST=1) may be replaced while the run lasts. E2E_HOST_NAME never
    // touches the real name (testHost), so it runs beside an installed copy.
    const key = "HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\com.misterdownloadmanager.host";
    const installed = WIN ? spawnSync("reg", ["query", key], { stdio: "ignore" }).status === 0 : regSnapshot().some((m) => m.bytes) && process.env.E2E_REPLACE_HOST !== "1";
    if (installed && !process.env.E2E_HOST_NAME) {
      throw new Error("Mister Download Manager's host is already registered (an installed copy): not touching it (E2E_HOST_NAME runs beside it)");
    }
    regBefore = regSnapshot(path.join(work, "registry", "before"));
    if (process.env.E2E_HOST_NAME) {
      test = testHost(process.env.E2E_HOST_NAME, names, nmhost);
    } else {
      registered = true;
      const reg = spawnSync(nmhost, ["--register"], { encoding: "utf8" });
      // Where it wrote (Linux, macOS), for the restore.
      written = [...(reg.stdout || "").matchAll(/^registered \S+ in (.+)$/gm)].map((m) => path.join(m[1].trim(), `${HOST}.json`));
      if (reg.status !== 0) throw new Error(`nmhost --register: ${reg.stderr || reg.stdout}`);
    }
    for (const name of names) {
      const [kind, candidates] = BROWSERS[name];
      const bin = candidates.find((p) => fs.existsSync(p));
      // A Snap (Ubuntu's Firefox) reads its profile only from its own folder, and starts the host
      // through the desktop portal, without the browser's environment (MDM_DATA_DIR): the host
      // would reach this computer's own MDM, not the test instance, so only the remote case runs.
      const snap = Boolean(bin) && bin.startsWith("/snap/");
      const profile = snap ? path.join(process.env.HOME, "snap", name, "common", "mdm-e2e-profile") : path.join(work, `profile-${name}`);
      const dl = path.join(work, `browser-${name}`);
      fs.rmSync(profile, { recursive: true, force: true });
      fs.mkdirSync(profile, { recursive: true });
      fs.mkdirSync(dl, { recursive: true });
      // Linux: with --user-data-dir, Chromium reads the host's manifest from that folder.
      const chromiumManifest = written.find((w) => !/mozilla/i.test(w));
      if (!WIN && kind === "chromium" && chromiumManifest) {
        fs.mkdirSync(path.join(profile, "NativeMessagingHosts"), { recursive: true });
        fs.copyFileSync(chromiumManifest, path.join(profile, "NativeMessagingHosts", `${HOST}.json`));
      }
      const started = Date.now();
      let session;
      try {
        if (!bin) throw new Error("not installed");
        await startApp();
        // The extension, installed, asked the host for the rules: the host notes the browser in
        // extension-seen (by the name Options gives it).
        const seen = path.join(data, "extension-seen");
        fs.rmSync(seen, { force: true });
        let seenAs = "";
        // E2E_OLD_APP=1: an app older than extension-seen (0.2.4, for the compatibility runs:
        // a new extension with a released app): the extension is given time to reach it instead.
        const ready = () =>
          snap
            ? sleep(3000).then(() => (seenAs = "(a Snap: not this test's instance; only the remote case runs)"))
            : process.env.E2E_OLD_APP
            ? sleep(8000).then(() => (seenAs = "(an older app: not reported)"))
            : until(`${name}'s extension reaching the host`, 20000, () => {
            try {
              seenAs = fs.readFileSync(seen, "utf8").split("\t")[0];
              return seenAs;
            } catch {
              return false;
            }
          }).then(() => sleep(300)); // its answer stored
        session = await (kind === "firefox" ? firefox : chromium)(bin, profile, dl, `${LAB}/page?link=none`, ready);
        const t = { name, kind, session, dl, snap, last: name === names[names.length - 1], ua: await session.evaluate("navigator.userAgent") };
        // Firefox's family: whether it asks what to do with each new type (E2E_FIREFOX_ASK).
        const asks = session.inBrowser
          ? `, asks for new types: ${await session.inBrowser(`Services.prefs.getBoolPref("browser.download.always_ask_before_handling_new_types", false)`)}`
          : "";
        results.push([name, "browser", "ok", `${kind} ${session.major} (to MDM: ${seenAs}), extension ${session.extension}${asks}`, Date.now() - started]);
        for (const [label, fn] of Object.entries(CASES)) {
          if (only.length && !only.includes(label)) continue;
          const at = Date.now();
          try {
            if (snap && label !== "remote") throw new Skip("a Snap browser starts the host without the test's environment");
            await startApp();
            await clearLog();
            const detail = await fn(t);
            results.push([name, label, "ok", detail || "", Date.now() - at]);
          } catch (e) {
            const bug = KNOWN_BUGS[`${name}:${label}`] || KNOWN_BUGS[label];
            const status = e instanceof Skip ? "skipped" : bug ? "BUG" : "FAILED";
            results.push([name, label, status, bug ? `${bug} -- ${e.message}` : e.message, Date.now() - at]);
            // What the browser kept, so the next case starts clean.
            for (const f of fs.readdirSync(dl)) fs.rmSync(path.join(dl, f), { force: true, recursive: true });
          }
          console.error(`  ${name} ${label}: ${results[results.length - 1][2]}`);
          // E2E_DEBUG: what the extension did (Chromium).
          if (session.trace && process.env.E2E_DEBUG) console.error(`${name} ${label} trace:`, await session.trace());
        }
      } catch (e) {
        results.push([name, "browser", "FAILED", e.message, Date.now() - started]);
      } finally {
        if (session) session.close();
        if (snap) fs.rmSync(profile, { recursive: true, force: true, maxRetries: 5, retryDelay: 500 });
      }
    }
  } finally {
    if (test) testLeft = test.remove();
    else if (registered) spawnSync(nmhost, ["--unregister"]);
    if (regBefore) regCheck = regRestore(regBefore, path.join(work, "registry", "after"), written);
    await stopApp().catch(() => {});
    await stopRemote().catch(() => {});
    for (const pid of ourPids()) killPid(pid);
    killTree(server);
    proxy.close();
  }
  for (const [name, label, status, detail, ms] of results) {
    console.log(`${name.padEnd(8)} ${label.padEnd(9)} ${status.padEnd(8)} ${(ms / 1000).toFixed(1).padStart(5)}s  ${detail}`);
  }
  if (regCheck) {
    const kept = regBefore.filter((b) => b.bytes).map((b) => b.key);
    console.log(
      regCheck.differ.length
        ? `${WIN ? "registry" : "host manifests"}: DIFFERS after the run: ${regCheck.differ.join(", ")}`
        : `${WIN ? "registry" : "host manifests"}: ${regBefore.length} ${WIN ? "keys" : "files"} byte-identical to before the run${regCheck.changed.length ? ` (put back: ${regCheck.changed.join(", ")})` : ""}; present before: ${kept.join(", ") || "none"}`,
    );
  }
  if (test) console.log(testLeft.length ? `test host: STILL REGISTERED: ${testLeft.join(", ")}` : `test host: ${test.keys.join(", ")} registered for the run, removed`);
  console.log(`total ${((Date.now() - t0) / 1000).toFixed(0)}s`);
  if (results.some((r) => r[2] === "FAILED" || r[2] === "BUG") || (regCheck && regCheck.differ.length) || testLeft.length) process.exit(1);
}

// Run when started, not when another script imports its helpers (tools/readme/browser-shots.mjs).
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) run().catch((e) => {
  console.error(e.message);
  process.exit(1);
});
