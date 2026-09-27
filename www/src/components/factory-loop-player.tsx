"use client";

/**
 * Adapted from Assistant UI's TerminalBlock:
 * https://github.com/assistant-ui/assistant-ui/blob/main/packages/ui/src/components/elements/terminal-block.tsx
 *
 * MIT License
 *
 * Copyright (c) 2025 AgentbaseAI Inc.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { StageList } from "@/components/stage-list";
import type { SessionLine } from "@/lib/highlight";

const FIRST_LINE_DELAY = 650;
const LINE_DELAY = 320;
const STAGE_DELAY = 1100;
const ENTER = { duration: 0.3, ease: [0.16, 1, 0.3, 1] } as const;

export type FactoryStage = {
  title: string;
  owner: string;
  ownerShort: string;
  lines: readonly SessionLine[];
};

type FactoryLine = {
  stage: FactoryStage;
  stageIndex: number;
  line: SessionLine;
  lineIndex: number;
};

/** Trusted Shiki markup produced by `highlightSession` on the server. */
function HighlightedLine({ line }: { line: SessionLine }) {
  return (
    <span className="shiki" dangerouslySetInnerHTML={{ __html: line.html }} />
  );
}

function TerminalBlock({
  stages,
  lines,
  visibleCount,
  done,
  reduced,
  onSelectStage,
}: {
  stages: readonly FactoryStage[];
  lines: readonly FactoryLine[];
  visibleCount: number;
  done: boolean;
  reduced: boolean;
  onSelectStage: (index: number) => void;
}) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const lastVisible = lines[Math.max(0, visibleCount - 1)];
  const activeStage = lastVisible?.stageIndex ?? 0;
  const taskStages = useMemo(
    () =>
      stages.map((stage) => ({
        id: stage.title.toLowerCase(),
        label: stage.title,
        meta: stage.ownerShort,
      })),
    [stages]
  );

  useEffect(() => {
    const viewport = viewportRef.current;
    if (viewport) {
      viewport.scrollTo({
        behavior: reduced ? "auto" : "smooth",
        top: viewport.scrollHeight,
      });
    }
  }, [visibleCount, reduced]);

  const statusKey = done ? "complete" : (stages[activeStage]?.title ?? "empty");
  const selectStage = (index: number) => {
    if (reduced) {
      const viewport = viewportRef.current;
      const target = viewport?.querySelector<HTMLElement>(
        `[data-stage="${index}"]`
      );
      if (viewport && target) {
        viewport.scrollTop +=
          target.getBoundingClientRect().top -
          viewport.getBoundingClientRect().top;
      }
      return;
    }
    onSelectStage(index);
  };

  return (
    <figure className="mat-panel rounded-[14px] p-[5px]">
      <figcaption className="flex min-h-8 items-center gap-3 pr-[3px] pl-2.5 font-mono text-[11.5px]">
        <span className="meta text-ink-3 min-w-0 flex-1">
          factory loop · five stages
        </span>
        <AnimatePresence initial={false} mode="wait">
          <motion.span
            animate={{ opacity: 1, y: 0 }}
            aria-hidden="true"
            className="meta text-ink-3 flex shrink-0 items-center gap-1.5"
            exit={reduced ? { opacity: 0 } : { opacity: 0, y: -2 }}
            initial={reduced ? false : { opacity: 0, y: 2 }}
            key={statusKey}
            transition={reduced ? { duration: 0 } : ENTER}
          >
            {done ? (
              <>
                ✓ {stages.length}/{stages.length} complete
              </>
            ) : (
              <>
                <span className="bg-accent size-[6px] rounded-[1.5px]" />
                {activeStage + 1}/{stages.length} · {stages[activeStage]?.title}
              </>
            )}
          </motion.span>
        </AnimatePresence>
      </figcaption>

      <div className="grid gap-2 sm:grid-cols-[170px_minmax(0,1fr)]">
        <div className="px-1 py-1 sm:py-3">
          <StageList
            current={done ? stages.length : activeStage}
            label="Factory loop stages"
            onSelect={selectStage}
            stages={taskStages}
          />
        </div>

        <div
          aria-label="Factory loop transcript, scrollable"
          className="mat-well scroll-inset text-ink-2 focus-visible:ring-accent h-[208px] overflow-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[22px] focus-visible:ring-2 focus-visible:outline-none focus-visible:ring-inset sm:h-[264px]"
          ref={viewportRef}
          role="group"
          tabIndex={0}
        >
          <div aria-hidden="true">
            <AnimatePresence initial={false}>
              {lines.slice(0, visibleCount).map((entry) => (
                <motion.div
                  animate={{ opacity: 1, y: 0 }}
                  data-stage={entry.stageIndex}
                  exit={reduced ? { opacity: 0 } : { opacity: 0, y: -2 }}
                  initial={reduced ? false : { opacity: 0, y: 4 }}
                  key={`${entry.stage.title}-${entry.lineIndex}`}
                  layout="position"
                  transition={reduced ? { duration: 0 } : ENTER}
                >
                  {entry.lineIndex === 0 && (
                    <p
                      className={`meta text-ink-3 mb-1 ${entry.stageIndex === 0 ? "" : "mt-2"}`}
                    >
                      {entry.stage.title} · {entry.stage.owner}
                    </p>
                  )}
                  <div className="min-h-[22px] whitespace-pre">
                    <HighlightedLine line={entry.line} />
                  </div>
                </motion.div>
              ))}
            </AnimatePresence>

            {!done && (
              <motion.span
                animate={reduced ? { opacity: 1 } : { opacity: [1, 1, 0, 0] }}
                className="bg-accent mt-1 inline-block h-[14px] w-[6px]"
                transition={
                  reduced
                    ? { duration: 0 }
                    : {
                        duration: 1.06,
                        ease: "linear",
                        repeat: Infinity,
                        times: [0, 0.5, 0.5, 1],
                      }
                }
              />
            )}
          </div>
        </div>
      </div>

      <div className="sr-only">
        {stages.map((stage) => (
          <section key={stage.title}>
            <h3>
              {stage.title} · {stage.owner}
            </h3>
            {stage.lines.map((line, index) => (
              <p key={`${index}-${line.text}`}>{line.text}</p>
            ))}
          </section>
        ))}
      </div>
    </figure>
  );
}

