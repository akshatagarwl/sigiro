import { Effect } from 'effect';
import { SteppedTerminal, type TerminalStep } from '@/components/terminal';
import { highlightSession, type SessionLine } from '@/lib/highlight';
import { HOW } from '@/lib/site';

/**
 * Connect, ask, correlate — as three terminals you step through.
 *
 * The same job `factory-loop.tsx` does for `LOOP`: map the data onto the
 * terminal's shape, run Shiki over each session on the server, and supply the
 * no-JS fallback. It is a server component for that middle job — the terminal
 * animates and so must be a client component, and Shiki must not follow it into
 * the browser.
 *
 * The one thing this section needs that the loop does not is several commands
 * per step, which is what `transcript` is for.
 */

/** A line with nothing on it, where the shell left a gap. */
const BLANK: SessionLine = { text: '', html: '' };

/**
 * One step's commands, highlighted into a single continuous transcript.
 *
 * `highlightSession` covers one command and its output, which is all the loop
 * section ever needs. A step here is often four commands, so the sessions are
 * concatenated. Two rules keep the result reading like a real shell: a command
 * that prints nothing keeps only its prompt line, and a blank line goes in
 * wherever output was printed before the next prompt.
 */
function transcript(blocks: readonly { readonly cmd: string; readonly out: string }[]) {
  return Effect.map(
    Effect.all(
      blocks.map((b) => highlightSession(b.cmd, b.out)),
      { concurrency: 'unbounded' },
    ),
    (sessions): SessionLine[] =>
      sessions.flatMap((lines, i) => {
        const kept = blocks[i].out ? lines : lines.slice(0, 1);
        return i > 0 && blocks[i - 1].out ? [BLANK, ...kept] : kept;
      }),
  );
}

/**
 * What no-JS readers get: the same three sessions, printed in full, with the
 * prose each one belongs to. Nothing is behind an interaction they cannot
 * perform.
 *
 * The transcript is one `pre` rather than a row per line — a `pre` keeps the
 * blank lines inside the session, which a stack of empty boxes would collapse.
 */
function Stacked({ sessions }: { sessions: readonly (readonly SessionLine[])[] }) {
  return (
    <ol className="mt-8">
      {HOW.map((s, i) => (
        <li key={s.title} className="border-t border-hairline py-9 first:border-t-0">
          <h3 className="text-[20px] font-medium tracking-[-0.03em] text-ink">{s.title}</h3>
          <p className="mt-2.5 max-w-[62ch] text-[13.5px] leading-relaxed text-ink-2">{s.say}</p>
          <figure className="mat-panel mt-5 rounded-[14px] p-[5px]">
            <figcaption className="meta flex h-8 items-center pl-2.5 text-ink-3">
              your shell
            </figcaption>
            <div className="mat-well scroll-inset overflow-x-auto rounded-[9px] px-3.5 py-3 font-mono text-[12.5px] leading-[22px]">
              <pre
                className="shiki whitespace-pre"
                // Shiki output, generated from literals in this repo.
                dangerouslySetInnerHTML={{
                  __html: sessions[i].map((line) => line.html).join('\n'),
                }}
              />
            </div>
          </figure>
        </li>
      ))}
    </ol>
  );
}

export async function HowItWorks() {
  const sessions = await Effect.runPromise(
    Effect.all(
      HOW.map((s) => transcript(s.run)),
      { concurrency: 'unbounded' },
    ),
  );

  // No badge: every one of these is your own shell, and a chip that reads the
  // same on every step is furniture rather than information. The caption already
  // carries the step name and the count.
  const steps: TerminalStep[] = HOW.map((s, i) => ({
    lines: sessions[i],
    label: s.title,
    note: s.say,
  }));

  return (
    <div className="mt-8">
      {/* 30vh per step, not the component's 45vh default. The loop section earns
          the longer travel because each of its stages is an argument; these three
          are one shell session each, and they replaced three static code blocks
          that cost 436px apiece. The track is still the reason this section is
          longer than what it replaced — that is inherent to a pinned sequence. */}
      <SteppedTerminal
        steps={steps}
        label="The four steps"
        heading={{ label: 'how you use it', title: 'Four steps' }}
        track={`calc(100vh + ${steps.length * 30}vh)`}
        fallback={<Stacked sessions={sessions} />}
      />
    </div>
  );
}
