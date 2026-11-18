# Native production systemd contract

These units are templates for the adopted Ubuntu host. They deliberately use
loopback application listeners; only kamal-proxy owns ports 80 and 443.

`deploy-native-host.sh` installs them, reloads systemd, and restarts them.
Secrets are supplied through
root-owned files under `/etc/sigiro/secrets` and are never embedded in units.

LinguaBench web and worker load the versioned `profiler.conf` drop-in after their
preserved Docker-era environment files. It sets Pyroscope to the loopback
`sigiro-ingest-relay` (not the obsolete `api.sigiro.com` hostname). The relay
accepts only bounded, authenticated `/ingest` uploads, exchanges its SOPS
client credentials for a short-lived `telemetry:ingest` token, and retries
profile admission 429 at most twice. The web and worker receive only the
existing relay key. Verify delivery by checking fresh `sigiro_profiles` rows
for `worldpol-api` and `worldpol-pipeline-worker`; HTTP 200 alone is not proof
of persistence.

## Contract

- PostgreSQL is PGDG PostgreSQL 18 at `/var/lib/postgresql/18/main`.
- Fresh Sigiro telemetry is `/var/lib/sigiro/telemetry`; extensions are cached
  in `/var/cache/sigiro/duckdb`.
- Better Auth is NOT hosted here. It runs as a Cloudflare Worker on D1 behind
  `sigiro.com/api/auth` and the website reaches it through a service binding, so
  there is no `sigiro-auth` unit, no `cloudflared` tunnel unit, and no
  `auth-origin.sigiro.com` origin on this host.
- Releases are immutable directories under `/opt/<service>/releases/<digest>`;
  `current` is the only mutable pointer.
- `kamal-proxy` is the native v0.9.2 binary built at commit
  `c195e5bfd056ba311a391bba30e9dbf4689be2ec` with Go 1.25.5.
- The host runs no container engine; `native-host.sh verify` fails if one is
  installed.