export function FactoryLoopPlayer({
  stages,
}: {
  stages: readonly FactoryStage[];
}) {
  const reduced = useReducedMotion() === true;
  const [visibleCount, setVisibleCount] = useState(0);
  const [playback, setPlayback] = useState({ epoch: 0, start: 0 });
  const lines = useMemo(
    () =>
      stages.flatMap((stage, stageIndex) =>
        stage.lines.map((line, lineIndex) => ({
          stage,
          stageIndex,
          line,
          lineIndex,
        }))
      ),
    [stages]
  );
  const stageStarts = useMemo(() => {
    let offset = 0;
    return stages.map((stage) => {
      const start = offset;
      offset += stage.lines.length;
      return start;
    });
  }, [stages]);

  const selectStage = useCallback(
    (index: number) => {
      const start = stageStarts[index];
      if (start === undefined) return;
      setPlayback((current) => ({
        epoch: current.epoch + 1,
        start: start + 1,
      }));
    },
    [stageStarts]
  );

  useEffect(() => {
    if (reduced || lines.length === 0) {
      setVisibleCount(lines.length);
      return;
    }

    setVisibleCount(playback.start);
    let count = playback.start;
    let timer: ReturnType<typeof setTimeout>;

    const reveal = () => {
      count += 1;
      setVisibleCount(count);
      if (count < lines.length) {
        const nextStartsStage = lines[count]?.lineIndex === 0;
        timer = setTimeout(reveal, nextStartsStage ? STAGE_DELAY : LINE_DELAY);
      }
    };

    if (count < lines.length) {
      timer = setTimeout(reveal, count === 0 ? FIRST_LINE_DELAY : LINE_DELAY);
    }
    return () => clearTimeout(timer);
  }, [lines, playback, reduced]);

  const shown = reduced ? lines.length : visibleCount;

  return (
    <TerminalBlock
      done={shown >= lines.length}
      lines={lines}
      onSelectStage={selectStage}
      reduced={reduced}
      stages={stages}
      visibleCount={shown}
    />
  );
}
