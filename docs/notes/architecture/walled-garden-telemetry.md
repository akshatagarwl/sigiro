---
title: "Walled Garden Telemetry: Architectural Decisions"
created: 2026-03-21
updated: 2026-04-15
type: concept
tags: ['ai-agent', 'architecture', 'auth', 'competitor', 'data-model', 'decision']
---

# Walled Garden Telemetry: Architectural Decisions

> How to get observability data from platforms where users can't deploy infrastructure.

## The Problem

Vibe coder platforms range from "full Linux shell" (Replit) to "you can only inject a `<script>` tag" (Lovable). Sigiro needs a telemetry strategy for each tier.

## Platform Capability Matrix (researched 2026-03-21)

| Platform | npm install | Server entry point | Env vars | Build config | `<script>` tags | Supabase/Firebase | Code export |
|---|---|---|---|---|---|---|---|
| Replit | Yes (shell) | Yes | Yes (Secrets) | Yes (.replit) | Yes | N/A | Yes (GitHub) |
| Softgen | Post-export | Post-export | Post-export | Post-export | Post-export | Firebase | Yes (GitHub→Vercel) |
| Emergent | Via AI agent | Via AI agent | Yes | Unclear | Yes | N/A | Yes (GitHub) |
| Bolt.new | Yes (WebContainers) | Yes | Partial (maturing) | Yes | Yes | N/A | Yes (GitHub/Netlify) |
| Lovable | Via AI prompt only | **No server** (SPA) | Supabase secrets | **No** | **Yes** | **Supabase** | Yes (GitHub) |
| Create.xyz | **No** | **No** | **No** | **No** | Via prompt maybe | N/A | Yes (download) |

## Three-Tier Architecture

### Tier A: Full OTel Auto-Instrumentation
**Platforms:** Replit, Softgen (post-export), Emergent, Bolt.new (after deploy to Vercel/Netlify)

**Pattern:** Standard [[architecture/write-path.md|OpenTelemetry]] auto-instrumentation sending to hosted sigiro.

```
User's app (Node.js/Python)
    |
    | OTEL_EXPORTER_OTLP_ENDPOINT=https://ingest.sigiro.com
    | OTEL_SERVICE_NAME=my-app
    v
@opentelemetry/auto-instrumentations-node (or Python equivalent)
    |
    v
Hosted sigiro (OTLP receiver → DuckDB → /diagnose)
```

**What sigiro needs:**
- Hosted [[architecture/write-path.md|OTLP]] endpoint at `ingest.sigiro.com` (auth via Bearer token)
- Per-user [[architecture/data-model.md|DuckDB]] instances (or multi-tenant schema)
- Setup guide: "Run these 2 commands, set these 2 env vars"

**For Replit specifically:**
- Replit Extension that auto-configures OTel in the user's project
- Or: MCP server that Replit Agent uses to add observability via prompt

**What this captures:** Everything — HTTP requests, DB queries, errors, traces, metrics. Full server-side + client-side if browser SDK also added.

### Tier B: Browser SDK (the universal injection point)
**Platforms:** Lovable, Create.xyz (via prompt), ALL platforms as a supplement

**This is the key architectural decision.** Every platform allows `<script>` tags. Build `@sigiro/browser` — a lightweight browser SDK.

```html
<!-- One line. Works everywhere. -->
<script src="https://cdn.sigiro.com/browser.js" data-token="<oauth-access-token>"></script>
```

**What it captures (client-side only, no server access):**
- Page loads + navigation timing (Performance API)
- All fetch/XHR requests: URL, method, [[plans/STATUS.md|status]] code, duration
- Uncaught JS errors + unhandled promise rejections
- [[designs/future/web-vitals.md|Web Vitals]]: LCP, CLS, INP
- User interactions: clicks, form submissions (anonymized)
- Console errors

**What it misses:** Server-side execution, DB queries, internal service calls, errors that return 200 with wrong data.

**Implementation:**
```
@sigiro/browser (~15-20KB gzipped)
    |
    | Patches fetch/XHR, hooks window.onerror
    | Collects Web Vitals via web-vitals library
    | Batches events, sends via navigator.sendBeacon
    |
    v
POST https://ingest.sigiro.com/v1/browser (JSON, not OTLP protobuf)
    |
    v
Hosted sigiro converts to OTLP spans/logs → DuckDB
```

**Why not just use `@opentelemetry/sdk-trace-web`?**
- It's ~60KB gzipped (full auto-instrumentation bundle)
- Configuration is complex (exporters, resource, instrumentations)
- Requires `init()` call — can't be a simple script tag
- Designed for developers, not vibe coders

**`@sigiro/browser` design principles:**
1. Single `<script>` tag, zero config (token in `data-` attribute)
2. < 20KB gzipped
3. No dependencies
4. Captures fetch/XHR [[plans/STATUS.md|status]] codes (this is the killer feature — "your API is returning 500s")
5. Sends to sigiro's hosted endpoint, not a generic [[architecture/write-path.md|OTLP]] collector
6. Auto-detects SPA navigation (Lovable/Create.xyz are React SPAs)

