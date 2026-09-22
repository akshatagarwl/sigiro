/**
 * Drive a real browser through the production sign-up flow.
 *
 * This exists because the API-level checks all passed while the product was
 * broken twice over: the sign-in pages 404'd after the D1 cutover, and every
 * verification email was silently dropped while `send-verification-otp` kept
 * answering `{"success":true}`. Neither was visible to a curl against
 * `/api/auth/*`, because neither the page nor the mailbox was ever involved.
 *
 * It reads the real OTP out of a real inbox (AgentMail) rather than using
 * AUTH_TEST_OTP, which is forbidden in production for good reason. So a pass
 * here means a human with a browser and an email address can actually onboard.
 *
 *   aube run check:signup
 *
 * Requires the `agentmail` CLI to be authenticated for INBOX.
 */
import { spawn } from 'node:child_process';
import assert from 'node:assert/strict';
import { chromium, type Page } from 'playwright';

import { INSTALL_COMMAND } from '../src/lib/site.shared';

const BASE = process.env.SIGIRO_SIGNUP_BASE ?? 'https://sigiro.com';
const INBOX = process.env.SIGIRO_SIGNUP_INBOX ?? 'akshat@agentmail.to';
const HEADLESS = process.env.SIGIRO_SIGNUP_HEADED !== '1';

const run = (command: string, args: readonly string[]): Promise<string> =>
  new Promise((resolve, reject) => {
    const child = spawn(command, [...args], { stdio: ['ignore', 'pipe', 'pipe'] });
    let out = '';
    let err = '';
    child.stdout.on('data', (c) => (out += String(c)));
    child.stderr.on('data', (c) => (err += String(c)));
    child.on('error', reject);
    child.on('close', (code) =>
      code === 0 ? resolve(out) : reject(new Error(`${command} exited ${code}: ${err || out}`)),
    );
  });

interface InboxMessage {
  readonly subject?: string;
  readonly timestamp?: string;
  readonly created_at?: string;
  readonly message_id?: string;
  readonly id?: string;
}

interface InboxListing {
  readonly messages?: ReadonlyArray<InboxMessage>;
  readonly data?: ReadonlyArray<InboxMessage>;
}

interface InboxBody {
  readonly text?: string;
  readonly preview?: string;
}

interface OTPReceipt {
  readonly code: string;
  readonly text: string;
}

/**
 * The newest verification code that arrived after `since`. Anchoring to a
 * timestamp matters: this inbox accumulates codes from earlier runs, and
 * reading a stale one would make a broken mailer look healthy.
 */
const latestOTP = async (since: Date): Promise<OTPReceipt | undefined> => {
  const listed = await run('agentmail', [
    'inboxes',
    'messages',
    'list',
    '--inbox-id',
    INBOX,
    '--limit',
    '5',
    '--format',
    'json',
  ]);
  // SAFETY: `agentmail ... --format json` is contractually JSON; a shape
  // mismatch surfaces as an undefined field and then as a missing-code error
  // below, never as a silent pass.
  const parsed = JSON.parse(listed) as InboxListing;
  const messages = parsed.messages ?? parsed.data ?? [];
  for (const message of messages) {
    const subject = message.subject ?? '';
    const stamp = message.timestamp ?? message.created_at ?? '';
    if (!subject.toLowerCase().includes('verification code')) continue;
    if (!stamp || new Date(stamp) < since) continue;
    const id = message.message_id ?? message.id;
    if (!id) continue;
    const body = await run('agentmail', [
      'inboxes',
      'messages',
      'get',
      '--inbox-id',
      INBOX,
      '--message-id',
      id,
      '--format',
      'json',
    ]);
    // SAFETY: same JSON contract as the listing above.
    const text = JSON.parse(body) as InboxBody;
    const content = text.text ?? text.preview ?? '';
    const code = /\b(\d{6})\b/.exec(content);
    if (code) return { code: code[1], text: content };
  }
  return undefined;
};

const waitForOTP = async (since: Date): Promise<OTPReceipt> => {
  for (let attempt = 0; attempt < 24; attempt += 1) {
    const code = await latestOTP(since);
    if (code) return code;
    await new Promise((resolve) => setTimeout(resolve, 5000));
  }
  throw new Error(
    'no verification code arrived within 2 minutes. The OTP endpoint returns ' +
      '{"success":true} even when delivery fails, so check the mailer, not the route.',
  );
};

interface SessionPayload {
  readonly user?: { readonly email?: string; readonly emailVerified?: boolean };
  readonly session?: { readonly activeOrganizationId?: string | null };
}

