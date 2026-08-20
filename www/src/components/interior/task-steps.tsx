'use client';

/**
 * `useTaskSteps` — the step-status state machine, vendored from interior.dev.
 *
 * NO LONGER VERBATIM. The upstream file also exported a `TaskSteps` view
 * component (plus its `TaskStepsProps` type and three inline SVG icons); that
 * view was replaced by `@/components/stage-list`, which needs clickable rows and
 * theme-aware tokens the vendored one could not express — see the reasoning in
 * `stage-list.tsx`. The view was left in place after the swap and became dead
 * code: nothing imported `TaskSteps`, and the only references to the name were
 * comments in `stage-list.tsx` and `terminal.tsx` saying it had been replaced.
 *
 * Removing it took the whole render half of the file with it, because none of it
 * had another consumer: the `react` and `motion/react` imports, the three
 * animation constants, and the `Tick`/`Cross`/`Arc` icons were all used only by
 * the deleted view. What is left is pure state — no imports, no JSX.
 *
 * So a re-sync against upstream is a manual merge rather than a copy. That is
 * the deliberate trade: the view will never come back, and carrying ~180 lines
 * of unreachable code to preserve a copy-paste property is the worse deal.
 */

export type TaskStep = {
  id: string;
  label: string;

  meta?: string;
};

export type TaskStepStatus = 'pending' | 'active' | 'done' | 'error';

export type UseTaskStepsOptions = {
  steps: TaskStep[];

  current: number;

  failed?: boolean;
};

function taskStepStatus(
  index: number,
  current: number,
  failed: boolean,
  complete: boolean,
): TaskStepStatus {
  if (index < current) return 'done';
  if (index !== current) return 'pending';
  if (failed) return 'error';
  return complete ? 'pending' : 'active';
}

export function useTaskSteps({ steps, current, failed = false }: UseTaskStepsOptions) {
  const complete = !failed && current >= steps.length;

  const rows = steps.map((step, i) => ({
    ...step,
    status: taskStepStatus(i, current, failed, complete),
  }));

  const active = rows.find((r) => r.status === 'active');
  const sentence = failed
    ? `Failed at ${steps[Math.min(current, steps.length - 1)]?.label ?? 'step'}`
    : complete
      ? `All ${steps.length} steps complete`
      : active
        ? `${active.label}, step ${current + 1} of ${steps.length}`
        : '';

  return { rows, complete, failed, sentence };
}
