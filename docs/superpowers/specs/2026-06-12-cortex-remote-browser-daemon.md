# Cortex — Remote Browser-Control Daemon (Design Proposal)

**Date:** 2026-06-12
**Status:** PROPOSAL — for review, not yet adopted
**Author:** drafted in a Claude Code remote session (sandbox-side perspective)
**Related:** `rust/crates/chromeport/` (the CDP capture driver), `scripts/launch-windows-chrome-cdp.ps1`, `src/skills/scrape-ota/SKILL.md`

> **One-line goal:** let a Claude Code **remote/web session** (running in an ephemeral
> cloud sandbox with no browser and restricted egress) drive a **real Chrome on the
> user's hosting machine** — for OTA scraping and live-dashboard verification — without
> exposing the user's home network or raw CDP to the internet.

---

## 1. Problem

`chromeport` (the Rust CDP driver) assumes it can reach a Chrome with
`--remote-debugging-port` **on localhost**. That holds when chromeport runs on the
user's own machine. It does **not** hold for a Claude Code remote session:

- The sandbox is an isolated, ephemeral container, cloned fresh from GitHub.
- There is **no Chrome** in the sandbox, and no `.env`/Turso creds.
- Outbound egress is governed by a network policy (today: live OTA/dashboard URLs
  returned `403`/were unreachable from the sandbox).
- There is **no SSH** back to the user's machine (no client, no keys, no agent).

So today, anything that needs a real browser — scraping JS-heavy OTA sites, or
visually verifying the deployed dashboard renders correctly — **cannot be done from a
remote session**. It requires the human to be at their local machine. (This is exactly
what blocked verifying the `trip-dashboard-rs` meal-link bug from a remote session.)

**Cortex** is a small long-running daemon on the user's hosting machine that bridges
this gap: it owns a Chrome instance and exposes a **narrow, authenticated, audited**
control channel that a sandbox-side `chromeport` can dial into.

## 2. Goals / Non-goals

### Goals
- A remote sandbox session can run the existing `chromeport` capture/verify/parse flow
  against a **real Chrome on the host**, with a one-line config change (point at the
  broker instead of `localhost:9222`).
- **No inbound ports opened on the home network** — the host dials *out* to a
  rendezvous; the sandbox connects to the same rendezvous. (Avoids home-router port
  forwarding and exposing CDP to the public internet.)
- **Raw CDP is never exposed.** Cortex mediates: auth, a command **allowlist**, a
  navigation **domain allowlist**, per-session isolation, and an audit log.
- **Revocable + ephemeral**: short-lived session tokens; the human can kill the daemon
  (or a single session) instantly.
- Reuse `chromeport` as-is for capture/parse logic; only its **transport** changes.

### Non-goals
- Not a general remote-desktop or a public scraping service. Single-user, single-host.
- Not a replacement for the deploy path (wrangler/npm stays).
- No headless cloud Chrome (that loses the "real residential browser session" property
  that OTA sites need, and the human-in-the-loop login/CAPTCHA handling).
- Not in scope: driving arbitrary apps — **only Chrome via CDP**.

## 3. Why raw CDP cannot be exposed directly

