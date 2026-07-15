<p align="center">
  <img src="native/ui/icons/app.png" width="96" alt="Chud" />
</p>

<h1 align="center">Chud</h1>

<p align="center">
  <b>The all-in-one League of Legends companion — and the only one that scans your mods for malware before they touch your disk.</b>
</p>

<p align="center">
  Change skins, build your own announcer, sync cosmetics with your lobby, never miss a queue — from one tiny, self-updating, Rust-built <code>.exe</code>.
</p>

<p align="center">
  <a href="https://github.com/ChudTonic/League-Of-Legends-Auto-Accept-Skins/releases/latest"><b>⬇️ Download</b></a>
  &nbsp;·&nbsp;
  <a href="https://discord.gg/SxS5yjdnwR"><b>💬 Discord</b></a>
</p>

---

## Only in Chud

Every other tool in this space cuts the same corners. These exist **nowhere else**:

- 🛡️ **A malware scanner for the mods themselves** — nobody else scans a skin before you run it. Chud does, in memory, before it hits disk.
- 🎙️ **Build your own announcer** — turn any clip or your own mic into a real League announcer pack. No Wwise, no external tools.
- 🤝 **Zero-identity party sync** — sync skins with your lobby with no summoner IDs on the wire and cryptographically signed selections. Everyone else needs manual tokens and broadcasts your account in the clear.
- 🔒 **A ranked kill-switch that's *always* on** — injection is refused in ranked no matter what, and fails closed when it can't be sure.
- 🦀 **One tiny self-updating Rust `.exe`** — the whole suite in a single signed binary that installs its own updates. No Python runtime, no reinstalls.

Details below.

---

## The things no one else does

### 🛡️ ModScan — it scans the mods themselves

Reviews of skin changers openly warn that some of them are *"straight malware in a zip file."* Every other tool hands you a random `.fantome` and tells you to run it. **Chud is the only one that inspects the mod first.**

- **Caught before it lands.** Every download is analyzed *in memory, before a single byte is written to disk* — a malicious mod never reaches your machine.
- **It understands the format.** It parses the WAD/fantome structure and flags what a cosmetic mod must never contain: zip-slip / path-traversal, executables disguised as textures (magic-byte sniffing, not just extensions), non-cosmetic payloads, symlinks, reserved-name tricks, and zip-bombs.
- **Multi-engine reputation.** Structural analysis is backed by a VirusTotal reputation check — hash-only and cached, so your files are never uploaded.
- **You stay in control.** Clean installs are silent; a flagged mod is blocked with a plain-English reason and an explicit *"install anyway"* for anything you insist on — except mods a lobbymate tried to push onto you, which are hard-blocked.
- **A whole scan panel.** Sweep your entire mods folder on demand, see a verdict badge on every installed mod, re-scan any of them, or scan a file by hand.

**No other League mod tool ships a scanner. This is the one.**

### 🎙️ Announcer Studio — make your *own* announcer

Drop an audio clip or **record your mic** for any call — First Blood, Ace, Pentakill, objectives — and Chud builds a real announcer pack and installs it to your wheel. No Wwise, no external tools, no file wrangling. It works on Summoner's Rift, ARAM, and every map variant, and party-syncs to friends who don't even have the audio. **Nothing else on the market does this.**

### 🤝 Party sync that respects your privacy

Friends in your lobby who use Chud see each other's skins *and* announcers, auto-detected with zero setup — no codes to paste. And it's built privacy-first: **no summoner IDs ever leave your machine** (an ephemeral per-session identity is used instead), every selection is **cryptographically signed** so nobody can spoof what you're "wearing," and it stays **off until you accept a plain-English data disclosure.** Other sync tools need manual token swaps and broadcast your account identity in the clear.

### 🔒 Ranked-safe by design

Chud's ranked kill-switch runs **always** — a background monitor that refuses every injection in a confirmed ranked (or unverifiable) game, independent of which tools you have armed. It fails *closed*: if the game state can't be verified, injection is denied, not allowed.

---

## What it does

- 🎨 **Any skin, any champion** — pick skins & chromas right inside the League client (press **`C`**), owned or not. Only you need Chud to see them. Chromas, custom mods, "historic" older skins, and a random-skin roll included.
- 📚 **Skin Library** — browse and one-click install thousands of community skins, maps, announcers, and fonts. Everything you install shows up on the in-client **Custom Mods** button in champ select.
- 🎙️ **Announcer Studio** + **🥜 Chud Originals** — build your own announcer, or grab a curated first-party pack like the **BurntPeanut** announcer.
- 🛡️ **ModScan** — the malware scanner above, with its own dashboard tab.
- ✅ **Auto-Accept** — snaps up every ready check the instant it pops.
- 🎯 **Auto-Range** — keeps your attack-range indicator on screen during a match (auto-disabled in ranked).
- 🎵 **Runes & builds** — optional one-tap import of a recommended build the moment you lock in.
- 🧹 **Client declutter** — optionally hide store nudges, ads, and attention-nag badges in the League client.
- 🕵️ **Appear offline** — stay hidden on your friends list while you play.
- 📊 **Live profile** — rank, recent form, champ pool, and match history from your client.
- 🔄 **Self-updating** — one signed `.exe` that installs its own updates. No reinstalls, ever.

---

## Quick start

