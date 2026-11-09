# Results-first logs: local ClickStack-inspired iteration

Iteration base: `660b4e28b0367cab2c95de7aad834e98203b32a6`, branch `feat/base-ui-explorer`. This follows the [initial extraction](observability-ui.md), not a dashboard-wide Base UI migration. **Solo local self-verification, not independent review or CI.** No push, PR, merge, deployment, infrastructure change or publication.

Preview: <http://127.0.0.1:19990/dashboard/logs/?timestamp=1794243300000-1794243450000>.

## What changed

- **Results get the workspace.** Logs default to a compact icon navigation rail on desktop; labeled tooltips, accessible link names and the header toggle keep navigation discoverable. Expand it when needed. Mobile navigation remains opt-in. Entering Logs collapses the desktop rail; other pages retain their expanded default. Mobile navigation closes on route changes and restores focus to its header trigger when dismissed.
- **Facets are opt-in.** The Filters button opens desktop facets or the existing mobile filter drawer. Logs use their own `data-table-logs-facets` local-storage preference, initially false; the operator can persist an explicit opt-in without changing other explorers' preference. Closed desktop facets are inert and hidden from the accessibility tree.
- **Query context does not disappear.** Matching/window counts, removable active time/service/level/message/trace chips, the existing query command and histogram remain visible with facets closed. Date chips use local display time; their title exposes full ISO endpoints. Non-date multi-values use commas, not a misleading range arrow. Mod+K still opens the existing filter command; Mod+B on Logs toggles navigation only, not navigation and facets together.
- **Details do not take a column.** A row's chevron/click/Enter opens a bounded, scrollable inline desktop inspector beneath that row. The results and message columns keep exactly the same width. A 380px maximum leaves surrounding rows visible at the verified laptop size. Close, repeat row activation or Escape dismisses it. Mobile keeps the full-screen Base UI dialog, focus trap and focus return. UUID deep links remain controlled by the existing adapter.
- **Less toolbar overhead.** View groups wrap, density and column checkboxes in one Base UI menu. Loaded-event Find stays visible on desktop, is opt-in on mobile, and closes/clears its local query on mobile. It is not a new server search. The viewport-bounded shell lets the results list, rather than the document, scroll; footers remain in view.
- **Boundaries remain.** EventExplorer/AttributeTree/contracts/primitives stay in the MIT reusable directory; no API/auth/router/SQL/tenant dependencies were added there. TanStack Virtual now measures expanded rows, including the inspector. Appending a page does not scroll back to a previously selected event. Existing charts, query transport, auth, tenant and Laminar trace/profile source remain unchanged.

The reference was actual ClickStack evidence, reinspected **before editing**: `02-demo-connected.png`, `04-expanded-row.png`, `03-event-inspector.png`, plus the existing report/mobile observations in the local ClickStack handoff. This adopts the inline expansion/cell-to-inspection workflow, **not** ClickStack's default broad navigation/facet columns or its backend. The public demo's deployed SHA is still unknown; pinned upstream reference is `2bad9af1723bd74ba5473a617f8688a0ac365282`. No ClickStack code/assets were copied.

## Measured space gains

Fresh browser contexts, same two real backend events/time window, light theme, identical viewports and default controls. Before captures are the actual `660b4e28` preview, saved before edits. Values are DOM bounding boxes in CSS pixels, not screenshot estimates. [Before measurements](assets/observability-ui/results-first/before.json) · [After measurements](assets/observability-ui/results-first/after.json).

| Viewport | Default results width, before → after | Default message column, before → after | First row/list top, before → after |
| --- | --- | --- | --- |
| Desktop 1600×1000 | 1063 → **1519** (+456px, 42.9%) | 624 → **1080** (+456px, 73.1%) | 387.5 → **339.5** (48px earlier) |
| Laptop 1366×900 | 829 → **1285** (+456px, 55.0%) | 390 → **846** (+456px, 116.9%) | 387.5 → **339.5** (48px earlier) |
| Mobile 390×844 | 390 → **390** (already full width) | 366 → **366** | 446 → **357** (89px earlier) |

With details selected, the old split reduced the desktop event-list viewport to **688.3px** and laptop to **536.3px**. They now remain **1519px / 1285px**, respectively. The old laptop's rendered 200px minimum message column could overflow its narrower pane; the new 846px message column fits. Selection no longer causes toolbar wrapping/column-width loss.

