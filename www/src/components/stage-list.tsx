"use client";

import { motion, useReducedMotion } from "motion/react";

import { useTaskSteps } from "@/components/interior/task-steps";

/**
 * The stepped terminal's stage rail, as buttons.
 *
 * This replaces the vendored `TaskSteps` *view* and keeps its `useTaskSteps`
 * *hook*, which is exactly the seam that split exists for: the hook owns the
 * pending/active/done/error derivation and the live-region sentence, and nothing
 * about that needed changing.
 *
 * Two reasons the view had to be ours rather than a prop on the vendored one:
 *
 * 1. **Each stage has to be clickable.** `TaskSteps` renders plain `<li>` rows and
 *    takes no selection callback, so the affordance cannot be passed in — the
 *    element that must become a button is inside the component. Editing it in
 *    place would fork a file DESIGN.md §1b keeps verbatim.
 * 2. **`textContent` read "Detectsigiro".** The label and meta were adjacent spans
 *    with no separator between them. Flex laid them out correctly, so it was
 *    invisible on screen and wrong everywhere text is extracted rather than
 *    rendered — copy-paste, and anything reading the page as text.
 *
 * A side benefit of owning it: the vendored version writes literal `stone-*` and
 * `emerald-*` values, which §1b requires of a copied component and which have
 * never quite been this site's greys. These rows use the ink ramp and `--moss`, so
 * they follow the theme like everything else.
 */

export type Stage = {
  id: string;
  label: string;
  /** Who owns this stage. Shown once it is done — see the width note below. */
  meta?: string;
};

export function StageList({
  stages,
  current,
  label,
  onSelect,
}: {
  stages: Stage[];
  current: number;
  /** Names the rail for a screen reader. */
  label: string;
  /** Called with the stage index to move to. */
  onSelect: (index: number) => void;
}) {
  const { rows, complete, sentence } = useTaskSteps({ steps: stages, current });
  const reduced = useReducedMotion() === true;

  return (
    <div className="w-full">
      <ol aria-label={label} className="space-y-0.5">
        {rows.map((row, i) => {
          const tone =
            row.status === "done"
              ? "text-ink-2"
              : row.status === "active"
                ? "font-medium text-ink"
                : "text-ink-3";

          return (
            <li key={row.id}>
              {/*
                The whole row is the target, not just the words: a 12.5px label is
                a poor thing to hit, and the row is already the unit the eye reads.
                `h-7` is the vendored row height, kept so the rail's rhythm and the
                terminal beside it still line up.

                `aria-current="step"` stays on the element that carries the state,
                which is now the button.
              */}
              <button
                type="button"
                aria-current={row.status === "active" ? "step" : undefined}
                onClick={() => onSelect(i)}
                className="press hover:bg-well focus-visible:outline-accent flex h-7 w-full items-center gap-2.5 rounded-[6px] px-1 text-left focus-visible:outline-2 focus-visible:outline-offset-1"
              >
                <span className="relative grid size-4 shrink-0 place-items-center">
                  {row.status === "done" ? (
                    <span className="text-moss grid size-4 place-items-center rounded-[5px] bg-[color-mix(in_oklab,var(--moss)_16%,transparent)]">
                      <svg
                        viewBox="0 0 256 256"
                        width="11"
                        height="11"
                        fill="none"
                        aria-hidden
                      >
                        <polyline
                          points="216 72 104 184 48 128"
                          stroke="currentColor"
                          strokeWidth="26"
                          strokeLinecap="round"
                          strokeLinejoin="round"
                        />
                      </svg>
                    </span>
                  ) : row.status === "active" ? (
                    <motion.svg
                      viewBox="0 0 16 16"
                      className="text-ink-3 size-3"
                      aria-hidden
                      animate={reduced ? { rotate: 0 } : { rotate: 360 }}
                      transition={
                        reduced
                          ? { duration: 0 }
                          : { duration: 0.8, ease: "linear", repeat: Infinity }
                      }
                    >
                      <circle
                        cx="8"
                        cy="8"
                        r="6"
                        fill="none"
                        stroke="currentColor"
                        strokeOpacity="0.25"
                        strokeWidth="2"
                      />
                      <path
                        d="M8 2 a6 6 0 0 1 6 6"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                        strokeLinecap="round"
                      />
                    </motion.svg>
                  ) : (
                    <span className="bg-hairline-strong size-[5px] rounded-[2px]" />
                  )}
                </span>

                {row.status === "active" && !reduced ? (
                  /* The sweep marks which stage is running. Our ink ramp rather
                     than the vendored literal stones, so it flips with the theme
                     instead of being tuned for one. */
                  <motion.span
                    className="min-w-0 flex-1 truncate bg-[linear-gradient(90deg,var(--ink-3)_38%,var(--ink)_50%,var(--ink-3)_62%)] [background-size:220%_100%] bg-clip-text text-[12.5px] font-medium text-transparent"
                    animate={{ backgroundPosition: ["120% 0", "-120% 0"] }}
                    transition={{
                      duration: 1.6,
                      ease: "linear",
                      repeat: Infinity,
                    }}
                  >
                    {row.label}
                  </motion.span>
                ) : (
                  <span
                    className={`min-w-0 flex-1 truncate text-[12.5px] transition-colors duration-200 ${tone}`}
                  >
                    {row.label}
                  </span>
                )}

                {row.meta ? (
                  /*
                    Reserved, not conditional. The owner appears only once a stage
                    is done, but the column holds its width the whole time —
                    rendering it conditionally would resize every label the moment
                    a stage completed, which is a layout shift mid-scroll.

                    The `sr-only` separator is the fix for `textContent` reading
                    "Detectsigiro": two adjacent spans have nothing between them,
                    and a plain space would become an anonymous flex item with
                    width of its own. It shares the `aria-hidden` state of the
                    value it introduces, so a screen reader hears "Detect, sigiro"
                    once the stage is done and just "Detect" before that.
                  */
                  <span
                    aria-hidden={row.status !== "done"}
                    className={`shrink-0 transition-opacity duration-200 ${
                      row.status === "done" ? "opacity-100" : "opacity-0"
                    }`}
                  >
                    <span className="sr-only">, </span>
                    <span className="meta text-ink-3">{row.meta}</span>
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ol>

      {/* The hook's own sentence, announced politely. Unthrottled it would fire on
          every step of a scroll; the terminal only re-renders once per stage, so
          this is already one announcement per stage. */}
      <span role="status" className="sr-only">
        {complete ? "All stages complete" : sentence}
      </span>
    </div>
  );
}
