---
"sigiro": patch
---

Add `SIGIRO_BIND` to choose the listener address for the HTTP, OTLP/HTTP and OTLP/gRPC listeners outside quack mode. When unset, open mode binds `127.0.0.1` and hosted mode binds every interface. Containers that publish ports need `SIGIRO_BIND=0.0.0.0`, as the quickstart shows.
