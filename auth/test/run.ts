const root = new URL("..", import.meta.url).pathname;

process.env.NO_PROXY = "127.0.0.1,localhost";
process.env.no_proxy = process.env.NO_PROXY;

const required = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
};

const compose = Bun.spawn(["docker", "compose", "up", "-d", "--wait"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
if ((await compose.exited) !== 0) process.exit(1);

const migrate = Bun.spawn(["bun", "src/migrate.ts"], {
  cwd: root,
  env: process.env,
  stdout: "inherit",
  stderr: "inherit",
});
if ((await migrate.exited) !== 0) process.exit(1);

const server = Bun.spawn(
  [
    "bunx",
    "wrangler",
    "dev",
    "--local",
    "--ip",
    "127.0.0.1",
    "--port",
    "3100",
    "--local-upstream",
    "127.0.0.1",
    "--var",
    "ENVIRONMENT:test",
    "--var",
    `BETTER_AUTH_SECRET:${required("BETTER_AUTH_SECRET")}`,
    "--var",
    `AUTH_TEST_OTP:${required("AUTH_TEST_OTP")}`,
    "--var",
    `BETTER_AUTH_URL:${required("BETTER_AUTH_URL")}`,
    "--var",
    `SIGIRO_RESOURCE:${required("SIGIRO_RESOURCE")}`,
    "--var",
    `EMAIL_FROM:${process.env.EMAIL_FROM ?? "noreply@sigiro.com"}`,
  ],
  {
    cwd: root,
    env: {
      ...process.env,
      CLOUDFLARE_HYPERDRIVE_LOCAL_CONNECTION_STRING_HYPERDRIVE: required("DATABASE_URL"),
    },
    stdout: "inherit",
    stderr: "inherit",
  },
);

const stopServer = () => {
  server.kill();
};
process.on("SIGINT", stopServer);
process.on("SIGTERM", stopServer);

let ready = false;
for (let attempt = 0; attempt < 100; attempt += 1) {
  if (server.exitCode !== null) break;
  try {
    const response = await fetch("http://127.0.0.1:3100/health");
    if (response.ok) {
      ready = true;
      break;
    }
  } catch {
    // Wrangler has not started listening yet.
  }
  await Bun.sleep(100);
}
if (!ready) {
  stopServer();
  await server.exited;
  throw new Error("Worker did not become ready");
}

try {
  const tests = Bun.spawn(["bun", "test/e2e.ts"], {
    cwd: root,
    env: process.env,
    stdout: "inherit",
    stderr: "inherit",
  });
  process.exitCode = await tests.exited;
} finally {
  stopServer();
  await server.exited;
  await Bun.$`docker compose down`.cwd(root).quiet();
}
