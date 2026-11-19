---
"sigiro": patch
---

Report the server's authentication mode through a public, non-cacheable bootstrap endpoint. Local dashboards bypass hosted sign-in and send no bearer tokens; unavailable backends display a connection error with retry instead of a sign-in prompt. Hide hosted account actions in local mode. Proxy development API requests to the local backend. Default open-mode listeners to loopback on all platforms; network exposure now requires an explicit bind address and perimeter protection.
