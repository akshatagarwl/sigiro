---
"sigiro": patch
---

Trace detail no longer hangs on traces whose spans were received more than once. Repeated copies of a span are dropped from the response, and the critical path is computed in linear time and stops at parent cycles.
