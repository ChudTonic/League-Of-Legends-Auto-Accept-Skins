/**
 * Chud skins catalog Worker — caches the upstream-source.dev mod catalog + images so
 * Chud clients query OUR cache, never upstream-source directly (their explicit ask:
 * don't hammer their site as our userbase grows).
 *
 * Used with upstream-source.dev's permission (the maintainers, 2026-07-12). Chud MUST
 * credit upstream-source.dev in the app.
 *
 * Gentle-load design:
 *  - Catalog is crawled in SMALL SPURTS: a cron fires every 15 min and each run
 *    grabs only CHUNK_PAGES pages, advancing a cursor. Once the day's full
 *    catalog is assembled the cron idles until the next day. So upstream-source sees a
 *    slow trickle (~3 page fetches / 15 min), never a 33-page burst.
 *  - Images are self-hosted: the catalog points thumbnails at THIS worker's
 *    /img/{key}, which mirrors each image from upstream-source exactly once (into the
 *    Cloudflare cache, and R2 if bound), then serves from us forever.
 *  - Download URLs are resolved on first download and cached (7 days).
 *
 * Endpoints:
 *   GET /catalog?search=&champion=&category=&page=&pageSize=  -> cached, filtered, paginated
 *   GET /img/{thumbnailKey}                                   -> self-hosted mirrored image
 *   GET /download/{modId}[?redirect=1]                        -> resolves+caches the .fantome URL
 *   GET /meta                                                 -> {count, crawledAt, crawlProgress}
 *   GET /crawl?key=SECRET[&full=1]                            -> manual spurt (or full seed)
 */

const RF = "https://upstream-source.dev";
const IMG = "https://r2-images-prod.upstream-source.dev";
const UA = "Chud-Desktop/1.0 (+https://github.com/ChudTonic; upstream-source partner; catalog cache)";
const PAGE_SIZE = 100;   // upstream-source /api/mods page size
const CHUNK_PAGES = 3;   // pages fetched per cron spurt (gentle)

function cors(resp) {
  const h = new Headers(resp.headers);
  h.set("Access-Control-Allow-Origin", "*");
  h.set("Access-Control-Allow-Methods", "GET, OPTIONS");
  h.set("Access-Control-Allow-Headers", "*");
  return new Response(resp.body, { status: resp.status, headers: h });
}
function json(obj, status = 200) {
  // no-store: these API responses are dynamic; the Cloudflare edge otherwise
  // caches GET responses and serves stale catalog/meta.
  return cors(new Response(JSON.stringify(obj), { status, headers: { "Content-Type": "application/json", "Cache-Control": "no-store" } }));
}

function normalize(m) {
  return {
    id: m.id,
    name: m.name,
    champions: (m.champions || []).map((c) => ({ id: c.id, name: c.name })),
    category: m.category || null,
    themes: (m.themes || []).map((t) => (t && t.name) || t).filter(Boolean),
    thumbKey: m.thumbnailKey || null,
    publisher: (m.publisher && m.publisher.username) || null,
    downloads: m.downloadCount || 0,
    likes: m.likeCount || 0,
    status: m.status || null,
    updatedAt: m.updatedAt || null,
  };
}

async function fetchPage(page) {
  // NOTE: do NOT pass categories/champions/etc as empty arrays — upstream-source's
  // validator rejects `categories=[]` as "expected array, received string" and
  // 400s the whole request. Omitting them returns the full unfiltered catalog.
  const u = `${RF}/api/mods?page=${page}&pageSize=${PAGE_SIZE}&sortBy=recently_updated`;
  const r = await fetch(u, { headers: { "User-Agent": UA } });
  if (!r.ok) return null;
  const d = await r.json();
  return (d && d.mods) || [];
}

function today() {
  return new Date().toISOString().slice(0, 10);
}

