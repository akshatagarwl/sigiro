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
 * One step, one terminal. An earlier version ran all the steps as a single
 * continuous session, which was wrong for the thing this is usually explaining:
 * when consecutive steps have different owners, one shared shell says one actor
 * ran all of them. A coding agent's work does not happen in your sigiro shell.
 * Separate frames make the ownership structural instead of a caption.
 *
 * The handoff between frames is scrubbed too, not timed. Each boundary owns a
 * stretch of scroll (`GAP`, measured in the same character units as the typing):
 * the finished frame leaves over the first half, the next arrives empty over the
 * second, and it all reverses if you scroll back. A timed transition bolted onto
 * a scroll-driven reveal would be the one part of this that could not be undone.
 *
 * Design constraints, from interior's DESIGN.md:
 * - §5   the displacements are the specified ones — in at `y 10, blur(6px)`, out
 *        at `y 6, blur(3px)` — driven by scroll rather than by a duration.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site.
 * - §3   no idle loops, so the cursor is a block rather than a blinking caret.
 * - §22  scroll-driven transforms are the accepted exception to the
 *        `useReducedMotion` rule. Reduced motion gets `fallback` instead, which
 *        is why `fallback` is required rather than optional.
 */

export type TerminalStep = {
  cmd: string;
  out: string;
  /** Names the step in the caption — "Diagnose", "Connect". */
  label: string;
  /** A chip beside the label: whose shell this is. */
  badge?: string;
  /** Ink rather than muted ink on the chip, for steps that are ours. */
  badgeStrong?: boolean;
  /** Prose under the well, swapped as the step changes. */
  note?: string;
};

/** 12.5px type on a 22px rhythm, explicit so the frame maths stays integer. */
const LINE = 22;
/** Vertical padding inside the well, top and bottom. */
const PAD = 24;

/**
 * Scroll spent on each handoff, in the same character units as the typing. Big
 * enough that the swap is a moment rather than a jump cut; small enough that it
 * is not a pause. Four boundaries at 50 add ~18% to the track.
 */
const GAP = 50;

type Line = { text: string; cmd: boolean };
type Plan = { lines: Line[]; start: number; chars: number; height: number };

function plan(steps: readonly TerminalStep[]) {
  const plans: Plan[] = [];
  let at = 0;
  steps.forEach((s, i) => {
    if (i > 0) at += GAP;
    const lines: Line[] = [{ text: `$ ${s.cmd}`, cmd: true }];
    for (const text of s.out.split('\n')) lines.push({ text, cmd: false });
    const chars = lines.reduce((n, l) => n + l.text.length + 1, 0);
    plans.push({ lines, start: at, chars, height: lines.length * LINE + PAD });
    at += chars;
  });
  return { plans, total: at };
}

/**
 * The controlled screen. Renders the first `revealed` characters of one step's
 * lines and nothing else — no timers, no state, no opinion about why the number
 * changed.
 */
function Screen({
  lines,
  revealed,
  height,
  style,
}: {
  lines: Line[];
  revealed: number;
  height: number;
  style?: React.CSSProperties;
}) {
  let seen = 0;
  return (
    <div className="mat-well overflow-hidden rounded-[9px] px-3.5 py-3" style={{ height }}>
      <div
        aria-hidden
        className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
        style={style}
      >
        {lines.map((line, i) => {
          const start = seen;
          seen += line.text.length + 1;
          const done = revealed >= start + line.text.length;
          const typing = revealed > start && !done;
          return (
            <div
              // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
              key={i}
              className={line.cmd ? 'text-ink' : 'text-ink-2'}
              style={{ height: LINE }}
            >
              {done ? line.text : typing ? line.text.slice(0, revealed - start) : ''}
              {typing ? <span className="ml-px inline-block h-[13px] w-[7px] bg-ink" /> : null}
            </div>
          );
        })}
      </div>
    </div>
  );
}

export function ScrubbedTerminal({
  steps,
  fallback,
  track = '400vh',
  settle = 0.9,
}: {
  steps: readonly TerminalStep[];
  /** Shown to reduced-motion and no-JS readers, who have nothing to scrub with. */
  fallback: React.ReactNode;
  /** Scroll distance the session is spread over. More track, slower typing. */
  track?: string;
  /**
   * Fraction of the track after which the session is complete. The remainder is
   * dead on purpose: the last command wants a beat to be read before the pin
   * releases, rather than flicking away on the frame it finishes.
   */
  settle?: number;
}) {
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const { plans, total } = useMemo(() => plan(steps), [steps]);

  const { scrollYProgress } = useScroll({ target: ref, offset: ['start start', 'end end'] });
  const cursor = useTransform(scrollYProgress, [0, settle], [0, total], { clamp: true });

  const [revealed, setRevealed] = useState(0);
  useMotionValueEvent(cursor, 'change', (v) => {
    const next = Math.round(v);
    setRevealed((prev) => (prev === next ? prev : next));
  });

  // Which frame is on screen, and how far through a handoff we are.
  let index = 0;
  for (let i = 0; i < plans.length; i++) if (revealed >= plans[i].start) index = i;
  const here = plans[index];
  const past = revealed - (here.start + here.chars);
  const handoff = past > 0 && index < plans.length - 1 ? Math.min(1, past / GAP) : 0;

  // First half the finished frame leaves, second half the next one arrives.
  const leaving = handoff > 0 && handoff < 0.5;
  const arriving = handoff >= 0.5;
  const shown = arriving ? index + 1 : index;
  const step = steps[shown];

  let style: React.CSSProperties | undefined;
  if (leaving) {
    const t = handoff / 0.5;
    style = { opacity: 1 - t, transform: `translateY(${-6 * t}px)`, filter: `blur(${3 * t}px)` };
  } else if (arriving) {
    const t = (handoff - 0.5) / 0.5;
    style = {
      opacity: t,
      transform: `translateY(${10 * (1 - t)}px)`,
      filter: `blur(${6 * (1 - t)}px)`,
    };
  }

  const local = arriving ? 0 : revealed - here.start;

  /**
   * The well is sized per step rather than to the tallest, so a short step does
   * not sit above two dead rows. Resizing is free: it happens across the handoff,
   * where the content it would jump is already faded out.
   */
  const height = handoff
    ? here.height + (plans[index + 1].height - here.height) * handoff
    : plans[shown].height;

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
                {shown + 1} of {steps.length}
              </span>
            </figcaption>

            <Screen
              lines={plans[shown].lines}
              revealed={arriving ? 0 : local}
              height={height}
              style={style}
            />

            {step.note ? (
              <p className="mt-2 min-h-[3.4rem] max-w-[62ch] px-2.5 pb-1 text-[13px] leading-relaxed text-ink-2">
                {step.note}
              </p>
            ) : null}
          </figure>
        </div>
      </div>

      {/* What is on screen depends on scroll position, and a screen reader has no
          scroll position. This is every session, whole, once. */}
      <pre className="sr-only">
        {plans.map((p) => p.lines.map((l) => l.text).join('\n')).join('\n\n')}
      </pre>
    </>
  );
}