interface OrganizationPayload {
  readonly organization?: { readonly id?: string; readonly name?: string };
}

const step = (name: string, detail = '') =>
  console.log(`  ok   ${name}${detail ? `  ${detail}` : ''}`);

const main = async () => {
  const browser = await chromium.launch({ headless: HEADLESS });
  const page: Page = await browser.newPage();
  const failures: string[] = [];
  page.on('pageerror', (error) => failures.push(`page error: ${error.message}`));
  page.on('response', (response) => {
    if (response.status() >= 500) failures.push(`${response.status()} ${response.url()}`);
  });

  try {
    console.log(`browser sign-up against ${BASE}`);

    await page.goto(`${BASE}/sign-up`, { waitUntil: 'domcontentloaded' });
    step('/sign-up rendered', await page.title());

    const emailForm = page.locator('#email-form');
    await emailForm.waitFor({ state: 'visible', timeout: 15000 });
    step('email form visible');

    const requestedAt = new Date(Date.now() - 5000);
    await page.fill('#email-form input[name="name"]', 'Browser check');
    await page.fill('#email-form input[name="email"]', INBOX);
    await page.click('#email-form button[type="submit"]');
    step('code requested', INBOX);

    const otpForm = page.locator('#otp-form');
    await otpForm.waitFor({ state: 'visible', timeout: 20000 });
    step('OTP form revealed');

    const receipt = await waitForOTP(requestedAt);
    assert.match(receipt.text, /expires in five minutes/i);
    assert.match(receipt.text, /if you didn't request this code/i);
    step('code received by email', receipt.code.replace(/\d/g, '*'));
    step('email explains expiry and unsolicited-code handling');

    // There is one native input behind the six visual slots. Filling it mirrors
    // mobile autofill and password-manager input without moving focus between
    // six competing controls.
    await page.locator('#otp-form input[name="verification-code"]').fill(receipt.code);

    // The island replaces the form outright on success rather than hiding it.
    await page.waitForFunction(() => document.querySelector('#otp-form') === null, undefined, {
      timeout: 20000,
    });
    step('OTP accepted, signed in');

    const session = await page.evaluate(async (): Promise<SessionPayload> => {
      const response = await fetch('/api/auth/get-session', { credentials: 'include' });
      // SAFETY: better-auth's get-session contract; a shape change fails the
      // explicit email/emailVerified assertions immediately below.
      return (await response.json()) as SessionPayload;
    });
    if (session.user?.email !== INBOX)
      throw new Error(`session holds ${session.user?.email}, expected ${INBOX}`);
    if (!session.user?.emailVerified) throw new Error('session user is not emailVerified');
    step('session established', session.user.email);

    const organization = await page.evaluate(async (): Promise<OrganizationPayload> => {
      const response = await fetch('/api/auth/onboarding/ensure-personal-organization', {
        method: 'POST',
        credentials: 'include',
        headers: { 'content-type': 'application/json' },
        body: '{}',
      });
      // SAFETY: the onboarding route's contract; a missing id throws below.
      return (await response.json()) as OrganizationPayload;
    });
    if (!organization.organization?.id) throw new Error('no personal organization was created');
    step('workspace provisioned', organization.organization.name ?? '');

    const settled = await page.evaluate(async (): Promise<string | null> => {
      const response = await fetch('/api/auth/get-session', { credentials: 'include' });
      // SAFETY: as above; a null result fails the assertion that follows.
      const body = (await response.json()) as SessionPayload;
      return body.session?.activeOrganizationId ?? null;
    });
    if (!settled) throw new Error('session has no activeOrganizationId after onboarding');
    step('tenant bound to session', settled);

    const success = page.locator('#success');
    await success.waitFor({ state: 'visible', timeout: 15000 });
    const setup = await success.textContent();
    assert.match(setup ?? '', /same workspace/i);
    assert.match(setup ?? '', new RegExp(INSTALL_COMMAND.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
    assert.match(setup ?? '', /sigiro auth login/);
    assert.doesNotMatch(setup ?? '', /sigiro signup/);
    const guide = success.locator('a[href="/docs/how-to/hosted-onboarding#1-connect-the-cli"]');
    assert.equal(await guide.count(), 1);
    step('success screen hands the existing workspace to the CLI');

    if (failures.length > 0)
      throw new Error(`browser reported failures:\n  ${failures.join('\n  ')}`);
    console.log('browser sign-up: a new user can register and onboard');
  } finally {
    await browser.close();
  }
};

await main();