// One gentle spurt: fetch CHUNK_PAGES pages, store each as pg:{n}, advance the
// cursor; when the last page is reached, assemble the full catalog and idle.
async function crawlSpurt(env) {
  let state;
  try { state = JSON.parse((await env.CATALOG.get("crawl:state")) || "{}"); } catch (e) { state = {}; }
  const day = today();
  if (state.day !== day) state = { day, nextPage: 0, done: false, lastPage: null };
  if (state.done) return { idle: true, day };

  let reachedEnd = false;
  for (let i = 0; i < CHUNK_PAGES; i++) {
    const page = state.nextPage + i;
    const mods = await fetchPage(page);
    if (mods === null) { reachedEnd = true; state.lastPage = page - 1; break; }
    await env.CATALOG.put(`pg:${page}`, JSON.stringify(mods.map(normalize)));
    if (mods.length < PAGE_SIZE) { reachedEnd = true; state.lastPage = page; break; }
  }
  if (!reachedEnd) {
    state.nextPage += CHUNK_PAGES;
    await env.CATALOG.put("crawl:state", JSON.stringify(state));
    return { spurt: true, nextPage: state.nextPage, day };
  }

  // Assemble pg:0..lastPage into the live catalog.
  const all = [];
  for (let p = 0; p <= state.lastPage; p++) {
    const raw = await env.CATALOG.get(`pg:${p}`);
    if (raw) all.push(...JSON.parse(raw));
  }
  await env.CATALOG.put("catalog:v1", JSON.stringify(all));
  await env.CATALOG.put("meta:v1", JSON.stringify({ count: all.length, crawledAt: new Date().toISOString() }));
  state.done = true;
  await env.CATALOG.put("crawl:state", JSON.stringify(state));
  return { assembled: all.length, day };
}

// Full one-shot crawl (initial seed only — triggered manually, not by users).
async function crawlFull(env) {
  const all = [];
  for (let page = 0; page < 80; page++) {
    const mods = await fetchPage(page);
    if (mods === null) break;
    all.push(...mods.map(normalize));
    if (mods.length < PAGE_SIZE) break;
  }
  if (all.length) {
    await env.CATALOG.put("catalog:v1", JSON.stringify(all));
    await env.CATALOG.put("meta:v1", JSON.stringify({ count: all.length, crawledAt: new Date().toISOString() }));
    await env.CATALOG.put("crawl:state", JSON.stringify({ day: today(), nextPage: 0, done: true, lastPage: null }));
  }
  return all.length;
}

async function getCatalog(env) {
  const raw = await env.CATALOG.get("catalog:v1");
  return raw ? JSON.parse(raw) : [];
}

async function resolveDownload(env, modId) {
  const cacheKey = `dl:${modId}`;
  const cached = await env.CATALOG.get(cacheKey);
  if (cached) return cached;
  const u = `${RF}/mods/${encodeURIComponent(modId)}/releases.data?_routes=routes%2Fmods%2F%24modId%2Flayout%2Croutes%2Fmods%2F%24modId%2Freleases%2Findex`;
  let r;
  try { r = await fetch(u, { headers: { "User-Agent": UA } }); } catch (e) { return null; }
  if (!r.ok) return null;
  const text = await r.text();
  const m = text.match(/https:\/\/r2-prod\.upstream-source\.dev\/mod_release_artifacts[^"\\]*?\.fantome[^"\\]*/);
  if (!m) return null;
  await env.CATALOG.put(cacheKey, m[0], { expirationTtl: 604800 });
  return m[0];
}

