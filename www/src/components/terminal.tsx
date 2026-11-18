'use client';

import {
  motion,
  useMotionTemplate,
  useMotionValueEvent,
  useScroll,
  useTransform,
} from 'motion/react';
import { useCallback, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { StageList } from '@/components/stage-list';
import type { SessionLine } from '@/lib/highlight';

/**
 * One terminal per step, stepped by scroll, with a task list beside it.
 *
 * An earlier version typed the transcript out character by character, bound to
 * scroll position. It was a good demo of scrubbing and a bad terminal: real
 * shells do not type their own output. You run a command and the output lands.
 * So nothing types, and nothing waits either: the output is there on the frame
 * you scroll to.
 *
 * There used to be a beat before it. A `SkeletonSwap` held grey bars for a
 * minimum of 380ms on arrival at every frame, on the argument that a command
 * takes a moment to run. It is gone. The reader is not waiting on a command here
 * — the transcript is a literal in this repo, highlighted at build time — so the
 * bars were animating a wait that did not exist, and scrubbing back through a
 * frame you had already read replayed it. A transition that shows you less than
 * the frame you left is a cost with nothing bought.
 *
 * Removing it took the machinery with it: the skeleton's `ready` prop was the
 * only reader of a two-state XState machine and its 300ms delay, and with the
 * timer gone the machine had one state. What it did beyond the timer — drop a
 * step that names the frame already shown, so the scroll listener can fire every
 * frame and React still renders once per step — is `useState`'s own bail-out on
 * an unchanged value. So `xstate` and `@xstate/react` left with it.
 *
 * `StageList` carries the argument the section exists to make. Five stages, the
 * ones you have passed ticked off, the one you are on running, the rest waiting
 * — and `meta` naming the owner of each, so "two of these five are ours" is
 * something you watch accumulate rather than a claim in a caption. Every row is
 * a button: `goToStage` below turns a click into a scroll position, so the rail
 * navigates the sequence instead of only reporting on it.
 *
 * It renders the vendored `useTaskSteps` hook's rows with our own markup. The
 * vendored `TaskSteps` view could not take a click handler and is kept verbatim
 * under §1b, so the hook/view split is where this parts company from it — see the
 * note in `stage-list.tsx`.
 *
 * Design constraints, from DESIGN.md (now vendored at www/DESIGN.md):
 * - §5h  React does not re-render at 60fps. The step index is React state; the
 *        handoff is MotionValues off `scrollYProgress` and never touches React at
 *        all. A previous version quantised progress to two decimals and drove the
 *        fade from state, leaving the handoff ~6 frames long — §6a: "quantising
 *        the sweep would make the fill look like it was stuttering".
 * - §6a  `setShown` is called on every scroll frame and passed the same index most
 *        of them. React compares with `Object.is` and bails out, so the listener
 *        can fire at frame rate and this still renders once per step. No guard is
 *        written for it because the guard is the hook's own behaviour.
 * - §5   the handoff uses the specified displacements — out at `y 6, blur(3px)`,
 *        in at `y 10, blur(6px)`.
 * - §3   no `rounded-full`, so no traffic-light chrome. This is the same
 *        `mat-well` cut into a `mat-panel` as every code block on the site —
 *        and, since `highlightSession`, wearing the same Shiki colours. The
 *        markup arrives already highlighted: this component is a client
 *        component and Shiki is a server-side dependency, so what crosses is
 *        one string of HTML per line, never a highlighter.
 * - §5h  a scroll-driven component needs no reduced-motion branch — every value
 *        resolves on the frame it is scrolled to. The no-JS fallback is a
 *        `<noscript>` in the calling `.astro` file, deliberately outside this
 *        component: it used to be a `fallback` prop, which meant the whole stacked
 *        transcript was serialised into the island's props to be shown to readers
 *        who by definition never load the island.
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

/*
 * An XState machine stood here. It had two states, `running` and `ready`, a 300ms
 * delayed transition between them, and a `STEP` event that re-entered `running` so
 * scrubbing could not stack timers. All of it existed to tell the skeleton when a
 * frame's command had "finished", and the skeleton is gone.
 *
 * What is left of the discrete half is one number: which frame is mounted. That is
 * `useState` below. Everything continuous — opacity, y, blur, height — is a
 * MotionValue and never was state (§5h).
 */

export function SteppedTerminal({
  steps,
  label,
  heading,
  track,
}: {
  steps: readonly TerminalStep[];
  /**
   * Names the task list for a screen reader. Required, because it was hardcoded
   * to "The five stages" while this component had one caller, and the second
   * caller — a three-step sequence — then announced the wrong count to anyone
   * who could not see it. A caller cannot forget a required prop.
   */
  label: string;
  /**
   * The section's whole header — label, headline, lede — pinned with the frame.
   *
   * This is the section's only header now. It used to be duplicated: the real one
   * in the page's `<section>`, plus a condensed one-line copy in here, marked
   * `aria-hidden` so a screen reader met the heading once. That was a workaround
   * for the header scrolling away the moment the frame pinned, and it meant the
   * argument the section exists to make was never on screen at the same time as
   * the thing making it.
   *
   * So it moved in whole, and the page keeps none of it. The duplicate is gone,
   * the `aria-hidden` with it, and `id` puts the real `h2` here for the section's
   * `aria-labelledby` to point at.
   *
   * `tail` is the muted half of the headline — "Five stages." in ink, then the
   * rest in `ink-3`. It is a separate field rather than markup in a string
   * because the copy has to stay copy.
   *
   * Height is the constraint this has to respect. The terminal is already ~477px
   * of a 720px viewport; the header adds roughly 130px more. See the note at the
   * sticky container for what gives way, and where it stops fitting.
   */
  heading?: { id: string; label: string; title: string; tail?: string; lede?: string };
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

  // The discrete half: which frame's text is mounted. One render per step —
  // `setShown` is handed the same index on most scroll frames and React bails out.
  const [shown, setShown] = useState(0);

  useMotionValueEvent(scrollYProgress, 'change', (v) => {
    const { index, handoff } = locate(v, steps.length);
    setShown(handoff >= 0.5 ? index + 1 : index);
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

  /**
   * Jump to a stage, by scrolling to where that stage is at rest.
   *
   * The rail cannot set the step directly, and that is not a limitation to work
   * around — scroll position IS the state here (§5h). Setting `shown` would put the
   * frame out of agreement with the scrollbar, and the next scroll event would
   * snap it back. So the click moves the page and the existing listener does the
   * rest, which means clicking and scrolling can never disagree.
   *
   * The arithmetic inverts `locate()`. `useScroll` with `offset: ['start start',
   * 'end end']` runs progress 0 → 1 as the track's top reaches the viewport top
   * and its bottom reaches the viewport bottom, so the scrollable span is
   * `trackHeight - viewportHeight`. Step `i` holds still while its fraction is
   * below HOLD, so `HOLD / 2` lands mid-hold — clear of the handoff at either end,
   * where the frame is mid-fade.
   */
  const goToStage = useCallback(
    (index: number) => {
      const track = ref.current;
      if (!track) return;
      const top = track.getBoundingClientRect().top + window.scrollY;
      const span = Math.max(0, track.offsetHeight - window.innerHeight);
      const progress = (index + HOLD / 2) / steps.length;
      window.scrollTo({
        top: top + span * progress,
        // A jump this long is disorienting without the sweep, but "smooth" is
        // exactly what a reduced-motion preference is asking us not to do.
        behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth',
      });
    },
    [steps.length],
  );

  return (
    <>
      <div
        ref={ref}
        className="relative"
        style={{ height: track ?? `calc(100vh + ${steps.length * 45}vh)` }}
      >
        {/* `min-h-screen`, not `h-screen`. The header and the frame together are
            close to a short viewport's whole height, and a fixed `h-screen` box
            whose content exceeds it centres the overflow out of both ends — the
            lede clipped at the top, the note at the bottom, with no way to scroll
            to either because the scrollport is the page. `min-h-screen` lets the
            box grow instead; past that point the section scrolls as a tall block
            rather than pinning, which degrades honestly.

            The gap tightens from 5 to 4 below `sm` for the same reason: on a phone
            every 4px is a line of the transcript. */}
        <div className="sticky top-0 flex min-h-screen flex-col justify-center gap-4 sm:gap-5">
          {/* The section's header, on screen for as long as the frame is pinned —
              which is the point. A reader arriving mid-scroll can see what the
              five stages are FOR while watching them run, instead of having read
              it two screens ago. */}
          {heading ? (
            <div className="px-1">
              <p className="label">{heading.label}</p>
              <h2
                id={heading.id}
                className="mt-2.5 max-w-[34ch] text-[22px] leading-[1.2] font-medium tracking-[-0.03em] text-ink sm:text-[27px]"
              >
                {heading.title}
                {/* `{' '}` and not a space inside the span or a newline between
                    them. A newline between JSX text and an element collapses to
                    nothing, which is exactly how the page's own markup shipped
                    "Five stages.A repair loop is not complete…" to production. An
                    explicit expression cannot be reformatted away. */}
                {heading.tail ? (
                  <>
                    {' '}
                    <span className="text-ink-3">{heading.tail}</span>
                  </>
                ) : null}
              </h2>
              {heading.lede ? (
                <p className="mt-2.5 max-w-[62ch] text-[13.5px] leading-relaxed text-ink-2 sm:text-[15px]">
                  {heading.lede}
                </p>
              ) : null}
            </div>
          ) : null}

          <div className="flex flex-col gap-5 lg:grid lg:grid-cols-[190px_1fr] lg:items-center">
            {/* The rail says which stage you are on, and now moves you to any
                other one. The heading above says which section they belong to. */}
            <StageList stages={tasks} current={shown} label={label} onSelect={goToStage} />

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
                here because the div inside is itself the scroll container. Putting
                `overflow-x-auto` here as well was tried and is dead config —
                scrollWidth equals clientWidth, so it can never scroll. */}
              <motion.div
                className="mat-well overflow-hidden rounded-[9px] px-3.5 py-3"
                style={{ height }}
              >
                {/* The x scroller. At a 390px viewport this well is 324px while
                  the widest transcript row is 495px, so something has to scroll
                  sideways — it used to be SkeletonSwap's shell, which was the one
                  load-bearing thing that component did beyond the skeleton itself.
                  Dropping it without moving the scroll here would have clipped the
                  end of every long line on a phone with no way to reach it.

                  `tabIndex` unconditionally, not measured: this is a scroll
                  container on every viewport narrow enough to need one, and the
                  vendored shell's own `ResizeObserver` only ever detected the
                  vertical case — so the clipped end of a line was unreachable by
                  keyboard. A tab stop on a scrollable region is what makes it
                  reachable, and it is cheaper than measuring. */}
                <motion.div
                  tabIndex={0}
                  className="overflow-x-auto overscroll-contain font-mono text-[12.5px] leading-[22px] whitespace-pre"
                  style={{ opacity, y, filter }}
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
