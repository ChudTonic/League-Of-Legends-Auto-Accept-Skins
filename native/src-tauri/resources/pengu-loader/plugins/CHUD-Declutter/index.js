/**
 * @name CHUD-Declutter
 * @author Chud Team
 * @description Hide League client clutter and notification nags — promos, Store/Loot tabs, battle-pass, missions, event banners, RP top-up, and attention-nag pips. Controlled from the Chud app.
 * @link https://github.com/ChudTonic/League-Of-Legends-Auto-Accept-Range
 */
(function chudDeclutter() {
  let bridgePort = 50000;

  // ---- optional bridge logging (boot + on-change only; visible in Chud log) ----
  let ws = null;
  const wsQueue = [];
  function wsFlush() { if (ws && ws.readyState === 1) while (wsQueue.length) ws.send(JSON.stringify(wsQueue.shift())); }
  function report(event, data) { wsQueue.push({ type: "chroma-log", source: "CHUD-Declutter", event, data, timestamp: Date.now() }); wsFlush(); }
  function wsConnect() {
    try {
      ws = new WebSocket(`ws://127.0.0.1:${bridgePort}`);
      ws.onopen = () => { report("online", {}); wsFlush(); };
      ws.onclose = () => setTimeout(wsConnect, 3000);
      ws.onerror = () => {};
    } catch (e) { setTimeout(wsConnect, 3000); }
  }

  // Selectors captured from the live League client DOM. Every entry hides a
  // real, verified element (or a contextual one that appears in some lobbies).
  const RULES = {
    hide_store:         [".main-navigation-menu-item.menu_item_navbar_store"],
    hide_loot:          [".main-navigation-menu-item.menu_item_navbar_loot"],
    hide_missions:      [".progression-or-mission-wrapper", ".mission-button-component"],
    hide_pass:          [".pass-progression-widget-wrapper", ".pass-progression-widget"],
    hide_promos:        [".deep-links-promo", ".deep-links-promo-element", ".discord-banner"],
    hide_rp_topup:      [".currency-rp-top-up", ".currency-rp-top-up-enabled"],
    hide_challenges:    [".v2-banner-component", ".lobby-banner", ".challenge-banner-container", ".challenge-banner-token-container-component"],
    hide_event_timers:  [".parties-game-select-event-countdown-component"],
    hide_home_video:    [".play-button-video", ".play-button-hover-magic"],
    // Notification DND — attention-nag pips/badges only (never functional UI).
    hide_notif_badges:  [".activity-center__tab_label_notification_pip", ".call-to-action-pip", ".pip-cta-loop", ".clash-pip-counter", ".clash-section-header-pip-counter", ".loyalty-navbar-badge", ".navigation-badge-container"],
  };
  const MARK = "data-chud-hidden";

  let cfg = null;
  let lastReport = null;

  function activeSelectors() {
    if (!cfg || !cfg.enabled) return [];
    const out = [];
    for (const key of Object.keys(RULES)) if (cfg[key]) out.push(...RULES[key]);
    return out;
  }

  // Set inline `display:none !important` on each match — an element's own inline
  // !important sits at the top of the cascade, so the client's own !important
  // display rules (which defeat a plain injected <style>) cannot override it.
  function apply() {
    // Un-hide anything no longer targeted (handles toggling an item OFF).
    const sels = activeSelectors();
    const active = new Set();
    const counts = {};
    const computed = {};
    for (const key of Object.keys(RULES)) {
      if (!cfg || !cfg.enabled || !cfg[key]) continue;
      let n = 0, sample = null;
      for (const sel of RULES[key]) {
        let els; try { els = document.querySelectorAll(sel); } catch (e) { continue; }
        els.forEach((el) => {
          active.add(el);
          if (el.style.display !== "none") el.style.setProperty("display", "none", "important");
          el.setAttribute(MARK, "1");
          n++;
          if (!sample) { try { sample = getComputedStyle(el).display; } catch (e) {} }
        });
      }
      counts[key] = n;
      if (sample) computed[key] = sample;
    }
    document.querySelectorAll(`[${MARK}]`).forEach((el) => {
      if (!active.has(el)) { el.style.removeProperty("display"); el.removeAttribute(MARK); }
    });
    const rk = JSON.stringify({ e: !!(cfg && cfg.enabled), counts });
    if (rk !== lastReport) { lastReport = rk; report("hiding", { enabled: !!(cfg && cfg.enabled), counts, computed }); }
  }

  // Debounced re-apply so a burst of client re-render mutations coalesces.
  let pending = null;
  function schedule() { if (pending) return; pending = setTimeout(() => { pending = null; apply(); }, 120); }

  async function fetchConfig() {
    try { const r = await fetch(`http://127.0.0.1:${bridgePort}/client-customization`, { cache: "no-store" }); if (r.ok) return await r.json(); }
    catch (e) {}
    return null;
  }
  async function discoverPort() {
    for (let p = 50000; p <= 50010; p++) {
      try { const r = await fetch(`http://127.0.0.1:${p}/bridge-port`, { cache: "no-store" }); if (r.ok) { bridgePort = parseInt((await r.text()).trim(), 10) || p; return; } }
      catch (e) {}
    }
  }

  (async function start() {
    await discoverPort();
    wsConnect();
    report("boot", { bridgePort });
    cfg = (await fetchConfig()) || cfg;
    apply();

    // Instant, flicker-free hiding: re-hide the moment the client (re)renders
    // any targeted element. Cheap — a handful of querySelectorAll per burst.
    try { new MutationObserver(schedule).observe(document.documentElement, { childList: true, subtree: true }); } catch (e) {}

    // Poll config every 3s so Chud-app toggle changes apply without a reload;
    // a 2s sweep backstops any mutation the observer coalesced away.
    setInterval(async () => { const c = await fetchConfig(); if (c) { cfg = c; apply(); } }, 3000);
    setInterval(apply, 2000);
  })();
})();
