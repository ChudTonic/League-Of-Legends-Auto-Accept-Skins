/**
 * @name CHUD-Declutter
 * @author Chud Team
 * @description Hide League client clutter — promos, Loot/Store tabs, battle-pass, missions, RP top-up, event banners. Controlled from the Chud app.
 * @link https://github.com/ChudTonic/League-Of-Legends-Auto-Accept-Range
 */
(function chudDeclutter() {
  const LOG = "[CHUD-Declutter]";
  const STYLE_ID = "chud-declutter-style";
  let bridgePort = 50000;

  // Selectors captured from the live League client DOM. Each toggle maps to the
  // real element classes the current client uses (see CHUD-Inspector findings).
  const RULES = {
    hide_store:        [".main-navigation-menu-item.menu_item_navbar_store"],
    hide_loot:         [".main-navigation-menu-item.menu_item_navbar_loot"],
    hide_missions:     [".progression-or-mission-wrapper", ".mission-button-component"],
    hide_pass:         [".pass-progression-widget-wrapper", ".pass-progression-widget"],
    hide_promos:       [".deep-links-promo", ".deep-links-promo-element", ".discord-banner"],
    hide_rp_topup:     [".currency-rp-top-up", ".currency-rp-top-up-enabled"],
    hide_challenges:   [".v2-banner-component", ".lobby-banner", ".challenge-banner-container", ".challenge-banner-token-container-component"],
    hide_event_timers: [".parties-game-select-event-countdown-component"],
    hide_home_video:   [".play-button-video", ".play-button-hover-magic"],
  };

  function styleEl() {
    let el = document.getElementById(STYLE_ID);
    if (!el) {
      el = document.createElement("style");
      el.id = STYLE_ID;
      document.head.appendChild(el);
    }
    return el;
  }

  function buildCss(cfg) {
    if (!cfg || !cfg.enabled) return "";
    const sels = [];
    for (const key of Object.keys(RULES)) {
      if (cfg[key]) sels.push(...RULES[key]);
    }
    if (!sels.length) return "";
    // One combined rule; !important to beat the client's own styles.
    return `${sels.join(",\n")} { display: none !important; }`;
  }

  let lastCss = null;
  function apply(cfg) {
    const css = buildCss(cfg);
    if (css === lastCss) return;
    lastCss = css;
    styleEl().textContent = css;
    console.log(`${LOG} applied ${css ? css.split(",").length + " hidden group(s)" : "nothing (disabled)"}`);
  }

  async function fetchConfig() {
    try {
      const r = await fetch(`http://127.0.0.1:${bridgePort}/client-customization`, { cache: "no-store" });
      if (r.ok) return await r.json();
    } catch (e) { /* bridge not up yet */ }
    return null;
  }

  // Discover the bridge port once (default 50000, else scan the small range).
  async function discoverPort() {
    for (let p = 50000; p <= 50010; p++) {
      try {
        const r = await fetch(`http://127.0.0.1:${p}/bridge-port`, { cache: "no-store" });
        if (r.ok) { bridgePort = parseInt((await r.text()).trim(), 10) || p; return; }
      } catch (e) { /* try next */ }
    }
  }

  async function tick() {
    const cfg = await fetchConfig();
    if (cfg) apply(cfg);
  }

  (async function start() {
    await discoverPort();
    await tick();
    // Poll so changes made in the Chud app apply within a few seconds without a
    // client reload. Cheap: one tiny loopback GET.
    setInterval(tick, 3000);
    console.log(`${LOG} online (bridge :${bridgePort})`);
  })();
})();
