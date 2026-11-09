# Observability UI

Reusable React presentation components, MIT licensed under `LICENSE` in this directory. The Sigiro application outside this directory retains its existing license.

- Base UI 1.9.0 supplies interaction primitives. CSS Modules use the host’s shadcn semantic color tokens.
- TanStack Virtual measures event rows; react-resizable-panels supplies the desktop inspector separator. These engines are not Base UI components.
- `EventExplorer` accepts typed events and controlled selection, optional pagination callbacks, filter callbacks and correlation-link renderers. `AttributeTree` accepts values and a copy callback.
- No Next router, OAuth, SQL generation, telemetry transport, tenant context or API calls. Hosts own pagination, filtering semantics, matching counts and URLs.
- `onFilter` currently exposes service inclusion only. No exclude control is shown because Sigiro’s current transport does not implement exclusions.
- Find searches loaded rows. Row previews are limited to 500 characters; inspector message/event-JSON previews to 50,000. Attribute trees display up to 200 keys per object and descend eight levels; leaf previews are limited to 10,000 characters. Copy actions retain complete values, independently of displayed preview budgets.
- JSON copies represent BigInt as decimal strings and true circular references as `[Circular]`; repeated non-cyclic references are serialized normally. This is a JSON-safe representation, **not a lossless JavaScript graph encoding**. Clipboard rejection/unavailability is announced explicitly; it never reports an unsuccessful copy as success.
- New production dependency notices are served from `dashboard/public/licenses/base-ui-MIT.txt`, `base-ui-utils-MIT.txt` and `babel-runtime-MIT.txt`. Existing Laminar Apache license/NOTICE files remain unchanged.

Visual workflow references: ClickHouse/ClickStack at `2bad9af1723bd74ba5473a617f8688a0ac365282` (MIT, DeploySentinel), Laminar trace workspace and telemetry.dev hierarchy. This implementation is newly authored; no ClickStack source or assets have been copied here. Existing Laminar-derived code elsewhere retains its Apache-2.0 notices.

These files are an extraction boundary within Sigiro, not a published npm package or separate repository. Runtime dependencies have their own licenses. Accessibility/security/performance checks and limitations must be recorded before release; library choice alone is not a certification.
