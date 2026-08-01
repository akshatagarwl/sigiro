import { SteppedTerminal, type TerminalStep } from '@/components/terminal';
import { LOOP } from '@/lib/site';

/**
 * Where sigiro sits in a software factory, as five terminals you step through.
 *
 * The five commands are one incident in order: a +48σ shift on checkout, the
 * suspect deploy, the agent's patch, the recovery, the playbook. Two of them
 * visibly run something that is not sigiro, which is the argument the section
 * exists to make — it is watched rather than asserted.
 *
 * All this file does now is map `LOOP` onto the terminal's shape and supply the
 * fallback. The scroll behaviour lives in `components/terminal.tsx`, where a
 * second section can reach it.
 */

/**
 * One word per owner, for the task list's narrow meta column. The full phrasing
 * still runs in the caption chip, where there is room for it.
 */
const SHORT_OWNER: Record<string, string> = {
  sigiro: 'sigiro',
  'your coding agent': 'agent',
  'your pipeline, on our evidence': 'pipeline',
  'your memory': 'memory',
};

const STEPS: TerminalStep[] = LOOP.map((s) => ({
  cmd: s.cmd,
  out: s.out,
  label: s.title,
  badge: s.owner,
  badgeStrong: Boolean(s.ours),
  badgeShort: SHORT_OWNER[s.owner] ?? s.owner,
  note: s.say,
}));

/**
 * What reduced-motion and no-JS readers get: the same five sessions, printed in
 * full, with the prose each one belongs to. Nothing is behind an interaction
 * they cannot perform.
 */
function Stacked() {
  return (
    <ol className="mt-8">
      {LOOP.map((s) => (
        <li key={s.title} className="border-t border-hairline py-9 first:border-t-0">
          <p className="flex items-center gap-2.5">
            {/* Ink and depth, not accent: "this stage is ours" is a persistent
                state, and if every marked thing were blue, focus would stop
                meaning anything. */}
            <span
              className={`mat-cap inline-flex h-[22px] items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                s.ours ? 'text-ink' : 'text-ink-3'
              }`}
            >
              {s.owner}
            </span>
          </p>
          <h3 className="mt-3.5 text-[20px] font-medium tracking-[-0.03em] text-ink">{s.title}</h3>
          <p className="mt-2.5 max-w-[62ch] text-[13.5px] leading-relaxed text-ink-2">{s.say}</p>
          <figure className="mat-panel mt-5 rounded-[14px] p-[5px]">
            <figcaption className="meta flex h-8 items-center pl-2.5 text-ink-3">
              your shell
            </figcaption>
            <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[22px]">
              <p className="flex gap-2 whitespace-pre text-ink">
                <span aria-hidden className="shrink-0 text-ink-3">
                  $
                </span>
                <span>{s.cmd}</span>
              </p>
              <pre className="whitespace-pre text-ink-2">{s.out}</pre>
            </div>
          </figure>
        </li>
      ))}
    </ol>
  );
}

export function FactoryLoop() {
  return (
    <div className="mt-8">
      <SteppedTerminal steps={STEPS} fallback={<Stacked />} />
    </div>
  );
}
