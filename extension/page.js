// Mister Download Manager's link watcher (a content script, in every page and frame):
//  - a link clicked with Alt held (Option on a Mac) is the browser's to download (background.js
//    leaves its download alone), the key the app's help names for it. The background hears of
//    it as the button goes down, before the browser can start a download (Chromium and Safari
//    download such a link themselves at the click). Firefox does nothing with that click (its
//    Alt+click saves a link only with browser.altClickSave set, off by default), so there the
//    link is followed as a plain click follows it, once the background knows (#332);
//  - Safari only: it has no downloads API, so background.js can't take a download from the
//    browser. The click on a link to a file type Options lists is held here instead and offered
//    to the app (message `link`); if the app doesn't take it, the click goes on as if never held.
(() => {
  // One watcher per page: a copy cut off from the extension (after an update) gives way.
  const prev = globalThis.__mdmPage;
  if (prev && typeof prev === "object" && prev.alive()) return;
  if (prev && typeof prev.stop === "function") prev.stop();
  const ext = globalThis.browser ?? globalThis.chrome;
  const SAFARI = Boolean(ext.runtime.getManifest?.().browser_specific_settings?.safari);
  const FIREFOX = /^moz-extension:/i.test(ext.runtime.getURL?.("") || "");

  const tell = (message) => {
    try {
      return ext.runtime.sendMessage(message).catch(() => null);
    } catch {
      // The extension was updated or removed since this page loaded: nothing to tell.
      return Promise.resolve(null);
    }
  };

  // ---- Safari: the file types to hold links for (Options, as background.js keeps them) --------

  let patterns = [];
  const learn = (rules) => {
    // As background.js typePatterns reads the list: names separated by spaces, `*` any run.
    patterns =
      rules && rules.capture
        ? String(rules.types || "")
            .split(/\s+/)
            .filter(Boolean)
            .map((t) => new RegExp("^" + t.replace(/[.+?^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*") + "$", "i"))
        : [];
  };
  const onStorage = (changes, area) => {
    if (area === "local" && changes.rules) learn(changes.rules.newValue);
  };
  if (SAFARI) {
    ext.storage.local.get({ rules: null }).then((v) => learn(v.rules), () => {});
    ext.storage.onChanged.addListener(onStorage);
    // Safari's background script sees no downloads, so nothing else would ask the app for
    // Options' rules once they are old: changed types, or capture turned back on, would never
    // reach the stored copy this reads. Asked here, it refreshes them when stale (onStorage).
    tell({ type: "rules" });
  }

  // The file type a link leads to: its `download` name's, else its path's last name's.
  const typeOf = (a) => {
    let name = a.getAttribute("download") || "";
    if (!name) {
      try {
        name = new URL(a.href).pathname;
      } catch {
        return "";
      }
    }
    const base = name.split("/").pop() || "";
    const dot = base.lastIndexOf(".");
    return dot > 0 ? base.slice(dot + 1) : "";
  };

  const hold = (e, a) => {
    e.preventDefault();
    // The `download` name, when the link has one: the app's rules decide by it, as page.js did.
    const name = a.getAttribute("download") || "";
    tell({ type: "link", url: a.href, ...(name ? { name } : {}) }).then((reply) => {
      if (reply && reply.taken) return;
      // Not the app's (Options, an excluded site, MDM not running): the click as it was, its
      // Cmd/Shift/Ctrl too (a new tab or window stays one). This one isn't trusted, so `note`
      // leaves it alone.
      const keys = { ctrlKey: e.ctrlKey, metaKey: e.metaKey, shiftKey: e.shiftKey };
      try {
        a.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, ...keys }));
      } catch {
        a.click();
      }
    });
  };

  // The web link a person's click (or press) is on, else null.
  const linkOf = (e) => {
    if (!e.isTrusted || !(e.target instanceof Element)) return null;
    const a = e.target.closest("a[href]");
    return a && /^https?:/i.test(a.href) ? a : null;
  };

  // Alt held as the button goes down on a link: said before the click starts any download.
  const press = (e) => {
    if (!e.altKey || e.button > 1) return;
    const a = linkOf(e);
    if (a) tell({ type: "bypass", url: a.href });
  };

  // Firefox: an Alt+click on a link the page left alone (no script of its own took it) is
  // followed as a plain click once the background has it; the browser's own handling of it
  // (nothing, or a save with browser.altClickSave) is stopped, so it happens once.
  const follow = (e) => {
    if (!FIREFOX || !e.altKey || e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey) return;
    if (e.defaultPrevented) return;
    const a = linkOf(e);
    if (!a) return;
    e.preventDefault();
    // Not trusted: `note` and `follow` leave this click alone.
    tell({ type: "bypass", url: a.href }).then(() => a.click());
  };

  const note = (e) => {
    const a = linkOf(e);
    if (!a) return;
    if (e.altKey) {
      tell({ type: "bypass", url: a.href });
      return;
    }
    // Safari: a plain click (or Cmd/Shift: a link opened elsewhere is held the same).
    if (SAFARI && e.type === "click" && e.button === 0 && patterns.length) {
      const type = typeOf(a);
      if (type && patterns.some((p) => p.test(type))) hold(e, a);
    }
  };
  addEventListener("pointerdown", press, true);
  addEventListener("click", note, true);
  addEventListener("auxclick", note, true);
  // After the page's own listeners on the link and its document: a click bubbles to the window
  // last.
  addEventListener("click", follow, false);
  globalThis.__mdmPage = {
    alive: () => Boolean(ext.runtime?.id),
    stop: () => {
      removeEventListener("pointerdown", press, true);
      removeEventListener("click", note, true);
      removeEventListener("auxclick", note, true);
      removeEventListener("click", follow, false);
      if (SAFARI) ext.storage.onChanged.removeListener(onStorage);
    },
  };
})();
