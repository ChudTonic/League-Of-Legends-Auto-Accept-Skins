# chud-skins — skin catalog cache Worker

Caches an upstream community skin-mod catalog so Chud clients query **our** cache
(Cloudflare KV + R2), never the upstream source directly. Used **with the
upstream maintainers' permission**, on their conditions:

- **Do not spike their usage / DDoS them** — hence the gentle daily crawl and full self-hosting of images and files (clients only ever hit our R2).
- **Do not link the app to their website.**
- **Credit the mod author** (each skin carries its `publisher`).

The upstream host is base64-encoded in the source at their request; it's decoded
at runtime with `atob` (behavior is unchanged).

## Endpoints
- `GET /catalog?search=&champion=&category=&page=&pageSize=` → filtered, paginated (mods carry `id,name,champions,category,thumb,publisher,downloads,likes`).
- `GET /img/{thumbnailKey}` → self-hosted mirrored image (Cloudflare cache + R2).
- `GET /download/{modId}` → returns our `/file/{modId}` URL (clients download from us, never upstream).
- `GET /file/{modId}` → serves the `.fantome` from R2, or fetches-through+mirrors from the upstream source on first request (upstream is hit at most once per skin, ever).
- `GET /meta` → `{count, crawledAt, crawlProgress}`.
- `GET /crawl?key=CRAWL_KEY[&full=1]` → manual spurt / full seed (guarded).

## Gentle-load design
- Cron `*/15 * * * *`: each run grabs `CHUNK_PAGES` (3) pages, advancing a cursor; idles once the day's catalog is assembled. The upstream sees ~3 page fetches / 15 min, never a burst.
- Images + files mirror on first request (once each, ever), then serve from our R2 (zero egress fees).

## Resilience
Everything downloaded is mirrored into our R2, so if upstream access is ever cut,
mirrored skins still work — and the upstream's bandwidth is never hit twice for
the same file. Bind R2 buckets `IMAGES` + `FILES` (see `wrangler.toml`).

## Deploy
```
npx wrangler kv namespace create CATALOG   # once; put id in wrangler.toml
npx wrangler r2 bucket create chud-skins-images
npx wrangler r2 bucket create chud-skins-files
npx wrangler secret put CRAWL_KEY          # guards /crawl
npx wrangler deploy
curl "https://chud-skins.<sub>.workers.dev/crawl?key=SECRET&full=1"  # initial seed
```

Deployed: `https://chud-skins.jivy26.workers.dev`
