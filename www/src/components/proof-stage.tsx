'use client';

import { useMachine } from '@xstate/react';
import { motion, useInView, useReducedMotion } from 'motion/react';
import { useEffect, useRef } from 'react';
import { createMachine } from 'xstate';
import { CopySource } from '@/components/site/copy-source';

/**
 * The 04 Prove demo, staged.
 *
 * Every value here is looked up from interior's DESIGN.md rather than chosen:
 *
 * - §5a  two curves only. EASE is the arriving one; nothing here departs.
 * - §5   entrance is 0.22s at `opacity 0, scale 0.97, y 10, blur(6px)`.
 * - §5   disclose is 0.28s of height with opacity trailing at 0.18s.
 * - §5d  "physics beats taste" — rows are a query returning, which is a real
 *        process, so they arrive on a linear cadence rather than an eased one.
 * - §3   no idle loops. This plays once on entry and holds. Re-watching is a
 *        button, never a rewind on re-scroll.
 * - §22  reduced motion delivers the information and skips the trip.
 *
 * The one thing not in the spec is the 50ms row cadence, which is picked.
 *
 * SSR and no-JS get the finished artifact, because `still` is the machine's
 * initial state and only the mount effect leaves it. That ordering is deliberate
 * rather than incidental: it means the complete evidence is in the HTML for
 * anyone whose JavaScript never runs, and the hide-then-play flip happens while
 * section 04 is still far below the fold, so nobody watches it blink.
 */

const EASE = [0.23, 1, 0.32, 1] as const;
const ENTER = { duration: 0.22, ease: EASE };
const HIDDEN = { opacity: 0, scale: 0.97, y: 10, filter: 'blur(6px)' };
const SHOWN = { opacity: 1, scale: 1, y: 0, filter: 'blur(0px)' };

/** Height and opacity are separate tracks so opacity can trail, per §5. */
const DISCLOSE = {
  height: { duration: 0.28, ease: EASE },
  opacity: { duration: 0.18, ease: EASE, delay: 0.1 },
};

const ROW_CADENCE = 0.05;

/** ms after entering view. The SQL lands, then the rows it returns. */
const AT_SQL = 260;
const AT_ROWS = 700;

const REPLAY = { REPLAY: { target: 'finding', reenter: true } } as const;

/**
 * The staging, as five states rather than an `armed` flag, a `step` number, a
 * `replay` counter and two effects reading all three.
 *
 * `still` is the one that carries the SSR promise in the docblock above: it is
 * the initial state, it renders the finished artifact, and only the mount effect
 * leaves it. Reduced motion never sends `ARM`, so it stays there — which is §22,
 * expressed as an unreachable subtree rather than as a branch in every animate
 * prop.
 *
 * `waiting` is armed but below the fold: collapsed, and going nowhere until
 * `SEEN`. Replay is only reachable from the three playing states, which is where
 * the button is.
 */
const staging = createMachine({
  initial: 'still',
  states: {
    still: { on: { ARM: { target: 'waiting' } } },
    waiting: { on: { SEEN: { target: 'finding' } } },
    finding: { after: { [AT_SQL]: { target: 'sql' } }, on: REPLAY },
    sql: { after: { [AT_ROWS - AT_SQL]: { target: 'rows' } }, on: REPLAY },
    rows: { on: REPLAY },
  },
});

/** How many of the three panels are open. `still` opens all of them. */
const OPENED = { still: 2, waiting: 0, finding: 0, sql: 1, rows: 2 } as const;

export function ProofStage({
  finding,
  sql,
  sqlSource,
  rows,
}: {
  finding: React.ReactNode;
  sql: React.ReactNode;
  /** Plain text behind `sql`, for the clipboard. */
  sqlSource: string;
  rows: React.ReactNode[];
}) {
  const reduced = useReducedMotion();
  const ref = useRef<HTMLDivElement>(null);
  const inView = useInView(ref, { once: true, amount: 0.35 });

  const [state, send] = useMachine(staging);

  // Declared before the one below it, so on a mount that is already in view the
  // machine arms before it is told what it can see. Swapping them would leave it
  // in `still` forever.
  useEffect(() => {
    if (reduced) return;
    send({ type: 'ARM' });
  }, [reduced, send]);

  useEffect(() => {
    if (inView) send({ type: 'SEEN' });
  }, [inView, send]);

  const still = state.matches('still');
  const step = OPENED[state.value];
  const armed = !still;

  return (
    <figure ref={ref} className="not-prose mat-panel my-6 rounded-[14px] p-[5px]">
      <figcaption className="flex h-8 items-center gap-2.5 pr-[3px] pl-2.5">
        <span className="meta min-w-0 flex-1 truncate text-ink-3">
          POST /v1/investigate · one finding
        </span>
        {armed ? (
          <button
            type="button"
            onClick={() => send({ type: 'REPLAY' })}
            className="mat-cap h-[22px] shrink-0 rounded-[6px] px-2 text-[11px] text-ink-3 transition-colors duration-150 hover:text-accent focus-visible:text-accent"
          >
            Replay
          </button>
        ) : null}
      </figcaption>

      <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[1.7]">
        {finding}
      </div>

      {/* Stage two: the query the finding just handed you, as its own call. */}
      <motion.div
        initial={false}
        animate={still || step >= 1 ? { height: 'auto', opacity: 1 } : { height: 0, opacity: 0 }}
        transition={DISCLOSE}
        className="overflow-hidden"
      >
        {/* The copy cap belongs here rather than on the outer caption: the SQL is
            the one thing on this page a reader is actually meant to take away. */}
        <div className="flex h-8 items-center gap-2.5 pr-[3px] pl-2.5">
          <span className="meta min-w-0 flex-1 truncate text-ink-3">POST /v1/query · pasted</span>
          <CopySource code={sqlSource} label="the query" />
        </div>
        <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[1.7]">
          {sql}
        </div>
      </motion.div>

      {/* Stage three: what came back. */}
      <motion.div
        initial={false}
        animate={still || step >= 2 ? { height: 'auto', opacity: 1 } : { height: 0, opacity: 0 }}
        transition={DISCLOSE}
        className="overflow-hidden"
      >
        <p className="meta flex h-8 items-center pl-2.5 text-ink-3">
          {rows.length} rows · the claim, checked
        </p>
        <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[1.7]">
          {rows.map((row, i) => (
            <motion.div
              // biome-ignore lint/suspicious/noArrayIndexKey: fixed literal list
              key={i}
              initial={false}
              animate={still || step >= 2 ? SHOWN : HIDDEN}
              transition={{ ...ENTER, delay: still ? 0 : i * ROW_CADENCE }}
              className="whitespace-nowrap"
            >
              {row}
            </motion.div>
          ))}
        </div>
      </motion.div>
    </figure>
  );
}