// Self-hosted image: serve from Cloudflare cache -> R2 (if bound) -> upstream-source
// (mirrored once). Upstream-source's image CDN is hit at most once per image, ever.
async function serveImage(req, env, ctx, key) {
  const cache = caches.default;
  const cacheKey = new Request(new URL(req.url).toString());
  let hit = await cache.match(cacheKey);
  if (hit) return hit;

  if (env.IMAGES) {
    const obj = await env.IMAGES.get(key);
    if (obj) {
      const resp = cors(new Response(obj.body, { headers: { "Content-Type": (obj.httpMetadata && obj.httpMetadata.contentType) || "image/png", "Cache-Control": "public, max-age=2592000" } }));
      ctx.waitUntil(cache.put(cacheKey, resp.clone()));
      return resp;
    }
  }

  let r;
  try { r = await fetch(`${IMG}/${key}`, { headers: { "User-Agent": UA } }); } catch (e) { return cors(new Response("img fetch failed", { status: 502 })); }
  if (!r.ok) return cors(new Response("not found", { status: 404 }));
  const buf = await r.arrayBuffer();
  const ct = r.headers.get("Content-Type") || "image/png";
  if (env.IMAGES) ctx.waitUntil(env.IMAGES.put(key, buf.slice(0), { httpMetadata: { contentType: ct } }));
  const resp = cors(new Response(buf, { headers: { "Content-Type": ct, "Cache-Control": "public, max-age=2592000" } }));
  ctx.waitUntil(cache.put(cacheKey, resp.clone()));
  return resp;
}

