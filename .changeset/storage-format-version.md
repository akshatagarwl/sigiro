---
"sigiro": minor
---

Each project's DuckLake catalog now records a storage format version. sigiro refuses a catalog written in another format, or one that holds telemetry but no version, before it accepts traffic, and the error names the data directory and how to reset it. Data directories written by earlier releases hold telemetry without a version, so move them aside (or delete them) before upgrading.