Desktop list height is now **577.5px**, versus 430px fixed before; laptop **477.5px**, versus 430px. Mobile list height is **395.5px**, versus the former 480px list that extended below the viewport. This is deliberately **not** claimed as a mobile height increase: rows start earlier and the whole 844px page, including footers, now fits without document overflow. Open facets/navigation are explicit space tradeoffs, not part of these default-state gains.

## Before / after: actual events

No fixture data is presented as a product screenshot.

|  | Before | After | Selected event before | Selected event after |
| --- | --- | --- | --- | --- |
| Desktop | [Before](assets/observability-ui/results-first/before-desktop.png) | [After](assets/observability-ui/results-first/after-desktop.png) | [Split](assets/observability-ui/results-first/before-desktop-selected.png) | [Inline](assets/observability-ui/results-first/after-desktop-selected.png) |
| Laptop | [Before](assets/observability-ui/results-first/before-laptop.png) | [After](assets/observability-ui/results-first/after-laptop.png) | [Split](assets/observability-ui/results-first/before-laptop-selected.png) | [Inline](assets/observability-ui/results-first/after-laptop-selected.png) |
| Mobile | [Before](assets/observability-ui/results-first/before-mobile.png) | [After](assets/observability-ui/results-first/after-mobile.png) | [Before dialog](assets/observability-ui/results-first/before-mobile-selected.png) | [After dialog](assets/observability-ui/results-first/after-mobile-selected.png) |

Opt-in states: [desktop facets](assets/observability-ui/results-first/after-laptop-facets.png), [mobile facets with actual service filter](assets/observability-ui/results-first/after-mobile-facets.png), [mobile navigation](assets/observability-ui/results-first/after-mobile-navigation.png), [dark explorer](assets/observability-ui/results-first/05-real-dark.png).

## Self-verification and fixes found

App browser checks ran against the exact production static build on :19990; the graph/serializer harnesses exercise the same source separately. [Build SHA256 manifest](assets/observability-ui/results-first/artifact-manifest.json) identifies the served files; source/artifact commit is the commit containing this document (`git log -1 --format=%H -- docs/logs-results-first.md`).

- Typecheck, static production build, targeted project oxlint/formatter checks and serializer tests passed.
- **Real read-only backend:** 1600×1000, 1366×900 and 390×844; two existing events; default/opt-in navigation and facets; counts and time chip; selected-row/list-width preservation; Close/Escape focus return; actual View trace → 82-span workspace → back; Traces/Logs navigation and mobile sheet closure. No runtime errors. [Flow results](assets/observability-ui/results-first/flow-results.json).
- **Real service queries:** include → URL → parsed state → actual outgoing SQL → actual two rows; reload; opt-in checkbox removal; active chip removal with facets closed; quote/backslash service produces the correctly quoted predicate and a real 200/empty result; back/forward. Query caching can avoid a fresh request when removing a chip, so that check also reloads and verifies the actual unfiltered SQL/rows. [Queries/results](assets/observability-ui/results-first/filter-roundtrip.json).
- **Keyboard/focus:** Arrow/Home/End/Enter on rows; tab-list Home/End and input Home are not swallowed by the enclosing virtual-list handler; mobile modal trap/Escape; explicit focus return after navigation/filter/detail dismissal and virtual-row remount. Mobile Find reveal/clear, View checkbox menu, copy, JSON and attributes, themes/reduced motion. Clipboard failure is visible **inside** the mobile modal rather than behind it. [Real browser checks](assets/observability-ui/results-first/real-results.json).
- **axe 4.14.0:** zero violations in the default full logs page at all three viewports; the opt-in mobile facets with time/service filters; scoped new explorer in light/dark; mobile inspector including visible copy-denial status; synthetic error/info rows in light/dark. This is a set of checked states, not a dashboard-wide audit or accessibility certification.
- **Presentation bugs exposed/fixed:** unique checkbox IDs per mounted filter instance (hidden desktop and mobile copies formerly shared IDs/label targets); reset action moved outside the accordion trigger (nested button); navigation landmark and avatar-initial contrast; invisible navigation tooltips no longer mount layers that can eat Escape; explicit mobile navigation focus-return anchor and route-change closure. These do not change filter values, query parsing, auth or transport.
- **Synthetic intercepted API:** unchanged 50+10 cursor append/stop/End behavior, plus retained selection and no scroll-back to an older selected first event when appending at the bottom. 5,000 rows and a 1,113,190-byte hostile message: **25 mounted rows** at start/end; full clipboard equality; zero payload execution, unexpected outbound requests, prototype pollution or runtime errors. Existing quoting checks pass. [Fixture results](assets/observability-ui/results-first/fixture-results.json).
- **Standalone same-source harness:** 1.4M-character message and 1.2M-character attribute; cyclic/BigInt/repeated-reference copies; collapsed 40,000-leaf fan-out; excessive nesting reports explicit serialization failure; rejected/unavailable clipboard visibly reports failure. This graph harness is not JSON API transport. [Graph results](assets/observability-ui/results-first/graph-results.json).
- **Existing Laminar regression rerun:** 82-span workspace search/tree/input-output/events/resource attributes/error/zoom/keyboard/resizing/deep links; 2,102-span trace capped at 2,000 and virtualized; invalid/empty traces; actual correlated log; profile trace/span/service/window navigation (legitimately no matching profile samples); mobile trace sheet; no runtime errors. [Trace results](assets/observability-ui/results-first/trace-results.json). Its separate existing local script uses the previously recorded real workload IDs; no server was seeded by this iteration.

