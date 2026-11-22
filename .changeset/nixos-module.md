---
"sigiro": minor
---

The flake now provides a NixOS module, `nixosModules.default`, that runs `sigiro serve` as a hardened systemd service (`services.sigiro`). It binds loopback by default and keeps its data in `/var/lib/sigiro`. Options cover the listen address, the three ports, the firewall and any `SIGIRO_*` setting, and secrets can be loaded from an environment file.
