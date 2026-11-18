import {
  siClaude,
  siCloudflare,
  siCursor,
  siDocker,
  siDotnet,
  siFlydotio,
  siGo,
  siKubernetes,
  siLangchain,
  siNodedotjs,
  siOllama,
  siOpenjdk,
  siOpentelemetry,
  siPython,
  siRailway,
  siRender,
  siRuby,
  siRust,
  siVercel,
  // `simple-icons` under an alias — see the note in `site/ask-ai.tsx` for why the
  // bare specifier has to stay Blume's.
} from 'sigiro-icons';
import { LogoMarquee } from '@/components/interior/logo-marquee';
import { getCopy, type Locale } from '@/lib/site';

/**
 * Three belts: what emits the telemetry, where sigiro runs, and what reads the
 * answers.
 *
 * `interior/logo-marquee` is the component for this, and a marquee is the one
 * place the design language allows an infinite loop — the loop *is* the component.
 * It also stops looping under `prefers-reduced-motion` and becomes a real scroll
 * container with a tab stop, which a static wrapped grid could not do and a
 * hand-rolled belt would not have done.
 *
 * The three run in alternating directions so the eye reads them as three separate
 * claims rather than one long list that happens to wrap.
 *
 * Brand marks come straight from `simple-icons` — the upstream data package, path
 * strings only. The React wrapper (@icons-pack/react-simple-icons) scored 31/100
 * on Socket's supply-chain axis, and all it did was turn these same paths into
 * components.
 *
 * Rendered in `currentColor` rather than each brand's own hue: sixteen coloured
 * logos would be the loudest thing on the page, and the accent has to stay the
 * only colour that means something.
 */
const ICONS = {
  SiClaude: siClaude,
  SiCloudflare: siCloudflare,
  SiCursor: siCursor,
  SiDocker: siDocker,
  SiDotnet: siDotnet,
  SiFlydotio: siFlydotio,
  SiGo: siGo,
  SiKubernetes: siKubernetes,
  SiLangchain: siLangchain,
  SiNodedotjs: siNodedotjs,
  SiOllama: siOllama,
  SiOpenjdk: siOpenjdk,
  SiOpentelemetry: siOpentelemetry,
  SiPython: siPython,
  SiRailway: siRailway,
  SiVercel: siVercel,
  SiRender: siRender,
  SiRuby: siRuby,
  SiRust: siRust,
} as const;

function Mark({ path }: { path: string }) {
  return (
    <svg viewBox="0 0 24 24" width="15" height="15" fill="currentColor" aria-hidden>
      <path d={path} />
    </svg>
  );
}

export function Integrations({ locale = 'en' }: { locale?: Locale }) {
  const INTEGRATIONS = getCopy(locale).integrations;
  return (
    <div className="mt-6 flex flex-col gap-5">
      {INTEGRATIONS.map((group, i) => (
        <div key={group.label}>
          <p className="label mb-2.5">{group.label}</p>
          <LogoMarquee
            label={group.label}
            direction={i % 2 === 0 ? 'left' : 'right'}
            items={group.items.map((item) => {
              const icon = item.icon ? ICONS[item.icon as keyof typeof ICONS] : undefined;
              return {
                id: item.name,
                label: item.name,
                /* No mark means the belt shows the label. A wrong logo is worse
                   than a name, and four of these brands have no mark in the set. */
                mark: icon ? (
                  <span className="inline-flex items-center gap-2">
                    <Mark path={icon.path} />
                    {item.name}
                  </span>
                ) : undefined,
              };
            })}
          />
        </div>
      ))}
    </div>
  );
}
