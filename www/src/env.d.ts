declare namespace Cloudflare {
  interface Env {
    API_ORIGIN_URL: string;
    // Production: a service binding to the auth worker (see alchemy.run.ts).
    AUTH: Fetcher;
  }
}

interface Env {
  API_ORIGIN_URL: string;
  AUTH: Fetcher;
}

// worker-edge.ts is the preview/local variant. check-edge.ts boots auth as a
// plain process on a port, so there is no binding to hand it and the proxy
// still takes a URL.
interface EdgeEnv {
  API_ORIGIN_URL: string;
  AUTH_ORIGIN_URL: string;
}
