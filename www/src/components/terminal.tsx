'use client';

import { useMachine } from '@xstate/react';
import {
  motion,
  useMotionTemplate,
  useMotionValueEvent,
  useScroll,
  useTransform,
} from 'motion/react';
import { useMemo, useRef, useSyncExternalStore } from 'react';
import { createMachine, types } from 'xstate';
import { SkeletonSwap } from '@/components/interior/skeleton-swap';
import { TaskSteps } from '@/components/interior/task-steps';
import type { SessionLine } from '@/lib/highlight';

/**
 * One terminal per step, stepped by scroll, with a task list beside it.
 *
 * An earlier version typed the transcript out character by character, bound to
 * scroll position. It was a good demo of scrubbing and a bad terminal: real
 * shells do not type their own output. You run a command and the output lands.
 * So nothing types — but a command does take a beat to run, which is what
 * `SkeletonSwap` is for: arrive at a frame, the shell is busy, then the output
 * appears. Its 120ms delay and 380ms floor are the reason this reads as a
 * command running rather than as a flicker (§19).
 *
 * `TaskSteps` carries the argument the section exists to make. Five stages, the
 * ones you have passed ticked off, the one you are on running, the rest waiting
 * — and `meta` naming the owner of each, so "two of these five are ours" is
 * something you watch accumulate rather than a claim in a caption. It also
 * brings `aria-current="step"` and a debounced live region, which the rail I
 * hand-rolled did not have.
 *
 * Both are vendored verbatim into components/interior/, so they write literal
 * stone and emerald values rather than our tokens. That is §1b, on purpose: a
 * copied component must not depend on the host's palette.
 *
 * Design constraints, from DESIGN.md (now vendored at www/DESIGN.md):
 * - §5h  React does not re-render at 60fps. The step index is machine context;
 *        the handoff is MotionValues off `scrollYProgress` and never touches
 *        React or the machine. A previous version quantised progress to two
 *        decimals and drove the fade from state, leaving the handoff ~6 frames
 *        long — §6a: "quantising the sweep would make the fill look like it was
 *        stuttering".
 * - §6a  the machine's `STEP` transition returns nothing when the index has not
 *        changed. XState treats that as no transition and hands back the same
 *        snapshot object, so the scroll listener can fire every frame and React
 *        still renders once per step — the machine's version of
 *        `setStep(prev => prev === s ? prev : s)`.
 * - §5   the handoff uses the specified displacements — out at `y 6, blur(3px)`,
 *        in at `y 10, blur(6px)`.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site —
 *        and, since `highlightSession`, wearing the same Shiki colours. The
 *        markup arrives already highlighted: this component is a client
 *        component and Shiki is a server-side dependency, so what crosses is
 *        one string of HTML per line, never a highlighter.
 * - §5h  a scroll-driven component needs no reduced-motion branch — every value
 *        resolves on the frame it is scrolled to. `fallback` covers no-JS only,
 *        and that is deliberate rather than an oversight.
 */

export type TerminalStep = {
  /**
   * The session, one line per line, highlighted on the server by
   * `highlightSession` — the `$ command` row first, then its output.
   */
  lines: readonly SessionLine[];
  /** Names the step in the task list and the caption — "Diagnose", "Connect". */
  label: string;
  /** Whose shell this is. Shown against the step once it is done. */
  badge?: string;
  /** Ink rather than muted ink on the chip, for steps that are ours. */
  badgeStrong?: boolean;
  /**
   * Short form of `badge`, for the task list's `meta` column.
   *
   * That column is `shrink-0` against a `flex-1 truncate` label, so a long owner
   * squeezes the label to zero width and the step loses its name — invisibly,
   * because meta is `opacity-0` until the step is done. "your pipeline, on our
   * evidence" did exactly that to "Verify". Falls back to `badge`.
   */
  badgeShort?: string;
  /** Prose under the well, swapped as the step changes. */
  note?: string;
  /**
   * Two ways to do this step, for the reader to choose between.
   *
   * Both are highlighted on the server and both cross to the client, because the
   * highlighter must not. When present, `lines` is whichever the reader picked
   * and this is the pair the picker offers.
   */
  choices?: readonly { id: string; label: string; lines: readonly SessionLine[] }[];
};

/**
 * Where this terminal's install preference lives.
 *
 * It used to mirror fumadocs' storage convention — sessionStorage first, then
 * localStorage, both written — so that picking binary or docker here was also
 * picked in the docs. That is gone, and not because the code was wrong: the
 * docs moved to docs.sigiro.com, which is a different origin, and storage is
 * per-origin. No amount of convention-matching crosses that line. The docs tabs
 * now keep their own choice, deep-linkable as `?install=docker`.
 *
 * `localStorage` can throw — Safari in private mode, storage blocked. A missing
 * preference is just the default, so failing soft is right.
 */