### Cost observations, not performance promises

Final one-shot synthetic observation: **617ms** to rows, **148ms** to inspector, approximately **88.7MB observed JS heap**, and a maximum recorded main-thread task of **151ms**. Heap is an unforced, timing-sensitive browser observation, not a bound or a comparable improvement over the earlier slice. DOM remains bounded despite the deliberately oversized page; this is not zero-jank.

Three fresh contexts against the same backend and the original `516a2563` build (:19989) gave median **427.6ms baseline / 431.0ms current**. Requested JS/CSS including viewport prefetch costs **+192,056 raw bytes / +64,081 offline-gzip bytes** over that original base. Offline gzip is an estimate, not on-wire compression. This is not a bundle optimization; partial legacy/Base UI coexistence still has a cost. [Raw comparison](assets/observability-ui/results-first/bundle-comparison.json).

## Reproduce locally

From `dashboard/` with :19990 serving the current production build against the existing backend:

```sh
aube run typecheck
aube run build
aube run test:values
# Set CHROMIUM_PATH to a local Chrome binary, or install Playwright Chromium.
EVIDENCE_DIR=/tmp/sigiro-flow-evidence aube run test:flow
EVIDENCE_DIR=/tmp/sigiro-flow-evidence aube run test:browser
EVIDENCE_DIR=/tmp/sigiro-flow-evidence aube run test:graph
EVIDENCE_DIR=/tmp/sigiro-flow-evidence BASELINE_ORIGIN=http://127.0.0.1:19989 node tests/bundle-comparison.mjs
```

`UI_ORIGIN`, `LOG_WINDOW`, `CHROMIUM_PATH` and `EVIDENCE_DIR` are configurable. Real scripts expect the documented two `dashboard-e2e` logs and existing trace workload; they contain no credentials or data-creation commands. Other data needs updated assertions. Before measurements are preserved evidence, not regenerated by a current-build test. The pre-existing trace regression command was `node /tmp/sigiro-ui-browser/base-ui-trace-regression.mjs` with its local workload manifest; it is not a clean-checkout, data-independent test.

## Remaining boundary / explicit non-goals

- The shell and existing filter drawer/accordion/checkbox/date-picker/command controls still use legacy shadcn/Radix/Vaul/cmdk implementations. This iteration changes their layout/defaults and targeted accessibility defects; it does **not** complete the migration. No competing dialog primitives were wrapped around the legacy date picker.
- Laminar trace presentation has not been extracted into the MIT directory. Its resizable timeline/inspector and specialized profile/chart engines remain intact. No coordinated token export or #168 integration is claimed.
- Inline details spend vertical space, are bounded and can scroll; they can leave the virtual viewport when the operator scrolls away. They are not a permanently pinned inspector. Facets opt-in can be persisted; no claim that every returning user will always see them closed after previously opting in.
- Existing service exclusion limitations and JSON-safe (not lossless graph) serialization bounds remain as documented in the first slice. The real local events have empty attribute objects; rich/deep/malicious attribute coverage is explicitly synthetic.
- Rejected console worktree/preview, PR #168 and ports 19993–19997 were preserved. No push/PR/merge/deployment/publication was performed.
