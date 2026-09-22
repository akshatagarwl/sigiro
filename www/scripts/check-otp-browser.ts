import assert from 'node:assert/strict';
import { chromium, type Page, type Route } from 'playwright';

const BASE = process.env.SIGIRO_OTP_BASE ?? 'http://127.0.0.1:4330';

type JsonResponse =
  | null
  | boolean
  | number
  | string
  | readonly JsonResponse[]
  | { readonly [key: string]: JsonResponse };

const json = async (route: Route, status: number, body: JsonResponse): Promise<void> => {
  await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) });
};

const visibleTextCount = async (page: Page, text: string): Promise<number> =>
  page.getByText(text, { exact: true }).evaluateAll(
    (nodes) =>
      nodes.filter((node) => {
        const style = getComputedStyle(node);
        return (
          style.display !== 'none' &&
          style.visibility !== 'hidden' &&
          node.getClientRects().length > 0
        );
      }).length,
  );

const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({ viewport: { width: 320, height: 900 } });
  await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: BASE });
  const page = await context.newPage();
  const errors: string[] = [];
  let sends = 0;
  let attempts = 0;

  page.on('pageerror', (error) => errors.push(error.message));
  await page.clock.install();

  await page.route('**/api/auth/get-session', (route) => json(route, 200, null));
  await page.route('**/api/auth/email-otp/send-verification-otp', (route) => {
    sends += 1;
    if (sends === 2) return json(route, 503, { message: 'Email delivery unavailable' });
    return json(route, 200, { success: true });
  });
  await page.route('**/api/auth/onboarding/ensure-personal-organization', (route) =>
    json(route, 200, { organization: { id: 'org_otp_check', name: 'OTP check' } }),
  );
  await page.route('**/api/auth/sign-in/email-otp', async (route) => {
    attempts += 1;
    // SAFETY: this intercepted route receives the auth form's JSON body; an
    // absent or non-string otp falls into the explicit unexpected-code branch.
    const body = route.request().postDataJSON() as { otp?: string };
    if (body.otp === '111111') {
      await new Promise((resolve) => setTimeout(resolve, 100));
      return json(route, 400, { message: 'Invalid OTP' });
    }
    if (body.otp === '222222') return json(route, 400, { message: 'OTP expired' });
    if (body.otp === '123456') {
      return json(route, 200, {
        user: { email: 'reader@example.com', emailVerified: true },
        session: { activeOrganizationId: 'org_otp_check' },
      });
    }
    return json(route, 400, { message: 'Unexpected verification code' });
  });

  await page.goto(`${BASE}/sign-up`, { waitUntil: 'domcontentloaded' });
  await page.locator('#name').fill('OTP check');
  await page.locator('#email').fill('reader@example.com');
  await page.locator('#email-form button[type="submit"]').click();

  const form = page.locator('#otp-form');
  await form.waitFor({ state: 'visible' });
  const input = form.locator('input[name="verification-code"]');
  assert.equal(await input.count(), 1, 'OTP must use one native input');
  assert.equal(await input.getAttribute('autocomplete'), 'one-time-code');
  assert.equal(await input.getAttribute('inputmode'), 'numeric');
  assert.equal(await visibleTextCount(page, 'Enter or paste the six-digit code.'), 1);
  const otpOverflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  assert.ok(otpOverflow <= 1, `OTP form horizontal overflow: ${otpOverflow}px`);
  const lastSlot = await form.locator('[data-slot="6"]').boundingBox();
  assert.ok(lastSlot && lastSlot.x + lastSlot.width <= 320, 'all six OTP slots must be visible');

  await input.focus();
  await page.keyboard.type('111111');
  assert.equal(await input.isDisabled(), false, 'corrections must remain editable while verifying');
  await input.press('ControlOrMeta+A');
  await page.keyboard.type('333333');
  const wrong = 'That verification code is incorrect. Try again or request a new code.';
  await page.getByText(wrong, { exact: true }).waitFor();
  assert.equal(await visibleTextCount(page, wrong), 1);
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('name') === 'verification-code',
  );
  assert.equal(await input.inputValue(), '333333', 'an in-flight correction must not be discarded');
  assert.deepEqual(
    await page.evaluate(() => {
      const field = document.querySelector<HTMLInputElement>(
        '#otp-form input[name="verification-code"]',
      );
      return [field?.selectionStart, field?.selectionEnd];
    }),
    [0, 6],
  );

  await form.getByRole('button', { name: 'Verify code' }).click();
  await page.waitForFunction(() => {
    const button = document.querySelector<HTMLButtonElement>('#otp-form button[type="submit"]');
    return button?.disabled === false;
  });
  assert.equal(attempts, 2, 'the explicit submit button must retry an unchanged code');

  await page.evaluate(() => navigator.clipboard.writeText('222222'));
  await input.press('ControlOrMeta+V');
  const expired = 'That verification code has expired. Request a new code.';
  await page.getByText(expired, { exact: true }).waitFor();
  assert.equal(await visibleTextCount(page, expired), 1);
  assert.equal(await input.inputValue(), '222222');

  const resend = page.locator('#resend');
  assert.equal(await resend.isDisabled(), true);
  await page.clock.fastForward(30_000);
  await resend.click();
  await page.getByText('Email delivery unavailable', { exact: true }).waitFor();
  assert.equal(await visibleTextCount(page, 'Email delivery unavailable'), 1);
  assert.equal(await resend.isDisabled(), false);

  await resend.click();
  await page.getByText('A new verification code was sent.', { exact: true }).waitFor();
  assert.equal(sends, 3);
  await page.waitForFunction(
    () => document.activeElement?.getAttribute('name') === 'verification-code',
  );

  await input.fill('123456');
  await page.locator('#success').waitFor({ state: 'visible' });
  assert.equal(attempts, 4);
  assert.equal(errors.length, 0, errors.join('\n'));

  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
  );
  assert.ok(overflow <= 1, `horizontal overflow: ${overflow}px`);
  console.log(
    'OTP browser check: rapid input, paste, expiry, failed/successful resend, autofill, and recovery passed',
  );
} finally {
  await browser.close();
}
