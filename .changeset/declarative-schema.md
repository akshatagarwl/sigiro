---
"sigiro": minor
---

The schema is now declared in one place with no migrations. A data directory written by an older schema is rejected with an error asking you to delete it, instead of being upgraded.
