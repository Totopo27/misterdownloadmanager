// The toolbar button's popup: whether the browser reaches Mister Download Manager (and, when it
// doesn't, a link to the help page's section for why), pausing capture, leaving the current
// site to the browser, opening the app or its Options, and the media the tab loaded (#38), each
// to download. background.js does the work.
const ext = globalThis.browser ?? globalThis.chrome;
const HELP = "https://misterdownloadmanager.com/help/browser-help.html";
// The creator's GitHub Sponsors page (the app's Help → Support MDM): a plain link, in a new tab.
const SPONSORS = "https://github.com/sponsors/anefzaoui";
const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || key;
const $ = (id) => document.getElementById(id);
const ask = (m) => ext.runtime.sendMessage(m).catch(() => ({ ok: false }));
const lang = ext.i18n.getUILanguage();

document.documentElement.lang = lang;
document.documentElement.dir = t("@@bidi_dir") === "rtl" ? "rtl" : "ltr";
for (const el of document.querySelectorAll("[data-i18n]")) el.textContent = t(el.dataset.i18n);
for (const el of document.querySelectorAll("[data-i18n-placeholder]")) el.placeholder = t(el.dataset.i18nPlaceholder);
$("help").href = HELP;
$("support").href = SPONSORS;

// The tab it is about: the window's active tab; with `?page=ADDRESS` (this page opened in a tab of
// its own, for the tests and the pictures), the tab showing that address.
async function currentTab() {
  const page = new URLSearchParams(location.search).get("page");
  if (page) return (await ext.tabs.query({}).catch(() => [])).find((x) => x.url === page) || null;
  const [tab] = await ext.tabs.query({ active: true, currentWindow: true }).catch(() => []);
  return tab || null;
}

async function show() {
  const s = await ask({ type: "state" });
  const box = $("status");
  const text = $("status-text");
  box.classList.remove("ok", "bad", "warn");
  if (s.connected && s.why === "update-app") {
    box.classList.add("warn");
    text.textContent = t("status_update_app");
  } else if (s.connected && s.remote) {
    box.classList.add(s.paused ? "warn" : "ok");
    text.textContent = s.paused ? t("status_paused") : t("status_remote", s.remote);
  } else if (s.connected) {
    box.classList.add(s.paused ? "warn" : "ok");
    text.textContent = s.paused ? t("status_paused") : s.app ? t("status_connected_version", s.app) : t("status_connected");
  } else {
    box.classList.add("bad");
    text.textContent = t(`status_${(s.why || "not_installed").replace(/-/g, "_")}`);
  }
  const fix = $("fix");
  fix.hidden = s.connected && s.why !== "update-app";
  fix.href = `${HELP}#${s.why || "not-installed"}`;
  $("take").checked = !s.paused;
  showDeclined(s.connected && s.declined);
  $("take").disabled = !s.connected;
  $("open").disabled = $("options").disabled = !s.connected;
  $("version").textContent = s.version ? `v${s.version}` : "";
  // Send downloads to: this computer, or another MDM (#157).
  $("to-here").checked = !s.remote && $("remote-form").hidden;
  $("to-remote").checked = Boolean(s.remote) || !$("remote-form").hidden;
  $("open").disabled = $("open").disabled || Boolean(s.remote);
  $("options").disabled = $("options").disabled || Boolean(s.remote);
  const tab = await currentTab();
  let site = "";
  try {
    const u = new URL(tab && tab.url);
    if (/^https?:$/.test(u.protocol)) site = u.hostname;
  } catch {
    // Not a web page (the browser's own): nothing to leave.
  }
  const exclude = $("exclude");
  exclude.hidden = !site || !s.connected;
  exclude.textContent = t("popup_exclude_site", site);
  exclude.title = exclude.textContent;
  exclude.onclick = async () => {
    const r = await ask({ type: "exclude-site", host: site });
    if (r.ok) {
      exclude.hidden = true;
      $("excluded").hidden = false;
      $("excluded").textContent = t("popup_excluded", site);
    }
  };
  if (site) await showMedia(tab, s.connected);
}

