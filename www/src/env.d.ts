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
