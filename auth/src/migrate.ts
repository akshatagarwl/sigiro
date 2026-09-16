import { getMigrations } from "better-auth/db/migration";
import { Redacted } from "effect";
import { Pool } from "pg";

import { createAuthOptions } from "./auth.js";
import { makePgRuntime } from "./database.js";
import { migrateSecondaryStorage } from "./secondary-storage.js";

const required = (name: string): string => {
  const value = process.env[name];
  if (!value) throw new Error(`Missing environment variable: ${name}`);
  return value;
};

const environment = required("ENVIRONMENT");
if (environment !== "production" && environment !== "test") {
  throw new Error("ENVIRONMENT must be production or test");
}

const databaseUrl = required("DATABASE_URL");
const databaseSsl =
  environment === "production"
    ? {
        ca: Buffer.from(required("DATABASE_CA_CERT_B64"), "base64").toString("utf8"),
        rejectUnauthorized: true,
      }
    : undefined;
const betterAuthDatabase = new Pool({
  connectionString: databaseUrl,
  application_name: "sigiro-auth-migrate",
  ssl: databaseSsl,
});
const postgres = makePgRuntime({
  url: Redacted.make(databaseUrl),
  ssl: databaseSsl,
  connectTimeout: "5 seconds",
  multiplex: true,
  applicationName: "sigiro-auth-migrate",
});

try {
  await postgres.runPromise(migrateSecondaryStorage());
  const migrations = await getMigrations(
    createAuthOptions({
      baseURL: required("BETTER_AUTH_URL"),
      secret: required("BETTER_AUTH_SECRET"),
      resource: required("SIGIRO_RESOURCE"),
      betterAuthDatabase,
      runPg: postgres.runPromise,
      emailFrom: process.env.EMAIL_FROM ?? "noreply@sigiro.com",
      async sendEmail() {
        throw new Error("Email sending is unavailable during migrations");
      },
    }),
  );
  if (migrations.unsafeChanges.length > 0) {
    throw new Error(`unsafe migrations: ${migrations.unsafeChanges.join("; ")}`);
  }
  await migrations.runMigrations();
  console.log("Better Auth migrations applied");
} finally {
  await Promise.all([betterAuthDatabase.end(), postgres.dispose()]);
}
