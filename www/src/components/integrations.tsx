import {
  SiAnthropic,
  SiClaude,
  SiCloudflare,
  SiCursor,
  SiDocker,
  SiDotnet,
  SiFlydotio,
  SiGo,
  SiKubernetes,
  SiNodedotjs,
  SiOpenjdk,
  SiOpentelemetry,
  SiPython,
  SiRailway,
  SiRender,
  SiRuby,
  SiRust,
} from '@icons-pack/react-simple-icons';
import { INTEGRATIONS } from '@/lib/site';

/**
 * Brand marks from simple-icons, rendered in `currentColor` rather than each
 * brand's own hue — the palette is achromatic and a row of coloured logos would
 * be the loudest thing on the page. Server-rendered SVG, no client JavaScript.
 */
const ICONS = {
  SiAnthropic,
  SiClaude,
  SiCloudflare,
  SiCursor,
  SiDocker,
  SiDotnet,
  SiFlydotio,
  SiGo,
  SiKubernetes,
  SiNodedotjs,
  SiOpenjdk,
  SiOpentelemetry,
  SiPython,
  SiRailway,
  SiRender,
  SiRuby,
  SiRust,
} as const;

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
              const Icon = item.icon ? ICONS[item.icon as keyof typeof ICONS] : undefined;
              return (
                <li
                  key={item.name}
                  className="flex items-center gap-2 rounded-lg border border-line bg-card px-2.5 py-1.5 text-[0.8125rem] text-muted-ink"
                >
                  {Icon ? (
                    <Icon size={14} className="shrink-0 text-faint" color="currentColor" />
                  ) : null}
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
