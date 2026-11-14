// Mister Download Manager's download button over videos (a content script, in every page and
// frame). Our own design: a pill of dark frosted glass at the video's edge that appears while
// the pointer moves over the video (or it has the keyboard's focus) and fades when the pointer
// rests or leaves. As it appears the app looks the video up (an embedded player's own address,
// which yt-dlp understands, with the video's own address when it has a plain one, #703): its
// chip then names the best quality, which one click downloads without the app's window; a video
// that plays a plain file needs no look-up and is downloaded as a file. Its menu (the arrow)
// lists every quality with its size, opens the app's Download video window for the rest (as
// does a click when nothing was found, or with an app that can't look up), hides it on this
// video, keeps it off the site (the app's Options → General → Download panels lists those
// sites), or opens those settings. After a click it says what happened in its own colour, with
// the one action that helps. It can be dragged along the video's edge, and stays where it was put
// on that site. The app decides where it may show at all (the native host: integration on, this
// browser captured, the button on, the site not excluded).
(() => {
  // One button per page: a copy cut off from the extension (after an update) gives way to the
  // new one. Chromium runs the new copy where the old one ran, so it stops the old one here
  // (`teardown`). Firefox gives the new copy a world of its own: it can't reach the old copy,
  // whose listeners died with it while its button stayed on the page, frozen where it was, shown
  // if it was then: two buttons over the video (#336, seen in full screen). Any button already
  // on the page is such a copy's: it goes.
  const prev = globalThis.__mdmVideoButton;
  if (prev && typeof prev === "object" && prev.alive()) return;
  if (prev && typeof prev.teardown === "function") prev.teardown();
  for (const old of document.querySelectorAll("mdm-video-button")) old.remove();
  const ext = globalThis.browser ?? globalThis.chrome;

  // A video worth a button: big enough to be content, not a short muted looping clip (an
  // animated picture or a background). A sound player (<audio> with controls) gets one too.
  const MIN_W = 200;
  const MIN_H = 112;
  const MIN_AUDIO_H = 24;
  const CLIP_S = 15;
  // Shown this long after the pointer last moved over the video, kept while it is on the button
  // (#741: 1.5 s, it was 2.6); gone at once when the pointer leaves the video.
  const IDLE_MS = 1500;
  const INSET = 8;
  const DRAG_PX = 4;
  // Our app icon (extension/icons/icon-48.png, as data: no extension file is exposed to pages).
  const LOGO = "data:image/png;base64,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";

  const t = (key, ...subs) =>
    ext.i18n.getMessage(key, subs) || (FALLBACK[key] || key).replace(/\$(\d)/g, (_, n) => subs[n - 1] ?? "");
  const FALLBACK = {
    video_download: "Download video",
    video_more: "More options",
    video_hide_here: "Hide on this video",
    video_settings: "Button settings...",
    video_sent: "Sent to Mister Download Manager",
    video_off: "Off on this site",
    video_drm: "Protected video",
    video_cant_save: "can't be saved",
    video_unnamed: "Can't tell which video this is: open its own page and try there",
    video_open_post: "Open the post to download it",
    video_open_post_button: "Open post",
    video_drag_dots: "Drag the dots to move it along the edge",
    video_tip: "Download $1",
    video_tip_best: "Download the best quality",
    video_sending: "Sending to MDM...",
    video_added: "Added to MDM",
    video_show: "Show",
    video_no_answer: "MDM didn't answer",
    video_get_mdm: "Get MDM",
    video_retry: "Retry",
    video_turn_on: "Turn on",
    video_best: "best",
    video_original: "original",
    video_download_best: "Download best",
    video_all_qualities: "All qualities and subtitles...",
    video_looking: "Looking up the qualities…",
    video_no_choices: "No video to download here",
    video_playlist: "A list of videos: choose them in Mister Download Manager",
    video_no_ytdlp: "Mister Download Manager needs yt-dlp for this video",
    video_too_long: "Looking up took too long",
    video_busy: "Mister Download Manager is busy with other videos",
    video_queued: "Added to the queue",
    video_failed_lookup: "Could not look it up",
    video_tab_video: "Video",
    video_tab_audio: "Audio",
  };
  // Sizes as the browser's language writes them.
  const lang = ext.i18n.getUILanguage?.() || undefined;
  // To the background. After the extension was updated or removed, this page's copy of the script
  // is cut off from it (calls throw at once): then it takes itself away.
  const send = (m) => {
    try {
      if (!ext.runtime?.id) throw new Error("gone");
      return ext.runtime.sendMessage(m).catch(() => ({ ok: false }));
    } catch {
      teardown();
      return Promise.resolve({ ok: false, gone: true });
    }
  };
  const host = location.hostname;
  // The address the app looks the video up by — the video's, not the page's. Most pages that play
  // a video are not the video's page: YouTube's home, a search, a channel or a playlist carries a
  // list in its address ("recommended", the results, the channel's videos), and yt-dlp reads such
  // an address as the list, so the app offered the whole feed instead of the video the button was
  // on. A watch page opened from a mix or a playlist has the same problem one level down
  // (…&list=…&index=…): its own page is watch?v=<id>, and nothing else.
  //
  // Null when the page plays a video but its DOM doesn't name it: YouTube's channel masthead player
  // has no video-id attribute, no link to the video inside the player and a blob: source. The page
  // itself never goes to the app then (a channel is a whole list to yt-dlp): the background asks
  // the player what it plays, in the page's own world (background.js playingVideo), and this
  // answer is only what it falls back on.
  //
  // Elsewhere the same, generalised (#689, #643): a video in a feed (LinkedIn's, Facebook's, X's,
  // Reddit's…) sits in the post it belongs to, and that post names its own address (its
  // permalink, its timestamp link, the post id the page gives it); `ownAddress` finds it the way
  // a person would. Null there too when nothing names it: a feed's address is never the video's.
  //
  // ---- the video's own address: from here to its end mark, run as is on saved pages by
  // tools/extension/test.mjs (only `location`, `document` and the elements' parentElement,
  // parentNode.host, localName, getAttribute, querySelector(All) with simple selectors).
  function videoPage(v) {
    const watch = (id) =>
      /^[\w-]+$/.test(id || "") ? `https://www.youtube.com/watch?v=${id}` : null;
    try {
      const url = new URL(location.href);
      const name = url.hostname.replace(/^(www|m|music)\./, "");
      if (name === "youtu.be") {
        const id = url.pathname.split("/")[1];
        if (watch(id)) return `https://youtu.be/${id}`;
      }
      if (name === "youtube.com" || name === "youtube-nocookie.com") {
        // An embed's own page is the video's (…/embed/<id>).
        if (url.pathname.startsWith("/embed/")) {
          const id = watch(url.pathname.split("/")[2]);
          if (id) return id;
        }
        // A Short's or a live stream's own page (/shorts/<id>, /live/<id>). yt-dlp reads these as
        // the same one video as watch?v=<id>.
        const [, kind, pathId] = url.pathname.split("/");
        if (kind === "shorts" || kind === "live") {
          const id = watch(pathId);
          if (id) return id;
        }
        // The page is this video's own: a watch page, with or without a list on it.
        const own = watch(url.searchParams.get("v"));
        if (own && url.pathname.startsWith("/watch")) return own;
        const shown = watch(document.querySelector("ytd-watch-flexy")?.getAttribute("video-id"));
        if (shown && url.pathname.startsWith("/watch")) return shown;
        const player = watch(document.querySelector("#movie_player")?.getAttribute("video-id"));
        if (player) return player;
        // A player inside the card it belongs to (a feed, a search, a playlist): the card links to
        // the video. Only a container that names exactly one video counts — walking on into the
        // page's column would find some other card's link and send the wrong video, which is worse
        // than sending none.
        for (let node = v, i = 0; node && i < 12; i += 1, node = node.parentElement) {
          const ids = new Set();
          for (const a of node.querySelectorAll?.('a[href*="/watch?"], a[href*="youtu.be/"]') || []) {
            const link = new URL(a.href, location.href);
            const id = link.hostname.endsWith("youtu.be")
              ? link.pathname.split("/")[1]
              : link.searchParams.get("v");
            if (watch(id)) ids.add(id);
          }
          if (ids.size === 1) return watch([...ids][0]);
          if (ids.size > 1) break;
        }
        // A page that plays a video and names none: not the video's page, so not the app's.
        return null;
      }
      return ownAddress(v, url);
    } catch {
      // Not an address this page can reason about: its own follows.
    }
    return location.href;
  }

  // ---- the qualities: the app looks the video up (yt-dlp), the user picks one here ------------
  // How long a lookup is followed, from the click.
  const LOOKUP_MS = 60000;

  // The wait before asking how a lookup is going: 100 ms before the first question. An app that
  // held the last question while it looked (a newer one, up to ~1.5 s: `took` ms of HELD_MS or
  // more) is asked again at once (50 ms), so the answer reaches the page as soon as it is ready.
  // One that answers at once (an older one) is asked as seldom as ever: the old backoff's target
  // (1 s, a little longer each time, 2 s at most) less the time the question took, 50 ms at least.
  function pollDelay(n, took) {
    const HELD_MS = 1000;
    if (n <= 0) return 100;
    const spent = Number.isFinite(took) ? Math.max(0, took) : 0;
    if (spent >= HELD_MS) return 50;
    const target = Math.min(2000, Math.round(1000 * 1.25 ** n));
    return Math.max(50, Math.round(target - spent));
  }

  // A size as the browser's language writes it ("178.8 MB"); "" when unknown.
  function sizeText(bytes, lang) {
    const n = Number(bytes);
    if (!Number.isFinite(n) || n <= 0) return "";
    const units = [["gigabyte", 2 ** 30], ["megabyte", 2 ** 20], ["kilobyte", 2 ** 10]];
    const [unit, per] = units.find(([, u]) => n >= u) || ["byte", 1];
    try {
      return new Intl.NumberFormat(lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(n / per);
    } catch {
      return `${(n / per).toFixed(1)} ${unit}`;
    }
  }

  // A quality's size: exactly the bytes of the streams it downloads, or "~" before one the app
  // only estimates (`approx`, #801: "~532 MB"); "" when unknown.
  function choiceSize(c, lang) {
    const size = sizeText(c && c.size, lang);
    return size && c.approx === true ? `~${size}` : size;
  }

  // What a quality's row says: the app's name for it (in the app's language) and its size. Only
  // ever set as text, never as markup.
  function rowText(choice, lang) {
    const c = choice && typeof choice === "object" ? choice : {};
    return { name: String(c.label ?? "").slice(0, 200), size: choiceSize(c, lang) };
  }

  // Which of the app's qualities go in which tab: `choice.audio === true` is sound only, anything
  // else (an app that doesn't say, an older one) is video. Tabs only when both kinds are there;
  // otherwise one list (`all`) in the app's order. Every number is the quality's place in the
  // app's own list (what a pick sends), never its place in a tab.
  function groupChoices(choices, max) {
    const list = Array.isArray(choices) ? choices.slice(0, Math.max(0, Number.isFinite(max) ? max : 100)) : [];
    const video = [];
    const audio = [];
    list.forEach((c, i) => (c && typeof c === "object" && c.audio === true ? audio : video).push(i));
    return {
      tabbed: video.length > 0 && audio.length > 0,
      video,
      audio,
      all: list.map((_, i) => i),
      start: video.length || !audio.length ? "video" : "audio",
    };
  }

  // Follows a lookup the app is running, from its first answer: asks again only once the last
  // question was answered (never on a timer of its own), pollDelay later, until it is no longer
  // pending (that answer, with its token), the minute is up ({state: "late"}) or it is no longer
  // wanted (null: the menu closed, the video or the page changed). `ask(token, ms)`: the
  // background's answer, or {late: true} after `ms`; `sleep(ms)` may end early when dropped.
  async function followLookup(first, { ask, alive, sleep, now, end }) {
    let s = first;
    let token = "";
    let took = 0;
    for (let n = 0; ; n += 1) {
      if (!alive()) return null;
      if (!s || typeof s !== "object") return { state: "gone" };
      if (typeof s.token === "string" && s.token) token = s.token;
      if (s.state !== "pending") return { ...s, token };
      if (!token) return { state: "gone" };
      const left = end - now();
      if (left <= 0) return { state: "late" };
      await sleep(Math.min(pollDelay(n, took), left));
      if (!alive()) return null;
      if (end - now() <= 0) return { state: "late" };
      const asked = now();
      const r = await ask(token, end - asked);
      took = now() - asked;
      if (!alive() || !r || r.gone) return null;
      if (r.late) return { state: "late" };
      // Another MDM chosen meanwhile: the window, as for a click.
      s = r.fallback ? { state: "use-window" } : r.lookup;
    }
  }

  // The background's answer to a lookup or to a choice, as a lookup's state: null when there is
  // nothing to show (the extension gone); "late" past the minute, "unnamed" for a video it can't
  // tell, "feed" for a video in a feed no post names (#689), and "use-window" for no lookups here
  // (another MDM, an older one) or an answer that isn't one.
  function replyState(r) {
    if (r && r.gone) return null;
    if (r && r.late) return { state: "late" };
    if (r && (r.why === "unnamed" || r.why === "feed")) return { state: r.why };
    if (!r || r.fallback || !r.lookup || typeof r.lookup !== "object") return { state: "use-window" };
    return r.lookup;
  }

  // The app's word on what the look-up lacked, to show with its qualities (#852: YouTube without
  // a JavaScript runtime lists only some): its text, its link's words and the help page it
  // opens, on MDM's own site only. Null when there is none (an app older than it) or it isn't one.
  function noticeOf(s) {
    const n = s && typeof s === "object" ? s.notice : null;
    if (!n || typeof n !== "object" || typeof n.text !== "string" || !n.text.trim()) return null;
    const ok = typeof n.url === "string" && /^https:\/\/misterdownloadmanager\.com\//.test(n.url);
    const link = ok && typeof n.link === "string" && n.link.trim() ? n.link.slice(0, 100) : "";
    return { text: n.text.slice(0, 400), link, url: link ? n.url : "" };
  }

  // What a lookup's state (or replyState's) comes to, as the menu does it: "follow" it while the
  // app looks, "offer" its qualities, "say" a message in the menu (with the app's own words under
  // it), "flash" a word on the button (the menu closed), or "window": the click as it always was,
  // the app's Download video window. `t`: the texts. `picked`: the answer to a choice, which only
  // a download confirms; anything else there is the window's.
  function lookupAction(s, t, picked) {
    const state = s && typeof s === "object" && typeof s.state === "string" ? s.state : "";
    // A choice taken: downloading, in the queue, or handed to the app, which didn't say in time
    // what became of it ("sent": it will download it; the window would download it again).
    if (state === "started" || state === "sent") return { act: "flash", kind: "ok", text: t("video_added") };
    if (state === "queued") return { act: "flash", kind: "ok", text: t("video_queued") };
    if (!picked) {
      if (state === "pending") return { act: "follow" };
      if (state === "ready" && Array.isArray(s.choices) && s.choices.length) return { act: "offer" };
      if (state === "ready" || (state === "failed" && s.reason === "no-choices")) return { act: "say", text: t("video_no_choices") };
      if (state === "failed" && s.reason === "no-ytdlp") return { act: "say", text: t("video_no_ytdlp") };
      if (state === "failed") return { act: "say", text: t("video_failed_lookup"), detail: typeof s.message === "string" ? s.message : "" };
      if (state === "late") return { act: "say", text: t("video_too_long") };
      if (state === "list") return { act: "say", text: t("video_playlist") };
      if (state === "busy") return { act: "say", text: t("video_busy") };
      if (state === "off") return { act: "flash", kind: "off", text: t("video_off") };
      if (state === "unnamed") return { act: "flash", kind: "warn", text: t("video_unnamed") };
      if (state === "feed") return { act: "flash", kind: "warn", text: t("video_open_post") };
    }
    // "use-window", "gone" (the app forgot it: restarted), a choice the app didn't take, and
    // anything newer: the window.
    return { act: "window" };
  }

  // The click's window at most once, whichever way the click comes to it (an answer that isn't a
  // lookup, the lookup's end, a choice the app didn't take, "Open in…"): true the first time for
  // `j`, the click's lookup.
  function firstWindow(j) {
    if (!j || j.windowed) return false;
    j.windowed = true;
    return true;
  }

  // The video the pointer takes the button to: the one under it (`found`), but not another than
  // its own (`shown`) while the qualities' menu is open (`picking`): the menu reaches over the
  // videos around its own (a feed's), and the pointer crosses them on its way to a row. The menu
  // still closes when its video plays something else or the page changes (tick), or on a click
  // elsewhere.
  function pointerTarget(found, shown, picking) {
    if (!found || (picking && found !== shown)) return null;
    return found;
  }

  // Up from a node: its parent, or the element whose shadow root holds it (Reddit's player keeps
  // its <video> in one).
  const upFrom = (n) => n.parentElement || (n.parentNode && n.parentNode.host) || null;

  // `href` as an address (relative to the page), or null.
  function linkTo(href) {
    try {
      return href ? new URL(href, location.href) : null;
    } catch {
      return null;
    }
  }

  // The nearest container around the video that names its post: `read(node)` gives what a
  // container names, { sure, maybe } (Maps of an id to the address it stands for), `sure` the
  // post's or the video's own ids, `maybe` weaker ones (a link that may be the post's). The
  // innermost container naming anything decides, and only when it names exactly one: one naming
  // two has taken in the next post or the page's column, and a wrong video is worse than none.
  // The walk ends at the page's column (a feed, role="feed") and at a container holding another
  // video: from there up, what it names may be that one's (a post naming nothing next to one
  // that names its own). A page that breaks a reader's expectations names nothing (never the
  // page's own address instead).
  function nearest(v, read) {
    try {
      for (let node = v, i = 0; node && i < 16; i += 1, node = upFrom(node)) {
        if (node.localName === "body" || node.localName === "html") break;
        if (node !== v && (node.getAttribute?.("role") === "feed" || manyVideos(node))) break;
        const { sure, maybe } = read(node);
        if (sure.size) return sure.size === 1 ? [...sure.values()][0] : null;
        if (maybe.size) return maybe.size === 1 ? [...maybe.values()][0] : null;
      }
    } catch {
      // Nothing named.
    }
    return null;
  }

  const decoded = (s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };

  // The links in a container (the container itself when it is one), as addresses, with whether
  // each holds a <time> (a timestamp link: a post's own permalink).
  function linksIn(node, sel = "a[href]") {
    const found = [];
    const own = node.localName === "a" ? [node] : [];
    for (const a of [...own, ...(node.querySelectorAll ? node.querySelectorAll(sel) : [])]) {
      const u = linkTo(a.getAttribute && a.getAttribute("href"));
      if (u && /^https?:$/.test(u.protocol)) found.push({ u, time: Boolean(a.querySelector && a.querySelector("time")) });
    }
    return found;
  }

  // The page's site without its www. or mobile host (old.reddit.com is reddit.com).
  const siteName = (host) => host.replace(/^(www|m|mobile|web|touch|old|new|sh|music|geo|player)\./, "");

  // A video's own address on this page, by site; the page's own when it is that one video's page;
  // null when the page is a feed, a list or a search and nothing names the video.
  function ownAddress(v, url) {
    const site = SITES.find((s) => s.hosts.test(siteName(url.hostname)));
    if (site) {
      // A site's own player in another page (an embed) is its video's page: its address holds
      // what the links around it may lack (a private video's key).
      if (site.embed && site.embed(url)) return location.href;
      const found = nearest(v, (node) => site.read(node, url));
      if (found) return found;
      return site.own(url) ? location.href : null;
    }
    // Any other site: a player embedded in another page (its own frame), or the one video of its
    // page, is its video's own page, as before.
    if (framed() || (!inFeed(v) && document.querySelectorAll?.("video").length <= 1)) return location.href;
    // A page of many videos: the post around this one, when one names it; else the page in
    // general (an article with two players is still theirs to yt-dlp), but never a feed's
    // (role="feed") or a page whose address is a feed's, a home's or a search's.
    const found = nearest(v, (node) => genericRead(node, url));
    if (found) return found;
    return inFeed(v) || FEED_PATH.test(url.pathname) ? null : location.href;
  }

  // A feed's, a home page's, a search's address: many videos, none of them the page's.
  const FEED_PATH = /^\/(?:$|home\/?$|feed|explore|search|discover|trending|popular|latest|timeline|for-?you|following|watch\/?$)/i;
  // The video sits in a feed (role="feed": Mastodon's, the accessible feeds).
  function inFeed(v) {
    for (let node = upFrom(v); node; node = upFrom(node)) if (node.getAttribute?.("role") === "feed") return true;
    return false;
  }

  const framed = () => {
    try {
      return window.top !== window;
    } catch {
      return false;
    }
  };
  const manyVideos = (node) => Boolean(node.querySelectorAll && node.querySelectorAll("video").length > 1);

  // Any other site: a link of this site that has a post's or a video's shape, or that holds the
  // post's timestamp (Mastodon's, a forum's), and isn't the page itself.
  function genericRead(node, url) {
    const sure = new Map();
    const maybe = new Map();
    const here = `${url.origin}${url.pathname}${url.search}`;
    for (const { u, time } of linksIn(node)) {
      if (siteName(u.hostname) !== siteName(url.hostname)) continue;
      const address = `${u.origin}${u.pathname}${u.search}`;
      if (address === here) continue;
      if (time) sure.set(address, address);
      else if (/\/(?:status|statuses|videos?|watch|posts?|p|reels?|comments|clips?|v)(?:\/|\?|$)/i.test(u.pathname + (u.search ? "?" : ""))) maybe.set(address, address);
    }
    const permalink = linkTo(node.getAttribute?.("data-permalink"));
    if (permalink && siteName(permalink.hostname) === siteName(url.hostname)) sure.set(permalink.href, permalink.href);
    return { sure, maybe };
  }

  // LinkedIn's comment tools' key: base64url of a protobuf naming the post (field 1 its
  // activity, field 2 its ugcPost), whose one field is the id as a zigzag varint. [kind, id] or
  // null.
  function commentToolsUrn(key) {
    try {
      const bytes = [...atob(key.replace(/-/g, "+").replace(/_/g, "/"))].map((c) => c.charCodeAt(0));
      const kind = { 10: "activity", 18: "ugcPost" }[bytes[0]];
      if (!kind || bytes[1] !== bytes.length - 2 || bytes[2] !== 8) return null;
      let n = 0n;
      let shift = 0n;
      for (let i = 3; i < bytes.length; i += 1) {
        n |= BigInt(bytes[i] & 0x7f) << shift;
        shift += 7n;
        if (!(bytes[i] & 0x80)) {
          if (i !== bytes.length - 1 || n & 1n) return null;
          const id = String(n >> 1n);
          return /^\d{15,20}$/.test(id) ? [kind, id] : null;
        }
      }
    } catch {
      // Not such a key.
    }
    return null;
  }

  // The big sites with feeds: what names a post in their pages, the address yt-dlp reads it by
  // (its extractors' own address shapes), and their pages that are one video's own.
  const SITES = [
    {
      // A post's container carries its URN (data-urn, data-id); its links name it too. yt-dlp's
      // LinkedIn extractor reads /feed/update/urn:li:activity:<id>/ (and /posts/…); a post known
      // only by its ugcPost URN goes as /feed/update/urn:li:ugcPost:<id>/, the same public page,
      // where yt-dlp's generic reader finds the video.
      //
      // LinkedIn's feed since 2026 (its server-driven app: role="listitem" posts in a
      // data-testid="mainFeed" list) names a post nowhere near its video, only in the keys of
      // the post's parts (componentkey): its comments' URNs (urn:li:comment:(activity:<id>,…)),
      // its text's translation key (…userGeneratedContentId=<id>…, its ugcPost), and its comment
      // tools' key, <base64>-replaceableCommentTools, a protobuf of the post's URN (field 1 its
      // activity, field 2 its ugcPost, the id zigzag-encoded). Checked against a signed-in feed:
      // each decodes to the post its comments name, and each ugcPost's page plays the video.
      hosts: /^linkedin\.com$/,
      embed: (u) => /^\/embed\//.test(u.pathname),
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        const post = (kind, id) =>
          (kind === "activity" ? sure : maybe).set(`${kind}:${id}`, `https://www.linkedin.com/feed/update/urn:li:${kind}:${id}/`);
        const add = (s) => {
          s = String(s || "");
          for (const m of s.matchAll(/urn:li:(?:comment:\()?(activity|ugcPost|share):(\d{6,})/g)) post(m[1], m[2]);
          for (const m of s.matchAll(/\b(userGeneratedContent|share|activity)Id=(\d{6,})/g)) post(m[1] === "userGeneratedContent" ? "ugcPost" : m[1], m[2]);
          // A post's public address: /posts/<who>_<words>-activity-<id>-<four>.
          const p = /\/posts\/[^/?#]*-(activity|ugcPost)-(\d{6,})-\w{4}/.exec(s);
          if (p) post(p[1], p[2]);
          const k = /^([\w-]{12,40})-replaceableCommentTools/.exec(s);
          const urn = k && commentToolsUrn(k[1]);
          if (urn) post(urn[0], urn[1]);
        };
        // The container's own attributes, whatever their names (the page renames them), and its
        // insides' known ones.
        for (const a of node.attributes || []) add(a.value);
        for (const el of node.querySelectorAll?.("[data-urn], [data-id], [data-activity-urn], [data-entity-urn], [componentkey]") || []) {
          for (const a of ["data-urn", "data-id", "data-activity-urn", "data-entity-urn", "componentkey"]) add(el.getAttribute?.(a));
        }
        for (const { u } of linksIn(node)) if (/linkedin\.com$/.test(u.hostname)) add(decoded(u.pathname));
        return { sure, maybe };
      },
      own: (u) => /^\/(?:feed\/update\/urn:li:|posts\/|embed\/feed\/update\/|events\/|learning\/)/.test(u.pathname),
    },
    {
      // A video's own links (a reel, a watch page, a page's video, data-video-id) before a post's
      // (its permalink); fb.watch short links as they are (they lead to the video's page).
      hosts: /^(?:[\w-]+\.)?facebook\.com$/,
      embed: (u) => /^\/plugins\/video\.php$/.test(u.pathname),
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        const video = (id) => sure.set(`v:${id}`, `https://www.facebook.com/watch/?v=${id}`);
        const id = node.getAttribute?.("data-video-id");
        if (/^\d{6,}$/.test(id || "")) video(id);
        for (const el of node.querySelectorAll?.("[data-video-id]") || []) {
          const d = el.getAttribute("data-video-id");
          if (/^\d{6,}$/.test(d || "")) video(d);
        }
        for (const { u } of linksIn(node)) {
          if (u.hostname === "fb.watch") {
            const code = u.pathname.split("/")[1];
            if (/^\w+$/.test(code || "")) sure.set(`w:${code}`, `https://fb.watch/${code}/`);
            continue;
          }
          if (!/(^|\.)facebook\.com$/.test(u.hostname)) continue;
          const p = u.pathname;
          let m;
          if ((m = /^\/reel\/(\d+)/.exec(p))) sure.set(`v:${m[1]}`, `https://www.facebook.com/reel/${m[1]}/`);
          else if (/^\/watch\/?$/.test(p) && /^\d+$/.test(u.searchParams.get("v") || "")) video(u.searchParams.get("v"));
          else if (/^\/video\.php$/.test(p) && /^\d+$/.test(u.searchParams.get("v") || "")) video(u.searchParams.get("v"));
          else if ((m = /^\/[^/]+\/videos\/(?:[^/]+\/)?(\d+)/.exec(p))) video(m[1]);
          else if ((m = /^\/([^/]+)\/posts\/(pfbid\w+|\d+)/.exec(p))) maybe.set(`p:${m[2]}`, `https://www.facebook.com/${m[1]}/posts/${m[2]}`);
          else if ((m = /^\/groups\/([^/]+)\/(?:permalink|posts)\/(\d+)/.exec(p))) maybe.set(`p:${m[2]}`, `https://www.facebook.com/groups/${m[1]}/posts/${m[2]}/`);
          else if (/^\/(?:permalink|story)\.php$/.test(p) && /^(?:pfbid\w+|\d+)$/.test(u.searchParams.get("story_fbid") || "")) {
            const story = u.searchParams.get("story_fbid");
            const owner = u.searchParams.get("id");
            maybe.set(`p:${story}`, `https://www.facebook.com/permalink.php?story_fbid=${story}${/^\d+$/.test(owner || "") ? `&id=${owner}` : ""}`);
          }
        }
        return { sure, maybe };
      },
      own: (u) =>
        /^\/(?:reel\/\d|watch\/?$|video\.php$|[^/]+\/videos\/|[^/]+\/posts\/|permalink\.php$|story\.php$|groups\/[^/]+\/(?:permalink|posts)\/|plugins\/video\.php$|share\/[vrp]\/)/.test(u.pathname) &&
        (!/^\/watch\/?$/.test(u.pathname) || /^\d+$/.test(u.searchParams.get("v") || "")),
    },
    {
      // A post's timestamp links to it (/<user>/status/<id>); other status links (a quoted post,
      // its photos and its analytics) count only when there is no timestamp.
      hosts: /^(?:x|twitter)\.com$/,
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        for (const { u, time } of linksIn(node, 'a[href*="/status/"]')) {
          const m = /^\/(\w{1,15}|i\/web)\/status\/(\d+)/.exec(u.pathname);
          if (!m || !/(^|\.)(?:x|twitter)\.com$/.test(u.hostname)) continue;
          (time ? sure : maybe).set(m[2], `${location.origin}/${m[1]}/status/${m[2]}`);
        }
        return { sure, maybe };
      },
      own: (u) => /^\/(?:\w{1,15}|i\/web)\/status\/\d+|^\/i\/(?:broadcasts|spaces)\//.test(u.pathname),
    },
    {
      // A post's links (/p/<code>/, a reel's /reel/<code>/): its timestamp, its comments.
      hosts: /^instagram\.com$/,
      embed: (u) => /\/embed\/?$/.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node)) {
          // Not a reel's sound (/reels/audio/<id>/, the other reels using it).
          const m = /^(?:\/[\w.]+)?\/(p|tv|reels?)\/(?!audio\/)([\w-]{5,})/.exec(u.pathname);
          if (!m || !/instagram\.com$/.test(u.hostname)) continue;
          sure.set(m[2], `https://www.instagram.com/${m[1] === "tv" ? "p" : m[1].replace(/s$/, "")}/${m[2]}/`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => /^(?:\/[\w.]+)?\/(?:p|tv|reels?)\/[\w-]{5,}|^\/stories\/[^/]+\/\d/.test(u.pathname),
    },
    {
      // A post (shreddit-post, old Reddit's .thing) names its permalink in its attributes; its
      // links (title, comments) name the same /comments/<id>/.
      hosts: /^reddit\.com$/,
      read(node) {
        const sure = new Map();
        const add = (s) => {
          const m = /^(\/(?:r|u|user)\/[^/]+)?\/comments\/(\w+)/.exec(linkTo(s)?.pathname || "");
          if (m) sure.set(m[2], `${location.origin}${m[1] || ""}/comments/${m[2]}/`);
        };
        add(node.getAttribute?.("permalink"));
        add(node.getAttribute?.("data-permalink"));
        for (const el of node.querySelectorAll?.("[permalink], [data-permalink]") || []) {
          add(el.getAttribute("permalink"));
          add(el.getAttribute("data-permalink"));
        }
        for (const { u } of linksIn(node, 'a[href*="/comments/"]')) if (/reddit\.com$/.test(u.hostname)) add(u.href);
        return { sure, maybe: new Map() };
      },
      own: (u) => /\/comments\/\w+|^\/r\/[^/]+\/s\/\w+/.test(u.pathname),
    },
    {
      // A video's link (/@<user>/video/<id>); else its player's id (xgwrapper-<n>-<id>), sent in
      // yt-dlp's own form for a video whose author isn't known (/@_/video/<id>).
      hosts: /^tiktok\.com$/,
      embed: (u) => /^\/embed\//.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node, 'a[href*="/video/"]')) {
          const m = /^\/@([\w.-]+)\/video\/(\d+)/.exec(u.pathname);
          if (m && /tiktok\.com$/.test(u.hostname)) sure.set(m[2], `https://www.tiktok.com/@${m[1]}/video/${m[2]}`);
        }
        if (!sure.size) {
          for (const el of [node, ...(node.querySelectorAll?.('[id^="xgwrapper-"]') || [])]) {
            const m = /^xgwrapper-\d+-(\d{15,})$/.exec(el.getAttribute?.("id") || "");
            if (m) sure.set(m[1], `https://www.tiktok.com/@_/video/${m[1]}`);
          }
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => /^\/@[\w.-]+\/(?:video|photo)\/\d+|^\/embed\//.test(u.pathname),
    },
    {
      // A clip's links: vimeo.com/<id> (with an unlisted video's hash), in a channel, a group, a
      // showcase.
      hosts: /^(?:player\.)?vimeo\.com$/,
      embed: (u) => u.hostname.startsWith("player."),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node)) {
          if (!/(^|\.)vimeo\.com$/.test(u.hostname)) continue;
          const m = /^(?:\/video|\/channels\/[^/]+|\/groups\/[^/]+\/videos|\/showcase\/[^/]+\/video)?\/(\d{4,})(\/[\da-f]{10})?\/?$/.exec(u.pathname);
          if (m) sure.set(m[1], `https://vimeo.com/${m[1]}${m[2] || ""}`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) =>
        u.hostname.startsWith("player.") ||
        /^(?:\/channels\/[^/]+|\/groups\/[^/]+\/videos|\/showcase\/[^/]+\/video|\/[^/]+\/review)?\/\d{4,}|^\/ondemand\//.test(u.pathname),
    },
    {
      hosts: /^(?:geo\.)?dailymotion\.com$/,
      embed: (u) => u.hostname.startsWith("geo.") || /^\/(?:embed|player)\b/.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node, 'a[href*="/video/"]')) {
          const m = /^\/(?:embed\/)?video\/([a-z0-9]+)/i.exec(u.pathname);
          if (m && /dailymotion\.com$/.test(u.hostname)) sure.set(m[1], `https://www.dailymotion.com/video/${m[1]}`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => u.hostname.startsWith("geo.") || /^\/(?:embed\/)?video\/[a-z0-9]+|^\/player/i.test(u.pathname),
    },
  ];
  // ---- the video's own address: end.

  // ---- the button's words (its chip, its tip, the menu's rows): plain functions, run as they are
  // by tools/extension/test.mjs. A quality is one of the app's choices (`label`, `ext`, `size`,
  // `audio`, and from a newer app `height`, `fps`, `kbps`, then `codec`, `range`, `approx`); the
  // video's own plain file is {file: true, height, ext}.

  // A quality's short name: 4K and 8K as people say them, else the picture's height, the sound,
  // or the app's own label (a file known only by its type).
  function qualityName(c, t) {
    if (!c || typeof c !== "object") return "";
    if (c.audio === true) return t("video_tab_audio");
    const h = Number(c.height) || 0;
    if (h >= 4000) return "8K";
    if (h >= 2000) return "4K";
    if (h > 0) return `${h}p`;
    if (c.file) return String(c.ext || "").toUpperCase();
    return String(c.label ?? "").slice(0, 200);
  }

  // What else the menu says of a quality: its file type, its codec, its frame rate and its
  // range (#755: "VP9 · 60 fps · HDR"), its sound's bitrate. The type goes without saying when
  // it is MP4 and the codec is said (every merged video is an MP4). An older app's choice (no
  // height) says it all in its label: nothing more.
  function qualityMeta(c, t) {
    if (!c || typeof c !== "object") return "";
    // The sound's rows say what they are in their label; their bitrate when the app says it.
    // (fps, kbps, codecs and HDR are the same in every language.)
    if (c.audio === true) return Number(c.kbps) > 0 ? `${Math.round(c.kbps)} kbps` : "";
    if (!c.height && !c.file) return "";
    const word = (x) => (typeof x === "string" ? x.trim().slice(0, 24) : "");
    const codec = word(c.codec);
    const ext = String(c.ext || "").toUpperCase();
    const parts = [codec && ext === "MP4" ? "" : ext, codec];
    if (Number(c.fps) > 0) parts.push(`${Math.round(c.fps)} fps`);
    parts.push(word(c.range));
    return parts.filter(Boolean).join(" · ");
  }

  // The quality one click downloads: the app's first (best) video choice, else its first; the
  // video's own plain file; none while it isn't known.
  function bestOf(look) {
    if (!look || typeof look !== "object") return null;
    if (look.file) return look;
    if (look.state !== "ready" || !Array.isArray(look.choices) || !look.choices.length) return null;
    return look.choices.find((c) => c && c.audio !== true) || look.choices[0];
  }

  // The chip: what one click gets ({text, hd}), a skeleton while the look-up runs, or nothing.
  function chipFor(look, t) {
    if (look && look.state === "pending") return { skeleton: true };
    const c = bestOf(look);
    const text = c && (c.height || c.file || c.audio === true) ? qualityName(c, t) : "";
    return text ? { text, hd: (Number(c.height) || 0) >= 720 } : null;
  }

  // The hover tip's first line: exactly what one click downloads ("Download 1080p MP4 · 412 MB").
  function tipFor(look, t, lang) {
    const c = bestOf(look);
    if (!c) return look && look.state === "pending" ? t("video_tip_best") : t("video_download");
    const name = qualityName(c, t);
    const what = c.height || c.file ? [name, String(c.ext || "").toUpperCase()].filter(Boolean).join(" ") : name;
    const size = choiceSize(c, lang);
    return t("video_tip", size ? `${what} · ${size}` : what);
  }

  // A length as players show it: 9:56, 1:02:03; "" when unknown.
  function durationText(s) {
    const n = Math.round(Number(s));
    if (!(n > 0)) return "";
    const h = Math.floor(n / 3600);
    const m = Math.floor((n % 3600) / 60);
    const sec = String(n % 60).padStart(2, "0");
    return h ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
  }

  // The menu's header line: the video's title, its length, the site.
  function headerLine(title, seconds, site) {
    return [String(title || "").slice(0, 200), durationText(seconds), site].filter(Boolean).join(" · ");
  }

  // The item the arrows go to from `i` (of `n`; -1: none yet), around the ends; Home and End.
  function moveIn(i, n, key) {
    if (!n) return -1;
    if (key === "Home") return 0;
    if (key === "End") return n - 1;
    if (key === "ArrowDown") return i < 0 ? 0 : (i + 1) % n;
    if (key === "ArrowUp") return i < 0 ? n - 1 : (i + n - 1) % n;
    return i;
  }

  // Its words fit: a video at least 360 px wide, with room for the button's full width at its
  // edge. Else only the round button and its menu (a small video, a long language).
  function fitsWords(videoWidth, fullWidth, inset) {
    return videoWidth >= 360 && fullWidth <= videoWidth - 2 * inset;
  }
  // ---- the button's words: end.

  // The address this page first loaded at (`download`: a page that moves in place keeps its first
  // page's og:title).
  const landedAt = location.href;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // "Hide on this video": the video's address (players reuse one element for the next video),
  // else the element.
  const dismissed = new WeakSet();
  const dismissedSrc = new Set();
  const isDismissed = (v) => dismissed.has(v) || (v.currentSrc && dismissedSrc.has(v.currentSrc));
  // May it show here? The app's answer (Options: the button on, the site not listed), asked
  // again when older than ASK_MS, so a change in Options reaches a page already open (#532: one
  // that never reloads, as YouTube's, kept the button for good). `never`: this page's own
  // "Never on this site".
  let allowed = null;
  let askedAt = 0;
  let asking = null;
  let never = false;
  let placed = false;
  const ASK_MS = 5000;
  const answered = () => allowed !== null && Date.now() - askedAt < ASK_MS;
  let place = { x: 1, y: 0 }; // where on the edge: fractions of the room, top right by default
  let chosen = false; // the user put it where it is (dragged it, on this site): theirs, never moved for a page's control
  let auto = { v: null, at: null, t: 0 }; // the spot found for this video, kept for a moment
  const AUTO_MS = 400;
  // Where it is on the screen, while the pointer is on it (#560): { v, left, top, w, end }.
  let pinned = null;
  let video = null; // the video it is over
  // A video gone from under its button (#503): a hover-to-play preview (YouTube's thumbnails)
  // stops as soon as the pointer leaves it for the button, since the page sees the pointer go.
  // The button stays where it was, for that video, while the pointer is on it (or its menu is
  // open), or for HOLD_MS while the pointer is still over where the video was, on its way to it;
  // a click downloads what the video was playing.
  let held = null; // { r: the video's last rectangle, at: the button's spot, d: what download sends, since }
  const HOLD_MS = 1500;
  let lastRect = null; // the video's rectangle when the button was last placed over it
  let entered = null; // what download would send, taken as the pointer reached the button: { v, d, t }
  let lastMove = 0;
  let pointer = { x: -1, y: -1 };
  let menuOpen = false;
  let dragging = null;
  let busy = false;
  let raf = 0;
  let scan = 0;
  // Kept on screen while the keyboard is on it (not after a mouse click: that leaves focus too),
  // or on the video's player (its own controls).
  const keyboardOn = () =>
    Boolean(shadow.activeElement && shadow.activeElement.matches(":focus-visible")) || playerFocused();
  let focusedPlayer = null;
  const playerFocused = () =>
    Boolean(video && focusedPlayer && document.activeElement === focusedPlayer &&
      focusedPlayer.contains(video) && focusedPlayer.matches(":focus-visible"));

  // ---- the button: a closed shadow tree the page's styles and scripts can't reach ----------
  const root = document.createElement("mdm-video-button");
  // Each with !important: a page's own rules can't hide or move it.
  for (const [k, v] of Object.entries({
    all: "initial", position: "fixed", "z-index": "2147483647", left: "0", top: "0", display: "none",
    contain: "layout style", opacity: "1", visibility: "visible", "pointer-events": "auto",
  })) root.style.setProperty(k, v, "important");
  const shadow = root.attachShadow({ mode: "closed" });
  // It sits on the video like the player's own controls: dark frosted glass that reads over any
  // frame (bright ones too), MDM's one blue for the action, and its own colour for each thing that
  // stops it (green done, amber the user's step, red MDM's problem, grey can't by design). Start
  // and end, never left and right: a right-to-left language mirrors it whole.
  const CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
.pill, .menu, .tip, .guide {
  --go: #57cc5a; --go-hi: #47b94a; --go-ink: #08200b; --stamp: #1b7a26; --accent: #1b7a26;
  --ok: #1b7a26; --warn: #8a5300; --err: #c42b1c; --warn-dot: #F7B500;
  --bg: #fcfcfb; --bg2: #eef1f5; --bd: #c4ccd8; --tx: #111318; --tx2: #5f6676; --hov: rgba(87, 204, 90, .16);
  --sh: 0 1px 2px rgba(17, 19, 24, .18), 0 6px 14px rgba(17, 19, 24, .18);
  --font: "Segoe UI Variable Text", "Segoe UI", system-ui, -apple-system, Roboto, "Noto Sans", sans-serif;
}
/* Windows (the system) in dark mode, not the page: MDM's navy board; the green stays. */
@media (prefers-color-scheme: dark) {
  .pill, .menu, .tip, .guide {
    --accent: #7fe07f; --ok: #7fe07f; --warn: #ffd36b; --err: #ff99a4;
    --bg: #0c1a33; --bg2: #081226; --bd: #2a3d63; --tx: #f2f5fb; --tx2: #b7c3da; --hov: rgba(87, 204, 90, .18);
    --sh: 0 2px 4px rgba(0, 0, 0, .45), 0 8px 16px rgba(0, 0, 0, .45);
  }
}
[dir="rtl"] { --font: "Segoe UI", "Segoe UI Variable Text", system-ui, -apple-system, Roboto, "Noto Sans Arabic", "Noto Sans", sans-serif; }
/* The tab: a light raised frame at the video's edge; the Download part in green, the arrow and
   the close (hide on this video) beside it, each its own square button. */
.pill {
  display: inline-flex; align-items: stretch; gap: 3px; height: 30px; padding: 2px;
  background: linear-gradient(var(--bg), var(--bg2)); border: 1px solid var(--bd); border-radius: 5px;
  box-shadow: var(--sh); color: var(--tx);
  font: 400 12px/1 var(--font); white-space: nowrap;
  opacity: 0; transform: translateY(-2px);
  transition: opacity .14s ease-out, transform .14s ease-out;
  user-select: none; -webkit-user-select: none; touch-action: none;
}
.pill.shown { opacity: 1; transform: none; }
button {
  /* all: unset resets box-sizing too, past the rule above: without it a menu item's 100 % width
     plus its padding ran past the menu's edge (#81). */
  all: unset; box-sizing: border-box; display: inline-flex; align-items: center;
  color: inherit; cursor: pointer; font: inherit;
}
.go { gap: 6px; padding-inline: 5px 9px; border-radius: 3px; cursor: grab; }
.pill:not(.bare) .go { background: var(--go); color: var(--go-ink); box-shadow: inset 0 -1px 0 rgba(0, 0, 0, .14); }
.pill:not(.bare) .go:hover { background: var(--go-hi); }
.pill:not(.bare) .go .label { font-weight: 700; }
.more, .x { width: 22px; justify-content: center; flex: none; color: var(--tx2);
  border: 1px solid var(--bd); border-radius: 3px; background: var(--bg); }
.more:hover, .x:hover, .more[aria-expanded="true"] { background: var(--hov); color: var(--tx); }
.more svg, .x svg { width: 10px; height: 10px; }
.more[aria-expanded="true"] svg { transform: rotate(180deg); }
/* The keyboard's ring: a dark one, outside the frame. */
.pill:has(button:focus-visible) { outline: 2px solid var(--tx); outline-offset: 2px; }
button:focus-visible { outline: none; }
.more:focus-visible, .x:focus-visible, .act:focus-visible { box-shadow: 0 0 0 1px var(--tx) inset; }
/* Its grip: there all along (the tab keeps its width: the arrow and ✕ never move from under the
   pointer), seen while the pointer is on it. */
.grip { display: grid; visibility: hidden; grid-template-columns: repeat(2, 2px); gap: 2px; align-content: center; padding-inline-start: 1px; }
.grip i { display: block; width: 2px; height: 2px; border-radius: 1px; background: currentColor; opacity: .55; }
.pill:not(.bare):hover .grip, .pill.dragging .grip { visibility: visible; }
.pill.bare .grip { display: none; }
.icon { display: flex; align-items: center; justify-content: center; width: 18px; height: 18px; flex: none; }
.icon > * { display: none; }
.icon img { width: 18px; height: 18px; }
.icon svg { width: 16px; height: 16px; }
.pill[data-state="ready"] .icon .logo, .pill[data-state="looking"] .icon .logo,
.pill[data-state="sending"] .icon .spin, .pill[data-state="sent"] .icon .i-sent,
.pill[data-state="warn"] .icon .i-warn, .pill[data-state="err"] .icon .i-err,
.pill[data-state="off"] .icon .i-off, .pill[data-state="drm"] .icon .i-drm { display: block; }
.spin { width: 14px !important; height: 14px !important; border-radius: 50%; border: 2px solid rgba(8, 32, 11, .25); border-top-color: var(--go-ink);
  animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
@keyframes shimmer { from { background-position: 100% 0; } to { background-position: -100% 0; } }
.chip { font: 700 10.5px/15px var(--font); color: var(--go-ink); background: rgba(255, 255, 255, .55); border-radius: 3px; padding: 0 4px; }
.chip.skel { width: 30px; height: 15px; padding: 0; background-size: 200% 100%;
  background-image: linear-gradient(90deg, rgba(255, 255, 255, .3), rgba(255, 255, 255, .8), rgba(255, 255, 255, .3));
  animation: shimmer 1.4s linear infinite; }
.chip:empty:not(.skel), .sub:empty, .label:empty, .acts:empty { display: none; }
.label { font-weight: 600; }
.sub { color: var(--tx2); }
/* A state says what happened in its own colour, on the frame (the green is for Download and
   arrived only). */
.pill[data-state="sent"] .label { color: var(--ok); }
.pill[data-state="warn"] .label { color: var(--warn); }
.pill[data-state="err"] .label { color: var(--err); }
.pill[data-state="off"] .label, .pill[data-state="drm"] .label { color: var(--tx2); }
.pill[data-state="sending"] .go { background: var(--go); color: var(--go-ink); }
.pill[data-state="sending"] .label { font-weight: 700; }
.acts { display: inline-flex; align-items: center; gap: 2px; padding-inline-end: 4px; }
.act { font: 600 11.5px/1 var(--font); color: var(--accent); padding: 4px 6px; border-radius: 4px; }
.act:hover { background: var(--hov); }
.act.fill { background: var(--go); color: var(--go-ink); }
.act.fill:hover { background: var(--go-hi); }
.pill.bare .go { cursor: default; padding-inline-end: 4px; }
.pill.bare .more { display: none; }
/* Small video, or words that wouldn't fit: the logo in green, the arrow, the close; a state keeps
   its message and its main action. */
.pill.compact .chip, .pill.compact .sub, .pill.compact .act:not(.fill) { display: none; }
.pill.compact:not(.bare) .label { display: none; }
.pill.compact:not(.bare) .go { padding-inline: 4px; }
.pill.dragging { cursor: grabbing; transition: none; transform: translateY(-1px);
  box-shadow: 0 4px 8px rgba(0, 0, 0, .25), 0 16px 32px rgba(0, 0, 0, .3); }
.pill.dragging .go { cursor: grabbing; }
/* The hover tip: exactly what one click gets, and how to move it. */
.tip { position: absolute; top: 36px; left: 0; max-width: 320px; width: max-content; padding: 6px 9px; border-radius: 6px;
  pointer-events: none; background: var(--bg); color: var(--tx); border: 1px solid var(--bd); box-shadow: var(--sh);
  font: 400 11.5px/1.45 var(--font); white-space: normal; display: none; }
.tip.on { display: block; }
.tip .what { display: block; font-weight: 600; }
.tip .how { display: block; color: var(--tx2); }
/* While dragging: the edge it slides along and the spots it snaps to. */
.guide { position: absolute; left: 0; top: 0; display: none; pointer-events: none; }
.guide.on { display: block; }
.line { position: absolute; }
.line.h { height: 0; border-top: 1.5px dashed rgba(255, 255, 255, .75); }
.line.v { width: 0; border-inline-start: 1.5px dashed rgba(255, 255, 255, .75); }
.snap { position: absolute; width: 10px; height: 10px; margin: -5px 0 0 -5px; border-radius: 50%;
  border: 2px solid #fff; background: var(--go); }
/* The list: paper white (MDM's navy board in dark mode), one line per quality. */
.menu {
  position: absolute; width: 318px; max-width: calc(100vw - 16px); padding: 4px; border-radius: 6px;
  background: var(--bg); color: var(--tx); border: 1px solid var(--bd); box-shadow: var(--sh);
  font: 400 12px/1.2 var(--font); display: none; white-space: nowrap;
}
.menu.open { display: block; }
.menu:focus { outline: none; }
.mh { display: flex; gap: 8px; align-items: center; padding: 6px 8px 7px; }
.mh img { width: 18px; height: 18px; flex: none; }
.mh .t { display: block; font-weight: 600; overflow: hidden; text-overflow: ellipsis; max-width: 260px; }
.mh .s { display: block; color: var(--tx2); font-size: 11px; margin-top: 2px; }
.hr { height: 1px; background: var(--bd); margin: 4px 2px; }
.cap { font: 600 10.5px/1 var(--font); color: var(--tx2); padding: 6px 8px 2px; letter-spacing: .2px; text-transform: uppercase; }
.cap[hidden] { display: none; }
.note { padding: 6px 8px; color: var(--tx2); white-space: normal; }
.note[hidden] { display: none; }
.note .why { display: block; margin-top: 3px; font-size: 11px; overflow-wrap: anywhere; }
.note.warn { color: var(--warn); }
.note a { display: inline-block; margin-top: 3px; color: var(--accent); text-decoration: underline; }
.rows { max-height: min(300px, 45vh); overflow: auto; overscroll-behavior: contain; }
.menu button { position: relative; display: flex; width: 100%; height: 28px; gap: 8px; padding-inline: 8px; border-radius: 4px; cursor: default; }
.menu button:hover { background: var(--hov); }
.menu button.sel { background: var(--hov); }
.menu button.sel::before { content: ""; position: absolute; inset-inline-start: 0; top: 6px; bottom: 6px; width: 3px; border-radius: 2px; background: var(--go); }
.menu .n { width: 14px; color: var(--tx2); font-size: 11px; text-align: end; flex: none; }
.menu .q { width: 46px; font-weight: 600; flex: none; }
.menu .f { color: var(--tx2); flex: 1; overflow: hidden; text-overflow: ellipsis; }
.menu .z { color: var(--tx2); font-variant-numeric: tabular-nums; }
.menu .k { margin-inline-start: auto; padding-inline-start: 12px; color: var(--tx2); font-size: 11px; }
.menu .top .lead { color: var(--accent); font-weight: 600; }
.menu .top .f { color: var(--tx); }
.menu .best { font: 700 10px/13px var(--font); color: var(--accent); border: 1.5px solid var(--accent); border-radius: 3px; padding: 0 3px; margin-inline-start: 4px; }
.menu .mi { width: 16px; display: flex; justify-content: center; color: var(--tx2); flex: none; }
.menu .mi svg { width: 13px; height: 13px; }
.menu .top .mi { color: var(--go); }
.menu .top .mi svg { width: 14px; height: 14px; }
.menu .skrow { height: 24px; margin: 2px 8px; border-radius: 4px; background-size: 200% 100%;
  background-image: linear-gradient(90deg, var(--hov), rgba(127, 127, 127, .12), var(--hov));
  animation: shimmer 1.4s linear infinite; }
@media (prefers-reduced-motion: reduce) {
  .pill { transition: none; transform: none; }
  .spin, .chip.skel, .menu .skrow { animation: none; }
}
`;
  // A constructed sheet: a page whose Content-Security-Policy refuses inline styles can't strip
  // it (Firefox applies the page's policy to what a content script adds).
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    shadow.adoptedStyleSheets = [sheet];
  } catch {
    const style = document.createElement("style");
    style.textContent = CSS;
    shadow.appendChild(style);
  }
  // Its markup, fixed (the words go in as text): the tab, its tip, the drag's guide, its list.
  const ICON = 'fill="none" stroke="currentColor" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"';
  shadow.innerHTML += `
<div class="guide"></div>
<div class="pill" part="pill" data-state="ready">
  <button class="go" type="button">
    <span class="grip" aria-hidden="true"><i></i><i></i><i></i><i></i><i></i><i></i></span>
    <span class="icon">
      <img class="logo" alt="" draggable="false">
      <span class="spin"></span>
      <svg class="i-sent" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="8" fill="var(--ok)"/><path d="M4.6 8.2 7 10.5l4.4-4.6" stroke="#fff" stroke-width="1.7" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
      <svg class="i-warn" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="8" fill="var(--warn-dot)"/><path d="M8 4.2v4.6" stroke="#1b1b1b" stroke-width="1.7" stroke-linecap="round"/><circle cx="8" cy="11.4" r="1" fill="#1b1b1b"/></svg>
      <svg class="i-err" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="8" fill="var(--err)"/><path d="M8 4.2v4.6" stroke="#fff" stroke-width="1.7" stroke-linecap="round"/><circle cx="8" cy="11.4" r="1" fill="#fff"/></svg>
      <svg class="i-off" viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.4" stroke="var(--tx2)" stroke-width="1.5" fill="none"/><path d="M3.6 12.4 12.4 3.6" stroke="var(--tx2)" stroke-width="1.5"/></svg>
      <svg class="i-drm" viewBox="0 0 16 16" aria-hidden="true"><rect x="3.5" y="7" width="9" height="7" rx="1.5" fill="var(--tx2)"/><path d="M5.5 7V5.2a2.5 2.5 0 0 1 5 0V7" stroke="var(--tx2)" stroke-width="1.5" fill="none"/></svg>
    </span>
    <span class="label"></span>
    <span class="sub"></span>
    <span class="chip"></span>
  </button>
  <span class="acts"></span>
  <button class="more" type="button" aria-haspopup="menu" aria-expanded="false">
    <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2 3.5 5 6.5 8 3.5" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg>
  </button>
  <button class="x" type="button">
    <svg viewBox="0 0 10 10" aria-hidden="true"><path d="M2.5 2.5l5 5M7.5 2.5l-5 5" stroke="currentColor" stroke-width="1.3" stroke-linecap="round"/></svg>
  </button>
</div>
<div class="tip" role="tooltip"><span class="what"></span><span class="how"></span></div>
<span class="tpl" hidden><svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M8 2.5v8M4.8 7.4 8 10.6l3.2-3.2M3 13.5h10"/></svg></span>
<div class="menu" role="menu" tabindex="-1">
  <div class="mh"><img alt="" draggable="false"><div><span class="t"></span><span class="s"></span></div></div>
  <div class="hr"></div>
  <div class="note" role="status" hidden></div>
  <div class="rows" role="group"></div>
  <div class="hr"></div>
  <button role="menuitem" data-act="window"><span class="mi"><svg viewBox="0 0 16 16" ${ICON} stroke-width="1.4"><path d="M2.5 4h11M2.5 8h11M2.5 12h7"/></svg></span><span></span></button>
  <div class="hr"></div>
  <button role="menuitem" data-act="hide"><span class="mi"><svg viewBox="0 0 16 16" ${ICON} stroke-width="1.4"><circle cx="8" cy="8" r="5.5"/><path d="M4.2 11.8l7.6-7.6"/></svg></span><span></span><span class="k">Esc</span></button>
  <button role="menuitem" data-act="never"><span class="mi"><svg viewBox="0 0 16 16" ${ICON} stroke-width="1.4"><circle cx="8" cy="8" r="5.5"/><path d="M2.5 8h11M8 2.5c2 2 2 9 0 11M8 2.5c-2 2-2 9 0 11"/></svg></span><span></span></button>
  <button role="menuitem" data-act="settings"><span class="mi"><svg viewBox="0 0 16 16" ${ICON} stroke-width="1.4"><circle cx="8" cy="8" r="2"/><path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4"/></svg></span><span></span></button>
</div>`;
  const $ = (sel) => shadow.querySelector(sel);
  const pill = $(".pill");
  const go = $(".go");
  const label = $(".label");
  const sub = $(".sub");
  const chip = $(".chip");
  const acts = $(".acts");
  const more = $(".more");
  const closeX = $(".x");
  const tip = $(".tip");
  const guide = $(".guide");
  const menu = $(".menu");
  const note = $(".note");
  const rows = $(".rows");
  const item = (act) => menu.querySelector(`[data-act="${act}"]`);
  const dir = ext.i18n.getMessage("@@bidi_dir") || "ltr";
  for (const el of [pill, menu, tip]) el.dir = dir;
  const idleLabel = t("video_download");
  $(".icon .logo").src = LOGO;
  $(".mh img").src = LOGO;
  more.title = t("video_more");
  more.setAttribute("aria-label", t("video_more"));
  closeX.title = t("video_hide_here");
  closeX.setAttribute("aria-label", t("video_hide_here"));
  menu.setAttribute("aria-label", idleLabel);
  item("window").lastElementChild.textContent = t("video_all_qualities");
  item("hide").children[1].textContent = t("video_hide_here");
  item("never").lastElementChild.textContent = ext.i18n.getMessage("video_never_site", [host]) || `Never on ${host}`;
  item("settings").lastElementChild.textContent = t("video_settings");
  $(".tip .how").textContent = t("video_drag_dots");

  // ---- the look-ups (#703): each video's qualities, asked as its button shows ---------------
  // By what the button sends for it (its address and its own file): one look-up each, kept for
  // the page's life (the app keeps its own ten minutes, and shares one between two askings). A
  // video that plays a plain file needs none: the browser says its picture's size, and it goes
  // to MDM as a file.
  const looks = new Map();
  // Shown this long over the same video first: previews the pointer only crosses aren't looked up.
  const LOOK_DELAY_MS = 600;
  // An answer that isn't a look-up's (no MDM, an older one, too many at once) is asked again after
  // this; meanwhile a click goes to MDM's window, as it did before.
  const LOOK_RETRY_MS = 15000;
  let lookTimer = 0;
  // A stream's manifest the page plays as its file (Safari's way, Chrome's own HLS): looked up.
  const STREAM = /\.(m3u8|mpd)(?:[?#]|$)/i;
  // A web server's programs and a page (background.js' SCRIPT_EXTENSIONS and PAGE_EXTENSIONS):
  // a file such an address sends is of another type, which only its server says (#695, #800).
  const SCRIPT_EXTENSIONS = ["php", "php3", "php4", "php5", "phtml", "asp", "aspx", "ashx", "axd", "jsp", "jspx", "do", "action", "cgi", "pl", "cfm", "py", "rb", "htm", "html", "xhtml", "shtml", "dhtml"];
  // A plain file's type as its address says it: its extension, or a script's `file=` value's
  // (stream.php?file=x.mp4: MP4); none for a script's alone (not "PHP": the file isn't one).
  function fileType(src) {
    let u;
    try {
      u = new URL(src);
    } catch {
      return "";
    }
    const ext = ((/\.([a-z0-9]{2,5})$/i.exec(u.pathname) || [])[1] || "").toLowerCase();
    if (!SCRIPT_EXTENSIONS.includes(ext) && ext) return ext;
    for (const [k, v] of u.searchParams) {
      const given = /^(file|filename)$/i.test(k) && /\.(mp4|m4v|webm|mkv|mov|avi|wmv|flv|3gp|ogv|mp3|m4a|aac|ogg|oga|opus|flac|wav|wma)$/i.exec(v.trim());
      if (given) return given[1].toLowerCase();
    }
    return "";
  }
  // What the button sends for a video, and its look-up's key (none when nothing names the video).
  const lookKey = (d) => (d.url === null ? null : `${d.url} ${d.src}`);
  // What download sends for the video now ({v, d, key}), kept a moment (the DOM walk is not free).
  let current = null;
  function describeNow(v) {
    if (!current || current.v !== v || Date.now() - current.t > 1000) {
      const d = describe(v, v.getBoundingClientRect());
      current = { v, d, key: lookKey(d), t: Date.now() };
    }
    return current;
  }
  // The look-up for what the button over `c.v` sends: its plain file's own (none asked), the
  // one asked, or null.
  function lookOf(c) {
    if (!c || c.d.drm) return null;
    if (c.d.src && !STREAM.test(c.d.src)) return { file: true, height: c.v.videoHeight || 0, ext: fileType(c.d.src) };
    return (c.key && looks.get(c.key)) || null;
  }
  // A look-up worth keeping: one under way, or one that found the video (and wasn't picked from:
  // the app forgets a look-up once a quality is taken from it).
  const usable = (l) =>
    Boolean(l) && !l.picked && (l.state === "pending" || l.state === "ready" || Date.now() - l.at < LOOK_RETRY_MS);

  // Starts the look-up of what the button sends for `c` (none for a plain file, a protected video
  // or one nothing names), unless one is kept. `l.done` resolves once it ended.
  function startLook(c) {
    if (!c || !c.key || c.d.drm || (c.d.src && !STREAM.test(c.d.src))) return null;
    const kept = looks.get(c.key);
    if (usable(kept)) return kept;
    const og = location.href === landedAt ? document.querySelector('meta[property="og:title"]')?.content : "";
    const asked = { src: c.d.src, title: (og || document.title || "").trim(), url: c.d.url, at: c.d.at };
    const l = { state: "pending", at: Date.now(), href: location.href, asked };
    looks.set(c.key, l);
    const alive = () => looks.get(c.key) === l && location.href === l.href;
    l.done = (async () => {
      const r = await within(send({ type: "video-lookup", ...asked }), LOOKUP_MS);
      if (r && r.show) l.show = String(r.show);
      if (r && r.post) l.post = String(r.post);
      let s = replyState(r) || { state: "gone" };
      if (s.state === "pending") {
        s =
          (await followLookup(s, {
            ask: (token, ms) => within(send({ type: "video-poll", token }), ms),
            alive,
            sleep: (ms) => new Promise((wake) => setTimeout(wake, ms)),
            now: Date.now,
            end: l.at + LOOKUP_MS,
          })) || { state: "gone" };
      }
      Object.assign(l, s, { at: Date.now() });
      if (alive() && current && current.key === c.key) {
        render();
        if (menuOpen) fillMenu();
      }
      return l;
    })();
    render();
    return l;
  }

  // The look-up starts once the button stayed a moment over the same video.
  // (Asked again for the same video, as each move of the pointer does, the wait goes on.)
  let lookingFor = null;
  function scheduleLook(v) {
    if (lookingFor === v) return;
    clearTimeout(lookTimer);
    lookingFor = v;
    lookTimer = setTimeout(() => {
      lookingFor = null;
      if (video === v && v.isConnected && pill.classList.contains("shown")) startLook(describeNow(v));
    }, LOOK_DELAY_MS);
  }

  // ---- what it shows ------------------------------------------------------------------------
  // `status`: what happened after a click (sending, sent, the user's step, MDM's problem, off),
  // shown instead of the quality until it ends: {state, text, sub, acts: [[name, words]], …}.
  let status = null;
  let statusTimer = 0;
  let fullWidth = 0; // its width with its words (compact or not is decided from it)
  function setStatus(s, ms) {
    status = s;
    clearTimeout(statusTimer);
    if (ms) statusTimer = setTimeout(endStatus, ms);
    render();
    lastMove = Date.now();
  }
  // A status ends after its time, but not while the pointer or the keyboard is on it.
  function endStatus(now) {
    if (!now && status && (root.matches(":hover") || keyboardOn())) {
      statusTimer = setTimeout(endStatus, 1000);
      return;
    }
    clearTimeout(statusTimer);
    status = null;
    render();
  }

  function render() {
    const c = video ? describeNow(video) : null;
    const look = lookOf(c);
    const state = status ? status.state : c && c.d.drm ? "drm" : look && look.state === "pending" ? "looking" : "ready";
    pill.dataset.state = state;
    let text = idleLabel;
    let small = "";
    let buttons = [];
    if (state === "drm") {
      text = t("video_drm");
      small = `· ${t("video_cant_save")}`;
    } else if (status) {
      text = status.text;
      small = status.sub ? `· ${status.sub}` : "";
      buttons = status.acts || [];
    }
    label.textContent = text;
    sub.textContent = small;
    const quality = state === "ready" || state === "looking" ? chipFor(look, t) : null;
    chip.className = `chip${quality && quality.skeleton ? " skel" : ""}${quality && quality.hd ? " hd" : ""}`;
    chip.textContent = quality && quality.text ? quality.text : "";
    acts.replaceChildren(
      ...buttons.map(([name, words]) => {
        const b = document.createElement("button");
        b.type = "button";
        // The one way on that matters most is green ("Retry", "Open post").
        b.className = name === "retry" || name === "post" ? "act fill" : "act";
        b.dataset.act = name;
        b.textContent = words;
        return b;
      }),
    );
    const bare = state !== "ready" && state !== "looking";
    pill.classList.toggle("bare", bare);
    pill.classList.toggle("acting", buttons.length > 0);
    if (bare && menuOpen) closeMenu();
    go.setAttribute("aria-label", [text, small.replace(/^· /, ""), quality && quality.text].filter(Boolean).join(", "));
    $(".tip .what").textContent = bare ? [text, small].filter(Boolean).join(" ") : tipFor(look, t, lang);
    // Its width with its words: compact or not follows from it and the video's.
    const was = pill.classList.contains("compact");
    pill.classList.remove("compact");
    fullWidth = pill.offsetWidth;
    if (was) pill.classList.add("compact");
    if (video) position();
  }

  // ---- which video ----------------------------------------------------------------------------
  const videos = document.getElementsByTagName("video");
  const audios = document.getElementsByTagName("audio");

  // Videos inside open shadow roots (players built as web components), looked for again every
  // few seconds at most.
  let shadowed = [];
  let shadowedAt = 0;
  function shadowVideos() {
    if (Date.now() - shadowedAt < 3000) return shadowed;
    shadowedAt = Date.now();
    const found = [];
    const walk = (root, depth) => {
      if (depth > 4) return;
      const w = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
      for (let n = w.nextNode(), seen = 0; n && seen < 20000; n = w.nextNode(), seen++) {
        if (n.shadowRoot) {
          found.push(...n.shadowRoot.querySelectorAll("video"));
          walk(n.shadowRoot, depth + 1);
        }
      }
    };
    walk(document.documentElement, 0);
    shadowed = found;
    return found;
  }

  function eligible(v) {
    if (!v.isConnected || isDismissed(v)) return false;
    const r = v.getBoundingClientRect();
    const audio = v instanceof HTMLAudioElement;
    if (audio ? !v.controls || r.width < MIN_W || r.height < MIN_AUDIO_H : r.width < MIN_W || r.height < MIN_H) {
      return false;
    }
    // A short muted loop without controls: a moving picture, not something to save.
    if (v.muted && v.loop && !v.controls && !(v.duration > CLIP_S)) return false;
    const s = getComputedStyle(v);
    return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0.05;
  }

  // The topmost eligible video under (x, y): players lay their controls over the video, so the
  // video itself rarely gets the pointer's events.
  function videoAt(x, y) {
    let best = null;
    for (const v of [...videos, ...audios, ...shadowVideos()]) {
      const r = v.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom || !eligible(v)) continue;
      if (!best || r.width * r.height < best.r.width * best.r.height) best = { v, r };
    }
    return best && best.v;
  }

  // ---- placing it -------------------------------------------------------------------------
  // `place` is where it sits in the room the video leaves around it: x and y from 0 to 1, one of
  // them 0 or 1 (on an edge).
  function rects() {
    const r = held ? held.r : (lastRect = video.getBoundingClientRect());
    const w = pill.offsetWidth || 150;
    const h = pill.offsetHeight || 30;
    return { r, w, h, roomX: Math.max(0, r.width - w - 2 * INSET), roomY: Math.max(0, r.height - h - 2 * INSET) };
  }

  // Is a control of the page under this spot (a viewport rectangle)? What counts is something
  // on top of the video that the user clicks and that is much smaller than the video: a button,
  // a small link, a watch-later or menu button on a preview (#421); a link or layer over the
  // whole video isn't one. On the video itself, and below it, is nothing to cover.
  function onControl(b, video, videoRect) {
    const area = (x) => Math.max(0, x.width) * Math.max(0, x.height);
    const small = (el) => area(el.getBoundingClientRect()) < area(videoRect) / 5;
    const points = [
      [(b.left + b.right) / 2, (b.top + b.bottom) / 2],
      [b.left + 4, b.top + 4],
      [b.right - 4, b.top + 4],
      [b.left + 4, b.bottom - 4],
      [b.right - 4, b.bottom - 4],
    ];
    for (const [x, y] of points) {
      for (const el of document.elementsFromPoint(x, y)) {
        if (el === root) continue;
        if (el === video || el.contains(video)) break;
        const control = el.closest("button, [role=button], a[href], input, select, [onclick]");
        if (control && !control.contains(video) && small(control)) return true;
        if (small(el) && getComputedStyle(el).cursor === "pointer") return true;
      }
    }
    return false;
  }

  // Where it goes now: the place the user gave it, else the default corner, or the next corner
  // free of the page's own controls (a thumbnail's watch-later button at the top right, #421).
  // Over the same video it only ever goes on to the next corner, never back to one it left: a
  // hover-to-play preview (YouTube's thumbnails, #560) shows its sound and captions buttons while
  // the pointer is over the video and hides them as the pointer reaches the button, so going back
  // when they hide, then away again when they show, kept the button jumping from under the pointer.
  const CORNERS = [{ x: 0, y: 0 }, { x: 0, y: 1 }, { x: 1, y: 1 }];
  function spot(r, w, h, roomX, roomY) {
    if (chosen) return place;
    // Still while its menu is open: the player's own controls come and go as the pointer moves
    // over the video, and the menu's rows would move away from under it.
    if (menuOpen && auto.v === video && auto.at) return auto.at;
    const now = Date.now();
    const same = auto.v === video;
    if (same && now - auto.t < AUTO_MS) return auto.at;
    const order = [place, ...CORNERS];
    const from = same ? Math.max(0, order.indexOf(auto.at)) : 0;
    let at = same ? auto.at : place;
    for (const c of order.slice(from)) {
      const left = r.left + INSET + c.x * roomX;
      const top = r.top + INSET + c.y * roomY;
      if (!onControl({ left, top, right: left + w, bottom: top + h }, video, r)) {
        at = c;
        break;
      }
    }
    auto = { v: video, at, t: now };
    return at;
  }

  function position() {
    if (!video) return;
    // A small video, or one its words wouldn't fit: the round button and its menu only.
    const vr = held ? held.r : video.getBoundingClientRect();
    pill.classList.toggle("compact", !fitsWords(vr.width, fullWidth, INSET));
    const { r, w, roomX, roomY } = rects();
    // The pointer on it (or its menu open): it stays where it is, whatever the video does
    // meanwhile. A preview that grows while the pointer is over it and shrinks back when the
    // pointer goes (#560) would otherwise take the button from under the pointer. Grown or
    // shrunk itself meanwhile (its grip shown, its words changed), it keeps its end at the
    // video's edge it was put against.
    if (!dragging && pinned && pinned.v === video && (menuOpen || root.matches(":hover"))) {
      if (w !== pinned.w && pinned.end) {
        pinned.left -= w - pinned.w;
        root.style.setProperty("transform", `translate(${Math.round(pinned.left)}px, ${Math.round(pinned.top)}px)`, "important");
      }
      pinned.w = w;
      if (menuOpen) placeMenu(pinned.top, pinned.left, w);
      placeTip(pinned.left, w);
      return;
    }
    // Held for a video gone (#503): where it was, not a corner found anew over what the page
    // shows there now.
    const h = pill.offsetHeight || 30;
    const at = dragging ? dragging.at : held && !chosen ? held.at : spot(r, w, h, roomX, roomY);
    const left = r.left + INSET + at.x * roomX;
    // In the part of the video on screen: one scrolled half out keeps its button in view.
    const visTop = Math.max(r.top, 0) + INSET;
    const visBottom = Math.min(r.bottom, innerHeight) - INSET - h;
    const top = Math.min(Math.max(r.top + INSET + at.y * roomY, visTop), Math.max(visTop, visBottom));
    root.style.setProperty("transform", `translate(${Math.round(left)}px, ${Math.round(top)}px)`, "important");
    pinned = { v: video, left, top, w, end: at.x > 0.5 };
    if (menuOpen) placeMenu(top, left, w);
    placeTip(left, w);
    if (dragging && dragging.moved) drawGuide(r, w, h, roomX, roomY, left, top);
  }

  // Which of its sides is nearer the video's middle: its menu and its tip line up with that one.
  function towardEnd(left, w) {
    const { r } = rects();
    return left + w / 2 > r.left + r.width / 2;
  }

  // The open menu opens below the button, or above it near the bottom of the window.
  function placeMenu(top, left, w) {
    const h = menu.offsetHeight;
    const below = top + 36 + h < innerHeight;
    menu.style.top = below ? "36px" : `${-h - 6}px`;
    menu.style.left = towardEnd(left, w) ? `${w - menu.offsetWidth}px` : "0";
  }

  function placeTip(left, w) {
    if (tip.classList.contains("on")) tip.style.left = towardEnd(left, w) ? `${w - tip.offsetWidth}px` : "0";
  }

  // While dragging: the edge it slides along (through the button's middle, from one end of the
  // video's edge to the other) and the spots it snaps to on it (its ends and its middle), from
  // the video's rectangle `r`, in the button's own coordinates (it sits at `left`, `top`).
  function drawGuide(r, w, h, roomX, roomY, left, top) {
    const edge = edgeOf(dragging.at);
    const across = edge === "top" || edge === "bottom";
    const x0 = r.left + INSET - left;
    const y0 = r.top + INSET - top;
    const at = across ? y0 + h / 2 + (edge === "bottom" ? roomY : 0) : x0 + w / 2 + (edge === "right" ? roomX : 0);
    const len = across ? r.width - 2 * INSET : r.height - 2 * INSET;
    const line = document.createElement("i");
    line.className = `line ${across ? "h" : "v"}`;
    Object.assign(line.style, across
      ? { left: `${x0}px`, top: `${at - 1}px`, width: `${len}px` }
      : { left: `${at - 1}px`, top: `${y0}px`, height: `${len}px` });
    const spots = [0, 0.5, 1].map((f) => {
      const s = document.createElement("i");
      s.className = "snap";
      Object.assign(s.style, across ? { left: `${x0 + f * len}px`, top: `${at}px` } : { left: `${at}px`, top: `${y0 + f * len}px` });
      return s;
    });
    guide.replaceChildren(line, ...spots);
    guide.classList.add("on");
  }

  // The edge nearest a free spot (fractions).
  function edgeOf(at) {
    const d = { left: at.x, right: 1 - at.x, top: at.y, bottom: 1 - at.y };
    return Object.keys(d).reduce((a, b) => (d[a] <= d[b] ? a : b));
  }

  // Snap a free spot (fractions, after a drag) to the nearest edge: into a corner, or the edge's
  // middle, when close to one; else where it was dropped along it.
  function snap(at) {
    const edge = edgeOf(at);
    const c = (n) => (n < 0.08 ? 0 : n > 0.92 ? 1 : Math.abs(n - 0.5) < 0.06 ? 0.5 : n);
    if (edge === "left") return { x: 0, y: c(at.y) };
    if (edge === "right") return { x: 1, y: c(at.y) };
    if (edge === "top") return { x: c(at.x), y: 0 };
    return { x: c(at.x), y: 1 };
  }

  // ---- showing and hiding ---------------------------------------------------------------------
  function attach() {
    // Over the video in fullscreen: inside the fullscreen element (a fullscreen <video> itself
    // can't hold it: then it hides).
    const fs = document.fullscreenElement;
    const parent = fs && fs !== video && fs.contains(video) ? fs : document.documentElement;
    if (root.parentNode !== parent) parent.appendChild(root);
    return !(fs && (fs === video || !fs.contains(video)));
  }

  function show(v) {
    // Cut off from the extension (updated or removed) and still running (Firefox may leave such
    // a copy going in its own world beside the new one): it goes instead of showing (#336).
    if (!ext.runtime?.id) return teardown();
    if (video !== v) {
      video = v;
      held = null;
      closeMenu();
      tipOff();
      // What happened is about the video it was clicked on (off on the site is the site's).
      if (status && status.state !== "off") endStatus(true);
      current = null;
    }
    if (!attach()) return hide(true);
    root.style.setProperty("display", "block", "important");
    const c = describeNow(v);
    if (!usable(looks.get(c.key))) scheduleLook(v);
    if (pill.dataset.key !== String(c.key) || !fullWidth) {
      pill.dataset.key = String(c.key);
      render();
    }
    position();
    if (!pill.classList.contains("shown")) requestAnimationFrame(() => pill.classList.add("shown"));
    if (!raf) raf = requestAnimationFrame(tick);
  }

  // `now`: at once and whatever has the focus (dismissed, the video gone).
  function hide(now) {
    if (!now && (menuOpen || dragging || keyboardOn())) return;
    pinned = null;
    clearTimeout(lookTimer);
    lookingFor = null;
    tipOff();
    if (now) {
      held = null;
      closeMenu();
      shadow.activeElement?.blur();
    }
    pill.classList.remove("shown");
    cancelAnimationFrame(raf);
    raf = 0;
    const was = video;
    setTimeout(() => {
      if (!pill.classList.contains("shown") && video === was) root.style.setProperty("display", "none", "important");
    }, now || reduced ? 0 : 140);
  }

  // What the button sends for `v` (its rectangle `r`): see `download`.
  function describe(v, r) {
    const url = videoPage(v);
    // Its own plain file (not a stream the page assembles, blob:). Not beside a post a big site's
    // reader knows (#689): the app would take the file over the post, its title and qualities.
    const file = /^https?:/i.test(v.currentSrc || "") ? v.currentSrc : "";
    const post = Boolean(url) && url !== location.href && SITES.some((s) => s.hosts.test(siteName(host)));
    return {
      drm: Boolean(v.mediaKeys),
      src: post ? "" : file,
      url,
      // The middle of the video: where the background asks the page's own player what it plays.
      at: { x: r.left + r.width / 2, y: r.top + r.height / 2 },
    };
  }

  // The video just went from under the button (#503): what it was, kept. What was taken as the
  // pointer reached the button, when that was a moment ago; else the video as it is now.
  function hold() {
    const now = Date.now();
    const d = entered && entered.v === video && now - entered.t < 1000 ? entered.d : describe(video, lastRect);
    return { r: lastRect, at: chosen ? place : (auto.v === video && auto.at) || place, d, since: now };
  }

  // While shown: follow the video (scrolling, layout) and fade when the pointer rests (IDLE_MS)
  // or at once when it leaves the video.
  function tick() {
    raf = 0;
    if (!video) return hide(true);
    const onButton = root.matches(":hover");
    if (!video.isConnected || !eligible(video)) {
      if (!held && lastRect && !isDismissed(video)) held = hold();
      const r = held && held.r;
      const over =
        r && pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom;
      if (!held || !(onButton || menuOpen || dragging || keyboardOn() || (over && Date.now() - held.since < HOLD_MS))) {
        const was = held;
        held = null;
        video = null;
        // Left after it was held: it fades, as when the pointer leaves a video.
        return was ? hide() : hide(true);
      }
      position();
      raf = requestAnimationFrame(tick);
      return;
    }
    held = null;
    const r = video.getBoundingClientRect();
    const over =
      pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom;
    if (!onButton && !menuOpen && !dragging && !keyboardOn() && (!over || Date.now() - lastMove > IDLE_MS)) {
      return hide();
    }
    position();
    raf = requestAnimationFrame(tick);
  }

  // At most one look per frame, however fast the pointer moves. Only the pointer really moving
  // counts (#741): a browser repeats the last move when the page changes under a still pointer (a
  // player's controls shown on pause or play), and that brought the button back.
  function onMove(e) {
    if (e.clientX === pointer.x && e.clientY === pointer.y) return;
    pointer = { x: e.clientX, y: e.clientY };
    lastMove = Date.now();
    if (!scan) scan = requestAnimationFrame(look);
  }

  function look() {
    scan = 0;
    if (dragging || never || (allowed === false && answered())) return;
    // Kept on its video while its menu is open (pointerTarget).
    const v = pointerTarget(videoAt(pointer.x, pointer.y), video, menuOpen);
    if (!v) return;
    if (!answered()) return void ask().then(() => allowed && look());
    show(v);
  }

  // One question at a time; while it is out, the button waits for its answer.
  function ask() {
    if (never) {
      allowed = false;
      return Promise.resolve();
    }
    if (answered()) return Promise.resolve();
    asking = asking || send({ type: "panel" }).then(async (r) => {
      allowed = !never && Boolean(r && r.on);
      askedAt = Date.now();
      // Switched off meanwhile: gone now, unless the menu, a drag or its "Turn on" has it in hand.
      if (!allowed && video && !menuOpen && !dragging && !(status && status.state === "off")) {
        video = null;
        hide(true);
      }
      if (allowed && !placed) {
        placed = true;
        try {
          const { positions } = await ext.storage.local.get({ positions: {} });
          if (positions[host]) {
            place = snap(positions[host]);
            chosen = true;
          }
        } catch {
          // Default spot.
        }
      }
    }).finally(() => (asking = null));
    return asking;
  }

  // ---- the hover tip ------------------------------------------------------------------------
  let tipTimer = 0;
  const TIP_MS = 500;
  function tipSoon() {
    clearTimeout(tipTimer);
    tipTimer = setTimeout(() => {
      if (dragging || menuOpen || !root.matches(":hover") || pill.classList.contains("bare")) return;
      tip.classList.add("on");
      if (pinned) placeTip(pinned.left, pill.offsetWidth);
    }, TIP_MS);
  }
  function tipOff() {
    clearTimeout(tipTimer);
    tip.classList.remove("on");
  }
  go.addEventListener("pointerenter", tipSoon);
  pill.addEventListener("pointerleave", tipOff);

  // ---- acting ---------------------------------------------------------------------------------
  // The middle of the video: where the background asks the page's own player what it plays.
  function middle(v) {
    const box = v.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  }

  // A promise's answer, or {late: true} once `ms` is up.
  const within = (promise, ms) => {
    let timer = 0;
    const late = new Promise((done) => (timer = setTimeout(() => done({ late: true }), Math.max(0, ms))));
    return Promise.race([promise, late]).finally(() => clearTimeout(timer));
  };

  // The click: the best quality its look-up found, started by MDM at once (while the look-up
  // runs, as soon as it ends); the video's own plain file as a file. `row`: a quality of its menu
  // instead. `toWindow`: "All qualities and subtitles…", MDM's Download video window, which is
  // also where a click goes when nothing was found (no look-ups here, yt-dlp missing, a list, an
  // error: the window says so and offers what to do).
  async function download(row, toWindow = false) {
    if (busy || !video) return;
    busy = true;
    try {
      closeMenu();
      tipOff();
      // Asked now, not kept: one <video> plays many videos at the same address (a feed's
      // previews). A video gone from under the button: what it played then (#503).
      const d = held ? held.d : describe(video, video.getBoundingClientRect());
      // Encrypted (DRM): no tool can save it; it says so rather than send it.
      if (d.drm) return render();
      const og = location.href === landedAt ? document.querySelector('meta[property="og:title"]')?.content : "";
      const title = (og || document.title || "").trim();
      const j = { video, src: video.currentSrc || "", href: location.href, asked: { src: d.src, title, url: d.url, at: d.at }, row, toWindow };
      const c = { v: video, d, key: lookKey(d) };
      let l = lookOf(c);
      setStatus({ state: "sending", text: t("video_sending") });
      if (toWindow) return await useWindow(j);
      if (l && l.file) return await useWindow(j, { direct: true, words: qualityName(l, t) });
      if (!usable(l)) l = startLook(c);
      if (!l) return await useWindow(j);
      if (l.state === "pending") await within(l.done, LOOKUP_MS);
      const a = lookupAction(l, t);
      if (a.act === "flash") return showFlash(a, l);
      // The post a video in a feed sits in, which can't be looked up from here (#689): opening
      // the post is the user's step ("Open post"; the menu's "All qualities…" still has the window).
      if (a.act !== "offer" && l.post && l.state === "failed") return showFlash({ kind: "warn", text: t("video_open_post") }, l);
      if (a.act !== "offer") return await useWindow(j);
      const choices = l.choices;
      const i = row && Number.isInteger(row.choice) ? row.choice : choices.indexOf(bestOf(l));
      return await pick(j, l, Math.max(0, i));
    } finally {
      busy = false;
    }
  }

  // A quality the look-up offered: MDM downloads it (its own choice of it, with what it looked the
  // video up with) without its window. Only the token and the quality's number go.
  async function pick(j, l, i) {
    if (!l.token || l.picked || !(i >= 0 && i < Math.min(l.choices.length, 100))) return useWindow(j);
    l.picked = true;
    const s = replyState(await send({ type: "video-pick", token: l.token, choice: i }));
    if (!s) return endStatus(true);
    // Downloading, queued, or handed to the app ("sent"): green, with "Show". Anything else (the
    // look-up forgotten, another window's, an older app): the window.
    const a = lookupAction(s, t, true);
    if (a.act === "flash") return showFlash(a, l, qualityName(l.choices[i], t));
    return useWindow(j);
  }

  // What a look-up or a choice came to, on the button: green with "Show", grey "off" with "Turn
  // on", amber for the user's step ("Open post" when the post's page is known).
  function showFlash(a, l, words = "") {
    if (a.kind === "ok") {
      lastShow = (l && l.show) || "";
      return setStatus({ state: "sent", text: a.text, sub: words, acts: lastShow ? [["show", t("video_show")]] : [] }, 6000);
    }
    if (a.kind === "off") return offStatus();
    return setStatus({ state: "warn", text: a.text, acts: l && l.post ? [["post", t("video_open_post_button")]] : [], post: l && l.post }, 7000);
  }

  // Off on this site (its "Never on" chosen, or Options keep it off here): grey, with "Turn on".
  function offStatus() {
    setStatus({ state: "off", text: t("video_off"), acts: [["on", t("video_turn_on")]] }, 6000);
  }

  // The click as it always was: the app's Download video window for this video, once per click;
  // or (`direct`) the video's own plain file, downloaded by MDM as a file at once.
  async function useWindow(j, { direct = false, words = "" } = {}) {
    if (!firstWindow(j)) return;
    // Where the video is now, for the background to ask the page's player there.
    const r = await send({ type: "video", ...j.asked, ...(direct ? { direct: true } : {}), ...(j.video.isConnected ? { at: middle(j.video) } : {}) });
    if (r.gone) return;
    if (r.ok) {
      lastShow = r.show || "";
      return direct
        ? setStatus({ state: "sent", text: t("video_added"), sub: words, acts: lastShow ? [["show", t("video_show")]] : [] }, 6000)
        : setStatus({ state: "sent", text: t("video_sent") }, 2600);
    }
    if (r.why === "options") return offStatus();
    if (r.why === "unnamed" || r.why === "feed") return showFlash({ kind: "warn", text: t(r.why === "unnamed" ? "video_unnamed" : "video_open_post") }, null);
    // MDM isn't there or didn't start: red, with the way to get it and another try.
    setStatus({ state: "err", text: t("video_no_answer"), acts: [["get", t("video_get_mdm")], ["retry", t("video_retry")]], again: { row: j.row, toWindow: j.toWindow } }, 9000);
  }

  // A status's buttons: "Show", "Open post", "Get MDM", "Retry", "Turn on". Only the user's own.
  let lastShow = "";
  acts.addEventListener("click", async (e) => {
    if (!e.isTrusted) return;
    const name = e.target.closest("[data-act]")?.dataset.act;
    const s = status;
    if (!name || !s) return;
    if (name === "show" && lastShow) {
      send({ type: "show-download", address: lastShow });
      endStatus(true);
    } else if (name === "post" && s.post) {
      send({ type: "open-post", url: s.post });
      endStatus(true);
    } else if (name === "get") {
      send({ type: "get-mdm" });
    } else if (name === "retry") {
      endStatus(true);
      download(s.again && s.again.row, Boolean(s.again && s.again.toWindow));
    } else if (name === "on") {
      never = false;
      allowed = null;
      askedAt = 0;
      await send({ type: "show-site" });
      endStatus(true);
    }
  });

  // ---- the menu -------------------------------------------------------------------------------
  // The video's qualities as its look-up found them: "Download best" first, then one numbered
  // line per quality (video, then sound); MDM's whole window; hiding and settings. Filled as it
  // opens and again when the look-up ends.
  function fillMenu() {
    const c = video ? describeNow(video) : null;
    const look = lookOf(c);
    const site = host.replace(/^www\./, "");
    $(".mh .t").textContent = String((look && look.title) || document.title || "").slice(0, 200);
    $(".mh .s").textContent = headerLine("", video && video.duration, site);
    rows.replaceChildren();
    note.hidden = true;
    note.classList.remove("warn");
    note.replaceChildren();
    const best = bestOf(look);
    if (look && look.file) {
      rows.append(topRow(best, { file: true }));
    } else if (look && look.state === "pending") {
      // Looking: the best already takes a click; its lines come when it ends.
      say(t("video_looking"));
      rows.append(topRow(null, {}));
      for (let i = 0; i < 3; i += 1) {
        const s = document.createElement("div");
        s.className = "skrow";
        s.setAttribute("aria-hidden", "true");
        rows.append(s);
      }
    } else if (look && look.state === "ready" && Array.isArray(look.choices) && look.choices.length) {
      const groups = groupChoices(look.choices, 100);
      rows.append(topRow(best, { choice: look.choices.indexOf(best) }));
      let n = 0;
      for (const [kind, words] of [["video", "video_tab_video"], ["audio", "video_tab_audio"]]) {
        if (!groups[kind].length) continue;
        const cap = document.createElement("div");
        cap.className = "cap";
        cap.dataset.kind = kind;
        cap.textContent = t(words);
        rows.append(cap);
        for (const i of groups[kind]) rows.append(row(look.choices[i], i, (n += 1), look.choices[i] === best));
      }
      tell(noticeOf(look));
    } else if (look) {
      // What the look-up came to, in its words; the window does the rest.
      const a = lookupAction(look, t);
      if (a.act === "say" || a.act === "flash") say(a.text, a.detail);
      tell(noticeOf(look));
    }
    if (menuOpen) position();
  }

  // The menu's message (plain text), with the app's own words under it.
  function say(text, detail) {
    note.textContent = text;
    if (detail) {
      const why = document.createElement("span");
      why.className = "why";
      why.textContent = String(detail).slice(0, 400);
      note.append(why);
    }
    note.hidden = !text;
  }

  // The app's word on what the look-up lacked (noticeOf), above the qualities, with its help
  // page's link; under the menu's own message when it has one.
  function tell(n) {
    if (!n) return;
    const words = document.createElement("span");
    words.className = "why";
    words.textContent = n.text;
    if (note.hidden) {
      note.replaceChildren();
      words.className = "";
    }
    note.append(words);
    if (n.link) {
      const a = document.createElement("a");
      a.href = n.url;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      a.textContent = n.link;
      note.append(document.createElement("br"), a);
    }
    note.classList.add("warn");
    note.hidden = false;
  }

  const span = (cls, text) => {
    const s = document.createElement("span");
    s.className = cls;
    s.textContent = text;
    return s;
  };

  // A quality's words on its line: "1080p", "MP4 · 60 fps"; the sound's "MP3", "320 kbps", and
  // the sound as the site sends it "128 kbps · original" (just "original" when its rate isn't said).
  function lineWords(c, t) {
    if (c && c.audio === true) {
      const rate = Number(c.kbps) > 0 ? `${Math.round(c.kbps)} kbps` : /\bVBR\b/.test(String(c.label || "")) ? "VBR" : "";
      return { q: String(c.ext || "").toUpperCase(), f: [rate, c.original === true ? t("video_original") : ""].filter(Boolean).join(" · ") };
    }
    return { q: qualityName(c, t), f: qualityMeta(c, t) };
  }

  // The first line: what one click downloads ("Download best · 1080p MP4 · 60 fps · 412 MB"),
  // chosen as the list opens.
  function topRow(c, { choice, file }) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.className = "top sel";
    if (Number.isInteger(choice) && choice >= 0) b.dataset.choice = String(choice);
    else b.dataset.row = file ? "file" : "best";
    const mi = span("mi", "");
    mi.append(arrowIcon.cloneNode(true));
    const name = c ? [qualityName(c, t), c.height || c.file ? String(c.ext || "").toUpperCase() : ""].filter(Boolean).join(" ") : "";
    const what = [name, c && c.fps ? `${Math.round(c.fps)} fps` : ""].filter(Boolean).join(" · ");
    b.append(mi, span("lead", t("video_download_best")), span("f", what), span("z", c ? choiceSize(c, lang) : ""));
    return b;
  }
  const arrowIcon = $(".tpl svg");

  // A quality's line: its number, its name, its type and frame rate (the best marked), its size;
  // all as text.
  function row(c, choice, n, best) {
    const b = document.createElement("button");
    b.type = "button";
    b.setAttribute("role", "menuitem");
    b.dataset.choice = String(choice);
    const { q, f } = lineWords(c, t);
    const fs = span("f", f);
    if (best) fs.append(span("best", t("video_best")));
    b.append(span("n", String(n)), span("q", q), fs, span("z", rowText(c, lang).size));
    return b;
  }

  // The menu's items the arrows go through.
  const menuItems = () => [...menu.querySelectorAll("button")];
  function focusItem(b) {
    if (!b) return;
    b.focus();
    // The line the arrows are on is the one Enter downloads.
    if (rows.contains(b)) for (const r of rows.querySelectorAll("button")) r.classList.toggle("sel", r === b);
  }

  function openMenu() {
    if (pill.classList.contains("bare")) return;
    tipOff();
    // Its look-up, when none was asked yet (the menu opened at once).
    if (video) {
      const c = describeNow(video);
      if (!usable(looks.get(c.key))) startLook(c);
    }
    menuOpen = true;
    fillMenu();
    menu.classList.add("open");
    more.setAttribute("aria-expanded", "true");
    lastMove = Date.now();
    position();
    // On the chosen line ("Download best"), else the first item.
    focusItem(rows.querySelector("button.sel") || menuItems()[0]);
  }

  function closeMenu() {
    if (!menuOpen) return;
    const back = menu.contains(shadow.activeElement);
    menuOpen = false;
    menu.classList.remove("open");
    more.setAttribute("aria-expanded", "false");
    // The keyboard's focus back on its button, not lost with the menu.
    if (back) more.focus();
  }

  // An item of the menu, clicked or by key.
  function runItem(b) {
    if (!b) return;
    if (b.dataset.choice !== undefined) return download({ choice: Number(b.dataset.choice) });
    if (b.dataset.row !== undefined) return download();
    run(b.dataset.act);
  }

  menu.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    runItem(e.target.closest("button"));
  });

  async function run(act) {
    if (act === "window") return download(null, true);
    closeMenu();
    if (act === "hide") return hideHere();
    if (act === "never") {
      never = true;
      allowed = false;
      send({ type: "hide-site" });
      // Said, with its way back, for a moment; then gone from this site.
      offStatus();
      return;
    }
    if (act === "settings") send({ type: "options" });
  }

  function hideHere() {
    closeMenu();
    held = null;
    if (!video) return;
    dismissed.add(video);
    if (video.currentSrc) dismissedSrc.add(video.currentSrc);
    video = null;
    hide(true);
  }

  more.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    menuOpen ? closeMenu() : openMenu();
  });
  // ✕: hidden on this video.
  closeX.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    hideHere();
  });

  // The pointer reached the button: what the video plays now, kept in case the page stops it for
  // that (a hover-to-play preview, #503).
  root.addEventListener("pointerenter", () => {
    if (video && !held && video.isConnected && eligible(video)) {
      entered = { v: video, d: describe(video, video.getBoundingClientRect()), t: Date.now() };
    }
  });

  // Keys (only the user's own, never ones a page script makes up): in the menu the arrows, Home
  // and End move, Enter and Space pick, Escape closes it; on the button ↓ opens the menu and
  // Escape hides it on this video. Each stops there: a key the button used is not the page's too
  // (a player's Space and arrows would pause it and change its volume behind the menu).
  shadow.addEventListener("keydown", (e) => {
    if (!e.isTrusted) return;
    const focused = shadow.activeElement;
    const inMenu = menuOpen && focused && menu.contains(focused);
    if (e.key === "Escape" && menuOpen) {
      closeMenu();
      more.focus();
      e.preventDefault();
      e.stopPropagation();
    } else if (e.key === "Escape" && focused && pill.contains(focused)) {
      hideHere();
      e.preventDefault();
      e.stopPropagation();
    } else if (inMenu && focused !== menu && (e.key === "Enter" || e.key === " ")) {
      // The focused item, whatever the browser does with a key on a button.
      runItem(focused);
      e.preventDefault();
      e.stopPropagation();
    } else if (menuOpen && ["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) {
      const items = menuItems();
      focusItem(items[moveIn(items.indexOf(focused), items.length, e.key)]);
      e.preventDefault();
      e.stopPropagation();
    } else if (!menuOpen && e.key === "ArrowDown" && (focused === go || focused === more)) {
      openMenu();
      e.preventDefault();
      e.stopPropagation();
    }
  });

  // The main part: a click downloads, a drag moves it (and is not a click).
  // Only the user's own clicks and keys (never events a page script makes up).
  go.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !video || !e.isTrusted) return;
    tipOff();
    const { r, roomX, roomY } = rects();
    dragging = {
      id: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      // Where in the button it was taken.
      dx: e.clientX - (r.left + INSET + (chosen ? place : auto.at || place).x * roomX),
      dy: e.clientY - (r.top + INSET + (chosen ? place : auto.at || place).y * roomY),
      at: chosen ? place : auto.at || place,
      moved: false,
    };
    go.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  go.addEventListener("pointermove", (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    if (!dragging.moved && Math.hypot(e.clientX - dragging.startX, e.clientY - dragging.startY) < DRAG_PX) return;
    if (!dragging.moved) {
      dragging.moved = true;
      closeMenu();
      pill.classList.add("dragging");
    }
    const { r, roomX, roomY } = rects();
    const c = (n) => Math.min(1, Math.max(0, n));
    dragging.at = {
      x: roomX ? c((e.clientX - dragging.dx - r.left - INSET) / roomX) : 0,
      y: roomY ? c((e.clientY - dragging.dy - r.top - INSET) / roomY) : 0,
    };
    position();
  });
  const endDrag = (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    const d = dragging;
    dragging = null;
    pill.classList.remove("dragging");
    guide.classList.remove("on");
    if (!d.moved) {
      // A click: the quality one click gets (a status's own buttons do the rest).
      if (e.type === "pointerup" && !pill.classList.contains("bare")) download();
      return;
    }
    place = snap(d.at);
    chosen = true;
    // It snaps to its spot although the pointer is still on it (#560's pin is for a video
    // that changes under a still pointer, not for the user's own move).
    pinned = null;
    position();
    lastMove = Date.now();
    // Remembered per site, not in a private window (it would keep the site's name), and the
    // 200 sites used last only.
    if (ext.extension?.inIncognitoContext) return;
    try {
      ext.storage.local
        .get({ positions: {} })
        .then(({ positions }) => {
          delete positions[host];
          positions[host] = place;
          const sites = Object.keys(positions);
          for (const s of sites.slice(0, Math.max(0, sites.length - 200))) delete positions[s];
          return ext.storage.local.set({ positions });
        })
        .catch(() => {});
    } catch {
      teardown();
    }
  };
  go.addEventListener("pointerup", endDrag);
  go.addEventListener("pointercancel", endDrag);
  // Keyboard: Enter / Space (a real button's click without a pointer).
  go.addEventListener("click", (e) => {
    if (e.detail === 0 && e.isTrusted && !pill.classList.contains("bare")) download();
  });

  // ---- wiring -----------------------------------------------------------------------------------
  // The extension updated or removed under this page: the button and its listeners go (a fresh
  // copy of this script comes with the new extension).
  function teardown() {
    looks.clear();
    clearTimeout(lookTimer);
    cancelAnimationFrame(raf);
    cancelAnimationFrame(scan);
    removeEventListener("pointermove", onMove, true);
    removeEventListener("keydown", onTab, true);
    root.remove();
    never = true;
    allowed = false;
    video = null;
  }
  globalThis.__mdmVideoButton = { alive: () => Boolean(ext.runtime?.id), teardown };
  addEventListener("pointermove", onMove, { capture: true, passive: true });
  addEventListener("pointerdown", (e) => {
    if (menuOpen && !e.composedPath().includes(root)) closeMenu();
  }, { capture: true, passive: true });
  // A video with the keyboard's focus, or a player around one (its controls), gets the button
  // too, kept while the focus stays there; Tab goes from there to the button. Not the focus a
  // click gives (#741): a click that plays or pauses the video shows nothing.
  addEventListener("focusin", (e) => {
    if (!(e.target instanceof Element) || e.composedPath().includes(root)) return;
    const t = e.target;
    if (!t.matches(":focus-visible")) return;
    const v =
      t instanceof HTMLMediaElement ? t : [...t.querySelectorAll("video, audio")].find(eligible) || null;
    if (!v || !eligible(v)) return;
    focusedPlayer = t;
    ask().then(() => {
      if (!allowed) return;
      lastMove = Date.now();
      show(v);
    });
  }, true);
  // Tab on the focused player moves into the button first; Tab from the button goes back to
  // the player and on from there (its own controls next), as if the button sat at its start.
  let tabOn = 0;
  function onTab(e) {
    if (e.key !== "Tab" || e.shiftKey) return;
    // The Tab that just left the button: on from the player, as the browser goes.
    if (Date.now() - tabOn < 100) {
      tabOn = 0;
      return;
    }
    if (video && playerFocused() && root.style.getPropertyValue("display") === "block") {
      go.focus();
      e.preventDefault();
    }
  }
  addEventListener("keydown", onTab, true);
  go.addEventListener("keydown", (e) => {
    if (e.key === "Tab" && !e.shiftKey && focusedPlayer && focusedPlayer.isConnected) {
      tabOn = Date.now();
      focusedPlayer.focus();
    }
  });
  document.addEventListener("fullscreenchange", () => video && show(video));
  addEventListener("blur", () => hide());
})();