const PREF = 'sigiro-install';

function readPref(): string | null {
  try {
    return localStorage.getItem(PREF);
  } catch {
    return null;
  }
}

/**
 * Same-document writes do not raise `storage` — that event is for *other* tabs
 * — so a subscriber in this document has to be told directly.
 */
const watchers = new Set<() => void>();

function writePref(id: string) {
  try {
    localStorage.setItem(PREF, id);
  } catch {
    // A reader who cannot store a preference still gets to change the view.
  }
  for (const notify of watchers) notify();
}

function subscribe(notify: () => void) {
  watchers.add(notify);
  // Another tab changing the preference. Free, and correct: the reader set it
  // once and means it everywhere.
  window.addEventListener('storage', notify);
  return () => {
    watchers.delete(notify);
    window.removeEventListener('storage', notify);
  };
}

/**
 * The reader's install preference, as a subscription rather than a copy.
 *
 * `useSyncExternalStore` is React's own hook for reading a store it does not
 * own, and storage is exactly that. It replaced a `useState` seeded by a
 * `useEffect`, which worked and was three wrong things: it kept a second copy
 * of a value that already had a home, it re-implemented the hydration rule the
 * hook enforces by taking a separate server snapshot, and it could not hear a
 * change it did not make. The third one is not hypothetical — the docs tabs
 * write this key too.
 *
 * The server snapshot is `null`, which renders the first choice. It has to be a
 * constant: returning `readPref()` there would read storage during render and
 * make the server and the first client render disagree.
 */
function useInstallPref() {
  return useSyncExternalStore(subscribe, readPref, () => null);
}

/** 12.5px type on a 22px rhythm, explicit so the frame maths stays integer. */
const LINE = 22;
/** Vertical padding inside the well, top and bottom. */
const PAD = 24;

/**
 * Fraction of a step's scroll spent holding still before the handoff begins.
 * The remainder is the handoff: frame out over the first half, in over the
 * second.
 */
const HOLD = 0.72;

/**
 * How long a command "runs" on arrival. Short, because it is a beat rather than
 * a claim about latency — `SkeletonSwap` stretches it to its own 380ms floor so
 * the skeleton never strobes.
 */
const RUN_MS = 300;

/**
 * The transcript a step shows, given what the reader picked.
 *
 * `pick` is read from storage and so can be anything at all — a stale id, a
 * value another page wrote. An unknown id falls back to the first choice rather
 * than rendering an empty terminal.
 */
function chosen(step: TerminalStep, pick: string | null) {
  if (!step.choices?.length) return step.lines;
  return (step.choices.find((c) => c.id === pick) ?? step.choices[0]).lines;
}

function plan(steps: readonly TerminalStep[], pick: string | null) {
  return steps.map((step) => {
    const lines = chosen(step, pick);
    return {
      lines,
      // A shell that has finished sits at a fresh prompt. One more row for it.
      height: (lines.length + 1) * LINE + PAD,
    };
  });
}

/** Where a scroll position falls: which step, and how far into its handoff. */
function locate(v: number, count: number) {
  const scaled = Math.min(Math.max(v, 0), 0.9999) * count;
  const index = Math.floor(scaled);
  const frac = scaled - index;
  const handoff = index >= count - 1 || frac <= HOLD ? 0 : (frac - HOLD) / (1 - HOLD);
  return { index, handoff };
}

/**
 * Stepping to the frame already shown is not a step. Returning nothing is how
 * XState says "this event changes nothing" — the actor keeps the snapshot it
 * had, so React never sees a new one.
 */
function step({
  context,
  event,
}: {
  context: { shown: number };
  event: { type: 'STEP'; index: number };
}) {
  if (event.index === context.shown) return;
  return { target: 'running', context: { shown: event.index }, reenter: true };
}

/**
 * The discrete half of the component: which frame is mounted, and whether its
 * command has finished running.
 *
 * Two states rather than a boolean beside an index, because "arriving at a frame
 * runs its command" is a transition, not a value. `STEP` re-enters `running`
 * whichever state it lands in, which cancels the in-flight delay and starts a
 * new one — scrubbing quickly cannot stack up timers because a state can only be
 * in one of them.
 *
 * Nothing continuous lives here. `context.shown` changes once per step; the
 * opacity, y, blur and height are MotionValues (§5h).
 */
