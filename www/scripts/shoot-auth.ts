/**
 * Screenshot the auth pages in both themes so they can be looked at.
 *
 * Structural assertions cannot tell you a card is off-centre or that a field
 * has the wrong well. `check:signup` and `check:device` prove the flow works;
 * this is for judging whether it looks right.
 */
import { mkdirSync } from 'node:fs';

import { chromium, type Page } from 'playwright';

const BASE = process.env.SIGIRO_SHOT_BASE ?? 'http://localhost:4330';
const OUT = new URL('../../.shots/', import.meta.url).pathname;

interface Shot {
  readonly name: string;
  readonly path: string;
  /** Optional interaction to reach a later step of the flow. */
  readonly prepare?: (page: Page) => Promise<void>;
}

const SHOTS: readonly Shot[] = [
  { name: 'sign-in', path: '/sign-in' },
  { name: 'sign-up', path: '/sign-up' },
  {
    name: 'sign-in-focus',
    path: '/sign-in',
    prepare: async (page) => {
      await page.click('input[name="email"]');
    },
  },
  {
    name: 'sign-in-filled',
    path: '/sign-in',
    prepare: async (page) => {
      await page.fill('input[name="email"]', 'reader@example.com');
    },
  },
  {
    // The OTP step is the reference field in DESIGN.md §3b and the reason for
    // this port, so it has to be looked at — not just the email step.
    name: 'otp',
    path: '/sign-in',
    prepare: async (page) => {
      await page.fill('input[name="email"]', 'reader@example.com');
      await page.click('#email-form button[type="submit"]');
      await page.waitForSelector('#otp-form', { state: 'visible', timeout: 25_000 });
    },
  },
  {
    name: 'otp-partial',
    path: '/sign-in',
    prepare: async (page) => {
      await page.fill('input[name="email"]', 'reader@example.com');
      await page.click('#email-form button[type="submit"]');
      await page.waitForSelector('#otp-form', { state: 'visible', timeout: 25_000 });
      await page.keyboard.type('204');
    },
  },
  { name: 'device-nocode', path: '/device' },
];

const main = async (): Promise<void> => {
  mkdirSync(OUT, { recursive: true });
  const browser = await chromium.launch();
  try {
    for (const theme of ['light', 'dark'] as const) {
      const context = await browser.newContext({
        colorScheme: theme,
        viewport: { width: 1280, height: 900 },
        deviceScaleFactor: 2,
      });
      for (const shot of SHOTS) {
        const page = await context.newPage();
        await page.goto(`${BASE}${shot.path}`, { waitUntil: 'networkidle' });
        // The card is a client:load island that asks /get-session before it
        // renders; without this every shot is of an empty card.
        await page
          .waitForSelector('#email-form, #device-flow, #success, #otp-form', { timeout: 15_000 })
          .catch(() => undefined);
        if (shot.prepare) await shot.prepare(page);
        await page.waitForTimeout(600);
        await page.screenshot({ path: `${OUT}${shot.name}-${theme}.png`, fullPage: false });
        console.log(`  ${shot.name}-${theme}.png`);
        await page.close();
      }
      await context.close();
    }
  } finally {
    await browser.close();
  }
  console.log(`shots in ${OUT}`);
};

await main();
