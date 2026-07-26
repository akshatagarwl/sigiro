import {
  siClaude,
  siCloudflare,
  siCursor,
  siDocker,
  siDotnet,
  siFlydotio,
  siGo,
  siKubernetes,
  siNodedotjs,
  siOpenjdk,
  siOpentelemetry,
  siPython,
  siRailway,
  siRender,
  siRuby,
  siRust,
} from 'simple-icons';
import { INTEGRATIONS } from '@/lib/site';

/**
 * Brand marks straight from `simple-icons` — the upstream data package, path
 * strings only. The React wrapper (@icons-pack/react-simple-icons) scored 31/100
 * on Socket's supply-chain axis, and all it was doing was turning these same paths
 * into components.
 *
 * Rendered in `currentColor` rather than each brand's own hue: the palette is
 * achromatic, and a row of coloured logos would be the loudest thing on the page.
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
  SiNodedotjs: siNodedotjs,
  SiOpenjdk: siOpenjdk,
  SiOpentelemetry: siOpentelemetry,
  SiPython: siPython,
  SiRailway: siRailway,
  SiRender: siRender,
  SiRuby: siRuby,
  SiRust: siRust,
} as const;

function Mark({ icon }: { icon: { title: string; path: string } }) {
  return (
    <svg
      role="img"
      aria-label={icon.title}
      viewBox="0 0 24 24"
      width="14"
      height="14"
      fill="currentColor"
      className="shrink-0 text-faint"
    >
      <path d={icon.path} />
    </svg>
  );
}

export function Integrations() {
  return (
    <div className="mt-6 flex flex-col gap-6">
      {INTEGRATIONS.map((group) => (
        <div key={group.label}>
          <p className="mb-2.5 font-mono text-[0.6875rem] tracking-[0.12em] text-faint uppercase">
            {group.label}
          </p>
          <ul className="flex flex-wrap gap-1.5">
            {group.items.map((item) => {
              const icon = item.icon ? ICONS[item.icon as keyof typeof ICONS] : undefined;
              return (
                <li
                  key={item.name}
                  className="flex items-center gap-2 rounded-lg border border-line bg-card px-2.5 py-1.5 text-[0.8125rem] text-muted-ink"
                >
                  {icon ? <Mark icon={icon} /> : null}
                  {item.name}
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </div>
  );
}
