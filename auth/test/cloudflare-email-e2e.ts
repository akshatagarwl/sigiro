import { createServer } from "node:http";

import Cloudflare from "cloudflare";

import { sendCloudflareEmail } from "../src/email.js";

const messages: unknown[] = [];
const server = createServer((request, response) => {
  const chunks: Buffer[] = [];
  request.on("data", (chunk: Buffer) => chunks.push(chunk));
  request.on("end", () => {
    if (request.method !== "POST" || request.url !== "/accounts/account-id/email/sending/send") {
      response.writeHead(404).end();
      return;
    }
    if (request.headers.authorization !== "Bearer test-token") {
      response.writeHead(401).end();
      return;
    }

    messages.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    response.setHeader("content-type", "application/json");
    response.end(
      JSON.stringify({
        success: true,
        errors: [],
        messages: [],
        result: messages.length === 1
          ? { delivered: [], permanent_bounces: [], queued: ["agent@example.com"] }
          : { delivered: [], permanent_bounces: ["agent@example.com"], queued: [] },
      }),
    );
  });
});

await new Promise<void>((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", resolve);
});

try {
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("email fixture did not bind to TCP");
  const client = new Cloudflare({
    apiToken: "test-token",
    baseURL: `http://127.0.0.1:${address.port}`,
    maxRetries: 0,
    timeout: 1_000,
  });
  const email = {
    from: "noreply@sigiro.com",
    to: "agent@example.com",
    subject: "Your Sigiro verification code",
    text: "Your verification code is 123456.",
  };

  await sendCloudflareEmail(client, "account-id", email);
  if (JSON.stringify(messages[0]) !== JSON.stringify({
    from: email.from,
    to: [email.to],
    subject: email.subject,
    text: email.text,
  })) {
    throw new Error(`unexpected Cloudflare request body: ${JSON.stringify(messages[0])}`);
  }

  let rejected = false;
  try {
    await sendCloudflareEmail(client, "account-id", email);
  } catch {
    rejected = true;
  }
  if (!rejected) throw new Error("permanently bounced email was treated as accepted");
  process.stdout.write("Cloudflare Email Sending request and delivery status verified\n");
} finally {
  await new Promise<void>((resolve, reject) => {
    server.close((error) => error ? reject(error) : resolve());
  });
}