**For Lovable specifically:**
- User tells AI: "Add this script tag to index.html"
- Or: sigiro provides a Lovable-specific prompt template: "Add observability to my app. Add this to the `<head>` of index.html: `<script src='https://cdn.sigiro.com/browser.js' data-token='YOUR_TOKEN'></script>`"

### Tier C: Backend Event Taps (Supabase/Firebase webhooks)
**Platforms:** Lovable (Supabase), Softgen (Firebase), any app using BaaS

**Pattern:** Tap into the backend-as-a-service layer for data mutation + auth events. No app code changes — configured in Supabase/Firebase dashboard.

```
Lovable app (SPA)
    |
    | writes to Supabase DB
    v
Supabase
    |
    | Database Webhook (pg_net, async)
    | → POST https://ingest.sigiro.com/v1/webhook/supabase
    |
    | Auth Audit Logs (queryable via SQL)
    |
    v
Sigiro converts webhook payloads to spans → DuckDB
```

**What this captures:**
- Every database INSERT/UPDATE/DELETE with row data
- [[designs/auth-billing.md|Auth]] events: login, signup, password reset, MFA
- Edge Function invocations and errors
- Supabase platform logs (BigQuery-backed)

**What sigiro needs:**
- `POST /v1/webhook/supabase` endpoint that converts Supabase webhook JSON to [[architecture/write-path.md|OTLP]] spans
- `POST /v1/webhook/firebase` endpoint for Firebase Cloud Function triggers
- Mapping: table mutations → span attributes (table name, operation, row count, latency)

**Combined with Tier B browser SDK, this gives:**
- Client-side: "POST /api/checkout returned 500 in 2.3s" (browser SDK)
- Backend: "INSERT INTO orders failed: constraint violation on user_id" (Supabase webhook)
- Together: the full picture without any server-side instrumentation

## The Compound Strategy

For maximum coverage on any platform, layer the tiers:

| Layer | What | Captures | Implementation |
|---|---|---|---|
| Browser SDK | `<script>` tag | User experience, HTTP errors, Web Vitals | `@sigiro/browser` npm + CDN |
| Backend taps | Supabase/Firebase webhooks | Data mutations, auth events | Webhook receiver endpoints |
| OTel auto-instrumentation | npm package + env vars | Full server traces, DB queries | `@sigiro/node` wrapper (when available) |
| Synthetic monitoring | Playwright scripts via MCP | Availability, functional correctness | Future: sigiro-managed checks |

**Lovable app gets:** Browser SDK + Supabase webhooks = ~80% of useful observability
**Replit app gets:** OTel auto-instrumentation + Browser SDK = ~95% coverage
**Any exported app gets:** Full OTel + Browser SDK + backend taps = ~100% coverage

## Architectural Requirements for Sigiro

### Must build (ordered by impact)

1. **Hosted [[architecture/write-path.md|OTLP]] endpoint** (`ingest.sigiro.com`)
   - [[designs/auth-billing.md|Auth]]: Bearer token per user/project
   - Accepts: [[architecture/write-path.md|OTLP]] gRPC + HTTP (already implemented in sigiro binary)
   - New: HTTP JSON ingest for browser SDK (lighter than protobuf)
   - Storage: per-user [[architecture/data-model.md|DuckDB]] (single-tenant initially, multi-tenant later)
   - This unlocks ALL tiers

2. **`@sigiro/browser` SDK** (~15-20KB)
   - Single `<script>` tag deployment
   - Patches fetch/XHR for HTTP monitoring
   - [[designs/future/web-vitals.md|Web Vitals]] collection
   - Error capture (window.onerror, unhandledrejection)
   - SPA navigation detection (React Router, History API)
   - Batched beacon export to hosted sigiro
   - This is the universal entry point for ALL platforms

3. **Supabase webhook receiver** (`/v1/webhook/supabase`)
   - Converts pg_net webhook JSON to [[architecture/write-path.md|OTLP]] spans
   - Maps: table name, operation (INSERT/UPDATE/DELETE), row data (redacted PII)
   - [[designs/auth-billing.md|Auth]] event enrichment from audit logs
   - This covers Lovable's backend layer

4. **"Add sigiro" prompt templates**
   - Platform-specific copy-paste prompts for AI-driven platforms
   - "Tell your AI: add this to index.html" (Lovable, Create.xyz)
   - "Tell your AI: install @sigiro/node and set these env vars" (Bolt.new, Emergent)
   - This is distribution, not engineering — but critical for adoption

### Won't build (and why)

