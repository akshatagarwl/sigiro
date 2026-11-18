# Native production systemd contract

These units are templates for the adopted Ubuntu host. They deliberately use
loopback application listeners; only kamal-proxy owns ports 80 and 443.

`deploy-native-host.sh` installs them with `systemctl daemon-reload`, but does
not enable or start them until the cutover phase. Secrets are supplied through
root-owned files under `/etc/sigiro/secrets` and are never embedded in units.

## Contract

- PostgreSQL is PGDG PostgreSQL 18 at `/var/lib/postgresql/18/main`.
- Fresh Sigiro telemetry is `/var/lib/sigiro/telemetry`; extensions are cached
  in `/var/cache/sigiro/duckdb`. Legacy catalog, control, and telemetry data are
  not migrated.
- Better Auth is NOT hosted here. It runs as a Cloudflare Worker on D1 behind
  `sigiro.com/api/auth` and the website reaches it through a service binding, so
  there is no `sigiro-auth` unit, no `cloudflared` tunnel unit, and no
  `auth-origin.sigiro.com` origin on this host.
- Releases are immutable directories under `/opt/<service>/releases/<digest>`;
  `current` is the only mutable pointer.
- `kamal-proxy` is the native v0.9.2 binary built at commit
  `c195e5bfd056ba311a391bba30e9dbf4689be2ec` with Go 1.25.5.
- The old Docker workload and its volumes are retained untouched as rollback
  assets and are not part of the normal deployment path.
