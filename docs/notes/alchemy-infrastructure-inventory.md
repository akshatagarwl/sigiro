# Alchemy infrastructure inventory

This inventory is the first migration slice for [issue #222](https://github.com/akshatagarwl/sigiro/issues/222). It records committed infrastructure and deployment surfaces at `f679bde8c5ecea42a2772aeb71ac0aed0a0dad77`. It does not claim that untracked dashboard resources are absent.

Disposition has three values:

- **Alchemy**: migrate to a maintained first-party Alchemy resource or runtime.
- **Retained external tool**: keep the named authoritative tool; do not wrap it in Alchemy.
- **Deleted**: remove after its replacement has been adopted and verified.

## Inventory

| Surface | Current owner | Disposition | Reason and cutover boundary |
| --- | --- | --- | --- |
| `sigiro-www` Worker, Astro server entrypoint, and static assets | `www/wrangler.jsonc`, generated `www/dist/server/wrangler.json`, `www/package.json` | **Alchemy** | Cloudflare Worker and asset resources are maintained first-party resources. The site needs Worker-first execution for content negotiation and `/dl/*`; Alchemy's current Astro guide explicitly does not verify Astro SSR, so the migration must use the maintained Worker/assets primitives unless SSR support becomes documented before implementation. Adopt the existing Worker; do not recreate it. |
| Routes `sigiro.com/*` and `www.sigiro.com/*`; `workers.dev` disabled | `www/wrangler.jsonc` | **Alchemy** | `Cloudflare.Workers.WorkerRoute` supports path patterns and explicit adoption. Keep route semantics rather than switching to a custom domain: the checked-in config records that existing DNS makes custom-domain ownership unsuitable. Adopt both routes and retain the zone on destroy. |
| Existing private `sigiro-dl` bucket | `www/wrangler.jsonc`, `www/src/dl.ts`, `.github/workflows/release.yml`, `deploy/nightly_release.py` | **Alchemy** | R2 buckets and scoped Worker bindings are maintained resources. Adopt the existing bucket with retention, and bind only read access to the website. Production adoption must be a separately reviewed human operation because replacement or deletion would lose release data. |
| Versioned release objects, `SHA256SUMS`, staging prefix, and `LATEST` pointer | `.github/workflows/release.yml`, `deploy/nightly_release.py`, `deploy/NIGHTLY-RELEASE.md` | **Alchemy** | Use the maintained R2 write capability for publication. Preserve the invariant already enforced by both publishers: upload and read back immutable artifacts and checksums before writing `LATEST` last. This cutover changes publication only; it does not absorb builds. |
| Cloudflare-backed state | no current equivalent | **Alchemy** | Use `Cloudflare.state()` so state is not laptop-local. The authoritative scripts fix Alchemy's required internal state key to production; callers cannot select an environment. All managed resources use explicit adoption and retention. |
| Cloudflare CI token, account configuration, and GitHub Actions secrets | manual credentials consumed by Wrangler and release workflows | **Alchemy** | Alchemy has maintained Cloudflare API-token and GitHub secret resources. Provision a least-privilege production token from a separately approved admin stack. A human performs the first privileged deploy and verifies names without reading secret values. |
| Website and deployment telemetry | Cloudflare Web Analytics literal in `www/src/lib/site.shared.ts`; release receipts in `deploy/nightly_release.py` | **Alchemy** for stack/Worker telemetry; **retained source** for the public Web Analytics token | Alchemy and Effect telemetry belongs in the website stack when a maintained exporter can send directly to Sigiro OTLP. Do not route through the documented Axiom integration or add a second backend. The existing browser beacon identifier is intentionally public application configuration, not a secret or infrastructure credential. |
| Live website, asset, redirect/header, and `/dl/*` checks | website probes in `deploy/nightly_release.py` | **Alchemy** | Run post-deploy assertions against production without creating or tearing down a separate environment. Preserve the existing rollback boundary for failed production verification. |
| Wrangler deployment scripts, checked-in config, dependency, and Wrangler-specific generated types | `www/package.json`, `www/wrangler.jsonc`, `www/worker-configuration.d.ts`, Wrangler helpers in `deploy/nightly_release.py` | **Deleted** | Remove these only after Alchemy owns the Worker, routes, assets, R2 binding, rollback/readback, and operator path. Keeping both deploy paths would violate the one-path cutover. Generate runtime binding types from the final Alchemy declaration instead of retaining a Wrangler shim. |
| Old `sigiro-docs` Worker and its `docs.sigiro.com` route/domain | historical state documented in `www/wrangler.jsonc` | **Deleted** | The current site intentionally replaced it with one Worker. Inventory it read-only before deletion, then remove it in a dedicated reviewed operation after redirect behavior has an explicit owner. It must not be adopted into the new primary site by accident. |
| Rust compilation, checks, packaging, and cross-compilation | `mise.toml`, `scripts/prepare-duckdb-static.sh`, `.github/workflows/release.yml` | **Retained external tool** | `mise` and `mbx` remain the authoritative gate/build path. Alchemy must consume verified artifacts, not wrap or duplicate these jobs. |
| OCI image build and GHCR publication | `docker-bake.hcl`, `mise.toml`, `deploy/nightly_release.py` | **Retained external tool** | Docker Buildx remains authoritative as required by #222. Alchemy must not introduce a second image builder or registry wrapper. |
| Hosted API deployment, PostgreSQL configuration, Docker network, and server secrets | `www/alchemy.workload.run.ts`, `ops/prod/compose.yaml`, `ops/prod/secrets.sops.json`, `deploy/README.md` | **Alchemy** | Alchemy owns the remote Docker context, dedicated workload network, and the only deployment command. Compose is executed as an Alchemy command resource to preserve hardening options absent from the pinned native container provider. Existing named volumes remain external and are never deleted. |
| Nightly workload cutover and API health | `deploy/nightly_release.py` | **Alchemy** | The deterministic driver passes the verified amd64 digest through SOPS to the Alchemy workload stack, then reads back the immutable live image and health. It no longer writes deployment commits or invokes Kamal. |
| Local development services | `www/alchemy.dev.run.ts` | **Migrated** | Postgres, RustFS, the bucket, the network and both volumes are Alchemy `Docker.*` resources; `docker-compose.dev.yml` is deleted. The storage-mode matrix runs against this stack. |
| E2E and test fixtures | `tests/e2e/docker/`, `tests/e2e/k8s/`, `tests/sdk/`, `dev/lima/` | **Retained external tool** | Kubernetes fixtures and Lima are test infrastructure explicitly outside the migration. Do not model them as Alchemy Docker resources. |
| Customer-deployable agent and coordinator | `agents/deploy/docker-compose.yml`, `agents/deploy/start-agent.sh`, `agents/coordinator/wrangler.jsonc` | **Retained external tool** | This is customer-owned Docker Compose/celld lifecycle, not Sigiro's hosted infrastructure. Although Worker and Durable Object resources exist in Alchemy, moving this boundary would change the customer deployment product and is not part of #222. |
| IndexNow submission | `www/scripts/indexnow.ts`, `www/package.json` | **Retained external tool** | This is an explicit search-engine notification after publication, not infrastructure ownership. Keep it independent; do not hide it in an Alchemy provider or deployment side effect. |

## Dependency order

Each numbered step is one reviewable slice. A later step must not start until the earlier production boundary has been verified or explicitly waived.

1. **Inventory (this document).** Verify source ownership and record the cutover order without changing infrastructure.
2. **Pin and scaffold.** Pin one exact Alchemy v2 beta in `www`, fix its required internal state key to production inside the authoritative scripts, use `Cloudflare.state()`, and add a safe `plan` command with no environment selector. Bootstrap of remote state is a human-approved operation, not part of the code-only slice.
3. **Read-only production declarations.** Declare the existing zone, `sigiro-dl`, `sigiro-www`, and two Worker routes with retain/adoption policies. Compare a non-mutating production plan with generated Wrangler configuration. Do not apply it yet.
4. **Human adoption.** An operator reviews and executes the first `prod` adoption, then reads back Worker identity, routes, bucket identity, binding access, state, and live website behavior. No resource may show replacement or deletion in the approved plan.
5. **Single website path.** Move website build/assets/runtime and the read-only R2 binding to Alchemy. Reproduce the current Worker-first paths, redirects, headers, custom routes, and disabled `workers.dev`; verify rollback before deleting Wrangler deploy/config/dependency/type-generation paths.
6. **CI credentials.** In a separate admin stack, declare the minimum Cloudflare token and GitHub secrets needed by production. A human performs and verifies the first privileged deploy; existing credentials remain until the new path succeeds.
7. **Release publication and workload.** Keep `mise`/`mbx`, Buildx, and verified artifact assembly while routing Cloudflare and hosted-workload writes through Alchemy. Sabotage-test that `LATEST` cannot advance before all versioned objects and checksums verify.
8. **Telemetry.** Enable deployment/resource telemetry only when the maintained Alchemy/Effect exporter can target Sigiro OTLP directly; otherwise leave this criterion blocked rather than introducing Axiom or custom provider code.
9. **Retire residue.** Remove superseded manual setup, stale secrets, Wrangler helpers, and the old `sigiro-docs` resources only after exact readback proves no active path depends on them.

## Safety invariants

- Callers use the authoritative Alchemy scripts without an environment selector; those scripts fix the tool's required internal state key to production.
- Production buckets, zone, routes, DNS records, and Workers default to retention.
- Remote state is shared by authorized operators and production workflows serialize updates to the stack.
- Secret values are redacted and are never read back into logs, plans, outputs, or comments.
- There is one website deployment path after cutover; there is no compatibility wrapper around Wrangler.
- There is one hosted-workload deployment path after cutover; no Ruby or Kamal CLI dependency remains.

## Documentation basis

API choices above were checked against the current Alchemy documentation on 2026-09-13: [adoption](https://alchemy.run/cli/adopting-resources), [Cloudflare Workers](https://alchemy.run/cloudflare/compute/workers), [Astro](https://alchemy.run/cloudflare/frontend/astro), [R2](https://alchemy.run/cloudflare/data/r2), [custom domains and routes](https://alchemy.run/cloudflare/networking/custom-domains), [stages](https://alchemy.run/environments/stages), [CI](https://alchemy.run/environments/ci), [state](https://alchemy.run/state-store), and [GitHub Actions configuration](https://alchemy.run/github/actions-config).