- **Reverse proxy / Cloudflare Worker interception**: Requires DNS control. Vibe coders don't manage DNS. Too much infrastructure for the audience.
- **Service Worker injection**: Requires serving a SW file at the domain root. Most platforms serve from their own domains (`.lovable.app`, `.bolt.host`). Can't register a SW on someone else's domain.
- **Build-time Vite/Webpack plugin**: Requires build config access. Lovable and Create.xyz don't expose this. Only works for Tier A platforms, where OTel auto-instrumentation is already better.
- **Platform-specific Extensions/Connectors**: Each platform has different APIs (Bolt Connectors, Replit Extensions). Building for each is N × effort for 1 platform reach. Instead, provide universal patterns (script tag, npm package, prompt template) that work across platforms.

## How This Changes Sigiro's Architecture

### Current: single-binary local mode
```
sigiro binary (local)
├─ :4317 gRPC OTLP
├─ :4318 HTTP OTLP
├─ :9999 /query /diagnose /health
└─ DuckDB file on local disk
```

### Future: hosted mode alongside local mode
```
ingest.sigiro.com (hosted)
├─ :443 OTLP gRPC + HTTP (TLS terminated)
├─ :443 /v1/browser (JSON ingest for browser SDK)
├─ :443 /v1/webhook/supabase (webhook receiver)
├─ :443 /v1/webhook/firebase (webhook receiver)
├─ Auth: Bearer token per project
├─ Per-project DuckDB (or shared with project_id column)
└─ /diagnose, /query, /health per project

cdn.sigiro.com
└─ browser.js (sigiro browser SDK, CDN-cached)

app.sigiro.com (dashboard)
└─ Health score, investigation UI, alert config
```

### Key decision: per-project DuckDB vs multi-tenant

**Per-project [[architecture/data-model.md|DuckDB]] (recommended initially):**
- Each user gets their own [[architecture/data-model.md|DuckDB]] file
- Isolation is trivial — no tenant leaks possible
- `sigiro diagnose` works unchanged (single-tenant queries)
- Scaling: one sigiro process per ~100 projects ([[architecture/data-model.md|DuckDB]] is ~50MB RAM idle)
- Limitation: can't query across projects (fine for vibe coders)

**Multi-tenant [[architecture/data-model.md|DuckDB]] (future, if needed):**
- Shared [[architecture/data-model.md|DuckDB]] with `project_id` column on every table
- All queries filtered by `WHERE project_id = ?`
- Pro: fewer processes, easier ops
- Con: tenant isolation bugs, schema migration complexity
- Only worth it at >1000 projects

### Browser SDK ingest format

The browser SDK should NOT send [[architecture/write-path.md|OTLP]] protobuf (too heavy for a `<script>` tag). Instead, send compact JSON:

```json
POST /v1/browser
Content-Type: application/json
Authorization: Bearer <oauth-access-token>

{
  "spans": [
    {"type": "fetch", "url": "/api/checkout", "method": "POST", "status": 500, "duration_ms": 2300, "ts": 1711036800000},
    {"type": "error", "message": "TypeError: Cannot read property 'id' of null", "stack": "...", "ts": 1711036801000}
  ],
  "vitals": {"lcp": 1200, "cls": 0.05, "inp": 80},
  "meta": {"url": "https://myapp.lovable.app/checkout", "ua": "...", "viewport": "390x844"}
}
```

Sigiro's hosted endpoint converts this to [[architecture/write-path.md|OTLP]]-compatible records and inserts into [[architecture/data-model.md|DuckDB]]. The `/diagnose` endpoint then works unchanged — it queries the same tables regardless of whether the data came from OTel auto-instrumentation or the browser SDK.

## Impact on Investigation Output

Browser-only telemetry changes what `sigiro diagnose` can say:

**With full OTel (Tier A):**
> "POST /api/checkout: 100% errors (baseline: 0%). Database query `SELECT * FROM users WHERE id = ?` timing out after 5s. 47 affected traces in the last 15 minutes."

**With browser SDK only (Tier C):**
> "POST /api/checkout: returning HTTP 500 for 100% of requests (baseline: 0%). Average response time: 2.3s (3x slower than yesterday). 12 uncaught TypeError errors on the checkout page. No server-side telemetry available — consider adding @sigiro/node for deeper [[architecture/investigation.md|investigation]]."

The browser-only output is less detailed but still actionable. The vibe coder can paste this into their AI and say "fix this." The AI gets enough context to [[architecture/investigation.md|investigate]]: which endpoint, what error, how severe, when it started.

## Summary: What To Build, In Order

1. **Hosted [[architecture/write-path.md|OTLP]] endpoint** — unlocks everything, all tiers need it
2. **`@sigiro/browser`** — universal `<script>` tag, covers Lovable + Create.xyz + supplements everything else
3. **Supabase webhook receiver** — covers Lovable's backend layer (biggest constrained platform)
4. **Prompt templates** — "tell your AI to add sigiro" guides per platform
5. **`@sigiro/node`** — thin wrapper around OTel auto-instrumentation with sigiro defaults (for Tier A platforms)
6. **Firebase webhook receiver** — covers Softgen and Firebase-based apps
