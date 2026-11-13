// Mister Download Manager's browser extension: the background part (Chrome, Edge, Brave, Opera
// and Vivaldi run it as a service worker, Firefox as a background script; one file for both, see
// manifest.json and manifest.firefox.json).
//
// Downloads of the file types Mister Download Manager takes are handed to it: the browser's own
// download is cancelled, and the address, referrer, cookies, User-Agent, file name, size, type
// and (for a form) the posted body go to the app through the native messaging host
// (crates/nmhost). When it can't be reached the download stays with the browser, so nothing is
// lost, and the toolbar button says why, linking the help page's section for it
// (docs/browser-help.md). The browser may stop this script whenever it is idle: what must last
// (the rules, the status, the pause) is kept in storage, not in memory.

const ext = globalThis.browser ?? globalThis.chrome;
const HOST = "com.misterdownloadmanager.host";
const VERSION = ext.runtime.getManifest().version;
const HELP = "https://misterdownloadmanager.com/help/browser-help.html";
const action = ext.action ?? ext.browserAction;
// Safari has no downloads API at all (nor proxy): it can't take a download from the browser, so
// the links clicked in a page are handed over instead (`link`, below; page.js).
const SAFARI = !ext.downloads;
// Firefox has no pause before the file name (onDeterminingFilename): its own paths below.
const FIREFOX = !SAFARI && !ext.downloads.onDeterminingFilename;
// Brave and Opera say who they are only in the client hints, Firefox's forks (Zen, LibreWolf,
// Floorp, Waterfox) only in runtime.getBrowserInfo, which only Firefox's family has. Both go to
// the host as `brands` (Options lists them by name; crates/ipc capture::browser_named).
const BRANDS = (globalThis.navigator?.userAgentData?.brands || []).map((b) => b.brand);
const APP_NAME = ext.runtime.getBrowserInfo
  ? ext.runtime.getBrowserInfo().then((i) => (i && i.name) || null, () => null)
  : Promise.resolve(null);

const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || key;

// ---- the native host ---------------------------------------------------------------------

// How long an answer may take: the host starts Mister Download Manager when it isn't running
// and waits for it (crates/nmhost START_WAIT); the browser's download waits meanwhile.
const HOST_WAIT = 30000;

function withTimeout(promise, ms) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("no answer")), ms);
    }),
  ]).finally(() => clearTimeout(timer));
}

// Why the browser couldn't reach the host, as the help page's sections name it.
function hostProblem(error) {
  const m = String((error && error.message) || error);
  if (/not found|no such native application/i.test(m)) return "not-installed";
  if (/forbidden/i.test(m)) return "forbidden";
  if (/no answer/i.test(m)) return "not-running";
  return "host-failed";
}

// Every message says which browser and extension version it comes from (the app keeps each
// browser's status, and the rules depend on the browser). Never throws: failures come back as
// `{ok: false, why}`.
async function native(message, ms = HOST_WAIT) {
  // A download for another MDM (a server, #157): sent there, not to this computer's.
  if (typeof message.url === "string" && message.url) {
    const target = await remoteTarget();
    if (target) return remoteSend(target, message);
  }
  const app = await APP_NAME;
  const brands = app ? [...BRANDS, app] : BRANDS;
  const sent = { ...message, userAgent: navigator.userAgent, brands, v: VERSION };
  try {
    const reply = (await withTimeout(ext.runtime.sendNativeMessage(HOST, sent), ms)) || {};
    if (reply.version) {
      await noteStatus(older(reply.version, VERSION) ? "update-app" : null, reply.version);
      // The answer to `rules` also says whether the app looks videos up for the button.
      await noteLookup(reply.videoLookup === true);
    } else if (reply.ok || reply.why === "options") await noteStatus(null);
    else if (reply.why && reply.why !== "refused") await noteStatus(reply.why);
    return reply;
  } catch (e) {
    const why = hostProblem(e);
    // With another MDM chosen, this computer's needn't be there.
    if (!(await remoteTarget())) await noteStatus(why);
    return { ok: false, why, error: String((e && e.message) || e) };
  }
}

// ---- another MDM (#157): a server's, through its remote control ------------------------------

// {url, token} while downloads go to another MDM instead of this computer's, else null.
async function remoteTarget() {
  const { remote } = await ext.storage.local.get({ remote: null });
  return remote && remote.url && remote.token ? remote : null;
}

// One request to the remote control's API at `base` (https://server:8463/). Never throws:
// {status, body}, status 0 when it isn't reached (not running, or its certificate not accepted
// in this browser yet). The browser's cookies never go with it.
async function remoteCall(base, path, body, token) {
  try {
    const r = await withTimeout(
      fetch(new URL(path, base), {
        method: "POST",
        headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
        body: JSON.stringify(body),
        credentials: "omit",
        cache: "no-store",
        redirect: "error",
      }),
      15000,
    );
    let json = null;
    try {
      json = await r.json();
    } catch {
      json = null;
    }
    return { status: r.status, body: json };
  } catch {
    return { status: 0, body: null };
  }
}

// A download handed to the other MDM: its address only (a site's sign-in stays in this browser).
async function remoteSend(target, message) {
  const r = await remoteCall(target.url, "api/add", { url: message.url, start: true }, target.token);
  if (r.status === 200 && r.body && !r.body.error) return { ok: true };
  if (r.status === 401) await ext.storage.local.set({ remote: null }); // unpaired there
  return { ok: false, why: "refused" };
}

// The file types the app takes by default (mdm_ipc::capture::DEFAULT_AUTO_TYPES; a test keeps
// them the same): what is caught for another MDM when this computer has none to ask.
const DEFAULT_TYPES =
  "00* 3DM 3DS 3GP 3MF 7Z AAB AAC ABC ACE AFDESIGN AFPHOTO AI AIF AIFF APE APK APKM APKS APPIMAGE APPX APPXBUNDLE ARJ ARROW ARW ASF AU AVI AVRO AZW AZW3 BAK BAM BIN BLEND BZ BZ2 C4D CAB CAF CBR CBZ CCD CHD CKPT CPIO CR2 CR3 CRAM CSO DAE DAT DCM DDS DEB DFF DIVX DJVU DMG DNG DOC DOCX DSF DWG DXF EPS EPUB EXE EXR F3D F4V FASTA FASTQ FB2 FBX FEATHER FIG FITS FLAC FLATPAK FQ GCODE GCZ GEM GGML GGUF GLB GLTF GPKG GRIB GRIB2 GZ GZIP H5 HDF5 HDR IGES IGS IMG INDD IPA ISO ISZ JAR JOBLIB JSONL KERAS KRA LAS LAZ LLAMAFILE LWO LZ LZ4 LZH LZMA M2T M2TS M2V M4A M4B M4V MA MAT MAX MB MBTILES MDF MDS MID MIDI MKA MKV MLMODEL MOBI MOV MP3 MP4 MPA MPE MPEG MPG MSI MSIX MSIXBUNDLE MSP MSU MTS MXF NC NDJSON NEF NEMO NII NPY NPZ NRG NSP NSZ NUPKG OBJ ODP ODS ODT OGG OGM OGV ONNX OPUS ORC ORF OTF OVA OVF PAK PARQUET PB PBF PDF PICKLE PK3 PK4 PKG PKL PLJ PLY POTX PPS PPSX PPT PPTX PSB PSD PT PTH QCOW QCOW2 QT R0* R1* R2* R3* R4* R5* R6* R7* R8* R9* RA RAF RAR RDS RM RMVB RPM RTF RUN RVZ RW2 SAFETENSORS SEA SF2 SFZ SHP SIT SITX SKETCH SKP SLDASM SLDPRT SNAP SQLITE SQLITE3 STEP STL STP TAR TBZ TBZ2 TFLITE TGA TGZ TIF TIFF TLZ TORRENT TTA TTC TTF TXZ TZST UNITYPACKAGE USD USDA USDC USDZ VDB VDI VHD VHDX VMDK VOB VPK VSIX WAV WBFS WEBM WHL WIM WMA WMV WRL WTV WUA WV X3D XAPK XCF XCI XLS XLSB XLSM XLSX XZ Z Z0* ZIP ZIPX ZPR ZST ZTL";

// Pair with the MDM at `address` with a code it shows; {ok} or {ok: false, why, left}.
async function remotePair(address, code) {
  let base;
  try {
    base = new URL(String(address).trim());
  } catch {
    return { ok: false, why: "address" };
  }
  if (base.protocol !== "https:") return { ok: false, why: "address" };
  base = `${base.origin}/`;
  const name = ext.i18n.getMessage("remote_device_name") || "Browser";
  const r = await remoteCall(base, "api/pair", { code: String(code), name }, null);
  if (r.status === 200 && r.body && typeof r.body.token === "string") {
    await ext.storage.local.set({ remote: { url: base, token: r.body.token } });
    await refreshRules();
    await showStatus();
    return { ok: true };
  }
  const why = { 0: "unreachable", 403: "wrong-code", 410: "no-code", 429: "too-soon" }[r.status] || "failed";
  return { ok: false, why, left: r.body && r.body.left };
}

// Back to this computer: unpaired there too (when it answers).
async function remoteOff() {
  const target = await remoteTarget();
  if (target) await remoteCall(target.url, "api/forget", {}, target.token);
  await ext.storage.local.set({ remote: null });
  await refreshRules();
  await showStatus();
  return { ok: true };
}

// a < b, both "x.y.z".
// Is version `a` an older release than `b`? Only the first two numbers count: the store can publish
// a bug-fix add-on before its app is out, and nothing it offers needs that app.
function older(a, b) {
  const pa = String(a).split(/[.-]/).map(Number);
  const pb = String(b).split(/[.-]/).map(Number);
  for (let i = 0; i < 2; i++) {
    if ((pa[i] || 0) !== (pb[i] || 0)) return (pa[i] || 0) < (pb[i] || 0);
  }
  return false;
}

// ---- status: the toolbar button's badge and title, the popup's first line ------------------

let status = null; // {why, app, at, videoLookup}
async function readStatus() {
  if (!status) status = (await ext.storage.local.get({ status: { why: null } })).status;
  return status;
}

// `why`: null when connected, else the reason (the help page's section). Written only when it
// changes.
async function noteStatus(why, app) {
  const now = await readStatus();
  if (now.why === why && (!app || now.app === app)) return;
  const was = !now.why || now.why === "update-app";
  status = { ...now, why, app: app || now.app || null, at: Date.now() };
  await ext.storage.local.set({ status });
  await showStatus();
  // Unreachable, PDFs and music open in the browser again (and back once connected).
  if (was !== (!why || why === "update-app")) await syncOpenedRule();
}

// Whether this computer's MDM looks videos up for the button over videos (its qualities listed in
// the page, video.js), kept with the status beside the app's version: from every answer to
// `rules`, and dropped when a lookup's answer says otherwise (an older host or app).
async function noteLookup(on) {
  const now = await readStatus();
  if (Boolean(now.videoLookup) === on) return;
  status = { ...now, videoLookup: on };
  await ext.storage.local.set({ status });
}

async function showStatus() {
  const why = (await remoteTarget()) ? null : (await readStatus()).why;
  const { paused } = await ext.storage.local.get({ paused: false });
  try {
    await action.setBadgeBackgroundColor({ color: why ? "#C42B1C" : "#6B6B6B" });
    await action.setBadgeText({ text: why && why !== "update-app" ? "!" : paused ? "II" : "" });
    await action.setTitle({ title: why ? t(`status_${why.replace(/-/g, "_")}`) : paused ? t("status_paused") : t("status_connected") });
  } catch {
    // No toolbar button (a browser without one): the popup and the help page still say it.
  }
}

// ---- the rules: Options in the app, kept here between the browser's stops ------------------

// {types, capture, captureOff, excludedSites, excludedAddresses, opened, minKb, at}; null until
// first asked. `captureOff`: why `capture` is false (a host before #208 doesn't say).
let rules = null;

async function storedRules() {
  if (!rules) rules = (await ext.storage.local.get({ rules: null })).rules;
  return rules;
}

// One question at a time: callers meanwhile get its answer.
let asking = null;
// When Safari's page.js last had them asked for (once a minute at most, from every page).
let rulesAskedAt = 0;
function refreshRules() {
  asking = asking || askRules().finally(() => (asking = null));
  return asking;
}

async function askRules() {
  const reply = await native({ rules: true });
  if (reply.ok && typeof reply.types === "string") {
    rules = {
      types: reply.types,
      capture: reply.capture !== false,
      captureOff: typeof reply.captureOff === "string" ? reply.captureOff : "",
      excludedSites: reply.excludedSites || "",
      excludedAddresses: Array.isArray(reply.excludedAddresses) ? reply.excludedAddresses : [],
      opened: reply.opened !== false,
      minKb: Number(reply.minKb) || 0,
      at: Date.now(),
    };
    await ext.storage.local.set({ rules });
    await syncOpenedRule();
    return rules;
  }
  // No MDM on this computer, another one chosen: the app's default file types (the browser
  // keeps the files it would open).
  if (await remoteTarget()) {
    rules = { types: DEFAULT_TYPES, capture: true, excludedSites: "", excludedAddresses: [], opened: false, minKb: 0, at: Date.now() };
    // Kept as the app's are: Safari's page.js holds links by the stored copy.
    await ext.storage.local.set({ rules });
    return rules;
  }
  return null;
}

// The rules to decide by now: the stored ones at once (refreshed meanwhile once a minute old;
// the host checks every offer again), else asked. None while the host can't be reached: then
// the browser keeps everything.
async function currentRules() {
  const r = await storedRules();
  const { why } = await readStatus();
  if (!r || (why && why !== "update-app")) return refreshRules();
  if (Date.now() - r.at > 60000) refreshRules();
  return r;
}