The Chrome DevTools Protocol is effectively **remote code execution over the browser**:
`Page.navigate`, `Runtime.evaluate` (arbitrary JS in any page), `Browser.*`,
`Fetch`/`Network` interception, cookie/credential access, file download to disk. An
open `--remote-debugging-port` reachable by an attacker = full takeover of the browser
profile (which may be logged into the user's real accounts). Chrome binds the port to
`localhost` for exactly this reason and refuses non-loopback `Host:` headers.

Therefore Cortex must **terminate** the CDP connection and re-originate a **filtered**
one — never tunnel the raw WebSocket end-to-end.

## 4. Architecture

```
┌─────────────────────────┐         ┌──────────────────────────────────────────────┐
│  Claude Code sandbox     │         │  User hosting machine (always-on, at home)    │
│  (ephemeral container)   │         │                                                │
│                          │         │   ┌──────────────┐   CDP/ws    ┌────────────┐ │
│  chromeport              │  WSS    │   │  Cortex      │◄───────────►│  Chrome     │ │
│   --broker wss://…  ─────┼────────►│   │  daemon      │  localhost   │ --remote-   │ │
│   --session <token>      │ (out)   │   │              │  :9222       │  debugging  │ │
│                          │         │   │ • auth       │              │  -port      │ │
│                          │         │   │ • allowlist  │              └────────────┘ │
│                          │         │   │ • audit log  │                              │
│                          │         │   │ • session mux│   (real, logged-in profile)  │
└─────────────────────────┘         │   └──────┬───────┘                              │
            ▲                        │          │ outbound dial                        │
            │                        └──────────┼──────────────────────────────────────┘
            │                                   │
            │        ┌──────────────────────────▼─────────────────┐
            └───────►│  Rendezvous broker (tiny relay)             │
                     │  • TLS, token auth, pairs sandbox↔host     │
                     │  • Cloudflare Tunnel / self-hosted relay    │
                     └─────────────────────────────────────────────┘
```

### Components

1. **Chrome (host)** — launched with `--remote-debugging-port=9222` bound to loopback,
   using a **dedicated profile** (see §7 — *not* the user's daily-driver profile).
   Reuse `scripts/launch-windows-chrome-cdp.ps1`.

2. **Cortex daemon (host)** — the new component. Connects *out* to the rendezvous,
   authenticates, and for each authorized sandbox **session**:
   - opens/owns a CDP target (tab) — one tab per session, isolated;
   - accepts a **filtered** command stream (see §5);
   - relays allowed CDP calls to localhost Chrome, returns results;
   - writes every command + verdict to an **audit log**.

3. **Rendezvous broker** — a tiny relay so neither side needs an inbound port. Options:
   - **Cloudflare Tunnel** (`cloudflared`) fronting Cortex with Access policy — least
     code, strong auth, no home port. **Recommended for v1.**
   - A self-hosted WebSocket relay (host dials out, sandbox dials out, relay pairs by
     token) — more control, more to build/run.

4. **chromeport (sandbox)** — gains a `--broker <wss-url> --session <token>` transport
   that targets the broker instead of `localhost:9222`. All existing
   `fetch/interact/verify/parse` logic is unchanged — it just speaks CDP to a different
   endpoint.

## 5. Control protocol (filtered CDP)

Cortex does **not** forward arbitrary CDP. It exposes a **capability-scoped** subset,
enforced server-side (host), not client-side:

| Capability | Allowed CDP (examples) | Notes |
|---|---|---|
| `navigate` | `Page.navigate`, `Page.reload` | **only** to URLs matching the navigation allowlist (§6) |
| `observe`  | `Page.captureSnapshot`, `DOM.*` (read), `Page.captureScreenshot`, `Runtime.evaluate` **read-only-sandboxed** | the workhorse for capture/verify |
| `interact` | `Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `DOM.querySelector`+click | drive the real UI (the `interact --step` flow) |
| `network`  | `Network.enable`, response bodies | for capture; **interception/redirect disabled** |

Hard-denied always: `Browser.close`, `Target.createBrowserContext` outside the session,
`Page.setDownloadBehavior` to arbitrary paths, `Fetch.fulfillRequest` (request forgery),
`Runtime.evaluate` that escapes the page (no `require`, no `chrome://`), `Storage`/cookie
export for non-session origins.

`Runtime.evaluate` is the dangerous one. v1 stance: **deny by default**; allow only a
fixed, named set of vetted snippets (e.g. "scroll to bottom", "read innerText") shipped
with Cortex, referenced by id — never free-form JS strings from the sandbox.

> Design rule: the sandbox sends **intents** (`navigate(url)`, `snapshot()`,
> `click(selector)`), not raw CDP. Cortex compiles intents → vetted CDP. This keeps the
> trust boundary on the host where the human controls it.

## 6. Navigation allowlist

A per-session config on the host pins what the browser may visit, e.g.:

```
allow:
  - "https://*.liontravel.com/**"
  - "https://*.settour.com.tw/**"
  - "https://trip-dashboard*.yanggf.workers.dev/**"
deny: ["*"]   # default
```

Cortex rejects `Page.navigate` to anything not matched, and (via `Target` policy) blocks
window.open/popups to off-list origins. This bounds blast radius even if the sandbox
session is compromised: it can scrape the OTAs you allowed and nothing else.

## 7. Security model

- **Dedicated Chrome profile.** Never the user's main profile. A throwaway profile that
  is logged into *only* the OTA accounts needed for scraping. Compromise ≠ access to
  email/bank.
- **Outbound-only host.** No inbound firewall rule. Host dials the rendezvous; the
  rendezvous enforces auth (Cloudflare Access / mTLS / token).
- **Two-layer auth:** (1) rendezvous admits only authenticated peers; (2) Cortex
  requires a **per-session token** minted by the human (short TTL, single-host, scoped to
  one nav-allowlist). Sandbox never holds a long-lived credential.
- **Server-side enforcement.** All allowlists/denials run on the host. The sandbox is
  treated as **untrusted** (it may run model-generated or prompt-injected commands).
- **Audit + kill switch.** Every intent → CDP translation is logged with timestamp,
  session id, url, verdict. A single `Ctrl-C` (or `cortex kill <session>`) tears down the
  tab and revokes the token.
- **Prompt-injection awareness.** A scraped page can contain text trying to steer the
  agent ("navigate to …", "paste your token"). Because the sandbox can only emit
  *allowlisted intents to allowlisted origins*, the worst case is bounded; Cortex logs
  and the human can review.
- **No secrets to the sandbox.** Turso/CF creds stay on the host. If captures must land
  in Turso, Cortex (host-side) does the write, or returns the capture blob to the sandbox
  which writes via its own (separately-scoped) path.

## 8. chromeport changes (sandbox side)

Minimal. Add a transport selector:

```
chromeport fetch interact "<url>" --source liontravel \
  --broker wss://cortex.example/ws --session $CORTEX_SESSION --step ...
```

- New `--broker <wss>` + `--session <token>` flags (env fallbacks
  `CORTEX_BROKER` / `CORTEX_SESSION`).
- A `transport` abstraction: `Local(localhost:9222)` (today) vs `Broker(wss, token)`
  (new). Everything above the transport — capture envelope, `verify`, `parse capture` —
  is unchanged.
- When `--broker` is set, chromeport speaks the **intent** protocol (§5), not raw CDP, so
  the sandbox binary itself never needs full CDP surface.

## 9. Phased plan

- **Phase 0 — spike (host-local):** Cortex on the host, sandbox replaced by a local
  test client over loopback. Prove the intent→CDP filter + audit log with a hardcoded
  allowlist. No rendezvous yet.
- **Phase 1 — rendezvous via Cloudflare Tunnel:** front Cortex with `cloudflared` +
  Access. Drive `trip-dashboard-rs` verification end-to-end from a remote sandbox
  (navigate + snapshot + grep meal links). This is the smallest real win and is
  read-only (lowest risk).
- **Phase 2 — `interact` capability:** enable click/type intents for one OTA
  (`settour`, already live-verified) behind the allowlist. Validate the capture→parse→
  Turso path from a remote session.
- **Phase 3 — hardening:** session TTLs, per-session nav-allowlists, vetted-snippet
  registry for `Runtime.evaluate`, structured audit export, `cortex kill`.

## 10. Open questions (for review)

1. **Rendezvous choice** — Cloudflare Tunnel + Access (least code, ties to CF you already
   use) vs a self-hosted relay (more control, more ops)? Recommend CF Tunnel for v1.
2. **Where do captures land?** Host-side Cortex writes to Turso (keeps creds on host) vs
   stream the blob back and let the sandbox write? Former is safer.
3. **`Runtime.evaluate` policy** — deny-all + vetted-snippet registry (safe, less
   flexible) vs an AST/allowlist sandbox (flexible, much harder to get right). Recommend
   deny-all for v1.
4. **Daemon language** — Rust (reuse `chromeport`'s CDP client + the workspace) vs a thin
   Node/Go service. Rust keeps it in-tree and shares the protocol types.
5. **Profile management** — one persistent scraping profile vs ephemeral profile per
   session (loses OTA logins). Likely persistent-but-dedicated.

## 11. What this is NOT replacing

`chromeport`'s local mode stays the default for when the human is at their machine. Cortex
is purely an **additional transport** for the remote-session case. If Cortex is down, the
sandbox simply can't drive a browser (today's status quo) — no regression.
