import { ScrollSpy } from '@/components/interior/scroll-spy';
import { LOOP } from '@/lib/site';

/**
 * Where sigiro sits in a software factory, told by scrolling.
 *
 * The rail is `interior/scroll-spy`: a well holding one chip per stage, with the
 * thumb travelling to whichever stage's prose you are reading. It replaces a
 * hand-rolled IntersectionObserver and a dot-grid diagram — the observer because
 * the component already owns that behaviour (with the keyboard and live-region
 * halves the hand-rolled one never had), and the diagram because a dot-grid
 * background is material difference faked with decoration.
 *
 * The rail is also the navigation: every chip scrolls to its stage, so a reader
 * who wants stage 04 does not have to find it.
 *
 * `offset` is the rail's own height plus the gutter above it, so the stage that
 * reads as current is the one below the rail rather than the one behind it.
 */
export function FactoryLoop() {
  const sections = LOOP.map((stage) => ({ id: `stage-${stage.n}`, label: stage.title }));

  return (
    <div className="mt-8">
      {/* Capped: five chips stretched across a full-width panel read as a
          toolbar, not as a rail. */}
      <div className="sticky top-0 z-10 -mx-1 max-w-[560px] bg-panel px-1 py-3">
        <ScrollSpy sections={sections} offset={112} label="The five stages" />
      </div>

      <ol>
        {LOOP.map((stage) => (
          <li
            key={stage.n}
            id={`stage-${stage.n}`}
            /* Enough height that the rail has travel to report. Below about 13rem
               two stages share the reading line and the thumb flickers between
               them on every scroll. */
            className="flex min-h-[13rem] scroll-mt-28 flex-col justify-center border-t border-hairline py-9 first:border-t-0"
          >
            <p className="flex items-center gap-2.5">
              <span className="meta text-ink-3">{stage.n}</span>
              {/* Ink and depth, not accent: "this stage is ours" is a persistent
                  state, and if every marked thing were blue, focus would stop
                  meaning anything. */}
              <span
                className={`mat-cap inline-flex h-[22px] items-center rounded-[6px] px-2 text-[11.5px] font-medium ${
                  stage.ours ? 'text-ink' : 'text-ink-3'
                }`}
              >
                {stage.owner}
              </span>
            </p>
            <h3 className="mt-3.5 text-[20px] font-medium tracking-[-0.03em] text-ink">
              {stage.title}
            </h3>
            <p className="mt-2.5 max-w-[62ch] text-[13.5px] leading-relaxed text-ink-2">
              {stage.say}
            </p>
            <p className="mt-3.5 font-mono text-[11.5px] text-ink-3">{stage.detail}</p>
          </li>
        ))}
      </ol>
    </div>
  );
}
