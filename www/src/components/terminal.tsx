'use client';

import { useMotionValueEvent, useReducedMotion, useScroll, useTransform } from 'motion/react';
import { useMemo, useRef, useState } from 'react';

/**
 * A terminal you scrub instead of one that plays itself.
 *
 * Every terminal component in the shadcn ecosystem owns its own clock:
 * MagicUI's is `setInterval` typing behind `useInView({once: true})`, shadcn.io
 * republishes the same design, and the typewriter primitives underneath
 * (`react-type-animation`, `typeit-react`) take a sequence of text and delays.
 * They are all built for a hero demo that plays once and cannot reverse. The
 * real terminal emulators — xterm, react-console-emulator — are interactive
 * shells with input handling, which is a different product entirely.
 *
 * What none of them expose is the one prop that makes scrubbing possible:
 * "render exactly N characters". So that is the contract here. `Screen` is dumb
 * and controlled; `ScrubbedTerminal` supplies N from scroll position. Splitting
 * them that way is the whole design — a different driver (a timer, a click, a
 * websocket) can render the same screen without touching it.
 *
 * Design constraints, from interior's DESIGN.md:
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site.
 * - §3   no idle loops, so the cursor is a block rather than a blinking caret.
 * - §5g  gradient overlays, never animated masks, for the fades.
 * - §22  scroll-driven transforms are the accepted exception to the
 *        `useReducedMotion` rule. Reduced motion gets `fallback` instead, which
 *        is why `fallback` is required rather than optional.
 */

export type TerminalStep = {
  cmd: string;
  out: string;
  /** Names the step in the caption — "Diagnose", "Connect". */
  label: string;
  /** A chip beside the label: whose step this is. */
  badge?: string;
  /** Ink rather than muted ink on the chip, for steps that are ours. */
  badgeStrong?: boolean;
  /** Prose under the well, swapped as the step changes. */
  note?: string;
};

/** 12.5px type on a 22px rhythm, explicit so the follow maths stays integer. */
const LINE = 22;

type Row = { text: string; cmd: boolean; step: number; start: number };

function flatten(steps: readonly TerminalStep[]) {
  const rows: Row[] = [];
  let chars = 0;
  steps.forEach((s, step) => {
    if (step > 0) {
      rows.push({ text: '', cmd: false, step, start: chars });
      chars += 1;
    }
    const cmd = `$ ${s.cmd}`;
    rows.push({ text: cmd, cmd: true, step, start: chars });
    chars += cmd.length + 1;
    for (const line of s.out.split('\n')) {
      rows.push({ text: line, cmd: false, step, start: chars });
      chars += line.length + 1;
    }
  });
  return { rows, total: chars };
}

/**
 * The controlled screen. Renders the first `revealed` characters of the
 * transcript and nothing else — no timers, no state, no opinion about why the
 * number changed.
 */
function Screen({
  rows,
  revealed,
  height,
  follow,
}: {
  rows: Row[];
  revealed: number;
  height: number;
  /** How far the cursor stays off the bottom, in lines. */
  follow: number;
}) {
  let at = 0;
  for (let i = 0; i < rows.length; i++) if (revealed >= rows[i].start) at = i;
  const lines = Math.floor(height / LINE);
  const offset = Math.max(0, at - (lines - follow)) * LINE;

  return (
    <div className="mat-well relative overflow-hidden rounded-[9px] px-3.5 py-3" style={{ height }}>
      <div
        aria-hidden
        className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
        style={{ transform: `translateY(${-offset}px)` }}
      >
        {rows.map((row, i) => {
          const done = revealed >= row.start + row.text.length;
          const typing = revealed > row.start && !done;
          return (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
              key={i}
              className={row.cmd ? 'text-ink' : 'text-ink-2'}
              style={{ height: LINE }}
            >
              {done ? row.text : typing ? row.text.slice(0, revealed - row.start) : ''}
              {typing ? <span className="ml-px inline-block h-[13px] w-[7px] bg-ink" /> : null}
            </div>
          );
        })}
      </div>

      {/* The visible rows are `aria-hidden` because what they contain depends on
          scroll position, and a screen reader has no scroll position. This is
          the same session, whole, for anything not reading with its eyes. */}
      <pre className="sr-only">{rows.map((r) => r.text).join('\n')}</pre>

      <div
        aria-hidden
        className="pointer-events-none absolute inset-x-0 top-0 h-6 bg-gradient-to-b from-well to-transparent"
      />
      <div
        aria-hidden
        className="pointer-events-none absolute inset-x-0 bottom-0 h-8 bg-gradient-to-t from-well to-transparent"
      />
    </div>
  );
}

export function ScrubbedTerminal({
  steps,
  fallback,
  track = '340vh',
  height = LINE * 16,
  follow = 4,
  settle = 0.88,
}: {
  steps: readonly TerminalStep[];
  /** Shown to reduced-motion and no-JS readers, who have nothing to scrub with. */
  fallback: React.ReactNode;
  /** Scroll distance the session is spread over. More track, slower typing. */
  track?: string;
  height?: number;
  follow?: number;
  /**
   * Fraction of the track after which the session is complete. The remainder is
   * dead on purpose: the last command wants a beat to be read before the pin
   * releases, rather than flicking away on the frame it finishes.
   */
  settle?: number;
}) {
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const { rows, total } = useMemo(() => flatten(steps), [steps]);

  const { scrollYProgress } = useScroll({ target: ref, offset: ['start start', 'end end'] });
  const cursor = useTransform(scrollYProgress, [0, settle], [0, total], { clamp: true });

  const [revealed, setRevealed] = useState(0);
  useMotionValueEvent(cursor, 'change', (v) => {
    const next = Math.round(v);
    setRevealed((prev) => (prev === next ? prev : next));
  });

  let at = 0;
  for (let i = 0; i < rows.length; i++) if (revealed >= rows[i].start) at = i;
  const index = rows[at]?.step ?? 0;
  const step = steps[index];

  if (reduced) return <>{fallback}</>;

  return (
    <>
      <noscript>{fallback}</noscript>

      <div ref={ref} className="relative" style={{ height: track }}>
        <div className="sticky top-0 flex h-screen items-center">
          <figure className="mat-panel w-full rounded-[14px] p-[5px]">
            <figcaption className="flex h-9 items-center gap-2.5 pr-[3px] pl-2.5">
              {step.badge ? (
                <span
                  className={`mat-cap inline-flex h-[22px] shrink-0 items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                    step.badgeStrong ? 'text-ink' : 'text-ink-3'
                  }`}
                >
                  {step.badge}
                </span>
              ) : null}
              <span className="text-[13px] font-medium tracking-[-0.01em] text-ink">
                {step.label}
              </span>
              <span className="meta ml-auto shrink-0 text-ink-3">
                {index + 1} of {steps.length}
              </span>
            </figcaption>

            <Screen rows={rows} revealed={revealed} height={height} follow={follow} />

            {step.note ? (
              <p className="mt-2 min-h-[3.4rem] max-w-[62ch] px-2.5 pb-1 text-[13px] leading-relaxed text-ink-2">
                {step.note}
              </p>
            ) : null}
          </figure>
        </div>
      </div>
    </>
  );
}