// The last download left to the browser by MDM's Options, and which one (#208); a later one
// taken clears it (background.js noteLeft). A reason this version can't name says no more.
function showDeclined(d) {
  const box = $("declined");
  box.hidden = !(d && d.why);
  if (box.hidden) return;
  const what = d.name ? t("popup_declined", d.name) : t("popup_declined_unnamed");
  const key = `declined_${String(d.why).replace(/-/g, "_")}`;
  const why = t(key);
  $("declined-what").textContent = what;
  $("declined-what").title = what;
  $("declined-why").textContent = why === key ? "" : why;
}

// ---- the media the tab loaded ------------------------------------------------------------------

const TYPES = {
  "video/mp4": "MP4",
  "audio/mp4": "M4A",
  "audio/mpeg": "MP3",
  "video/webm": "WEBM",
  "audio/webm": "WEBM",
  "audio/ogg": "OGG",
  "video/ogg": "OGV",
  "video/quicktime": "MOV",
  "video/x-matroska": "MKV",
  "audio/flac": "FLAC",
  "audio/wav": "WAV",
};

// Its type as a short tag: HLS, DASH, else the file's extension or its type's.
function tagOf(m) {
  if (m.kind === "hls") return "HLS";
  if (m.kind === "dash") return "DASH";
  const ext = (/\.([a-z0-9]{2,4})$/i.exec(m.url.split(/[?#]/)[0]) || [])[1];
  return (ext || TYPES[m.mime] || (m.kind === "audio" ? "AUDIO" : "VIDEO")).toUpperCase();
}

// The address's last part, readable.
function nameOf(url) {
  const last = url.split(/[?#]/)[0].split("/").filter(Boolean).pop() || url;
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
}

function clock(seconds) {
  const s = Math.round(seconds);
  const two = (n) => String(n).padStart(2, "0");
  return s >= 3600 ? `${Math.floor(s / 3600)}:${two(Math.floor(s / 60) % 60)}:${two(s % 60)}` : `${Math.floor(s / 60)}:${two(s % 60)}`;
}

// A size in the browser's language (its own unit names).
function sizeText(bytes) {
  const units = [["gigabyte", 1 << 30], ["megabyte", 1 << 20], ["kilobyte", 1 << 10]];
  const [unit, n] = units.find(([, n]) => bytes >= n) || ["byte", 1];
  try {
    return new Intl.NumberFormat(lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(bytes / n);
  } catch {
    return `${(bytes / n).toFixed(1)} ${unit}`;
  }
}

// What is known of it: its qualities (or picture size), length, size; "Live" for a live stream.
function metaOf(m) {
  const parts = [];
  if (m.width && m.heights.length === 1) parts.push(`${m.width}×${m.heights[0]}`);
  else if (m.heights.length) parts.push(m.heights.map((h) => `${h}p`).join(" · "));
  if (m.live) parts.push(t("popup_media_live"));
  else if (m.duration > 0) parts.push(clock(m.duration));
  if (m.size > 0) parts.push(sizeText(m.size));
  return parts.join(" · ");
}

function said(text, bad) {
  const p = $("media-said");
  p.hidden = !text;
  p.textContent = text;
  p.classList.toggle("bad", Boolean(bad));
}

async function showMedia(tab, connected) {
  const r = await ask({ type: "media", tab: tab.id });
  const list = (r && r.media) || [];
  const kept = Boolean(r && r.keptOff);
  $("media").hidden = false;
  // Why there's nothing to list, honestly: this copy leaves the site alone (Help says what's left
  // to do), the player streams pieces no address names, or the page hasn't loaded any yet.
  const none = $("media-none");
  none.hidden = list.length > 0 && !kept;
  none.textContent = t(kept ? "popup_media_store" : r && r.streamed ? "popup_media_streamed" : "popup_media_none");
  $("help").href = kept ? `${HELP}#store-youtube` : HELP;
  const ul = $("media-list");
  ul.replaceChildren();
  for (const m of list) {
    const li = document.createElement("li");
    li.dataset.kind = m.kind;
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = tagOf(m);
    const what = document.createElement("div");
    what.className = "what";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = nameOf(m.url);
    name.title = m.url;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = metaOf(m);
    what.append(name, meta);
    li.append(tag, what);
    if (m.drm) {
      // Copy-protected: listed, never sent.
      const drm = document.createElement("span");
      drm.className = "drm";
      drm.textContent = t("popup_media_protected");
      drm.title = t("video_drm");
      li.append(drm);
    } else {
      const get = document.createElement("button");
      get.textContent = t("popup_media_download");
      get.title = t("menu");
      get.disabled = !connected;
      get.onclick = async () => {
        get.disabled = true;
        const reply = await ask({ type: "media-download", tab: tab.id, url: m.url });
        said(reply.ok ? t("video_sent") : reply.why === "drm" ? `${t("video_drm")} · ${t("video_cant_save")}` : t("video_failed"), !reply.ok);
        get.disabled = false;
      };
      li.append(get);
    }
    ul.append(li);
  }
}

// This applies to files chosen in the media list, not ordinary browser downloads or streams.
const mediaTitleName = $("media-title-name");
ext.storage.local.get({ mediaTitleName: false })
  .then((v) => (mediaTitleName.checked = Boolean(v.mediaTitleName)))
  .catch(() => {});
mediaTitleName.addEventListener("change", () =>
  ext.storage.local.set({ mediaTitleName: mediaTitleName.checked }).catch(() => {}));

// #42: reading the browser's proxy (a VPN or proxy extension's) is an opt-in, this switch, off as
// installed. Firefox: the optional "proxy" permission, asked for on it. Chromium can't make that
// permission optional, so the extension has it from the install, and the switch is a setting of
// its own (`proxyOn`, which background.js reads before each download).
const PROXY = { permissions: ["proxy"] };
const manifest = ext.runtime.getManifest();
if (!(manifest.optional_permissions || []).includes("proxy") && (manifest.permissions || []).includes("proxy")) {
  const row = $("proxy-row");
  const box = $("proxy");
  row.hidden = false;
  row.title = t("popup_proxy_title_chromium");
  ext.storage.local
    .get({ proxyOn: false })
    .then((v) => (box.checked = Boolean(v.proxyOn)))
    .catch(() => {});
  box.addEventListener("change", () => ext.storage.local.set({ proxyOn: box.checked }).catch(() => {}));
}
if ((manifest.optional_permissions || []).includes("proxy")) {
  const row = $("proxy-row");
  const box = $("proxy");
  row.hidden = false;
  row.title = t("popup_proxy_title");
  const sync = () =>
    ext.permissions
      .contains(PROXY)
      .catch(() => false)
      .then((on) => (box.checked = Boolean(on)));
  sync();
  box.addEventListener("change", () => {
    // Asked for in the click itself (the browser allows it from a user's action only): nothing
    // awaited before it. Refused, or taken back: the switch shows what the browser says.
    const asked = box.checked ? ext.permissions.request(PROXY) : ext.permissions.remove(PROXY);
    asked.catch(() => false).then(sync);
  });
}

$("take").addEventListener("change", async (e) => {
  await ask({ type: "pause", paused: !e.target.checked });
  show();
});
$("open").addEventListener("click", async () => {
  await ask({ type: "open-app" });
  window.close();
});
$("options").addEventListener("click", async () => {
  await ask({ type: "open-options" });
  window.close();
});

// Send downloads to (#157): another MDM is paired here with its address and a code it shows.
$("to-remote").addEventListener("change", () => {
  $("remote-form").hidden = false;
  $("remote-url").focus();
});
$("to-here").addEventListener("change", async () => {
  $("remote-form").hidden = true;
  await ask({ type: "remote-off" });
  show();
});
$("remote-open").addEventListener("click", () => {
  try {
    const u = new URL($("remote-url").value.trim());
    if (u.protocol === "https:") ext.tabs.create({ url: u.origin + "/" });
  } catch {
    $("remote-said").textContent = t("remote_address");
    $("remote-said").hidden = false;
  }
});
$("remote-pair").addEventListener("click", async () => {
  const said = $("remote-said");
  said.hidden = true;
  const r = await ask({ type: "remote-pair", url: $("remote-url").value, code: $("remote-code").value });
  if (r.ok) {
    $("remote-form").hidden = true;
    $("remote-code").value = "";
    show();
    return;
  }
  said.textContent = t(`remote_${String(r.why || "failed").replace(/-/g, "_")}`);
  said.hidden = false;
});

show();
