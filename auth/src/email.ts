import Cloudflare from "cloudflare";

export interface TransactionalEmail {
  from: string;
  to: string;
  subject: string;
  text: string;
}

export const sendCloudflareEmail = async (
  client: Cloudflare,
  accountId: string,
  message: TransactionalEmail
): Promise<void> => {
  const result = await client.emailSending.send({
    account_id: accountId,
    from: message.from,
    to: [message.to],
    subject: message.subject,
    text: message.text,
  });
  if (
    !result.delivered.includes(message.to) &&
    !result.queued.includes(message.to)
  ) {
    throw new Error(`Cloudflare did not accept email for ${message.to}`);
  }
};
