---
"sigiro": patch
---

Add `SIGIRO_BIND` to choose the listener address outside quack mode. Android defaults to `127.0.0.1`; other platforms still bind every interface when unset. The Android build script uses its own target directory.