export default {
  async scheduled(event, env, ctx) {
    ctx.waitUntil(crawlSpurt(env));
  },

  async fetch(req, env, ctx) {
    if (req.method === "OPTIONS") return cors(new Response(null, { status: 204 }));
    const url = new URL(req.url);
    const origin = url.origin;
    const path = url.pathname.replace(/\/+$/, "") || "/";

    if (path === "/debug") {
      if (!env.CRAWL_KEY || url.searchParams.get("key") !== env.CRAWL_KEY) return json({ error: "forbidden" }, 403);
      const u = `${RF}/api/mods?page=0&pageSize=${PAGE_SIZE}&sortBy=recently_updated`;
      try {
        const r = await fetch(u, { headers: { "User-Agent": UA } });
        const body = await r.text();
        let fp = null, fperr = null;
        try { fp = (await fetchPage(0)); } catch (e) { fperr = String(e); }
        return json({ url: u, status: r.status, ct: r.headers.get("content-type"), bodyLen: body.length, bodyHead: body.slice(0, 200), fetchPageLen: fp ? fp.length : fp, fetchPageErr: fperr });
      } catch (e) {
        return json({ error: String(e) });
      }
    }

    if (path.startsWith("/mirror/")) {
      if (!env.CRAWL_KEY || url.searchParams.get("key") !== env.CRAWL_KEY) return json({ error: "forbidden" }, 403);
      const modId = decodeURIComponent(path.slice("/mirror/".length));
      const asset = await resolveDownload(env, modId);
      if (!asset) return json({ step: "resolve", error: "no asset" });
      let fr;
      try { fr = await fetch(asset, { headers: { "User-Agent": UA } }); } catch (e) { return json({ step: "fetch", error: String(e), asset }); }
      if (!fr.ok) return json({ step: "fetch", status: fr.status, asset });
      const buf = await fr.arrayBuffer();
      if (!env.FILES) return json({ step: "r2", error: "no FILES binding", bytes: buf.byteLength });
      try { await env.FILES.put(`f/${modId}.fantome`, buf, { httpMetadata: { contentType: "application/zip" } }); } catch (e) { return json({ step: "put", error: String(e), bytes: buf.byteLength }); }
      const head = await env.FILES.head(`f/${modId}.fantome`);
      return json({ ok: true, bytes: buf.byteLength, r2size: head ? head.size : null });
    }

    if (path === "/crawl") {
      if (!env.CRAWL_KEY || url.searchParams.get("key") !== env.CRAWL_KEY) return json({ error: "forbidden" }, 403);
      if (url.searchParams.get("full") === "1") return json({ full: await crawlFull(env) });
      return json(await crawlSpurt(env));
    }

    if (path === "/catalog") {
      const all = await getCatalog(env);
      const q = (url.searchParams.get("search") || "").toLowerCase().trim();
      const champ = url.searchParams.get("champion");
      const cat = url.searchParams.get("category");
      const page = Math.max(0, parseInt(url.searchParams.get("page") || "0", 10) || 0);
      const size = Math.min(60, Math.max(1, parseInt(url.searchParams.get("pageSize") || "48", 10) || 48));
      let items = all;
      if (q) items = items.filter((m) => m.name.toLowerCase().includes(q) || m.champions.some((c) => (c.name || "").toLowerCase().includes(q)) || (m.publisher || "").toLowerCase().includes(q));
      if (champ) items = items.filter((m) => m.champions.some((c) => String(c.id) === champ || (c.name || "").toLowerCase() === champ.toLowerCase()));
      if (cat) items = items.filter((m) => m.category === cat);
      const total = items.length;
      const mods = items.slice(page * size, page * size + size).map((m) => ({
        ...m,
        thumb: m.thumbKey ? `${origin}/img/${m.thumbKey}` : null,
      }));
      return json({ total, page, pageSize: size, mods });
    }

    if (path.startsWith("/img/")) {
      const key = decodeURIComponent(path.slice("/img/".length));
      if (!key) return cors(new Response("no key", { status: 400 }));
      return serveImage(req, env, ctx, key);
    }

    if (path.startsWith("/download/")) {
      const modId = decodeURIComponent(path.slice("/download/".length));
      if (!modId) return json({ error: "no mod id" }, 400);
      const asset = await resolveDownload(env, modId);
      if (!asset) return json({ error: "could not resolve download" }, 404);

      // Resilience mirror: if an R2 files bucket is bound, serve downloads from
      // OUR copy so we survive upstream-source blocking us AND never spike their
      // bandwidth. Mirror-on-first-download in the background; the first user
      // still gets the upstream-source URL immediately, everyone after gets our R2.
      if (env.FILES) {
        const fkey = `f/${modId}.fantome`;
        const head = await env.FILES.head(fkey);
        if (head) {
          if (url.searchParams.get("redirect") === "1") return cors(Response.redirect(`${origin}/file/${modId}`, 302));
          return json({ url: `${origin}/file/${modId}`, mirrored: true });
        }
        ctx.waitUntil((async () => {
          try {
            const fr = await fetch(asset, { headers: { "User-Agent": UA } });
            if (fr.ok) {
              const buf = await fr.arrayBuffer(); // buffer, not stream — the streamed body was being dropped post-response
              await env.FILES.put(fkey, buf, { httpMetadata: { contentType: "application/zip" } });
            }
          } catch (e) {}
        })());
        // first hit → upstream-source; next hits → our R2 (mirror completes async)
      }

      if (url.searchParams.get("redirect") === "1") return cors(Response.redirect(asset, 302));
      return json({ url: asset, mirrored: false });
    }

    if (path.startsWith("/file/")) {
      // Stream a mirrored .fantome from our R2.
      if (!env.FILES) return cors(new Response("no mirror", { status: 404 }));
      const modId = decodeURIComponent(path.slice("/file/".length)).replace(/\.fantome$/, "");
      const obj = await env.FILES.get(`f/${modId}.fantome`);
      if (!obj) return cors(new Response("not mirrored", { status: 404 }));
      return cors(new Response(obj.body, {
        headers: {
          "Content-Type": "application/zip",
          "Content-Disposition": `attachment; filename="${modId}.fantome"`,
          "Cache-Control": "public, max-age=2592000",
        },
      }));
    }

    if (path === "/meta") {
      const meta = await env.CATALOG.get("meta:v1");
      let progress = null;
      try { progress = JSON.parse((await env.CATALOG.get("crawl:state")) || "null"); } catch (e) {}
      const base = meta ? JSON.parse(meta) : { count: 0 };
      return json({ ...base, crawlProgress: progress });
    }

    return json({ service: "chud-skins catalog (upstream-source.dev, used with permission — credit upstream-source.dev)", endpoints: ["/catalog", "/img/{key}", "/download/{modId}", "/meta"] });
  },
};
