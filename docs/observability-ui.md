# Base UI observability components: inventory, scope and verification

This is the **first vertical slice**, not a completed dashboard-wide Radix migration. Base UI means [base-ui.com](https://base-ui.com/), not Uber Base Web. Base: `516a2563d5e905746293ef9263d7de4100e92134`.

The follow-up [results-first logs iteration](logs-results-first.md), based on `660b4e28b0367cab2c95de7aad834e98203b32a6`, replaces the initial desktop split inspector with inline expansion and removes default expanded navigation/facets. Its latest measurements and evidence are separate from the historical first-slice results below.

The later [six-route dashboard checkpoint](dashboard-wide-checkpoint.md) extends compact navigation, shared Base UI request states, time/scope correctness and bounded tables across the existing dashboard. It has separate current build fingerprints and real-versus-fixture evidence. The inventory and first-slice numbers below are historical; the dashboard remains a partial primitive migration.

## Inventory and extraction plan

| Layer | Existing implementation | This slice / next boundary |
| --- | --- | --- |
| Primitive wrappers | shadcn-owned `components/ui`, primarily Radix, plus cmdk, Sonner and Vaul | New reusable surfaces use pinned `@base-ui/react` **1.9.0**. Button, Input, Menu/CheckboxItem, Tabs, Collapsible and mobile Dialog. Retain shadcn semantic token styling. |
| App shell, filters, calendar, notifications | Existing wrappers and day-picker/date-fns | Logs now default to a compact navigation rail and closed, separately persisted facets, with active filter chips visible. Targeted navigation/facet accessibility fixes; underlying legacy shell/filter/calendar/command/notification primitives remain a partial migration. |
| Dense log rows / pagination | TanStack Table, existing query adapter | Reusable `EventExplorer` uses TanStack Virtual for rows. Existing Table/store/facets/histogram/query pipeline remains the app adapter. Explicit Base UI load-more control calls the unchanged cursor adapter. |
| Event inspection | Generic data-table sheet | Reusable body-first inspector, controlled selection, event JSON, bounded attribute tree, service inclusion, copy actions. Desktop details now expand inline without reducing results width; mobile uses a full-screen Base UI Dialog. |
| Charts, virtualization and panel resizing | Recharts/ECharts, TanStack, resizable panels | Keep specialized engines. They are **not Base UI components**. Base UI supplies appropriate interactive controls, not their rendering engines. |
| Traces / profiles | Laminar-style trace workspace, profile engine | Source unchanged and browser regression tested. Later extract trace tree/timeline/span inspector against the same presentation contracts. |
| Tokens | Current application shadcn variables; separate polish PR #168 | Consume existing tokens, no global stylesheet changes. Preserve #168; resolve token/trace overlap explicitly before any later integration. |

Dependency direction is `Sigiro adapter → observability presentation → Base UI / specialized engines`. Reusable files import no Next router, Sigiro transport, OAuth, SQL, tenant context or storage namespace.

### New reusable source

`dashboard/components/observability/` contains `EventExplorer`, `AttributeTree`, typed event/filter contracts, Button/Input/ActionMenu, serializer, and scoped CSS. Controlled props cover selection, optional pagination, filter intentions and correlation links. Column visibility, find, wrap and density are local view preferences. Find is explicitly **loaded-events only**. Authoritative matching counts/facets stay in the existing application toolbar, rather than pretending a sample is the whole query.

Presentational HTML such as messages and metadata does not require an artificial primitive abstraction. Appropriate new interactive controls use Base UI. Existing trace resizing retains its specialized panel engine; logs no longer have a split inspector separator.

This directory has an explicit **MIT** license and is excluded from the root FSL-1.1-ALv2 grant. The application remains FSL. This is a source extraction boundary, **not a new public repository or published package**. Before a future package publication: review naming, exported contracts, theme/token assets, full dependency notices and examples separately.

No ClickStack source/assets were copied. Its structured logs workflow at pinned commit `2bad9af1723bd74ba5473a617f8688a0ac365282` was a reference, alongside existing Laminar traces and telemetry.dev hierarchy. Existing Laminar Apache-2.0 license/NOTICE files are unchanged. New Base UI/react, Base UI/utils and Babel runtime MIT notices are retained in `dashboard/public/licenses/`.

## Separate pre-existing bug: dynamic logs service filter

The logs schema used `col.enum([])` for dynamic facets. Its generated `parseAsStringLiteral([])` rejected every actual service string: the URL could change while parsed query state discarded the filter.

The first-slice fix was **logs-only**: an explicit `service: field.array(field.string())` filter override. The later dashboard checkpoint applies the same bounded override to Traces and verifies its actual SQL. The empty-enum visual facet schema, generic generators/parsers, SQL/transport and auth code are unchanged. The explorer accepts an optional filter override to support this app-level declaration.

Real read-only backend checks verify:

1. Include `dashboard-e2e` in the inspector → URL → parsed filter → actual SQL `IN ('dashboard-e2e')` → two returned rows.
2. Reload preserves the filter; unchecking the service removes both URL state and SQL predicate.
3. A service containing a quote and backslash survives URL/reload; SQL doubles the quote, preserves the backslash and returns an actual 200/empty result, without modifying stored data.
4. Browser back/forward restores parsed state and displayed results.

See [recorded queries/results](assets/observability-ui/filter-roundtrip.json). Service exclusion is **not exposed** because the existing transport has no exclusion operator.

## Rendering/copy bounds and security behavior

- Virtualized rows preview 500 characters; inspector message/event-JSON previews cap at 50,000; leaf previews at 10,000.
- Attribute objects show at most 200 children per opened level, eight levels deep. Nested objects start collapsed and do not construct descendant trees until opened. No automatic expand-all of attacker-shaped payloads.
- Clipboard actions serialize/copy complete supported values, independently of preview limits. Promise rejection and unavailable clipboard produce explicit status messages, never false success.
- True cycles become `[Circular]`; BigInt becomes a decimal string; repeated non-cyclic references are retained. This is **JSON-safe representation, not lossless JS graph serialization**.
- Values that native JSON serialization cannot handle, including excessive nesting, produce a visible error and disabled event-JSON copy rather than crashing or claiming complete copy. Individual fields/message remain available.
- Messages/values render as React text, not HTML. No ANSI/OSC8 or URL execution is added. No decorative query modes, new endpoints or fake global attribute discovery.

Tests do not establish a universal security, accessibility or performance guarantee.

## Historical first-slice self-verification (`660b4e28`)

No independent review was performed: the user requested a single-agent workflow. No other agents were spawned for this work. These are local self-verification results, not CI results.

- Typecheck, production static build, targeted root oxlint and formatter checks passed.
- **Actual existing backend:** row Arrow/Home/Enter navigation, UUID URL selection, tabs/copy, wrap/columns/find, desktop resize, focus restoration after virtual-row remount, both themes, 390px no document overflow, full-screen dialog focus trap/Escape return, reduced motion. Scoped axe 4.14.0: **zero violations** on the new explorer in light/dark and mobile inspector. Not a dashboard-wide accessibility audit.
- **Actual trace regression:** existing 82-span trace selection/search/input-output/events/resource attributes/error status/zoom/keyboard/resize/deep links, 2,102-span fixture capped at 2,000 with virtualized DOM, empty/invalid states, real correlated log link, profile link trace/span/service/window and 390px trace sheet. No runtime errors. Profile navigation produced a legitimate empty sample state; no profile data was invented.
- **Synthetic exact-app API responses:** 50+10 cursor pages append; stop at authoritative count; End reaches appended final row. 5,000 loaded rows plus a 1,113,190-byte hostile message: **21 mounted rows** at start/end, full clipboard equality, no script execution, unexpected outbound requests or prototype pollution. Actual outgoing service SQL quoting is checked. Scoped axe for the fixture’s light/dark error/info rows: zero violations.
- **Standalone same-source component harness:** 1.4-million-character message and 1.2-million-character attribute copy fully; cyclic/BigInt/repeated-reference checks pass. A 40,000-leaf fan-out stays collapsed/bounded. Excessive nesting reports serialization failure; rejected/unavailable clipboard is visible. This harness is distinct from the app API tests because JSON transport cannot carry native BigInt/cyclic graphs.

### Performance observations (not benchmark claims)

Against an unmodified build of the same base on the same local read-only backend, three cold Chromium contexts per build gave median time to two real rows of **423ms baseline / 444ms current**. The requested JS/CSS set, including viewport prefetch, increased by **169,704 raw bytes / 56,059 offline-gzip bytes**. Offline gzip is an estimate, not this preview server’s compression. Partial coexistence with legacy Radix has a bundle cost; this is not represented as a bundle optimization.

The deliberately overfilled 5,000-row synthetic page reached rows in **718ms**, inspector in **238ms**, with ~39.0MB observed JS heap. It recorded main-thread tasks up to **162ms**. Virtual DOM stays bounded, but this is **not zero-jank** and not a proof of production-scale latency. The existing row hashing/adapter also processes this artificial oversized page; normal requests retain their existing page size. [Raw observations](assets/observability-ui/fixture-results.json) and [bundle comparison](assets/observability-ui/bundle-comparison.json).

These first-slice screenshots/measurements are retained unchanged; see [the follow-up](logs-results-first.md) for current layout, tests, performance observations and remaining boundaries.

## Reproduction

From `dashboard/`, with the static build served at `UI_ORIGIN` against the existing backend:

```sh
aube run typecheck
aube run build
aube run test:values
# Install Playwright Chromium, or set CHROMIUM_PATH to an existing Chrome binary.
aube run test:graph
UI_ORIGIN=http://127.0.0.1:19990 aube run test:browser
UI_ORIGIN=http://127.0.0.1:19990 aube run test:flow
BASELINE_ORIGIN=http://127.0.0.1:19989 node tests/bundle-comparison.mjs
```

The real-backend browser scripts intentionally target the existing local verification window/two `dashboard-e2e` logs. They do not seed a server or contain credentials. `LOG_WINDOW` can override the time window; another dataset requires adapting expected rows/service. Synthetic fixtures intercept requests in the browser and are labeled as fixtures. `EVIDENCE_DIR` controls output location.

## Real screenshots

No synthetic data is presented as a product screenshot.

[Events](assets/observability-ui/01-real-logs.png) · [Inspector](assets/observability-ui/02-real-inspector.png) · [Mobile events](assets/observability-ui/03-real-mobile.png) · [Mobile inspector](assets/observability-ui/04-real-mobile-inspector.png) · [Dark](assets/observability-ui/05-real-dark.png)

## Remaining migration boundary

Next slices: app-shell/filter interaction primitives; calendar/command/notifications with tested equivalent contracts; trace/span reusable presentation; token export after #168 coordination. Keep backend/query/auth and specialized chart/table/profile engines outside the reusable interaction layer.

No source from rejected console work was resumed. Rejected branch/worktree, PR #168 and preview ports 19993–19997 were preserved. No merge, deployment, public repository, package publication or infrastructure change occurred.