1. **[Download the installer](https://github.com/ChudTonic/League-Of-Legends-Auto-Accept-Skins/releases/latest)** (`Chud_x.y.z_x64-setup.exe`) and run it — it installs for your user only, no admin install.
2. Launch **Chud**, then start League. It finds your client automatically.
3. That's it. When a new version ships, a **✨ update pill** appears in the app; click it and Chud downloads, installs, and restarts itself.

> 💡 **Skins need one extra one-time file.** For legal reasons Chud can't ship the skin-injection library. Drop your own copy of `cslol-dll.dll` into `%LOCALAPPDATA%\Chud\cslol-tools\` once — it survives every update.

> 💬 Questions, or want to try skin sync? **[Join the Discord](https://discord.gg/SxS5yjdnwR).**

---

## ⚠️ Please read — this can get you banned

Chud changes skins by **injecting into the game**, and Auto-Range **synthesizes keyboard input**. **Riot's Vanguard anti-cheat can detect this and ban your account** — a real risk with any skin changer or input tool for League.

Chud operates **openly**: there is no anti-cheat evasion, and none will ever be added. Its safeguards reduce risk without removing it:

- 🔒 **Always-on ranked kill-switch** — injection is refused in any confirmed ranked or unverifiable game.
- ✋ **Versioned consent gate** — the riskier tools stay locked until you accept the risk; the disclosure is versioned, so a material change re-prompts.
- 🛡️ **ModScan** — malicious mods are blocked before they reach disk.
- 🧩 Auto-Accept is the safe core — it only talks to the local client API (no game memory, no injection).

**In practice:** the maintainer has run **Auto-Accept and Auto-Range daily for over a year with no detection or ban.** That's one account's real-world experience with the queue/range tools — reassuring, but not a guarantee, and Riot can change what Vanguard flags at any time. Skin injection is the higher-risk surface; weigh it separately.

**Don't use Chud on an account you aren't willing to lose.** Not affiliated with or endorsed by Riot Games.

---

## Under the hood (technical)

Chud is a **ground-up rewrite in Rust + Tauri 2** — a single, self-contained, self-updating `.exe` rather than a Python app with a runtime. A few deliberate choices:

**Injection safety is a fail-closed policy engine.** Every side effect — building the overlay, suspending the game process, patching a skin selection via the client API, and starting the hook — is gated by one `evaluate_injection_policy()` call immediately before it runs. The policy reads only live backend state (skins enabled, versioned consent, an always-on gameflow monitor, tool presence) and denies with a typed reason (`RANKED_QUEUE`, `CONSENT_MISSING`, `WRONG_PHASE`, `INTEGRITY_FAILED`, …) that the UI surfaces verbatim. A stale monitor or an unwired gate denies rather than allows.

**Two HTTP clients, never confused.** The League client speaks over a self-signed cert on loopback, so its client relaxes TLS — but *only* against `127.0.0.1`. Everything that leaves the machine goes through a separate client with normal certificate validation, HTTPS-only, an allowlist enforced on the initial request *and* every redirect hop, and streamed size caps against a hostile upstream.

**Party mode is authenticated, not trusted.** The relay is a dumb pipe. The client mints an ephemeral identity per session, the relay assigns each member a random id (clients can't claim one), and every skin selection is **ed25519-signed and bound to a per-room epoch + member id**. Peers verify signatures and cross-check the claimed champion against the live champ-select roster before anything is injected — a spoofed or non-lobby member can't influence what you see or trigger a download.

**ModScan** is a standalone pure-Rust crate (`modscan-core`) — bytes in, a structured verdict out — shared by the app's scan-on-download path and its scan panel. The reputation layer (VirusTotal, behind a hash-keyed cache) is advisory and can only *escalate* the structural verdict, so an outage or rate-limit never blocks a legitimate install.

### Build from source

Windows 10/11, the [Rust toolchain](https://rustup.rs) (MSVC), and the prerequisites in [`native/BUILD.md`](native/BUILD.md) (VS Build Tools, NASM, CMake).

```powershell
cargo install tauri-cli --version "^2"   # one-time
cd native
cargo tauri dev                          # run it
cargo tauri build --bundles nsis         # build the installer
```

Preview just the UI in a browser (no Rust build, mock data):

```powershell
cd native/ui
python -m http.server 8137   # then open http://localhost:8137
```

### Project layout

```
native/
  src-tauri/     ← Rust core (Tauri 2): LCU client, injection pipeline + safety gates,
                   party relay client, tray, updater
    resources/pengu-loader/plugins/CHUD-*   ← in-client menus (run inside League)
  modscan-core/  ← pure-Rust mod scanner (shared by the app + the scan panel)
  ui/            ← front-end: plain HTML/CSS/JS, no bundler (Neon Glass design)
docs/            ← PRIVACY-PARTY.md (party-mode data disclosure)
```

The catalog, reputation, and party relay run on Cloudflare Workers deployed separately (not in this repo — end users don't build or run them).

---

## Credits

**The skin-changing engine began as [Rose-Remastered by Alban1911](https://github.com/Alban1911/Rose).** The skin research, injection pipeline, Pengu Loader integration, and the party-sync idea were their work, and Chud stands on it — this is a Rust rewrite with heavy additions on top. If you like the skin side of Chud, go **star [Rose](https://github.com/Alban1911/Rose)**.

Also built on:

- **[Pengu Loader](https://pengu.lol/)** — the client mod loader Chud bundles for its in-client menus.
- **[cslol / LeagueToolkit](https://github.com/LeagueToolkit/cslol-manager)** — the `mod-tools` overlay/injection utilities.

---

<p align="center">
  Made as a personal project. Not affiliated with or endorsed by Riot Games.<br/>
  League of Legends is a trademark of Riot Games, Inc.
</p>