function typePatterns(list) {
  return list
    .split(/\s+/)
    .filter(Boolean)
    .map((t) => new RegExp("^" + t.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*") + "$", "i"));
}

function extensionOf(name) {
  const base = name.split(/[\\/]/).pop() || "";
  const dot = base.lastIndexOf(".");
  return dot > 0 ? base.slice(dot + 1) : "";
}

// As the app matches them (mdm_ipc::capture): `*.example.com` takes the domain and its
// subdomains; an address pattern's `*` any run of characters.
function hostMatches(host, pattern) {
  let p = pattern.trim().toLowerCase().replace(/^[a-z]+:\/\//, "").split("/")[0];
  if (p.startsWith("*.")) {
    p = p.slice(2);
    return host === p || host.endsWith("." + p);
  }
  return p !== "" && host === p;
}

function wildcard(pattern, text) {
  const re = new RegExp("^" + pattern.toLowerCase().split("*").map((s) => s.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*") + "$");
  return re.test(text.toLowerCase());
}

// An address pattern with a scheme is the whole address; without one, the rest after it.
function addressMatches(pattern, url) {
  const p = pattern.trim();
  if (!p) return false;
  return wildcard(p, p.includes("://") ? url : url.replace(/^[a-z][a-z0-9+.-]*:\/\//i, ""));
}

// Which of Options' lists leaves `url` to the browser: "site", "address", or "" (neither).
function excludedBy(r, url) {
  let host = "";
  try {
    host = new URL(url).hostname.toLowerCase();
  } catch {
    return "site";
  }
  if (r.excludedSites.split(/\s+/).some((p) => p && hostMatches(host, p))) return "site";
  if (r.excludedAddresses.some((p) => addressMatches(p, url))) return "address";
  return "";
}

// ---- downloads left to the browser: why, for the popup and MDM's log (#208) ----------------

// A host's refusal by Options names the rule (`declined`); one before #208 says it only in its
// error text, by the rule's Rust name (mdm_ipc::capture::Declined).
const DECLINED_NAMES = {
  IntegrationOff: "integration-off",
  Browser: "browser",
  FileType: "file-type",
  Site: "site",
  Address: "address",
  Small: "small",
};
function declinedOf(reply) {
  if (typeof reply.declined === "string" && reply.declined) return reply.declined;
  const m = /left to the browser by Options \((\w+)\)/.exec(String(reply.error || ""));
  return (m && DECLINED_NAMES[m[1]]) || "";
}

function siteOf(url) {
  try {
    return new URL(url).hostname;
  } catch {
    return "";
  }
}

// The last download left to the browser by Options (or because the extension may not read the
// site): kept for the popup ({why, name, site, at}; a private window's without its name or
// site), and, when the extension decided alone (`told` false), told to the host for MDM's log.
// The host logs its own refusals itself. A download taken later clears it.
async function noteLeft(why, name, url, incognito, told) {
  const declined = incognito ? { why, name: "", site: "", at: Date.now() } : { why, name, site: siteOf(url), at: Date.now() };
  await ext.storage.local.set({ declined });
  if (told) return;
  // Under a key of its own, with no address: an older host refuses it as one it can't
  // download, and hands nothing over.
  await native({ declined: { why, name: declined.name, site: declined.site }, ...(incognito ? { private: true } : {}) });
}

// ---- what the page told us: forms posted, links clicked with Alt -------------------------

// Forms posted in a page or frame, by address: a download they lead to is posted again by the
// app, with the same body (it can't be sent as a GET). Kept two minutes.
const posts = new Map();
const BODY_LIMIT = 512 * 1024;
// The Referer each page load was sent with, by request (Firefox, two minutes).
const referrers = new Map();

function forget(map, ms) {
  const now = Date.now();
  for (const [k, v] of map) if (now - v.at > ms) map.delete(k);
}

// A form's body as the browser sent it: its fields (formData: put together again in the
// form's own encoding once its Content-Type is known, `postOf`), or the raw bytes when they
// are text. Anything else (a file upload) can't be sent again: `body: null` leaves its download
// to the browser.
function formBody(details) {
  const rb = details.requestBody;
  if (!rb) return { body: null };
  if (rb.formData) {
    const fields = Object.entries(rb.formData).flatMap(([k, values]) => values.map((v) => [k, String(v)]));
    const size = fields.reduce((n, [k, v]) => n + k.length + v.length + 2, 0);
    return size > BODY_LIMIT ? { body: null } : { fields, type: "application/x-www-form-urlencoded" };
  }
  if (Array.isArray(rb.raw) && rb.raw.every((p) => p.bytes)) {
    const size = rb.raw.reduce((n, p) => n + p.bytes.byteLength, 0);
    if (size > BODY_LIMIT) return { body: null };
    const bytes = new Uint8Array(size);
    let at = 0;
    for (const p of rb.raw) {
      bytes.set(new Uint8Array(p.bytes), at);
      at += p.bytes.byteLength;
    }
    try {
      return { body: new TextDecoder("utf-8", { fatal: true }).decode(bytes), type: null };
    } catch {
      return { body: null };
    }
  }
  return { body: null };
}

if (ext.webRequest) {
  const navigations = { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] };
  ext.webRequest.onBeforeRequest.addListener(
    (d) => {
      if (d.method !== "POST") return;
      forget(posts, 120000);
      posts.set(d.url, { ...formBody(d), at: Date.now(), id: d.requestId });
    },
    navigations,
    ["requestBody"],
  );
  // The Content-Type the form went with (a multipart boundary must stay the same), and (Firefox)
  // the page a load came from, for a download caught from its answer (`headersReceived`).
  ext.webRequest.onBeforeSendHeaders.addListener(
    (d) => {
      const header = (n) => (d.requestHeaders || []).find((h) => h.name.toLowerCase() === n);
      if (FIREFOX) {
        forget(referrers, 120000);
        const ref = header("referer");
        if (ref && ref.value) referrers.set(d.requestId, { url: ref.value, at: Date.now() });
      }
      if (d.method !== "POST") return;
      const p = posts.get(d.url);
      const ct = header("content-type");
      if (p && p.id === d.requestId && ct && ct.value) p.type = ct.value;
    },
    navigations,
    ["requestHeaders"],
  );
}

// The form posted to `url` (within two minutes), as the app sends it again: urlencoded fields,
// or a raw text body with its own type. null: it can't be sent again (a file in it; a multipart
// form, whose fields the browser gives without its files): the browser keeps the download.
function postOf(url) {
  const p = posts.get(url);
  if (!p || Date.now() - p.at > 120000) return undefined;
  if (p.body === null || (p.fields && /multipart/i.test(p.type || ""))) return null;
  if (!p.fields) return { body: p.body, type: p.type };
  const params = new URLSearchParams();
  for (const [k, v] of p.fields) params.append(k, v);
  return { body: params.toString(), type: "application/x-www-form-urlencoded" };
}

// Where page loads' redirects started, by where they ended (two minutes): Firefox names a
// download by its last address only, and the app keeps the first (an expired signed link is
// renewed from it).
const redirected = new Map();
if (ext.webRequest && ext.webRequest.onBeforeRedirect) {
  ext.webRequest.onBeforeRedirect.addListener(
    (d) => {
      forget(redirected, 120000);
      const first = (redirected.get(d.url) || {}).from || d.url;
      redirected.set(d.redirectUrl, { from: first, at: Date.now() });
    },
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] },
  );
}

// Pages the browser loaded in a tab or frame (two minutes), by address (the listener below).
const loadedPages = new Map();

// The address a download was first asked for: the browser's own (Chromium), else the start of
// the redirects that led to it. Chromium also counts the page a script or a meta refresh left
// from as a download's start (#374: a thank-you page setting location.href to the installer);
// that page isn't the file, and asking it again would save the page under the file's name.
function firstUrl(item) {
  if (item.finalUrl && item.finalUrl !== item.url) {
    const page = loadedPages.get(item.url);
    return page && Date.now() - page.at < 120000 ? item.finalUrl : item.url;
  }
  const r = redirected.get(item.url);
  return r && Date.now() - r.at < 120000 ? r.from : item.url;
}

// Links clicked with Alt held (page.js): the browser keeps their download.
const bypass = new Map();

// ---- the media a tab loaded (#38) ------------------------------------------------------------

// What the tab's pages loaded that can be downloaded, per tab, in the order seen: streams'
// manifests (HLS .m3u8, DASH .mpd; a master playlist comes before its variants) and plain video
// and sound files (a <video>'s or <audio>'s own file, or a large whole file a player fetched). The
// popup lists them (`mediaList`); the download button over a video sends the stream its frame
// loaded first (`streamOf`): a player that plays from a blob: has no address of its own, and
// yt-dlp reads the manifest (qualities, separate audio). Segments and byte ranges are pieces of a
// stream, never listed. Bounded: MEDIA_MAX per tab, MEDIA_TABS tabs; forgotten when the tab goes
// to another page or closes.
const media = new Map(); // tab id -> {page, items: [{url, kind, frameId, mime, size, info}], frames}
const MEDIA_MAX = 60;
const MEDIA_TABS = 50;
const URL_MAX = 8192;
// A player's own fetch of a whole file is listed from this size (smaller ones are pieces).
const FETCHED_MIN = 1 << 20;
// Less than this of a single file, played by the page itself: a page's sound effect (mediaKind).
const SOUND_MIN = 64 << 10;
const MANIFEST = /\.(m3u8|mpd)(?:[?#]|$)/i;
const SEGMENT_TYPE = /^(video\/mp2t|video\/iso\.segment|audio\/iso\.segment)$/;
const SEGMENT_EXT = /\.(ts|m4s|m4f|cmfv|cmfa|cmft)$/;
const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|wmv|flv|3gp|ogv|mp3|m4a|aac|ogg|oga|opus|flac|wav|wma)$/;
const AUDIO_EXT = /^(mp3|m4a|aac|oga|ogg|opus|flac|wav|wma)$/;
// The types a manifest is sometimes served as, besides its own (#171): plain text, no type, a
// file's, XML. Such an answer is only noted (`maybe`, MAYBE_MAX a tab), never listed unread.
const MAYBE_TYPE = /^(text\/plain|(application|binary)\/octet-stream|(application|text)\/xml|)$/;
const MAYBE_MAX = 20;
// A manifest's own types: HLS's (Apple's and the x- ones), DASH's (and its older video/ name).
const MANIFEST_TYPE = /mpegurl|dash\+xml|dash\.mpd/;
const MANIFEST_MAX = 2 << 20;

const pathOf = (url) => url.split(/[?#]/)[0];
const headerOf = (d, name) => ((d.responseHeaders || []).find((h) => h.name.toLowerCase() === name) || {}).value || "";

// What a response the tab got is, for the list: {kind: "hls" | "dash" | "video" | "audio", mime,
// size}, or null (not media, a segment, a piece of a file, too small to be a file of its own).
function mediaKind(d) {
  if (d.tabId < 0 || !/^https?:/i.test(d.url) || d.url.length > URL_MAX) return null;
  if (d.statusCode && (d.statusCode < 200 || d.statusCode > 299)) return null;
  const mime = headerOf(d, "content-type").split(";")[0].trim().toLowerCase();
  const path = pathOf(d.url).toLowerCase();
  // The address says manifest, the type says it isn't: a page or JSON that names one (SVT Play's
  // switcher answers .../dash-full.mpd with JSON naming the real manifest; loaded first, it went
  // to the app and yt-dlp found no video in it, #377). Read before believed, like any `maybe`.
  if (/\.(m3u8|mpd)$/.test(path) && !MANIFEST_TYPE.test(mime) && !MAYBE_TYPE.test(mime)) {
    return Number(headerOf(d, "content-length")) > MANIFEST_MAX ? null : { kind: "maybe", mime, size: 0 };
  }
  if (/\.m3u8$/.test(path) || /mpegurl/.test(mime)) return { kind: "hls", mime, size: 0 };
  if (/\.mpd$/.test(path) || /dash\+xml/.test(mime)) return { kind: "dash", mime, size: 0 };
  if (SEGMENT_TYPE.test(mime) || SEGMENT_EXT.test(path)) return null;
  const ext = (MEDIA_EXT.exec(path) || [])[1] || "";
  // Maybe a manifest whose address and type don't say so (#171): a player's own small text or
  // untyped answer, read only when a stream is asked for and none is known (`sniffTab`).
  if (d.type !== "media" && !ext && MAYBE_TYPE.test(mime) && d.statusCode !== 206 && !headerOf(d, "content-range")) {
    const len = Number(headerOf(d, "content-length"));
    return len > MANIFEST_MAX ? null : { kind: "maybe", mime, size: 0 };
  }
  // A page's own short sound (a click, a voice-search chime: 6 kB on YouTube in Edge), not media
  // anyone came for: the list would show nothing else on a page whose video is streamed.
  const whole = Number(headerOf(d, "content-length"));
  if (d.type === "media" && whole > 0 && whole < SOUND_MIN) return null;
  const typed = /^(video|audio)\//.test(mime) || mime === "application/ogg";
  const untyped = !mime || /^(application|binary)\/octet-stream$/.test(mime);
  if (!typed && !(ext && untyped)) return null;
  // The whole file's size: a range's total, else the length of a whole answer.
  const range = headerOf(d, "content-range");
  const total = /\/\s*(\d+)\s*$/.exec(range);
  const size = total ? Number(total[1]) : d.statusCode === 206 ? 0 : Number(headerOf(d, "content-length")) || 0;
  // A <video>'s or <audio>'s own file comes in ranges; a player's fetch of a range, or of a
  // small file, is a piece of a stream.
  if (d.type !== "media" && (d.statusCode === 206 || range || /[?&](range|bytestart|byterange)=/i.test(d.url) || size < FETCHED_MIN)) {
    return null;
  }
  const audio = mime.startsWith("audio/") || (!mime.startsWith("video/") && AUDIO_EXT.test(ext));
  return { kind: audio ? "audio" : "video", mime, size };
}

const isStream = (it) => it.kind === "hls" || it.kind === "dash";
// The folder a manifest's pieces are in ("https://cdn/v/"), or null for a site's root (every
// file of the site would be its piece).
function folderOf(url) {
  const p = pathOf(url);
  const dir = p.slice(0, p.lastIndexOf("/") + 1);
  return /^https?:\/\/[^/]+\/.+/i.test(dir) ? dir : null;
}

function noteMedia(d, found, sent) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  const old = tab.items.find((it) => it.url === d.url);
  if (old) {
    if (found.size && !old.size) old.size = found.size;
    if (sent) old.sent = sent;
    return;
  }
  // A file in a stream's own folder is one of its pieces (a DASH init segment, a fragment).
  const inStream = (it) => isStream(it) && folderOf(it.url) && d.url.startsWith(folderOf(it.url));
  if (!isStream(found) && tab.items.some(inStream)) return;
  tab.items.push({ url: d.url, kind: found.kind, frameId: d.frameId || 0, mime: found.mime, size: found.size, ...(sent ? { sent } : {}) });
  // Over the cap: the oldest go, but each frame's first stream stays (the button sends it).
  while (tab.items.length > MEDIA_MAX) {
    const firsts = new Set();
    const i = tab.items.findIndex((it) => {
      if (!isStream(it) || firsts.has(it.frameId)) return true;
      firsts.add(it.frameId);
      return false;
    });
    tab.items.splice(Math.max(i, 0), 1);
  }
  // The tab seen last goes last; the least recent ones go past MEDIA_TABS.
  media.delete(d.tabId);
  media.set(d.tabId, tab);
  for (const id of media.keys()) {
    if (media.size <= MEDIA_TABS) break;
    media.delete(id);
  }
  keepMedia();
}

// A response that may be a manifest (#171), noted apart: the newest MAYBE_MAX of the tab.
function noteMaybe(d, found, sent) {
  const tab = media.get(d.tabId) || { page: null, items: [] };
  if (tab.items.some((it) => it.url === d.url)) return;
  tab.maybe = (tab.maybe || []).filter((m) => m.url !== d.url);
  tab.maybe.push({ url: d.url, frameId: d.frameId || 0, mime: found.mime, ...(sent ? { sent } : {}) });
  if (tab.maybe.length > MAYBE_MAX) tab.maybe.splice(0, tab.maybe.length - MAYBE_MAX);
  if (!media.has(d.tabId)) {
    media.set(d.tabId, tab);
    for (const id of media.keys()) {
      if (media.size <= MEDIA_TABS) break;
      media.delete(id);
    }
  }
  keepMedia();
}

// What a text starts as: an HLS playlist ("hls"), a DASH manifest ("dash"), or neither ("").
function manifestKind(text) {
  const t = String(text).replace(/^﻿/, "").trimStart();
  if (t.startsWith("#EXTM3U")) return "hls";
  // An XML declaration, comments, then the MPD element (with a namespace prefix or not).
  if (/^(?:<\?xml[^>]*\?>\s*)?(?:<!--[\s\S]*?-->\s*)*<(?:[\w-]+:)?MPD[\s>/]/.test(t)) return "dash";
  return "";
}

// The start of a response (SNIFF_BYTES), fetched again with the site's cookies as `probe` does:
// what it is (`manifestKind`), or "" (not a manifest, refused, too slow).
const SNIFF_BYTES = 4096;
async function sniff(url) {
  let reader = null;
  try {
    const r = await withTimeout(fetch(url, { credentials: "include" }), 5000);
    if (!r.ok) return "";
    if (!r.body || !r.body.getReader) return manifestKind((await withTimeout(r.text(), 5000)).slice(0, SNIFF_BYTES));
    reader = r.body.getReader();
    const dec = new TextDecoder();
    let text = "";
    while (text.length < SNIFF_BYTES) {
      const { done, value } = await withTimeout(reader.read(), 5000);
      if (done) break;
      text += dec.decode(value, { stream: true });
    }
    return manifestKind(text);
  } catch {
    return "";
  } finally {
    // The rest is never read.
    if (reader) reader.cancel().catch(() => {});
  }
}

// The tab's possible manifests read, those of `frames` only (a Set of frame ids, or null: all),
// each once: the ones that are become its streams, in the order they were loaded. One reading at
// a time per tab: a second ask waits for the first.
const sniffing = new Map();
function sniffTab(tabId, frames) {
  const run = async () => {
    const tab = media.get(tabId);
    if (!tab || !tab.maybe || !tab.maybe.length) return;
    const asked = tab.maybe.filter((m) => !frames || frames.has(m.frameId));
    if (!asked.length) return;
    tab.maybe = tab.maybe.filter((m) => !asked.includes(m));
    const kinds = await Promise.all(asked.map((m) => sniff(m.url)));
    // The tab went to another page meanwhile: its old page's answers don't count.
    if (media.get(tabId) !== tab) return;
    asked.forEach((m, i) => {
      if (kinds[i]) noteMedia({ tabId, url: m.url, frameId: m.frameId }, { kind: kinds[i], mime: m.mime, size: 0 }, m.sent);
    });
    keepMedia();
  };
  const next = (sniffing.get(tabId) || Promise.resolve()).then(run, run);
  sniffing.set(tabId, next);
  next.finally(() => sniffing.get(tabId) === next && sniffing.delete(tabId)).catch(() => {});
  return next;
}

// #900: what the browser sent with each of a tab's requests for media and data (its Referer, Origin
// and Cookie headers), by request, until its answer starts: a stream's stay with it, and go to the
// app with it, which asks for the stream, its playlists and each of its pieces as the page's
// player did. A site's video server may serve its own pages' players only (their Origin and
// Referer), and a player in another site's frame may have cookies no store lists (a browser that
// keeps a frame's cookies apart, for that page only). In memory only, as long as `media`.
const SENT_HEADERS = ["referer", "origin", "cookie"];
const sentWith = new Map();
const SENT_MAX = 512;
const SENT_TYPES = { urls: ["<all_urls>"], types: ["xmlhttprequest", "media", "other"] };
if (ext.webRequest && ext.webRequest.onSendHeaders) {
  try {
    ext.webRequest.onSendHeaders.addListener(
      (d) => {
        if (d.tabId < 0) return;
        const sent = {};
        for (const h of d.requestHeaders || []) {
          const name = String(h.name).toLowerCase();
          if (SENT_HEADERS.includes(name) && typeof h.value === "string" && h.value) sent[name] = h.value;
        }
        // A redirect's next request is the one answered.
        sentWith.delete(d.requestId);
        if (Object.keys(sent).length) sentWith.set(d.requestId, sent);
        // Requests never answered (cancelled, failed before an answer) don't pile up.
        while (sentWith.size > SENT_MAX) sentWith.delete(sentWith.keys().next().value);
      },
      SENT_TYPES,
      // Chromium shows a request's Referer, Origin and Cookie to extensions that ask for its
      // extra headers only; Firefox shows them all and knows no such option.
      FIREFOX ? ["requestHeaders"] : ["requestHeaders", "extraHeaders"],
    );
  } catch {
    // A browser without extraHeaders: streams go with their page as the Referer, as before.
  }
}
const takeSent = (requestId) => {
  const sent = sentWith.get(requestId);
  sentWith.delete(requestId);
  return sent;
};
if (ext.webRequest && ext.webRequest.onErrorOccurred) {
  ext.webRequest.onErrorOccurred.addListener((d) => sentWith.delete(d.requestId), SENT_TYPES);
}

if (ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(
    (d) => {
      if (d.type === "main_frame" || d.type === "sub_frame") {
        const type = (d.responseHeaders || []).find((h) => h.name.toLowerCase() === "content-type");
        if (d.statusCode === 200 && type && /^\s*(text\/html|application\/xhtml)/i.test(type.value || "")) {
          forget(loadedPages, 120000);
          loadedPages.set(d.url, { at: Date.now() });
        }
        return;
      }
      const sent = takeSent(d.requestId);
      const found = mediaKind(d);
      if (found) mediaBack.then(() => (found.kind === "maybe" ? noteMaybe(d, found, sent) : noteMedia(d, found, sent)));
    },
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame", "xmlhttprequest", "media", "other"] },
    ["responseHeaders"],
  );
}
ext.tabs.onRemoved.addListener((id) => mediaBack.then(() => media.delete(id) && keepMedia()));
// Another page in the tab: what the last one loaded goes (not for a move within the page, #part).
ext.tabs.onUpdated.addListener((id, change) => {
  if (!change.url) return;
  mediaBack.then(() => {
    const page = change.url.split("#")[0];
    const tab = media.get(id);
    if (tab && tab.page === page) return;
    media.set(id, { page, items: [] });
    keepMedia();
  });
});

// Kept in the session's storage (memory only, gone with the browser): the browser stops this
// script when idle, and a tab's player loaded its stream long before it is asked for. Read back
// once at start, merged with what was seen meanwhile; everything else waits for it.
const session = ext.storage.session;
let keeping = null;
function keepMedia() {
  if (!session || keeping) return;
  keeping = setTimeout(() => {
    keeping = null;
    session.set({ media: [...media] }).catch(() => {});
  }, 200);
}
const mediaBack = session
  ? session
      .get({ media: [] })
      .then((v) => {
        for (const [id, kept] of v.media || []) {
          const now = media.get(id);
          const fresh = ((now && now.items) || []).filter((s) => !kept.items.some((o) => o.url === s.url));
          const maybe = [...(kept.maybe || []), ...((now && now.maybe) || [])].filter((m, i, all) => all.findIndex((o) => o.url === m.url) === i);
          media.set(id, { ...kept, page: (now && now.page) || kept.page, items: [...kept.items, ...fresh].slice(-MEDIA_MAX), maybe: maybe.slice(-MAYBE_MAX) });
        }
      })
      .catch(() => {})
  : Promise.resolve();

// The stream a frame's player loaded first (its master playlist), else the tab's.
function streamOf(sender) {
  const list = ((sender.tab && media.get(sender.tab.id)) || { items: [] }).items.filter(isStream);
  const mine = list.find((s) => s.frameId === sender.frameId);
  return (mine || list[0] || {}).url || "";
}

// The same for a player that plays from a blob: (asked by the button over it or the right-click
// menu), reading the tab's possible manifests when no stream is known (#171): the frame's own
// first, then the tab's other frames'.
async function streamFor(sender) {
  const tabId = sender.tab && sender.tab.id;
  if (tabId === undefined || tabId === null) return streamOf(sender);
  const frameId = sender.frameId || 0;
  const own = () => ((media.get(tabId) || { items: [] }).items.some((it) => isStream(it) && it.frameId === frameId));
  if (!own()) await sniffTab(tabId, new Set([frameId]));
  if (!own() && !streamOf(sender)) await sniffTab(tabId, null);
  return streamOf(sender);
}

// ---- a stream's manifest: its qualities, length, and whether it is protected -------------------

// An HLS tag's attributes (KEY=value,KEY="value").
function hlsAttrs(s) {
  const out = {};
  for (const [, k, v] of s.matchAll(/([A-Z0-9-]+)=("[^"]*"|[^,]*)/g)) out[k] = v.replace(/^"|"$/g, "");
  return out;
}

// A copy-protected (DRM) key: a key system's (Apple's FairPlay, Google's Widevine, Microsoft's
// PlayReady), or SAMPLE-AES, which no download tool decrypts. AES-128 with a plain key
// ("identity") is not DRM: yt-dlp fetches its key as the player does.
function drmKey(a) {
  const method = (a.METHOD || "").toUpperCase();
  if (!method || method === "NONE") return false;
  return method.startsWith("SAMPLE-AES") || (a.KEYFORMAT !== undefined && a.KEYFORMAT !== "identity");
}

// {heights, duration, drm, live, children}: `children`, the addresses (without their query) a
// master playlist names: its variants and renditions, the same stream.
function parseHls(text, base) {
  const lines = text.replace(/^﻿/, "").split(/\r?\n/).map((l) => l.trim());
  if (!lines[0].startsWith("#EXTM3U")) return null;
  const info = { heights: [], duration: 0, drm: false, live: true, children: [] };
  const child = (uri) => {
    try {
      info.children.push(pathOf(new URL(uri, base).href));
    } catch {
      // Not an address.
    }
  };
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    const value = l.slice(l.indexOf(":") + 1);
    if (l.startsWith("#EXT-X-STREAM-INF:")) {
      const h = Number((/x(\d+)$/.exec(hlsAttrs(value).RESOLUTION || "") || [])[1]);
      if (h) info.heights.push(h);
      const uri = lines.slice(i + 1).find((x) => x && !x.startsWith("#"));
      if (uri) child(uri);
    } else if (l.startsWith("#EXT-X-MEDIA:") || l.startsWith("#EXT-X-I-FRAME-STREAM-INF:")) {
      const a = hlsAttrs(value);
      if (a.URI) child(a.URI);
    } else if (l.startsWith("#EXT-X-KEY:") || l.startsWith("#EXT-X-SESSION-KEY:")) {
      if (drmKey(hlsAttrs(value))) info.drm = true;
    } else if (l.startsWith("#EXTINF:")) {
      info.duration += parseFloat(value) || 0;
    } else if (l.startsWith("#EXT-X-ENDLIST") || /^#EXT-X-PLAYLIST-TYPE:\s*VOD/.test(l)) {
      info.live = false;
    }
  }
  // A master playlist says nothing of an end: its variants do.
  if (info.children.length) info.live = false;
  return info;
}

// Seconds of an ISO 8601 duration (PT1H2M3.5S).
function isoSeconds(d) {
  const m = /P(?:(\d+)D)?T?(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?/.exec(d || "") || [];
  return (+m[1] || 0) * 86400 + (+m[2] || 0) * 3600 + (+m[3] || 0) * 60 + (+m[4] || 0);
}

// A DASH manifest, read as text (a service worker has no XML parser). Any ContentProtection is
// encryption: Widevine, PlayReady, or CENC with a key the site gives its own player only.
function parseDash(text) {
  if (!/<(?:\w+:)?MPD[\s>]/.test(text)) return null;
  const attr = (tag, name) => (new RegExp(`\\s${name}\\s*=\\s*["']([^"']*)["']`).exec(tag) || [])[1];
  const mpd = (/<(?:\w+:)?MPD\b[^>]*>/.exec(text) || [""])[0];
  const heights = [];
  for (const [tag] of text.matchAll(/<(?:\w+:)?(?:Representation|AdaptationSet)\b[^>]*>/g)) {
    const h = Number(attr(tag, "height"));
    if (h) heights.push(h);
  }
  return {
    heights,
    duration: isoSeconds(attr(mpd, "mediaPresentationDuration")),
    drm: /<(?:\w+:)?ContentProtection\b/.test(text),
    live: attr(mpd, "type") === "dynamic",
    children: [],
  };
}

// A manifest, fetched again by the extension (the player's copy can't be read): its details, or
// {failed: true}: a site that refuses the extension's request (it checks the page that asks) is
// listed without them.
async function probe(item) {
  try {
    const r = await withTimeout(fetch(item.url, { credentials: "include" }), 5000);
    if (!r.ok || Number(r.headers.get("content-length")) > MANIFEST_MAX) return { failed: true };
    const text = await withTimeout(r.text(), 5000);
    if (text.length > MANIFEST_MAX) return { failed: true };
    return (item.kind === "hls" ? parseHls(text, item.url) : parseDash(text)) || { failed: true };
  } catch {
    return { failed: true };
  }
}

// Run in each frame of the tab: its address and its players (their file, picture size, length,
// and whether the page gave them a DRM key system).
function mediaElements() {
  const players = [...document.querySelectorAll("video, audio")].map((v) => ({
    src: v.currentSrc || v.src || "",
    w: v.videoWidth || 0,
    h: v.videoHeight || 0,
    duration: Number.isFinite(v.duration) ? v.duration : 0,
    drm: Boolean(v.mediaKeys),
  }));
  return { href: location.href, players };
}

// Whether a player in the tab plays from a blob: (a MediaSource fed by the page).
async function playsStream(tabId) {
  try {
    const found = await withTimeout(ext.scripting.executeScript({ target: { tabId, allFrames: true }, func: mediaElements }), 3000);
    return (found || []).some((f) => f && f.result && f.result.players.some((v) => /^blob:/i.test(v.src)));
  } catch {
    return false;
  }
}

// The tab's media as the popup lists them: a stream's variants inside it, each with what is known
// (qualities, length, size, protected). Manifests are read once.
async function mediaList(tabId) {
  await mediaBack;
  if (!media.get(tabId)) return [];
  const frames = {};
  try {
    const found = await withTimeout(ext.scripting.executeScript({ target: { tabId, allFrames: true }, func: mediaElements }), 3000);
    for (const f of found || []) if (f && f.result) frames[f.frameId || 0] = f.result;
  } catch {
    // A page extensions may not read: listed without its players' details.
  }
  // #171: the possible manifests of frames whose player plays from a blob: (a MediaSource), read.
  const fed = Object.entries(frames)
    .filter(([, f]) => f.players.some((v) => /^blob:/i.test(v.src)))
    .map(([id]) => Number(id));
  if (fed.length) await sniffTab(tabId, new Set(fed));
  const tab = media.get(tabId);
  if (!tab) return [];
  await Promise.all(tab.items.filter((it) => isStream(it) && !it.info).map(async (it) => (it.info = await probe(it))));
  tab.frames = Object.fromEntries(Object.entries(frames).map(([id, f]) => [id, f.href]));
  keepMedia();
  // A file's name as it will be saved (#800): the tab's title and the popup's switch name it too.
  const { mediaTitleName } = await ext.storage.local.get({ mediaTitleName: false });
  let title = "";
  try {
    title = (await ext.tabs.get(tabId)).title || "";
  } catch {
    // The tab is gone: names without its title.
  }
  const out = [];
  const shown = [];
  for (const it of tab.items) {
    // A variant of a master playlist listed already: it names it, or (unread) the variant is in
    // its folder or below.
    const p = pathOf(it.url);
    const variantOf = (s) =>
      s.info && !s.info.failed ? s.info.children.includes(p) : s.frameId === it.frameId && folderOf(s.url) && p.startsWith(folderOf(s.url));
    if (it.kind === "hls" && shown.some((s) => s.kind === "hls" && variantOf(s))) continue;
    shown.push(it);
    const frame = frames[it.frameId] || { players: [] };
    const player = frame.players.find((v) => v.src === it.url);
    const info = it.info && !it.info.failed ? it.info : null;
    // A stream is named in the Download video window; a file here, with its type (#800).
    const named = isStream(it) ? null : mediaName(it.url, it.mime, title, mediaTitleName).name;
    out.push({
      url: it.url,
      kind: it.kind,
      mime: it.mime,
      ...(named ? { name: named, ext: splitExtension(named)[1].toLowerCase() } : {}),
      size: it.size || 0,
      heights: info ? [...new Set(info.heights)].sort((a, b) => b - a) : player && player.h ? [player.h] : [],
      width: (!info && player && player.w) || 0,
      duration: (info ? info.duration : player && player.duration) || 0,
      live: Boolean(info && info.live),
      // Protected: its manifest says so, or (unread) its frame's player has a DRM key system.
      drm: info ? info.drm : isStream(it) && frame.players.some((v) => v.drm),
    });
  }
  return out;
}

// ---- a page's media file's name (#345, #493, #695, #800): as the app's engine names it
// (crates/engine/src/naming.rs, answer_name; tools/extension/test.mjs checks they agree) ---------

// The types a file's extension is taken from (naming.rs' TYPES): media, pictures, documents and
// archives, never a type that runs.
const TYPE_EXTENSIONS = {
  "video/mp4": "mp4",
  "video/x-m4v": "m4v",
  "video/webm": "webm",
  "video/x-matroska": "mkv",
  "video/quicktime": "mov",
  "video/x-msvideo": "avi",
  "video/avi": "avi",
  "video/x-ms-wmv": "wmv",
  "video/x-flv": "flv",
  "video/mpeg": "mpg",
  "video/mp2t": "ts",
  "video/3gpp": "3gp",
  "video/3gpp2": "3g2",
  "video/ogg": "ogv",
  "audio/mpeg": "mp3",
  "audio/mp3": "mp3",
  "audio/mp4": "m4a",
  "audio/x-m4a": "m4a",
  "audio/aac": "aac",
  "audio/aacp": "aac",
  "audio/webm": "weba",
  "audio/ogg": "ogg",
  "audio/opus": "opus",
  "audio/flac": "flac",
  "audio/x-flac": "flac",
  "audio/wav": "wav",
  "audio/x-wav": "wav",
  "audio/wave": "wav",
  "audio/x-ms-wma": "wma",
  "application/ogg": "ogg",
  "image/jpeg": "jpg",
  "image/png": "png",
  "image/gif": "gif",
  "image/webp": "webp",
  "image/avif": "avif",
  "application/pdf": "pdf",
  "application/epub+zip": "epub",
  "application/zip": "zip",
  "application/x-zip-compressed": "zip",
  "application/x-7z-compressed": "7z",
  "application/vnd.rar": "rar",
  "application/x-rar-compressed": "rar",
  "application/gzip": "gz",
  "application/x-gzip": "gz",
};
const TYPED = new Set(Object.values(TYPE_EXTENSIONS));
// A web server's programs (naming.rs' SCRIPT_EXTENSIONS) and a page's: such an address names the
// program that answers, not the file it sends.
const SCRIPT_EXTENSIONS = ["php", "php3", "php4", "php5", "phtml", "asp", "aspx", "ashx", "axd", "jsp", "jspx", "do", "action", "cgi", "pl", "cfm", "py", "rb"];
const PAGE_EXTENSIONS = ["htm", "html", "xhtml", "shtml", "dhtml"];

// A name's stem and extension ("clip.mp4": "clip", "mp4"); ".profile", "clip", "Mr. Smith" have none.
function splitExtension(name) {
  const dot = name.lastIndexOf(".");
  const ext = dot > 0 ? name.slice(dot + 1) : "";
  return ext && !/\s/.test(ext) ? [name.slice(0, dot), ext] : [name, ""];
}
const typeExtension = (mime) => TYPE_EXTENSIONS[String(mime || "").split(";")[0].trim().toLowerCase()] || "";
const isScriptName = (name) => SCRIPT_EXTENSIONS.includes(splitExtension(name)[1].toLowerCase());
// Its extension doesn't say what the file is: a script's, a page's, or none.
function untyped(name) {
  const ext = splitExtension(name)[1].toLowerCase();
  return !ext || SCRIPT_EXTENSIONS.includes(ext) || PAGE_EXTENSIONS.includes(ext);
}
// A name that says nothing of the file, for the tab's title to replace: a script's, none (a bare
// id, `videoplayback`), or a long hash (#345).
function meansNothing(name) {
  const [stem, ext] = splitExtension(name);
  const hash = stem.length >= 24 && /^[A-Za-z0-9=_-]+$/.test(stem) && (stem.match(/[-_]/g) || []).length <= 2;
  return !ext || isScriptName(name) || hash;
}
// The tab's title as a file's stem: what a file name can't hold becomes a space, at most 150 characters.
function cleanTitle(title) {
  const spaced = String(title || "")
    .replace(/[\u00AD\u200B-\u200F\u202A-\u202E\u2060-\u2064\u2066-\u206F\uFEFF]/g, "")
    .replace(/[\x00-\x1f\x7f-\x9f<>:"/\\|?*]/g, " ");
  return [...spaced.split(/\s+/).filter(Boolean).join(" ")].slice(0, 150).join("").replace(/[. ]+$/, "");
}
// The address's own name (naming.rs' address_name): a script's `file=` or `filename=` value naming
// a file of a known type (#695), else its last part.
function addressFileName(url) {
  const last = addressName(url);
  if (splitExtension(last)[1] && !isScriptName(last)) return last;
  try {
    for (const [k, v] of new URL(url).searchParams) {
      if (!/^(file|filename)$/i.test(k)) continue;
      const name = v.trim().split(/[\\/]/).pop().trim();
      const [stem, ext] = splitExtension(name);
      if (stem.trim() && TYPED.has(ext.toLowerCase())) return name;
    }
  } catch {
    // Not an address: its last part.
  }
  return last;
}

// A page's media file's name as the app saves it ({name, own}): its address's (a script's `file=`
// value), typed by the type its server said (`mime`) when its own extension doesn't say it
// (stream.php: stream.mp4), and the tab's title instead of a name that means nothing (a script's,
// a bare id, a hash; any on a server whose names are all random ids, #493: X's video.twimg.com).
// With the popup's switch on (`includeOriginal`), the title before a real name ("Title -
// clip.mp4"). `own`: the address names the file as the app would name it with no name sent, so
// none is (a server's Content-Disposition then still wins).
function mediaName(url, mime, title, includeOriginal = false) {
  const given = addressFileName(url);
  const typed = untyped(given) ? typeExtension(mime) : "";
  const [stem, ext] = splitExtension(given);
  const name = cleanTitle(title);
  const randomServer = siteOf(url).toLowerCase() === "video.twimg.com";
  const bare = given === addressName(url) && (meansNothing(given) || randomServer);
  const dotted = typed || ext ? `.${typed || ext}` : "";
  if (bare && name) return { name: `${name}${dotted}`, own: false };
  const own = typed ? `${stem || "download"}.${typed}` : given;
  if (includeOriginal && name && own) return { name: `${name} - ${own}`, own: false };
  return { name: own, own: !typed };
}

// One of the tab's media, handed to the app as the popup asked: a stream to the Download video
// window (yt-dlp), a file as a download; with the page that played it (the referrer), the cookies
// its own server gets, and the browser's User-Agent (`native`). A protected one is never sent.
async function mediaDownload(tabId, url) {
  const listed = await mediaList(tabId);
  const shown = listed.find((x) => x.url === url);
  if (!shown) return { ok: false, why: "gone" };
  if (shown.drm) return { ok: false, why: "drm" };
  const kept = media.get(tabId);
  const it = kept.items.find((x) => x.url === url);
  let tab;
  try {
    tab = await ext.tabs.get(tabId);
  } catch {
    return { ok: false, why: "gone" };
  }
  const frame = (kept.frames || {})[it.frameId];
  const page = frame && /^https?:/i.test(frame) ? frame : tab.url || "";
  const ctx = { incognito: Boolean(tab.incognito), storeId: tab.cookieStoreId };
  if (isStream(it)) {
    return native({
      video: true,
      // Asked for: whether the button over videos shows on this site doesn't matter.
      explicit: true,
      url: page,
      src: it.url,
      title: String(tab.title || "").slice(0, 300),
      referrer: tab.url && tab.url !== page ? tab.url : "",
      ...(await streamContext(tabId, it.url, ctx)),
      ...(ctx.incognito ? { private: true } : {}),
    });
  }
  const { mediaTitleName } = await ext.storage.local.get({ mediaTitleName: false });
  const named = mediaName(it.url, it.mime, tab.title, mediaTitleName);
  // The name the list shows; none when the address names the file as the app would (a server's
  // Content-Disposition may still name it). The title goes too: a file whose address names nothing
  // and whose type only the server or its bytes say is named by it in the app (#800).
  const title = String(tab.title || "").slice(0, 300);
  return handOff({ url: it.url, referrer: page, size: it.size, mime: it.mime, explicit: true, filename: named.own ? "" : named.name, ...(title ? { title } : {}), ...ctx });
}

// ---- the hand-off ------------------------------------------------------------------------

// Cookies from the store the download belongs to, never another: Firefox names it on the
// download (private windows and containers each have their own); Chrome's incognito store is "1".
async function cookiesFor(url, { incognito, storeId }) {
  try {
    const query = storeId ? { url, storeId } : incognito ? { url, storeId: "1" } : { url };
    return (await ext.cookies.getAll(query)) || [];
  } catch {
    return [];
  }
}

function header(list) {
  return list.map((c) => `${c.name}=${c.value}`).join("; ");
}

async function cookieHeader(url, ctx) {
  return header(await cookiesFor(url, ctx));
}

// For yt-dlp, which may ask another host of the site than the page's (a YouTube Music page is
// looked up on www.youtube.com): each cookie with the domain, path and flags the browser keeps,
// so the app files `.youtube.com`'s sign-in for every host of it and a host's own cookie for
// that host alone. The header goes too, for an older app.
async function videoCookies(url, ctx) {
  const list = await cookiesFor(url, ctx);
  const cookieJar = list.map((c) => ({
    name: c.name,
    value: c.value,
    domain: c.domain,
    hostOnly: Boolean(c.hostOnly),
    path: c.path || "/",
    secure: Boolean(c.secure),
    ...(typeof c.expirationDate === "number" ? { expirationDate: c.expirationDate } : {}),
  }));
  return { cookies: header(list), cookieJar };
}

// The cookies of a tab's stream (`videoCookies`) and what the browser sent with it (#900): the
// Referer and Origin its player asked for it with (`fetchedWith`: the app sends them again with
// its playlists and pieces), and the cookies that went with it that no store lists (a frame's
// own, kept for its page only), added for its host. Nothing more for an address the tab didn't
// load (a page's).
async function streamContext(tabId, url, ctx) {
  const c = await videoCookies(url, ctx);
  const tab = media.get(tabId);
  const it = tab && [...tab.items, ...(tab.maybe || [])].find((x) => x.url === url);
  const sent = (it && it.sent) || {};
  const web = (v) => typeof v === "string" && /^https?:\/\/\S+$/i.test(v) && v.length <= URL_MAX;
  const fetchedWith = {
    ...(web(sent.referer) ? { referer: sent.referer } : {}),
    ...(web(sent.origin) ? { origin: sent.origin } : {}),
  };
  if (typeof sent.cookie === "string" && sent.cookie) {
    let host = "";
    let secure = false;
    try {
      const u = new URL(url);
      host = u.hostname;
      secure = u.protocol === "https:";
    } catch {
      // Not an address: no cookie of its own.
    }
    const known = new Set(c.cookieJar.map((x) => x.name));
    for (const pair of sent.cookie.split(/;\s*/)) {
      const at = pair.indexOf("=");
      const name = at > 0 ? pair.slice(0, at).trim() : "";
      if (!host || !name || known.has(name)) continue;
      known.add(name);
      c.cookieJar.push({ name, value: pair.slice(at + 1), domain: host, hostOnly: true, path: "/", secure });
    }
    // The header the browser sent for this very address.
    c.cookies = sent.cookie;
  }
  return Object.keys(fetchedWith).length ? { ...c, fetchedWith } : c;
}

// Has the user left this extension the site? With access limited ("On click", some sites), its
// cookies can't be read, and a download handed over without them would fail.
async function siteAccess(url) {
  try {
    const u = new URL(url);
    // By host name: Firefox's match patterns take no port.
    return await ext.permissions.contains({ origins: [`${u.protocol}//${u.hostname}/*`] });
  } catch {
    return false;
  }
}

// ---- the browser's proxy (#42) -------------------------------------------------------------

// A VPN or proxy extension is a proxy for the browser only: the app downloads through the same
// one (Options → Proxy / Socks → "Use proxy/socks from a browser's request"), or the download
// would leave from the user's own address. What goes with each download:
// - `proxy`: the proxy the browser uses for its address, "scheme://host:port";
// - `proxyPac`: the browser's proxy auto-config script (its text), when it uses one;
// - `proxyLogin`: that proxy asked the browser for a login. The extension that set it answers
//   with its own, which no other program can have: the app says so rather than going without.
// Firefox tells each request's proxy (`proxyInfo`), what proxy extensions set per request
// (`proxy.onRequest`) included; Chromium has no such event, only its proxy settings
// (`proxy.settings.get`: a fixed server per scheme with its bypass list, or a PAC script).
const proxyLogins = new Set();
const proxyUsed = new Map();
const PAC_MAX = 256 * 1024;

// Reading the browser's proxy is the user's choice, the popup's switch (off as installed):
// - Firefox: the optional "proxy" permission, asked for by the switch and taken back by it.
// - Chromium can't make "proxy" optional (chrome.permissions), so its manifest has it from the
//   install, and the switch is the extension's own setting (`proxyOn` in storage.local). While it
//   is off, nothing of the browser's proxy is read or sent.
// `proxyAllowed` follows the permission or the setting as it changes.
const PROXY_PERMISSION = { permissions: ["proxy"] };
const PROXY_SWITCH = "proxyOn";
const [PROXY_DECLARED, PROXY_REQUIRED] = (() => {
  try {
    const m = ext.runtime.getManifest();
    const required = (m.permissions || []).includes("proxy");
    return [required || (m.optional_permissions || []).includes("proxy"), required];
  } catch {
    return [false, false];
  }
})();
// Off until checked.
let proxyAllowed = false;
async function checkProxy() {
  try {
    proxyAllowed = PROXY_REQUIRED
      ? Boolean((await ext.storage.local.get({ [PROXY_SWITCH]: false }))[PROXY_SWITCH])
      : PROXY_DECLARED && Boolean(await ext.permissions.contains(PROXY_PERMISSION));
  } catch {
    proxyAllowed = false;
  }
  if (!proxyAllowed) {
    // Nothing kept of what was seen while it was allowed.
    proxyLogins.clear();
    proxyUsed.clear();
  }
}
const proxyChecked = PROXY_DECLARED ? checkProxy() : Promise.resolve();
if (PROXY_REQUIRED && ext.storage.onChanged) {
  ext.storage.onChanged.addListener((changes, area) => {
    if (area === "local" && PROXY_SWITCH in changes) checkProxy();
  });
} else if (PROXY_DECLARED && ext.permissions && ext.permissions.onAdded) {
  ext.permissions.onAdded.addListener(checkProxy);
  ext.permissions.onRemoved.addListener(checkProxy);
}

if (PROXY_DECLARED && ext.webRequest && ext.webRequest.onAuthRequired) {
  try {
    ext.webRequest.onAuthRequired.addListener(
      (d) => {
        if (proxyAllowed && d.isProxy && d.challenger) proxyLogins.add(`${d.challenger.host}:${d.challenger.port}`.toLowerCase());
      },
      { urls: ["<all_urls>"] },
    );
  } catch {
    // Not offered here: no login is noticed.
  }
}
// Firefox: the proxy a request went through, kept for its hand-over. Noted as its response
// starts, and from its answer's headers when the extension takes the download there (#208): that
// request is cancelled, so it never starts.
function noteProxy(d) {
  if (!proxyAllowed || !d.proxyInfo || d.proxyInfo.type === "direct") return;
  forget(proxyUsed, 120000);
  proxyUsed.set(d.url, { info: d.proxyInfo, at: Date.now() });
}
if (PROXY_DECLARED && FIREFOX && ext.webRequest && ext.webRequest.onResponseStarted) {
  ext.webRequest.onResponseStarted.addListener(noteProxy, { urls: ["<all_urls>"] });
}

const hostPort = (host, port) => `${String(host).includes(":") ? `[${host}]` : host}:${port}`;

// "scheme://host:port" for one of the browser's proxy descriptions, or null.
function proxyUrl(scheme, host, port, remoteDns) {
  if (!host || !port) return null;
  const s = String(scheme || "http").toLowerCase();
  const kind =
    s === "socks" || s === "socks5" ? (remoteDns === false ? "socks5" : "socks5h") : s === "socks4" ? "socks4" : s === "https" ? "https" : s === "http" ? "http" : null;
  return kind ? `${kind}://${hostPort(host, port)}` : null;
}

// Does `pattern` (a proxy bypass entry: "<local>", "*.example.com", ".example.com", "host",
// "host:port") cover `u`? CIDR entries aren't read (the proxy is then used).
function bypassed(pattern, u) {
  const p = String(pattern || "").trim().toLowerCase();
  if (!p) return false;
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (p === "<local>") return !host.includes(".") || host === "localhost" || host === "127.0.0.1" || host === "::1";
  const bare = p.replace(/^[a-z]+:\/\//, "").replace(/:\d+$/, "");
  if (bare.startsWith("*.")) return host.endsWith(bare.slice(1));
  if (bare.startsWith(".")) return host.endsWith(bare) || host === bare.slice(1);
  return host === bare;
}

// Chromium's bypass list (its proxy rules' `bypassList`) for `u`, as Chromium reads one: "<local>"
// (a name without a dot), "[scheme://]host-pattern[:port]" with "*" anywhere (".example.com" is
// "*.example.com"), an IPv4 range "10.0.0.0/8"; and, unless "<-loopback>" is listed, the names
// Chromium never sends to a proxy (localhost, 127.x, ::1, link-local). IPv6 ranges aren't read
// (the proxy is then used).
function chromiumBypassed(list, u) {
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  const scheme = u.protocol.replace(/:$/, "");
  const port = u.port || (scheme === "https" ? "443" : "80");
  const v4 = /^\d{1,3}(\.\d{1,3}){3}$/.test(host) ? host.split(".").reduce((n, b) => n * 256 + Number(b), 0) : null;
  const ip = v4 !== null || host.includes(":");
  const rules = (list || []).map((r) => String(r).trim().toLowerCase()).filter(Boolean);
  if (!rules.includes("<-loopback>")) {
    if (host === "localhost" || host.endsWith(".localhost") || host === "::1" || (host.includes(":") && /^fe[89ab]/.test(host))) return true;
    if (v4 !== null && (host.startsWith("127.") || host.startsWith("169.254."))) return true;
  }
  return rules.some((r) => {
    if (r === "<-loopback>") return false;
    if (r === "<local>") return !ip && !host.includes(".");
    const range = /^(\d{1,3}(?:\.\d{1,3}){3})\/(\d{1,2})$/.exec(r);
    if (range) {
      if (v4 === null) return false;
      const base = range[1].split(".").reduce((n, b) => n * 256 + Number(b), 0);
      const bits = Number(range[2]);
      const size = 2 ** (32 - Math.min(bits, 32));
      return Math.floor(v4 / size) === Math.floor(base / size);
    }
    const m = /^(?:([a-z][a-z0-9+.-]*):\/\/)?(\[[^\]]+\]|[^:/]+)(?::(\d+))?\/?$/.exec(r);
    if (!m || (m[1] && m[1] !== scheme) || (m[3] && m[3] !== port)) return false;
    const pattern = m[2].replace(/^\[|\]$/g, "").replace(/^\./, "*.");
    const glob = pattern.replace(/[.+?^$|\\()[\]{}]/g, "\\$&").replace(/\*/g, ".*");
    return new RegExp("^" + glob + "$").test(host);
  });
}

// The port Chromium takes for a proxy given without one.
const chromiumPort = (scheme) => ({ https: 443, quic: 443, socks: 1080, socks4: 1080, socks5: 1080 })[String(scheme || "http").toLowerCase()] || 80;

async function pacText(url) {
  try {
    const r = await withTimeout(fetch(url, { credentials: "omit" }), 10000);
    if (!r.ok) return null;
    const text = await r.text();
    return text.length <= PAC_MAX && /FindProxyForURL/.test(text) ? text : null;
  } catch {
    return null;
  }
}

// Firefox never sends loopback addresses through its proxy settings' proxy
// (network.proxy.allow_hijacking_localhost, off by default): localhost and its names, 127.0.0.0/8, ::1.
function firefoxLoopback(u) {
  const host = u.hostname.toLowerCase().replace(/^\[|\]$/g, "");
  return host === "localhost" || host.endsWith(".localhost") || host === "::1" || /^127(\.\d{1,3}){3}$/.test(host);
}

// What the app needs to download `url` through the browser's proxy: {} when it uses none (or
// the system's, which the app reads itself), and {} unless the user switched it on (above).
async function browserProxy(url, incognito) {
  // Chromium's switch is read again for each download: the popup may have just changed it.
  await (PROXY_REQUIRED ? checkProxy() : proxyChecked);
  if (!proxyAllowed) return {};
  let u;
  try {
    u = new URL(url);
  } catch {
    return {};
  }
  const login = (host, port) => proxyLogins.has(`${host}:${port}`.toLowerCase());
  // Firefox: the proxy its own request for this address went through.
  const used = proxyUsed.get(url);
  if (used) {
    const p = used.info;
    const proxy = proxyUrl(p.type, p.host, p.port, p.proxyDNS);
    if (!proxy) return {};
    return { proxy, ...(p.username || login(p.host, p.port) ? { proxyLogin: true } : {}) };
  }
  if (!ext.proxy || !ext.proxy.settings) return {};
  let v;
  try {
    v = (await ext.proxy.settings.get(incognito ? { incognito: true } : {})).value || {};
  } catch {
    return {};
  }
  // Chromium: {mode, rules, pacScript}, the browser's effective proxy whoever set it (its
  // levelOfControl): a VPN or proxy extension ("controlled_by_other_extensions", the case this
  // is for), a policy or the command line ("not_controllable"), or nobody ({mode: "system"}).
  // "direct", "system" and "auto_detect" send nothing: the app then goes by its own Options
  // (which read the system's proxy). The proxy's login is never in it, on Chromium either.
  if (v.mode === "fixed_servers" && v.rules) {
    const r = v.rules;
    if (chromiumBypassed(r.bypassList, u)) return {};
    const s = r.singleProxy || (u.protocol === "https:" ? r.proxyForHttps : r.proxyForHttp) || r.fallbackProxy;
    const port = s && (s.port || chromiumPort(s.scheme));
    const proxy = s && proxyUrl(s.scheme, s.host, port);
    if (!proxy) return {};
    return { proxy, ...(login(s.host, port) ? { proxyLogin: true } : {}) };
  }
  if (v.mode === "pac_script" && v.pacScript) {
    const data = v.pacScript.data && v.pacScript.data.length <= PAC_MAX ? v.pacScript.data : null;
    const text = data || (v.pacScript.url ? await pacText(v.pacScript.url) : null);
    return text ? { proxyPac: text } : {};
  }
  // Firefox: {proxyType, http, ssl, socks, socksVersion, proxyDNS, autoConfigUrl, passthrough}.
  if (v.proxyType === "manual") {
    if (firefoxLoopback(u)) return {};
    if (String(v.passthrough || "").split(/[,\s]+/).some((b) => bypassed(b, u))) return {};
    const pick = (hp) => {
      const m = /^\[?([^\]]+?)\]?:(\d+)$/.exec(String(hp || "").trim());
      return m ? { host: m[1], port: Number(m[2]) } : null;
    };
    const http = pick(v.http);
    const ssl = v.httpProxyAll ? http : pick(v.ssl);
    const socks = pick(v.socks);
    const s = (u.protocol === "https:" ? ssl : http) || null;
    const proxy = s
      ? proxyUrl("http", s.host, s.port)
      : socks && proxyUrl(v.socksVersion === 4 ? "socks4" : "socks5", socks.host, socks.port, v.proxyDNS);
    if (!proxy) return {};
    const at = s || socks;
    return { proxy, ...(login(at.host, at.port) ? { proxyLogin: true } : {}) };
  }
  if (v.proxyType === "autoConfig" && v.autoConfigUrl) {
    const text = await pacText(v.autoConfigUrl);
    return text ? { proxyPac: text } : {};
  }
  return {};
}

// `explicit`: the user asked for this one (the right-click item), so Options' automatic-capture
// rules (file types, excluded sites) don't apply; the host checks them for everything else.
async function handOff(d) {
  const message = {
    url: d.url,
    finalUrl: d.finalUrl && d.finalUrl !== d.url ? d.finalUrl : "",
    referrer: d.referrer || "",
    cookies: await cookieHeader(d.url, d),
    filename: d.filename || "",
    ...(d.size > 0 ? { size: d.size } : {}),
    ...(d.mime ? { mime: d.mime } : {}),
    ...(d.post ? { post: d.post.body, postType: d.post.type || "" } : {}),
    ...(d.incognito ? { private: true } : {}),
    ...(d.explicit ? { explicit: true } : {}),
    // A page's media from the popup's list (#800): its page's title, for a name the address lacks.
    ...(d.title ? { title: d.title } : {}),
    // Through the browser's proxy too (#42): the one its own request for the file took.
    ...(await browserProxy(d.finalUrl || d.url, d.incognito)),
  };
  return native(message);
}

// Colab's runtime proxy uses an ephemeral token tied to the browser's partitioned session.
// Only the actual proxy transfer URL is left to the browser: a normal file linked from a Colab
// notebook must still be capturable by MDM (#603).
function colabRuntimeProxy(...urls) {
  return urls.some((value) => {
    if (!value) return false;
    try {
      const u = new URL(value);
      const host = u.hostname.toLowerCase();
      const proxyHost =
        host === "prod.r.colab.dev"
        || host.endsWith(".prod.r.colab.dev")
        || /(?:^|[-.])colab\.googleusercontent\.com$/.test(host);
      return proxyHost && u.searchParams.has("colab-runtime-proxy-token");
    } catch {
      return false;
    }
  });
}

// The file name to offer the app the download under (`{name}`), or why Options leave it to
// the browser (`{left, name}`), or null: the browser keeps it and there is nothing to say (not a
// web address, paused, Alt held, a form that can't be posted again, MDM not reached).
async function candidate(item, url) {
  if (!/^https?:/i.test(url)) return null; // blob:, data:, file: stay the browser's
  const { paused } = await ext.storage.local.get({ paused: false });
  if (paused) return null;
  forget(bypass, 10000);
  if (bypass.has(item.url) || bypass.has(url)) return null;
  const first = firstUrl(item);
  if (bypass.has(first) || postOf(first) === null) return null; // Alt; a form not to post again
  if (colabRuntimeProxy(first, url)) return null;
  const r = await currentRules();
  if (!r) return null;
  const name = (item.filename || "").split(/[\\/]/).pop() || url.split(/[?#]/)[0].split("/").pop() || "";
  const left = (why) => ({ left: why, name });
  if (!r.capture) return left(r.captureOff || "browser");
  const by = excludedBy(r, first) || excludedBy(r, url);
  if (by) return left(by);
  const type = extensionOf(name);
  if (!type || !typePatterns(r.types).some((p) => p.test(type))) return left("file-type");
  // Smaller than Options' minimum (when the browser knows the size already).
  const size = item.totalBytes > 0 ? item.totalBytes : item.fileSize;
  if (r.minKb > 0 && size > 0 && size < r.minKb * 1024) return left("small");
  if (!(await siteAccess(first))) return left("site-access");
  return { name };
}

// Why it stays with the browser, said (the popup, MDM's log).
function leftAlone(c, item, url) {
  if (c && c.left) noteLeft(c.left, c.name, firstUrl(item) || url, item.incognito, false).catch(() => {});
}

// Offer the download to the app under `name`; true if it took it.
async function offer(item, url, name) {
  const first = firstUrl(item);
  const reply = await handOff({
    url: first,
    finalUrl: url,
    referrer: item.referrer,
    filename: name,
    size: item.totalBytes > 0 ? item.totalBytes : item.fileSize,
    mime: item.mime,
    post: postOf(first),
    incognito: item.incognito,
    storeId: item.cookieStoreId,
  });
  if (reply.ok) {
    await ext.storage.local.remove("declined").catch(() => {});
  } else {
    // Refused by Options in the app (changed since the extension asked, or a rule only the app
    // checks, as a redirect's end): the host logged it; the popup says it.
    const why = declinedOf(reply);
    if (why) await noteLeft(why, name, first, item.incognito, true).catch(() => {});
  }
  return Boolean(reply.ok);
}

// Safari: a link clicked in a page (page.js) that may be a download, offered as the browser's own
// downloads are: through `candidate` (Options' rules, Alt, pause) then `offer`. `{taken: true}`
// when the app took it; else page.js lets the click go on, so nothing is lost. The link's file
// name is its `download` attribute's (`name`), else its address's (Safari names a download only
// when it starts, which is too late).
// The name an address gives, readable: the app saves a name as it is told it (the browsers' own
// downloads come already named), so `My%20file.zip` would stay that.
function addressName(url) {
  const last = url.split(/[?#]/)[0].split("/").pop() || "";
  try {
    return decodeURIComponent(last);
  } catch {
    return last; // not valid percent-encoding
  }
}

async function link(url, referrer, incognito, name) {
  // What the browser opens itself (a PDF, a song) is taken only when Options say so (`opened`,
  // "Also take files the browser would open"), as in the other browsers.
  const r = await currentRules();
  const type = extensionOf(name || url.split(/[?#]/)[0]).toUpperCase();
  if (r && r.opened === false && OPENED[type]) return { taken: false };
  const filename = name || addressName(url);
  const item = { url, referrer, incognito, ...(filename ? { filename } : {}) };
  const c = await candidate(item, url).catch(() => null);
  if (!c || c.left) {
    leftAlone(c, item, url);
    return { taken: false };
  }
  return { taken: await offer(item, url, c.name).catch(() => false) };
}

// Tabs opened in the last seconds: one the download was opened in (a link opened in a new tab
// or window, Shift+click) is closed with it, as when the browser keeps the download.
const newTabs = new Map();
ext.tabs.onCreated.addListener((tab) => {
  forget(newTabs, 10000);
  newTabs.set(tab.id, { at: Date.now() });
});

async function closeLeftTab(urls) {
  forget(newTabs, 10000);
  for (const id of newTabs.keys()) {
    try {
      const tab = await ext.tabs.get(id);
      const at = tab.pendingUrl || tab.url || "";
      // Only a tab opened for the download's own address (never a new tab the user opened).
      if (urls.includes(at)) {
        newTabs.delete(id);
        await ext.tabs.remove(id);
      }
    } catch {
      newTabs.delete(id);
    }
  }
}

// The browser's download, gone without a trace once the app has it.
async function drop(item) {
  const id = item.id;
  try {
    await ext.downloads.cancel(id);
  } catch {
    // Already finished: its file goes.
  }
  try {
    const [d] = await ext.downloads.search({ id });
    if (d && d.state === "complete" && d.exists !== false) await ext.downloads.removeFile(id);
  } catch {
    // Gone already.
  }
  ext.downloads.erase({ id }).catch(() => {});
  closeLeftTab([firstUrl(item), item.url, item.finalUrl].filter(Boolean)).catch(() => {});
}

// A download left to the browser keeps the name it has, or the one another extension gives it
// (Save In's folders, #578; a `downloads.download` with a `filename`). Chromium counts every
// listener's answer as a name of its own, an empty one too: `suggest()` without a suggestion
// reaches it as an empty name with no conflict action, and when the most recently installed
// extension answers so, its empty name wins and the others' are dropped (the download lands in
// the Downloads folder, and the browser may warn of a conflict). Only an answer with
// "uniquify" and no name is no suggestion at all (downloads_api.cc, DeterminerCallback), and
// the binding lets no empty name through: a name the browser refuses, which it empties after
// counting the answer, is how a listener gives one ("Invalid filename" is logged once).
function noName(suggest) {
  suggest({ filename: "..", conflictAction: "uniquify" });
}

if (SAFARI) {
  // Nothing to listen to: see `link`.
} else if (!FIREFOX) {
  // Chromium: once the final file name is known (after redirects and Content-Disposition). The
  // download waits for `suggest`, writing nothing, while the app is asked: taken, it is
  // cancelled; else it goes on as the browser's. So it is never lost or kept by both (the app
  // asks the address once more: a form's download is posted again with its body).
  ext.downloads.onDeterminingFilename.addListener((item, suggest) => {
    const url = item.finalUrl || item.url;
    candidate(item, url)
      .then(async (c) => {
        const taken = Boolean(c && c.name && !c.left) && (await offer(item, url, c.name));
        if (taken) return drop(item);
        // The browser goes on first; why is said after.
        noName(suggest);
        leftAlone(c, item, url);
      })
      .catch(() => noName(suggest));
    return true; // suggest() comes later
  });
} else {
  // Firefox names the file before the download is created and has no such pause. Most are
  // caught earlier, from the answer to the page load (`headersReceived`, below); the others (a
  // download started by a page's script, a type the answer didn't say) here: the download is
  // paused at once when it may be the app's, then cancelled or resumed; one that finished
  // meanwhile (a small file) has its file removed.
  // A page load decided from its answer already (`headersReceived`, #208) isn't asked again.
  ext.downloads.onCreated.addListener(async (item) => {
    const url = item.finalUrl || item.url;
    forget(decided, 120000);
    if (decided.has(url) || decided.has(firstUrl(item))) return;
    const c = await candidate(item, url).catch(() => null);
    if (!c || c.left) return leftAlone(c, item, url);
    const name = c.name;
    let paused = false;
    try {
      await ext.downloads.pause(item.id);
      paused = true;
    } catch {
      // Not pausable (no resume support), or finished already.
    }
    const taken = await offer(item, url, name).catch(() => false);
    if (taken) drop(item);
    else if (paused) ext.downloads.resume(item.id).catch(() => {});
  });
}

// ---- files the browser would open itself (a PDF, an MP3): made downloads -----------------

// Options → General in the app: also take the listed types the browser shows or plays when a
// link leads to one. Chromium: a session rule marks such answers as attachments, so they come
// down the download path above (where the rules and Alt apply). Firefox: taken from the answer
// as a download is (`headersReceived`), else it opens.
const OPENED = {
  PDF: ["application/pdf"],
  MP3: ["audio/mpeg", "audio/mp3"],
  MP4: ["video/mp4"],
  M4A: ["audio/mp4", "audio/x-m4a"],
  M4V: ["video/x-m4v"],
  OGG: ["audio/ogg"],
  OGV: ["video/ogg"],
  WAV: ["audio/wav", "audio/x-wav", "audio/wave"],
  AAC: ["audio/aac"],
  MOV: ["video/quicktime"],
  TIF: ["image/tiff"],
  TIFF: ["image/tiff"],
  WMA: ["audio/x-ms-wma"],
  WMV: ["video/x-ms-wmv"],
  AVI: ["video/x-msvideo", "video/avi"],
  MKV: ["video/x-matroska"],
};
const OPENED_RULE = 1;
// The allow rules for exact excluded hosts follow it (ids from 2).
const OPENED_ALLOW = 2;
let openedTypes = [];

// Options' excluded sites as the rules need them, read as the app reads them: `*.example.com`
// is the domain and its subdomains (a rule's domain condition), a plain name that host only
// (an allow rule of its own). As the browser writes a host: no port, punycode.
function excludedHosts() {
  const wide = [];
  const exact = [];
  for (const raw of ((rules && rules.excludedSites) || "").split(/\s+/)) {
    const p = raw.trim().replace(/^[a-z]+:\/\//i, "").split("/")[0];
    const sub = p.startsWith("*.");
    let host = "";
    try {
      host = new URL(`http://${sub ? p.slice(2) : p}`).hostname;
    } catch {
      continue;
    }
    if (!host || host.includes("*")) continue;
    (sub ? wide : exact).push(host);
  }
  return { wide, exact };
}

function openedMimes(r) {
  if (!r || !r.capture || !r.opened) return [];
  const listed = new Set(r.types.split(/\s+/).map((s) => s.toUpperCase()));
  return [...new Set(Object.entries(OPENED).flatMap(([k, m]) => (listed.has(k) ? m : [])))];
}

async function syncOpenedRule() {
  const { paused } = await ext.storage.local.get({ paused: false });
  const { why } = await readStatus();
  openedTypes = paused || (why && why !== "update-app") ? [] : openedMimes(rules);
  const dnr = ext.declarativeNetRequest;
  if (FIREFOX || !dnr || !dnr.updateSessionRules) return;
  const hosts = excludedHosts();
  const rule = {
    id: OPENED_RULE,
    priority: 1,
    action: {
      type: "modifyHeaders",
      responseHeaders: [{ header: "content-disposition", operation: "set", value: "attachment" }],
    },
    condition: {
      resourceTypes: ["main_frame"],
      responseHeaders: [{ header: "content-type", values: openedTypes.map((m) => `${m}*`) }],
      // Options' excluded sites keep opening in the browser.
      ...(hosts.wide.length ? { excludedRequestDomains: hosts.wide } : {}),
    },
  };
  const esc = (h) => h.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const allows = hosts.exact.slice(0, 500).map((h, i) => ({
    id: OPENED_ALLOW + i,
    priority: 2,
    action: { type: "allow" },
    condition: { resourceTypes: ["main_frame"], regexFilter: `^[a-z]+://${esc(h)}(:[0-9]+)?/` },
  }));
  try {
    const old = (await dnr.getSessionRules()).map((r) => r.id);
    await dnr.updateSessionRules({
      removeRuleIds: old.filter((id) => id >= OPENED_RULE),
      addRules: openedTypes.length ? [rule, ...allows] : [],
    });
  } catch {
    // A browser without header conditions (Chrome before 128): it opens them as before.
  }
}

// ---- Firefox: a download caught from its answer, before the browser's dialog (#208) ----------

// Firefox creates a download (downloads.onCreated, above) only once it has a file to save to:
// when it asks first ("What should Firefox do with this file?", which Waterfox always asks for
// a new type), only after the user answered, while it downloads the whole file behind the
// dialog. So a page load whose answer would be a download is caught as the answer arrives: the
// load waits while the app is asked, as Chromium's download does, and is cancelled when the app
// took it (no dialog, no browser download). Else it goes on as before, and the download it
// becomes is left to the browser (`decided`). A file the browser would show or play (a PDF, an
// MP3) is taken the same way when Options say so (`openedTypes`); else it opens.

// What Firefox shows or plays itself: a download only when its answer says "attachment".
const SHOWN = /^(text\/|image\/|video\/|audio\/|multipart\/|application\/(pdf|xhtml\+xml|xml|json|javascript|x-javascript|ecmascript|ogg|x-xpinstall|[\w.+-]+\+(xml|json))$)/;

// The file name a Content-Disposition gives (filename*=UTF-8''…, else filename="…"), or "".
function dispositionName(cd) {
  const star = /filename\*\s*=\s*([\w-]+)'[^']*'([^;]+)/i.exec(cd);
  if (star) {
    try {
      return decodeURIComponent(star[2].trim().replace(/^"|"$/g, ""));
    } catch {
      // Not percent-encoded as it says: the plain name below.
    }
  }
  const plain = /(?:^|;)\s*filename\s*=\s*("((?:[^"\\]|\\.)*)"|[^;]+)/i.exec(cd);
  if (!plain) return "";
  return decodedPlain((plain[2] !== undefined ? plain[2].replace(/\\(.)/g, "$1") : plain[1]).trim());
}

// A plain `filename` as sent, or its percent-encoded UTF-8 decoded when that is what it holds
// (non-ASCII once decoded: `100%25.txt` stays as it is), as the app reads it (naming.rs).
function decodedPlain(name) {
  if (!name.includes("%")) return name;
  try {
    const decoded = decodeURIComponent(name);
    return /[^\x00-\x7f]/.test(decoded) ? decoded : name;
  } catch {
    return name; // not percent-encoded UTF-8
  }
}

// A response header's value as text. Firefox gives its bytes one character each (ISO-8859-1), so
// a name sent as raw UTF-8 (`filename="تقرير.pdf"`) arrived as `ØªÙ…` (#447). It is read as UTF-8
// when those bytes are valid UTF-8, as Firefox reads the name for its own downloads and the app
// for its own requests; bytes that aren't (ISO-8859-1's `café`) stay as they came.
function headerText(value) {
  if (!/[\x80-\xff]/.test(value) || /[^\x00-\xff]/.test(value)) return value;
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(value, (c) => c.charCodeAt(0)));
  } catch {
    return value;
  }
}

// {name, size, mime, opened} when this answer to a page load becomes a download (or is one of
// the files Options take though the browser would open them), else null: a page, a picture, a
// redirect, an error, a piece of a file.
function answerFile(d) {
  const code = d.statusCode || 0;
  if (code < 200 || code > 299 || code === 204 || code === 205 || code === 206) return null;
  const headers = d.responseHeaders || [];
  const get = (n) => ((headers.find((h) => h.name.toLowerCase() === n) || {}).value || "").trim();
  const mime = get("content-type").split(";")[0].trim().toLowerCase();
  const cd = get("content-disposition");
  const attachment = /^attachment/i.test(cd);
  const opened = !attachment && openedTypes.some((m) => mime.startsWith(m));
  // No type: the browser guesses from the bytes (a page, mostly); a download it becomes is
  // still seen by onCreated.
  if (!attachment && !opened && (!mime || SHOWN.test(mime))) return null;
  let name = dispositionName(headerText(cd));
  if (!name) {
    try {
      name = decodeURIComponent(pathOf(d.url).split("/").pop() || "");
    } catch {
      name = pathOf(d.url).split("/").pop() || "";
    }
  }
  name = name.split(/[\\/]/).pop() || "";
  // The size the file will have: the length of an answer that isn't compressed on the way.
  const size = get("content-encoding") ? 0 : Number(get("content-length")) || 0;
  return { name, size, mime, opened };
}

// The page loads decided here (by their first and last addresses, two minutes): the download
// one of them becomes after the browser's dialog is left to the browser (asked once).
const decided = new Map();

// A tab opened for the download only (a link opened in a new tab or window), closed with it.
async function closeDownloadTab(tabId, urls) {
  forget(newTabs, 10000);
  if (!(tabId >= 0) || !newTabs.has(tabId)) return;
  try {
    const tab = await ext.tabs.get(tabId);
    const at = tab.url || "";
    if (at === "about:blank" || urls.includes(at)) {
      newTabs.delete(tabId);
      await ext.tabs.remove(tabId);
    }
  } catch {
    // Closed already.
  }
}

async function headersReceived(d) {
  if (d.method !== "GET" && d.method !== "POST") return {};
  const file = answerFile(d);
  if (!file) return {};
  const ref = referrers.get(d.requestId);
  const item = {
    url: d.url,
    filename: file.name,
    totalBytes: file.size,
    fileSize: file.size,
    mime: file.mime,
    referrer: (ref && ref.url) || "",
    incognito: Boolean(d.incognito),
    cookieStoreId: d.cookieStoreId,
  };
  if (PROXY_DECLARED) {
    await proxyChecked;
    noteProxy(d);
  }
  const first = firstUrl(item);
  forget(decided, 120000);
  decided.set(d.url, { at: Date.now() });
  decided.set(first, { at: Date.now() });
  const c = await candidate(item, d.url).catch(() => null);
  if (!c || c.left) {
    // A file the browser opens stays a page: there is no download to say anything about.
    if (!file.opened) leftAlone(c, item, d.url);
    return {};
  }
  const taken = await offer(item, d.url, c.name).catch(() => false);
  if (!taken) return {};
  closeDownloadTab(d.tabId, [first, d.url]).catch(() => {});
  return { cancel: true };
}

if (FIREFOX && ext.webRequest && ext.webRequest.onHeadersReceived) {
  ext.webRequest.onHeadersReceived.addListener(
    (d) => headersReceived(d).catch(() => ({})),
    { urls: ["<all_urls>"], types: ["main_frame", "sub_frame"] },
    ["blocking", "responseHeaders"],
  );
}

// ---- right-click menus -------------------------------------------------------------------

// Created at install and update (removed first: they persist, and a second create fails), in
// the browser's language (_locales, from the app's catalogs by tools/extension/locales.py).
function createMenus() {
  ext.contextMenus.removeAll(() => {
    const add = (id, contexts, key) =>
      ext.contextMenus.create({ id, title: t(key), contexts }, () => void ext.runtime.lastError);
    add("mdm-link", ["link", "video", "audio", "image"], "menu");
    add("mdm-selected", ["selection"], "menu_selected");
    add("mdm-all", ["page"], "menu_all");
  });
}

// The links in the page (all of them, or those in the selection), collected in the page.
function pageLinks(selectedOnly) {
  const ok = (h) => /^(https?|ftps?|ftpes):/i.test(h);
  let anchors = [...document.querySelectorAll("a[href]")];
  if (selectedOnly) {
    const sel = getSelection();
    anchors = sel.rangeCount ? anchors.filter((a) => sel.containsNode(a, true)) : [];
  }
  const seen = new Set();
  const links = [];
  for (const a of anchors) {
    if (ok(a.href) && !seen.has(a.href)) {
      seen.add(a.href);
      links.push(a.href);
    }
  }
  for (const m of document.querySelectorAll("video[src], audio[src], source[src]")) {
    if (ok(m.src) && !seen.has(m.src) && !selectedOnly) {
      seen.add(m.src);
      links.push(m.src);
    }
  }
  return links;
}

// When the user asked and it couldn't be done: the help page's section opens beside (a link
// MDM refused has none of its own: the connection's).
const SECTIONS = ["not-installed", "forbidden", "not-running", "host-failed", "too-large", "update-app", "options"];
function explain(why) {
  const at = SECTIONS.includes(why) ? why : "host-failed";
  ext.tabs.create({ url: `${HELP}#${at}` }).catch(() => {});
}

ext.contextMenus.onClicked.addListener(async (info, tab) => {
  const incognito = Boolean(tab && tab.incognito);
  const storeId = tab && tab.cookieStoreId;
  if (info.menuItemId === "mdm-link") {
    const url = info.linkUrl || info.srcUrl;
    // The tab's title, as the button's click sends it: the app names a video yt-dlp knows only
    // by its address (a stream's `master`, a long hash) after it (#345).
    const title = String((tab && tab.title) || "").slice(0, 300);
    // A player's own stream (blob:, a MediaSource) or a stream's manifest: the Download video
    // window, which reads streams (a manifest downloaded as a file is only a short list).
    // The manifest's own cookies (its server may not be the page's), as the media list sends.
    if (url && MANIFEST.test(url)) {
      await mediaBack;
      const reply = await native({ video: true, explicit: true, url: info.pageUrl, src: url, title, ...(await streamContext(tab && tab.id, url, { incognito, storeId })), private: incognito });
      if (!reply.ok && reply.why !== "options") explain(reply.why);
      return;
    }
    // A player playing from a blob: has no address yt-dlp can read: the stream its frame loaded,
    // as the button over videos sends (#155: the page alone was sent, and yt-dlp, not knowing
    // the site, answered "Unsupported URL"). Firefox names no address at all for a player fed
    // by a MediaSource (nothing was sent then).
    const player = info.mediaType === "video" || info.mediaType === "audio";
    if ((url && /^(blob|mediastream):/i.test(url)) || (player && !url)) {
      const page = info.frameUrl || info.pageUrl;
      await mediaBack;
      const src = await streamFor({ tab, frameId: info.frameId || 0 });
      const reply = await native({
        video: true,
        explicit: true,
        url: page,
        ...(src ? { src } : {}),
        ...(title ? { title } : {}),
        referrer: info.pageUrl && info.pageUrl !== page ? info.pageUrl : "",
        ...(await streamContext(tab && tab.id, src || page, { incognito, storeId })),
        private: incognito,
      });
      if (!reply.ok && reply.why !== "options") explain(reply.why);
      return;
    }
    // ftp links too: browsers no longer download them, MDM does.
    if (!url || !/^(https?|ftps?|ftpes):/i.test(url)) return;
    const reply = await handOff({ url, referrer: info.pageUrl, incognito, storeId, explicit: true });
    if (!reply.ok) explain(reply.why);
    return;
  }
  if ((info.menuItemId === "mdm-selected" || info.menuItemId === "mdm-all") && tab) {
    let links = [];
    try {
      const [res] = await ext.scripting.executeScript({
        target: { tabId: tab.id, frameIds: [info.frameId || 0] },
        func: pageLinks,
        args: [info.menuItemId === "mdm-selected"],
      });
      links = (res && res.result) || [];
    } catch {
      // A page extensions may not read (the browser's own, a store).
    }
    if (!links.length) return;
    const reply = await native({ links, referrer: info.pageUrl, cookies: await cookieHeader(info.pageUrl, { incognito, storeId }), private: incognito });
    if (!reply.ok) explain(reply.why);
  }
});

// ---- install, start, the toolbar popup ------------------------------------------------------

ext.runtime.onInstalled.addListener(async (details) => {
  createMenus();
  // Hello: the app learns the extension is there (its offer window says Connected).
  const r = await refreshRules();
  if (details.reason === "install" && !r) {
    const { why } = await readStatus();
    explain(why || "not-installed");
  }
  injectOpenTabs();
});

ext.runtime.onStartup.addListener(() => {
  refreshRules();
  showStatus();
});

// A match pattern's host and path as a test (`*://*.youtube.com/*`).
function matcher(pattern) {
  const [, host = "", path = "/*"] = /^[^:]+:\/\/([^/]*)(\/.*)?$/.exec(pattern) || [];
  const glob = (s) => s.split("*").map((x) => x.replace(/[.+?^${}()|[\]\\]/g, "\\$&")).join(".*");
  const h = host.startsWith("*.") ? new RegExp(`^(.*\\.)?${glob(host.slice(2))}$`, "i") : new RegExp(`^${glob(host)}$`, "i");
  const pth = new RegExp(`^${glob(path)}$`);
  return (u) => h.test(u.hostname) && pth.test(u.pathname + u.search);
}

// Whether this copy of the extension leaves the video button off `url` (the Chrome Web Store's
// copy: its policy, tools/extension/pack.ps1). Read from its manifest, so the popup says what is true.
function videoKeptOff(url) {
  const entry = (ext.runtime.getManifest().content_scripts || []).find((e) => (e.js || []).includes("video.js"));
  try {
    const u = new URL(url);
    return ((entry && entry.exclude_matches) || []).some((p) => matcher(p)(u));
  } catch {
    return false;
  }
}

// Pages open before the extension was installed or updated get its scripts now (else the
// button waits for a reload).
async function injectOpenTabs() {
  if (!ext.scripting) return;
  const tabs = await ext.tabs.query({ url: ["http://*/*", "https://*/*"] }).catch(() => []);
  for (const entry of ext.runtime.getManifest().content_scripts || []) {
    const excludes = (entry.exclude_matches || []).map(matcher);
    for (const tab of tabs) {
      let u;
      try {
        u = new URL(tab.url);
      } catch {
        continue;
      }
      if (excludes.some((m) => m(u))) continue;
      ext.scripting
        .executeScript({ target: { tabId: tab.id, allFrames: Boolean(entry.all_frames) }, files: entry.js })
        .catch(() => {});
    }
  }
}

// ---- messages from the page (video.js, page.js) and the popup (popup.js) -------------------

// The download button over videos asks: may it show on this page, the button was clicked, its
// menu. The app decides through the native host, from Options as saved now; its answer for a
// site is kept a few seconds (the page's frames ask together, and each page asks again when its
// answer is older: video.js ASK_MS), so a change in Options reaches the pages already open
// (#532: it was kept a minute, and by the page for good). A failure to reach it isn't kept.
// `top`: the tab's address, when the frame asking is another page's (a player embedded from
// another site): the button stays off there too when the tab's site is listed.
const PANEL_KEEP_MS = 5000;
const panelCache = new Map();
async function panelOn(page, top) {
  // The host name without a port: as video.js names the site, and as the app matches sites.
  const site = new URL(page).hostname;
  const tabSite = top ? new URL(top).hostname : "";
  const key = tabSite && tabSite !== site ? `${site} ${tabSite}` : site;
  forget(panelCache, PANEL_KEEP_MS);
  if (panelCache.get(site)?.hidden) return false;
  const kept = panelCache.get(key);
  if (kept) return kept.on;
  const reply = await native({ panel: true, url: page, ...(key !== site ? { referrer: top } : {}) });
  if (reply.ok || !reply.why) panelCache.set(key, { on: Boolean(reply.ok), at: Date.now() });
  return Boolean(reply.ok);
}

// The page the message came from, as the browser knows it (never what the page says). The top
// frame's is its tab's address, which follows a page that moves in place (history.pushState:
// YouTube, most video sites). `sender.url` is the document's and goes stale there: in Chromium it
// stays the address the document loaded at, in Firefox it lags a navigation behind, so the button
// sent the video watched before this one.
function senderPage(sender) {
  const top = sender.frameId === 0 && sender.tab && sender.tab.url;
  const u = top || sender.url || (sender.tab && sender.tab.url) || "";
  return /^https?:/i.test(u) ? u : null;
}

// A YouTube page's player, asked for what it plays. In the page's own world, the only place its
// player's API exists (content scripts are isolated from it, and an injected <script> is refused
// by its Trusted Types; `scripting` isn't). Run in the page: no outside names, only its arguments.
function playingAt(x, y) {
  let best = null;
  for (const v of document.querySelectorAll("video")) {
    const r = v.getBoundingClientRect();
    if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
    if (!best || r.width * r.height < best.area) best = { v, area: r.width * r.height };
  }
  const player = best && best.v.closest(".html5-video-player");
  try {
    return player && player.getVideoData ? String(player.getVideoData().video_id || "") : "";
  } catch {
    return "";
  }
}

const isYouTube = (url) => /(^|\.)(youtube\.com|youtube-nocookie\.com|youtu\.be)$/i.test(siteOf(url));

// The watch address of what the player at `at` (the video's middle, in the sender's frame) plays,
// or "": a page without such a player, one too old for `world`, a page extensions may not read.
async function playingVideo(sender, at) {
  if (!at || !ext.scripting || !sender.tab) return "";
  try {
    const [found] = await withTimeout(
      ext.scripting.executeScript({
        target: { tabId: sender.tab.id, frameIds: [sender.frameId || 0] },
        world: "MAIN",
        func: playingAt,
        args: [Number(at.x) || 0, Number(at.y) || 0],
      }),
      3000,
    );
    const id = found && found.result;
    return /^[\w-]{6,32}$/.test(id || "") ? `https://www.youtube.com/watch?v=${id}` : "";
  } catch {
    return "";
  }
}

// A site's name for comparing two addresses: without its www., mobile or other front host
// (old.reddit.com is reddit.com), its short-link hosts as the site (fb.watch is Facebook's,
// twitter.com is x.com).
const SITE_ALIASES = { "fb.watch": "facebook.com", "twitter.com": "x.com" };
function siteKey(url) {
  const host = siteOf(url).replace(/^(www|m|mobile|web|touch|old|new|sh|music|geo|player)\./, "");
  return SITE_ALIASES[host] || host;
}

// The address the app looks a video up by: the one video's, never a list's. On YouTube that is
// what the player plays; else the address the content script named (video.js videoPage: the post
// the video sits in on a feed, #689), if it is this site's; else the page's. "" where nothing
// names the video: on YouTube (its own address is a list, the home page, a channel, to yt-dlp),
// and on a feed (`url` null: video.js found the page is many videos', LinkedIn's or Facebook's
// home, and no post around the video naming it).
async function videoAddress(m, page, sender) {
  const named = typeof m.url === "string" && /^https?:/i.test(m.url) ? m.url : "";
  if (isYouTube(page)) {
    const playing = await playingVideo(sender, m.at);
    return playing || (isYouTube(named) ? named : "");
  }
  if (m.url === null) return "";
  return named && siteKey(named) === siteKey(page) ? named : page;
}

// What the button's click is about, as the browser knows it: the address yt-dlp is given (`own`,
// the video's page), the video's own file or stream (`src`), the page's title, the tab as
// referrer, and the cookies of what yt-dlp asks. Null when nothing names the video.
async function videoTarget(m, page, sender) {
  const tab = sender.tab || {};
  await mediaBack;
  const named = await videoAddress(m, page, sender);
  const youtube = isYouTube(page);
  // The post a video in a feed sits in (#689): the app looks that up. Nothing names it (`url`
  // null): the feed is never sent as its page, only the video's own plain file, if it has one.
  // Either way no stream the tab loaded goes with it (a feed's first stream may be another
  // video's), nor the page's title (the feed's).
  const post = !youtube && Boolean(named) && named !== page;
  const feed = !youtube && m.url === null;
  // The video's own file (video.js leaves it out beside a post a big site's reader knows), or a
  // stream it plays, still goes as `src`, with the page for its Referer.
  const file = typeof m.src === "string" && /^https?:/i.test(m.src) ? m.src : "";
  const src = file || (post || feed ? "" : await streamFor(sender));
  if (!named && !src) return null;
  const own = named || page;
  return {
    own,
    src,
    // The post's own page (#689), for the button's "Open post".
    post: post ? own : "",
    // The video's own plain file, sent as itself (#703).
    file,
    title: typeof m.title === "string" && !post && !feed ? m.title.slice(0, 300) : "",
    referrer: tab.url && tab.url !== own ? tab.url : "",
    // The cookies of what yt-dlp is given (the app files them under its host): the stream's
    // own server may not be the page's, and the page's cookies are not for it. With a stream's,
    // what the browser sent with it (#900).
    ...(await streamContext(tab.id, src || own, { incognito: tab.incognito, storeId: tab.cookieStoreId })),
  };
}

// ---- the button's qualities: the app looks the video up, the page lists what it found --------
// The click asks the app to look the video up (yt-dlp, in the background), video.js asks how it
// is going until it is done, and the quality picked is downloaded without the app's window. Only
// with an app that says it can (`videoLookup`, noteLookup) and never for another MDM: else
// ({fallback: true}) the click opens the app's Download video window, as it always did. None of
// these messages has `video` or a top-level `url`: an older host refuses them and does nothing.
// What a lookup comes to travels in `lookup`, never in `why`: the toolbar button's status (and
// the rule for PDFs and music that follows it) is the host's, not a video's.

async function videoLookup(m, page, sender) {
  if (await remoteTarget()) return { fallback: true };
  if (!(await readStatus()).videoLookup) {
    // An MDM updated since its rules were last asked may look up now: they are asked again,
    // for the next click.
    const r = await storedRules();
    if (!r || Date.now() - r.at > 60000) refreshRules();
    return { fallback: true };
  }
  const tab = sender.tab || {};
  const v = await videoTarget(m, page, sender);
  if (!v) return { ok: false, why: isYouTube(page) ? "unnamed" : "feed" };
  const reply = await native({
    videoLookup: {
      page: v.own,
      ...(v.src ? { src: v.src } : {}),
      ...(v.title ? { title: v.title } : {}),
      ...(v.referrer ? { referrer: v.referrer } : {}),
      // The lookup is the window's and the container's it was asked from (the app gives its
      // pick to no other).
      ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}),
    },
    cookies: v.cookies,
    cookieJar: v.cookieJar,
    ...(v.fetchedWith ? { fetchedWith: v.fetchedWith } : {}),
    private: Boolean(tab.incognito),
  });
  // `show`: the address MDM lists a download of it under (the button's "Show"); `post`: the
  // post's own page, when the video sits in a feed (its "Open post").
  if (reply.lookup && typeof reply.lookup === "object") return { lookup: reply.lookup, show: v.src || v.own, ...(v.post ? { post: v.post } : {}) };
  // No `lookup`: a host or an app that can't look up after all (an older one refuses the message),
  // or a host that failed. Not kept as able; its rules are asked again, and this click opens
  // the window.
  await noteLookup(false);
  refreshRules();
  return { fallback: true };
}

// The token is the app's, from the lookup's answer; it means nothing to anyone else.
const lookupToken = (token) => typeof token === "string" && token.length > 0 && token.length <= 200;

async function videoPoll(m) {
  if (await remoteTarget()) return { fallback: true };
  if (!lookupToken(m.token)) return { lookup: { state: "gone" } };
  const reply = await native({ videoPoll: { token: m.token } });
  return { lookup: reply.lookup && typeof reply.lookup === "object" ? reply.lookup : { state: "gone" } };
}

async function videoPick(m, sender) {
  if (await remoteTarget()) return { fallback: true };
  if (!lookupToken(m.token) || !Number.isInteger(m.choice) || m.choice < 0) return { fallback: true };
  const tab = sender.tab || {};
  // The token and the row's number from the page, nothing else: private or not and the
  // container are the tab's, as the browser says; the app checks them against the lookup's.
  const reply = await native({
    videoPick: { token: m.token, choice: m.choice, ...(tab.cookieStoreId ? { storeId: tab.cookieStoreId } : {}) },
    private: Boolean(tab.incognito),
  });
  return reply.lookup && typeof reply.lookup === "object" ? { lookup: reply.lookup } : { fallback: true };
}

const isWeb = (u) => typeof u === "string" && /^https?:\/\//i.test(u);
// Where the button's "Get MDM" goes.
const GET_MDM = "https://misterdownloadmanager.com/download";
// A new tab next to `tab`, opened from it.
const beside = (tab) => ({
  ...(Number.isInteger(tab.id) ? { openerTabId: tab.id } : {}),
  ...(Number.isInteger(tab.index) ? { index: tab.index + 1 } : {}),
});

async function pageMessage(m, sender) {
  const page = senderPage(sender);
  const tab = sender.tab || {};
  if (m.type === "panel") {
    const top = tab.url && /^https?:/i.test(tab.url) ? tab.url : "";
    return { on: page ? await panelOn(page, top) : false };
  }
  if (m.type === "link" && typeof m.url === "string") {
    const name = typeof m.name === "string" ? m.name.split(/[\\/]/).pop().slice(0, 255) : "";
    return link(m.url, page || "", Boolean(tab.incognito), name);
  }
  // Safari's page.js, at each page: the rules it holds links by, refreshed when old (see `link`).
  if (m.type === "rules") {
    const r = await storedRules();
    if ((!r || Date.now() - r.at > 60000) && Date.now() - rulesAskedAt > 60000) {
      rulesAskedAt = Date.now();
      refreshRules().catch(() => {});
    }
    return { ok: true };
  }
  if (m.type === "bypass" && typeof m.url === "string") {
    bypass.set(m.url, { at: Date.now() });
    return { ok: true };
  }
  if (m.type === "video" && page) {
    const v = await videoTarget(m, page, sender);
    if (!v) return { ok: false, why: isYouTube(page) ? "unnamed" : "feed" };
    // The video's own plain file (#703): downloaded as a file, no window; with the type its
    // server said when the tab loaded it, which names a script's answer at once (#800).
    const direct = Boolean(m.direct) && Boolean(v.file) && v.file === v.src;
    const loaded = direct && ((media.get(tab.id) || {}).items || []).find((it) => it.url === v.src);
    const reply = await native({
      video: true,
      url: v.own,
      ...(v.src ? { src: v.src } : {}),
      ...(direct ? { direct: true } : {}),
      ...(loaded && loaded.mime ? { mime: loaded.mime } : {}),
      title: v.title,
      referrer: v.referrer,
      cookies: v.cookies,
      cookieJar: v.cookieJar,
      ...(v.fetchedWith ? { fetchedWith: v.fetchedWith } : {}),
      ...(tab.incognito ? { private: true } : {}),
    });
    return { ok: Boolean(reply.ok), why: reply.why, ...(reply.ok && direct ? { show: v.src } : {}) };
  }
  if (m.type === "video-lookup" && page) return videoLookup(m, page, sender);
  // The button's "Show" (#703): MDM's window with that download selected.
  if (m.type === "show-download" && isWeb(m.address)) {
    return { ok: Boolean((await native({ showDownload: m.address })).ok) };
  }
  // The button's "Turn on" (after its "Never on this site", or off here in Options): on again.
  if (m.type === "show-site" && page) {
    const host = new URL(page).hostname;
    for (const k of [...panelCache.keys()]) if (k.split(" ").includes(host)) panelCache.delete(k);
    return { ok: Boolean((await native({ showSite: host })).ok) };
  }
  // The button's "Get MDM" (MDM didn't answer): its download page, beside this tab.
  if (m.type === "get-mdm") {
    await ext.tabs.create({ url: GET_MDM, ...beside(tab) });
    return { ok: true };
  }
  // The button's "Open post": the post a video in a feed sits in (#689), on this page's site.
  if (m.type === "open-post" && page && isWeb(m.url) && siteKey(m.url) === siteKey(page)) {
    await ext.tabs.create({ url: m.url, ...beside(tab) });
    return { ok: true };
  }
  if (m.type === "video-poll") return videoPoll(m);
  if (m.type === "video-pick") return videoPick(m, sender);
  if (m.type === "hide-site" && page) {
    const host = new URL(page).hostname;
    // Off at once on this site, whatever the tab around it, while the app saves it.
    panelCache.set(host, { on: false, hidden: true, at: Date.now() });
    const reply = await native({ hideSite: host });
    return { ok: Boolean(reply.ok) };
  }
  if (m.type === "options") return { ok: Boolean((await native({ openOptions: true })).ok) };
  return { ok: false };
}

async function popupMessage(m) {
  if (m.type === "state") {
    const r = await refreshRules();
    const { paused } = await ext.storage.local.get({ paused: false });
    const s = await readStatus();
    const { declined } = await ext.storage.local.get({ declined: null });
    const remote = await remoteTarget();
    if (remote) return { connected: true, why: null, remote: remote.url, version: VERSION, paused, declined };
    return { connected: Boolean(r) && (!s.why || s.why === "update-app"), why: s.why, app: s.app, version: VERSION, paused, declined };
  }
  if (m.type === "remote-pair" && typeof m.url === "string" && typeof m.code === "string") return remotePair(m.url, m.code);
  if (m.type === "remote-off") return remoteOff();
  if (m.type === "pause") {
    await ext.storage.local.set({ paused: Boolean(m.paused) });
    await syncOpenedRule();
    await showStatus();
    return { ok: true };
  }
  if (m.type === "exclude-site" && typeof m.host === "string" && m.host) {
    const reply = await native({ excludeSite: m.host });
    await refreshRules();
    return { ok: Boolean(reply.ok) };
  }
  if (m.type === "open-app") return { ok: Boolean((await native({ open: true })).ok) };
  if (m.type === "open-options") return { ok: Boolean((await native({ openOptions: true, page: "general" })).ok) };
  // The media the popup's tab loaded (#38), and one of them handed to the app.
  if (m.type === "media" && Number.isInteger(m.tab)) {
    const list = await mediaList(m.tab);
    const tab = await ext.tabs.get(m.tab).catch(() => null);
    return {
      media: list,
      // Why the list is empty, when there's more to say than "nothing yet": this copy keeps the
      // video button off the site, or the player streams pieces (blob:) no address can name.
      keptOff: Boolean(tab && videoKeptOff(tab.url)),
      streamed: !list.length && (await playsStream(m.tab)),
    };
  }
  if (m.type === "media-download" && Number.isInteger(m.tab) && typeof m.url === "string") {
    const reply = await mediaDownload(m.tab, m.url);
    return { ok: Boolean(reply.ok), why: reply.why };
  }
  return { ok: false };
}

ext.runtime.onMessage.addListener((m, sender, sendResponse) => {
  // Only our own scripts (pages can't message an extension without being allowed to).
  if (!m || typeof m.type !== "string" || sender.id !== ext.runtime.id) return false;
  // The popup, or its page opened in a tab (popup.html?page=), is one of the extension's own
  // pages; content scripts send from the web page they are in.
  const own = String(sender.url || "").startsWith(ext.runtime.getURL(""));
  const fromPage = Boolean(sender.tab) && !own;
  (fromPage ? pageMessage(m, sender) : popupMessage(m)).then(sendResponse, () => sendResponse({ ok: false }));
  return true; // answered later
});

showStatus();
// Rules kept from before, else asked now (a first start, storage cleared).
storedRules().then((r) => (r ? syncOpenedRule() : refreshRules()));
