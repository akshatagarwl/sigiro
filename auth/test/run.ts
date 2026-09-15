import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));

process.env.NO_PROXY = "127.0.0.1,localhost";
process.env.no_proxy = process.env.NO_PROXY;

const required = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
};

const run = (command: string, args: string[], env = process.env): Promise<number> =>
  new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: root, env, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code) => resolve(code ?? 1));
  });

if ((await run("docker", ["compose", "up", "-d", "--wait"])) !== 0) process.exit(1);
if ((await run("node", ["--import", "tsx", "src/migrate.ts"], { ...process.env, ENVIRONMENT: "test" })) !== 0)
  process.exit(1);

const server = spawn("node", ["--import", "tsx", "src/server.ts"], {
  cwd: root,
  env: {
    ...process.env,
    ENVIRONMENT: "test",
    PORT: "3100",
    BETTER_AUTH_SECRET: required("BETTER_AUTH_SECRET"),
    AUTH_TEST_OTP: required("AUTH_TEST_OTP"),
    BETTER_AUTH_URL: required("BETTER_AUTH_URL"),
    SIGIRO_RESOURCE: required("SIGIRO_RESOURCE"),
    DATABASE_URL: required("DATABASE_URL"),
    EMAIL_FROM: process.env.EMAIL_FROM ?? "noreply@sigiro.com",
    CLOUDFLARE_ACCOUNT_ID: "test-account",
    CLOUDFLARE_EMAIL_API_TOKEN: "test-token",
  },
  stdio: "inherit",
});

const stopServer = () => server.kill("SIGTERM");
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
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}
if (!ready) {
  stopServer();
  throw new Error("auth service did not become ready");
}

try {
  process.exitCode = await run("node", ["--import", "tsx", "test/e2e.ts"]);
} finally {
  stopServer();
  await new Promise((resolve) => server.once("exit", resolve));
  await run("docker", ["compose", "down"]);
}