const sequence = createMachine({
  // `types` is XState's own type-only schema: the machine's shape reaches
  // TypeScript without a validation library coming with it.
  schemas: {
    context: types<{ shown: number }>(),
    events: { STEP: types<{ index: number }>() },
  },
  initial: 'running',
  context: { shown: 0 },
  states: {
    running: {
      after: { [RUN_MS]: { target: 'ready' } },
      on: { STEP: step },
    },
    ready: {
      on: { STEP: step },
    },
  },
});

export function SteppedTerminal({
  steps,
  fallback,
  label,
  heading,
  track,
}: {
  steps: readonly TerminalStep[];
  /** Shown where JavaScript never runs. */
  fallback: React.ReactNode;
  /**
   * Names the task list for a screen reader. Required, because it was hardcoded
   * to "The five stages" while this component had one caller, and the second
   * caller — a three-step sequence — then announced the wrong count to anyone
   * who could not see it. A caller cannot forget a required prop.
   */
  label: string;
  /**
   * The section's own label and headline, condensed to one line and pinned above
   * the frame.
   *
   * The section `h2` scrolls away the moment this thing pins, which left the
   * reader looking at a terminal with nothing on screen naming what it was for.
   * The task list says which step; this says which section. It rides inside the
   * sticky box, so it pins and unpins with the frame and no page-level bar is
   * needed.
   *
   * `aria-hidden`, because it repeats the real `h2` that is still in the section
   * above — a screen reader should meet that heading once, not twice.
   */
  heading?: { label: string; title: string };
  /**
   * Scroll distance the whole sequence occupies. Defaults to one viewport for
   * the pin plus 45vh of travel per step.
   *
   * It was 55vh while the transcript typed itself, because the reader was
   * waiting on the machine. Nothing waits now beyond the run beat, so the only
   * thing the travel has to cover is reading, which the reader paces by
   * stopping.
   */
  track?: string;
}) {
  const ref = useRef<HTMLDivElement>(null);

  const pick = useInstallPref();
  const frames = useMemo(() => plan(steps, pick), [steps, pick]);
  const { scrollYProgress } = useScroll({ target: ref, offset: ['start start', 'end end'] });

  // The discrete half: which frame's text is mounted, and whether its command
  // has finished running. One render per step — the machine drops a `STEP` that
  // names the frame already shown.
  const [state, send] = useMachine(sequence);
  const shown = state.context.shown;
  const ran = state.matches('ready');

  useMotionValueEvent(scrollYProgress, 'change', (v) => {
    const { index, handoff } = locate(v, steps.length);
    send({ type: 'STEP', index: handoff >= 0.5 ? index + 1 : index });
  });

  // The continuous half: never React state, so it runs at frame rate.
  const opacity = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 1;
    return h < 0.5 ? 1 - h / 0.5 : (h - 0.5) / 0.5;
  });
  const y = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 0;
    return h < 0.5 ? -6 * (h / 0.5) : 10 * (1 - (h - 0.5) / 0.5);
  });
  const blur = useTransform(scrollYProgress, (v) => {
    const { handoff: h } = locate(v, steps.length);
    if (!h) return 0;
    return h < 0.5 ? 3 * (h / 0.5) : 6 * (1 - (h - 0.5) / 0.5);
  });
  const filter = useMotionTemplate`blur(${blur}px)`;

  // Resizing rides the handoff too, where the content it would jump is faded.
  const height = useTransform(scrollYProgress, (v) => {
    const { index, handoff: h } = locate(v, steps.length);
    const a = frames[index].height;
    const b = frames[Math.min(index + 1, frames.length - 1)].height;
    return a + (b - a) * h;
  });

  const step = steps[shown];
  const frame = frames[shown];

  const tasks = useMemo(
    () => steps.map((s) => ({ id: s.label, label: s.label, meta: s.badgeShort ?? s.badge })),
    [steps],
  );

  return (
    <>
      <noscript>{fallback}</noscript>

      <div
        ref={ref}
        className="relative"
        style={{ height: track ?? `calc(100vh + ${steps.length * 45}vh)` }}
      >
        <div className="sticky top-0 flex h-screen flex-col justify-center gap-5">
          {/* The section's name, kept on screen for as long as the frame is
              pinned. One line, so it reads as a bar rather than a block: the
              terminal is already 477px of a 720px viewport and cannot spare a
              heading's worth of height. */}
          {heading ? (
            <p aria-hidden className="flex items-center gap-2 px-1">
              <span className="label">{heading.label}</span>
              <span className="text-ink-3">·</span>
              <span className="text-[13px] font-medium tracking-[-0.01em] text-ink">
                {heading.title}
              </span>
            </p>
          ) : null}

          <div className="flex flex-col gap-5 lg:grid lg:grid-cols-[190px_1fr] lg:items-center">
            {/* The task list says which step you are on; the heading above says
                which section it belongs to. */}
            <TaskSteps steps={tasks} current={shown} label={label} />

            <figure className="mat-panel w-full min-w-0 rounded-[14px] p-[5px]">
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

                {/* The picker sits in the chrome rather than above the section,
                    because what it changes is inside this well and nowhere else.
                    §3 rules out a pill, so it is two cells in a tray; radii nest,
                    5 inside 7.

                    Ink for the selected cell, not accent. §7c lists "segmented
                    thumb" among the controls that mark selection with ink and
                    depth, and says why: accent means "the system is responding
                    to you right now", and if every selected thing were blue,
                    focus would stop meaning anything. This was `bg-accent` until
                    that section was re-read. It also matches the docs tabs, which
                    fumadocs draws in ink — one preference, two controls, one
                    idiom. */}
                {step.choices ? (
                  <span
                    role="group"
                    aria-label={`How to run ${step.label.toLowerCase()}`}
                    className="mat-cap ml-auto flex shrink-0 items-center gap-px rounded-[7px] p-px"
                  >
                    {step.choices.map((choice) => {
                      const on = chosen(step, pick) === choice.lines;
                      return (
                        <button
                          key={choice.id}
                          type="button"
                          aria-pressed={on}
                          onClick={() => writePref(choice.id)}
                          className={`press meta rounded-[5px] px-2 py-[5px] ${
                            on ? 'bg-ink text-panel' : 'text-ink-3 hover:text-ink'
                          }`}
                        >
                          {choice.label}
                        </button>
                      );
                    })}
                  </span>
                ) : null}

                <span className={`meta shrink-0 text-ink-3 ${step.choices ? 'pl-2.5' : 'ml-auto'}`}>
                  {shown + 1} of {steps.length}
                </span>
              </figcaption>

              {/* Clips both axes, and that is correct: `height` animates across the
                handoff so the y axis must clip, and the x axis never overflows
                here because `SkeletonSwap` inside is itself the scroll container.
                At a 390px viewport this well is 324px while the widest transcript
                row is 495px — the sideways scroll that reaches it belongs to
                SkeletonSwap's shell, not to this element. Putting `overflow-x-auto`
                here as well was tried and is dead config: scrollWidth equals
                clientWidth, so it can never scroll.

                Known gap, not fixed here: that shell only takes a tab stop when it
                detects itself scrollable, and it does not detect the horizontal
                case — so a keyboard user cannot reach the clipped end of a line.
                The component is vendored verbatim under §1b, so the fix belongs
                upstream or in the transcript width, not in a local patch. */}
              <motion.div
                className="mat-well overflow-hidden rounded-[9px] px-3.5 py-3"
                style={{ height }}
              >
                <motion.div style={{ opacity, y, filter }}>
                  <SkeletonSwap
                    ready={ran}
                    lines={frame.lines.length + 1}
                    lineHeight={LINE}
                    reserve={(frame.lines.length + 1) * LINE}
                    className="font-mono text-[12.5px] leading-[22px] whitespace-pre"
                  >
                    {/* One box per line, so the frame height stays integer. The
                      `shiki` class is on the line rather than on the well: its
                      `span` rule forces a transparent background, which would
                      erase the block cursor below. */}
                    {frame.lines.map((line, i) => (
                      <div
                        // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal transcript
                        key={i}
                        className="shiki"
                        style={{ height: LINE }}
                        // Shiki output, generated from literals in this repo.
                        dangerouslySetInnerHTML={{ __html: line.html }}
                      />
                    ))}
                    <div className="text-ink-3" style={{ height: LINE }}>
                      {'$ '}
                      <span className="ml-px inline-block h-[13px] w-[7px] bg-ink align-[-1px]" />
                    </div>
                  </SkeletonSwap>
                </motion.div>
              </motion.div>

              {step.note ? (
                <p className="mt-2 min-h-[3.4rem] max-w-[62ch] px-2.5 pb-1 text-[13px] leading-relaxed text-ink-2">
                  {step.note}
                </p>
              ) : null}
            </figure>
          </div>
        </div>
      </div>

      {/* Which frame is on screen depends on scroll position, and a screen reader
          has no scroll position. This is every session, whole, once. */}
      <pre className="sr-only">
        {frames.map((f) => f.lines.map((l) => l.text).join('\n')).join('\n\n')}
      </pre>
    </>
  );
}
