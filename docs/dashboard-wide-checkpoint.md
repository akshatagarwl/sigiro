# Dashboard verification checkpoints

Historical command names are preserved below. For current Nix/pnpm setup and
commands, see [Contributing](../CONTRIBUTING.md).

The six-route results below describe a historical build, not the current route
count or a promise that its preview is still running. `main` now has five routes
after Services was removed. Later entry/first-run observations and pending fixes
are recorded in the [follow-up](#entry-and-first-run-follow-up), without replacing
or relabeling the original artifact hashes and results.

## Exact local checkpoint

- UI source commit: **`3aff25cca105d33dbf611f86366a5421b53851e5`**, based on completed Logs checkpoint `c92d64ee69ade67e99a7d9bc7a4c7c299b6441fc`.
- Preview: **http://127.0.0.1:19990/dashboard/**. It serves the new dashboard-wide production export, not the old Logs-only checkpoint.
- Export tree SHA-256: **`33043839437ab3c88a6f38111a6c374519095547b1ab6a7c2d72081475b016f0`**. Every one of its **93 files** was fetched from :19990 and matched the [file manifest](assets/observability-ui/dashboard-wide/artifact-manifest.json); [HTTP verification](assets/observability-ui/dashboard-wide/served-build-verification.json).
- Source tree SHA-256: **`cad03106bf2f22568f02dba690103c7f472a88cb361b56cabbefdfe198c5f6ae`**. `node dashboard/tests/artifact-manifest.mjs /tmp/current-artifact.json` regenerates both digests. Source scope and digest algorithm are in the manifest. The documentation/evidence commit following the UI commit does not change this source tree or export.
- Solo `gpt-6.1-sol`, medium; local self-verification only, not independent review or CI. No agents, push, PR, merge, deployment, publication, infrastructure/backend/auth/tenant change. Other worktrees and :19989/:19991–19997 were not modified by this implementation. The existing local backend/workload was read, not reseeded.

## What changed and what is complete

The [route/state inventory](dashboard-state-machines.md) was written before broad edits. All **six actual routes** have bounded implementation and verification gates; there is no invented Metrics route.

| Route    | Completed bounded changes                                                                                                                                                                                                                                                             | Verification boundary                                                                                                                                                                             |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Overview | Shared compact shell; explicit panel loading/updating/partial failure/retry and service-scoped empty state; unavailable values are not silently zero                                                                                                                                  | Real local backend charts/KPIs/empty service/navigation; injected KPI failure then retry to real data                                                                                             |
| Services | Compact results-first table, own preferences namespace, loaded/matching counts, stable service identity, current detail payload and focus return; error/retry/empty/updating                                                                                                          | Real aggregates/empty; labeled two-service fixture for client filter/sort, live detail refresh and preference isolation                                                                           |
| Traces   | Dynamic service strings reach SQL; relative Refresh moves both effective bounds; explicit windows remain fixed; compact table; detail loading/focus return; related-profile failure/retry does not hide workspace                                                                     | Actual known/missing/quoted service URL/reload and checkbox SQL, row keyboard; injected list/detail/profile failures and delay; existing real 82/2,102-span workload regression                   |
| Logs     | Completed full-width inline/mobile design retained; shared navigation/time fixes; loading/failure no longer presented as successful empty/end-of-results                                                                                                                              | Existing real flow/filter/keyboard/theme/focus checks plus labeled hostile/pagination fixtures and standalone graph/value tests                                                                   |
| Profiles | Explicit service/type preserved without fallback; dependent queries gated to current discovery; no cross-scope placeholders; true errors/loading/compare state; clearable invalid custom time/correlation; atomic service/type changes, reset zoom and keyboard function highlighting | Actual no-profiles setup/deep links only. Populated stacks, labels, two services/types, baseline compare, failures/delay/history and desktop/mobile graph/table behavior use intercepted fixtures |
| SQL      | Single-flight Run/shortcut, Cancel and unmount abort; late-response identity guard; idle/running/error/empty/result; executed SQL disclosure distinct from edited draft; independent time scope and compact isolated results                                                          | Actual read-only SELECT success/empty/error; injected delayed requests for repeated shortcut/cancel/rerun/navigation; changed schema and preferences                                              |

Shared navigation preserves scope only where its representation is compatible: single-service Overview/Services/Profiles and multi-service-capable Logs/Traces are separate groups. Logs↔Traces also preserve explicit `timestamp`. Same-route links retain state. Relative header range selection atomically removes custom explorer/profile time overrides. SQL accurately states that SQL defines its own time. Cross-group service filters and profile/explorer custom windows are not silently converted into one another.

New reusable `RequestState` is Base UI-backed and MIT-scoped under `dashboard/components/observability/`, with no router/query/auth/tenant dependency. SQL uses Base UI Button/Collapsible. Existing Radix/Vaul/cmdk filters, tabs and overlays remain a **partial migration**; specialized Laminar trace/chart/flamegraph engines are retained, not described as Base UI widgets. No ClickStack source/assets were copied; licensing boundaries and notices are unchanged.

## Verification and durable evidence

All commands ran locally from `dashboard/`, against the production export on :19990. Browser commands used installed Chrome through Playwright.

```sh
aube run typecheck
aube run build
aube run test:values
aube run test:dashboard
aube run test:flow
aube run test:browser
aube run test:graph
BASELINE_ORIGIN=http://127.0.0.1:19989 node tests/bundle-comparison.mjs
node /tmp/sigiro-ui-browser/base-ui-trace-regression.mjs
```

Also passed: targeted oxlint, formatter checks on new/recent files, and `git diff --check`. The trace script is the existing local harness, not a newly packaged CI command. Its [results and screenshots](assets/observability-ui/dashboard-wide/trace-regression/browser-results.json) are retained here.

- [Dashboard results](assets/observability-ui/dashboard-wide/dashboard-results.json): **31 passing checks**, including 18 real route/viewport checks at **1600×1000**, **1366×900** and **390×844**; no document overflow or captured runtime errors. Eighteen current real screenshots, two populated Profiles fixtures and one Services-preferences fixture are in the same directory. Real means the existing local backend, not production customer data.
- **20 scoped axe runs**, zero recorded violations: six route defaults at three sizes, plus populated Profiles at desktop/mobile. Scope is stated per result; this is not exhaustive accessibility verification. Existing Logs light/dark/dialog checks also pass.
- Actual Refresh probes inspect the **first two SQL bounds**, not whole-string inequality. Logs moved from `[1794174664442000,1794261064442000]` to `[1794174666062000,1794261066062000]`; Traces also moved both bounds. Explicit windows remain fixed. See current run values in the results JSON; these quoted values are one passing run, not constants asserted by tests.
- [Logs flow results](assets/observability-ui/dashboard-wide/regression/flow-results.json), [real interactions](assets/observability-ui/dashboard-wide/regression/real-results.json), [filter SQL roundtrip](assets/observability-ui/dashboard-wide/regression/filter-roundtrip.json), and [layout measurements](assets/observability-ui/dashboard-wide/regression/after.json) preserve prior gains: desktop **1519px results / 1080px message**, laptop **1285 / 846**; mobile **390 / 366**, first list row **357px**, list **395.5px**. Mobile first-row position/list space now have explicit regression assertions. Details do not narrow desktop results.
- [Hostile fixture](assets/observability-ui/dashboard-wide/regression/fixture-results.json): 5,000 rows, 1,113,190-byte message, 25 mounted event rows, full copy and no recorded execution/unexpected requests/pollution/runtime errors. Latest local run: ready **954ms**, inspector **202ms**, maximum observed long task **223ms**, sampled heap **66,287,986 bytes**. These are observations, not performance guarantees.
- [Standalone graph results](assets/observability-ui/dashboard-wide/regression/graph-results.json) and value tests remain distinct from API fixtures: native cycles/BigInt/repeated references, bounded fan-out and serialization-depth/clipboard failure behavior cannot all be represented by JSON API responses. The graph harness passed with an in-memory bundle and a temporary server that closes afterward.
- [Trace results](assets/observability-ui/dashboard-wide/trace-regression/browser-results.json): **18 checks**, no runtime errors; real existing workspace/tree/search/span tabs/error-only/zoom/keyboard/resize/deep-link/log/profile/mobile/empty/invalid and capped large-workload flows. No injected populated profiles are claimed by that suite.
- [Bundle comparison](assets/observability-ui/dashboard-wide/regression/bundle-comparison.json): compared with original `516a2563d5e905746293ef9263d7de4100e92134` preview on :19989, not directly with the Logs checkpoint. Three cold-context median real-row readiness **433.6ms baseline / 459.8ms current**; **+204,564 raw JS/CSS / +69,932 offline-gzip bytes**, including viewport route prefetch. Offline gzip is not measured on-wire compression. No speedup claim.

## Explicitly pending / not verified

No implementation blocker remains for the bounded checkpoint. These are real remaining limits, not completed gates:

1. **Real populated Profiles**: no samples exist in the inspected local workload/window. Populated flamegraph/diff correctness against actual profile ingest and production datasets remains unverified; fixtures are labeled and not persisted to the backend.
2. **Timestamp-tie pagination**: strict timestamp-only pagination can omit tied rows at a boundary. A composite cursor needs a server/query contract decision; not silently fixed in UI. Synthetic 50+10 pagination tests do not prove tie correctness.
3. **Complete Base UI migration/extraction**: legacy primitives remain; no public package/repository was published. Existing specialized engines and application styling remain.
4. **Exhaustive state/quality matrix**: not all chart-legend permutations, calendar keyboard/date/DST/locale combinations, live reconnects/offline windows, browser engines, viewport sizes, long-duration sessions, profile stack shapes, cross-group scope conversions or every overlay/history combination were tested. SQL arbitrary writes/streaming and server cancellation are not claimed; client AbortController does not establish server execution cancellation.
5. **Backend/auth/tenancy/security/performance audits**: no changed contracts or new guarantees. No production verification, load test, independent review, CI run or full accessibility/security audit was performed. Public ClickStack demo deployed SHA remains unknown.

The rejected log-viewer worktree/preview, unmerged PR #168, existing license boundaries, and unrelated work remain preserved. Completion here means the six-route bounded UI checkpoint and recorded gates—not every dashboard issue or implicit state machine.

## Entry and first-run follow-up

### Scope and provenance

The follow-up describes local dashboard entry and the empty Overview, not all
telemetry workflows. Source was read at `239cf44c3`; supplementary browser checks
used `feat/dashboard-ui-sdk` at that base **plus uncommitted UI/auth changes**.
Those changes are not included in this documentation update to `main`. In
particular, the worktree's Local Sigiro menu, explicit authentication bootstrap,
credential-free queries, and connection-retry behavior must not be presented as
already available on `main`.

The [current state inventory](dashboard-state-machines.md#current-entry-and-first-run-behavior)
describes actual `main` behavior, including its older local-detection fallback.
The correct local exporter workflow is in the existing
[quickstart](../www/docs/tutorials/quickstart.mdx); the
[hosted guide](../www/docs/how-to/hosted-onboarding.md) is explicitly a different
distribution/access path, not a local prerequisite.

On 2026-11-16, the worktree's existing synthetic auth regression passed four
configuration-failure/retry cases and two OAuth compatibility/error boundaries.
Its real-backend regression visited all five routes, with 18 queries, no identity
requests, no API HTTP failures, and no page errors. A supplementary probe passed
21 assertions/observations: real local menu keyboard input, destination 404s,
mobile geometry, and one targeted axe rule; isolated mocked scenarios covered
query failure, invalid-time recovery availability, and custom-window title copy.
These are **local scripted observations of the worktree**, not tests of this
new `main` commit, a production deployment, or a complete manual pass. Their local
logs/screenshots are not added to or represented by the historical artifact set
above. The bounded findings below retain source causes and reproduction checks;
repeat them against the intended artifact before calling them release-verified.

### Consolidated first-run triage

IDs are stable within this follow-up; the older D1–D8 inventory remains unchanged.
Reproduce copy problems by reading the commands, never by running them against
Cloud or using customer telemetry.

| ID | Current mismatch | Source cause | Severity / decision | Reproduction and evidence boundary |
| --- | --- | --- | --- | --- |
| ENTRY-01 | Local setup directs exports to Cloud and asks for hosted login/token; says gRPC is unavailable. Expected: accurate local destination/transports without an account. | `components/sigiro/setup-empty.tsx:15,23–64` fixes the endpoint/guide to hosted values; `app/page.tsx` passes only a range label. | High / fix: deployment-aware ingestion addresses and transport guidance, not ports guessed from a frontend URL. | Empty local Overview → read snippet/prose. Reproduced on worktree with real backend; same fixed copy is present on `main`. No exporter was run. |
| ENTRY-02 | Both documentation escape routes return 404 in the frontend-only preview. Expected: usable local help. | Preview has no docs route; links target `/docs` and `/docs/how-to/hosted-onboarding/`. The binary separately embeds documentation. | Medium / fix: verify destination strategy in preview and packaged serving; do not claim binary docs are absent. | Follow both links in the actual preview; worktree recorded HTTP 404 for both. Packaged link behavior remains untested. |
| ENTRY-03 | Empty mobile Overview hides its useful next action below five summary cards. | `app/page.tsx` renders `SectionCards` before Setup; `components/section-cards.tsx` stacks cards on narrow screens. | Medium / product call: prioritize Setup, collapse empty summaries, or choose another first-run hierarchy. | At 390×844, inspect before scrolling. Worktree setup top y=976 and snippet y=1185.25, with a 214px header. Run-specific geometry, not `main` layout constants. |
| ENTRY-04 | Overflowing setup commands lack a keyboard-focusable scroll region. | `components/sigiro/setup-empty.tsx:37–42` has horizontal overflow without a tab stop/focusable child or Copy action. | Medium / fix: keyboard-readable, named command region; consider copying as an additional affordance. | At mobile width run axe `scrollable-region-focusable`, then manually read/scroll/copy by keyboard. Worktree: serious axe finding, 308px visible versus 580px scrollable; manual access still pending. |
| ENTRY-05 | Narrow horizontal navigation hides SQL without an explicit overflow cue. | Worktree `components/sigiro/app-shell.tsx` uses a horizontally overflowing five-link strip. | Low / product call: visible overflow cue or another accessible navigation layout. | Worktree only: at 320px, 288px visible versus 346px scrollable. Current `main` uses a sidebar; do not attribute this strip defect to that implementation. |
| ENTRY-06 | Empty-state title can say last 24 hours for a custom one-hour interval. | `app/page.tsx` labels Setup with `RANGES[controls.range]` while custom endpoints can determine the actual query window. | Medium / fix: describe the resolved window, not the leftover preset. | Open `/dashboard/?timeStart=now-1h&timeEnd=now` with empty results. Reproduced using worktree mock-empty responses; `main` shares the labeling expression but needs a fresh browser check. |

### Remaining verification gates

| Gate | Action and expected result | Status |
| --- | --- | --- |
| FIRST-01 | With the actual `main` artifact and local server, distinguish failed backend detection from a true hosted requirement. Verify no account is needed for the backend. | Source described; updated explicit entry/retry behavior exists only in the uncommitted worktree. |
| FIRST-02 | Confirm loading/updating/error never become successful empty; selected-service emptiness offers scope recovery. Retry must repeat the appropriate connection or data read. | Query-failure separation observed with worktree mocks; full current-artifact interaction pass pending. |
| FIRST-03 | After an approved deployment-aware fix, export non-sensitive test data only to the local ingestion endpoint, then refresh until it appears in scope. | Not run; no exporter/persistent telemetry mutation in this follow-up. |
| FIRST-04 | Click both help paths in the real preview and a packaged binary; inspect mobile/keyboard command access and screen-reader speech. | Preview 404 and worktree targeted axe reproduced; packaged, physical touch, and screen-reader checks pending. |
| FIRST-05 | Exercise Escape, Back/Forward, reload, close, navigation away, network failure, session boundaries, another tab, data arrival, and blur before/during interaction. Record exact URL/focus/scroll behavior. | Inventory recorded; not an exhaustive manual pass. |
| FIRST-06 | Verify custom-window title against query bounds, not just a preset control label; recheck narrow navigation against the implementation actually shipping. | Mock title mismatch and worktree navigation geometry reproduced; current-artifact checks pending. |

Metrics/profiles-only data and empty historical windows raise product questions:
should Setup be suppressed or should scope recovery take priority? They are not
asserted data-loss defects. No upstream issues, deployments, Publik integration,
license changes, or authentication-policy changes are part of this documentation
update. The older all-green axe results above apply only to their named build and
states; they do not override a later finding on a different worktree.
