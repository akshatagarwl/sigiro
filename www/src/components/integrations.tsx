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
} from "sigiro-icons";

import { Badge } from "@/components/ui/badge";
import { getCopy, type Locale } from "@/lib/site";

/**
 * Three belts: what emits the telemetry, where sigiro runs, and what reads the
 * answers.
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
    <svg
      viewBox="0 0 24 24"
      width="15"
      height="15"
      fill="currentColor"
      aria-hidden
    >
      <path d={path} />
    </svg>
  );
}

export function Integrations({ locale = "en" }: { locale?: Locale }) {
  const INTEGRATIONS = getCopy(locale).integrations;
  return (
    <div className="mt-6 flex flex-col gap-5">
      {INTEGRATIONS.map((group) => (
        <div key={group.label}>
          <p className="text-muted-foreground mb-2.5 text-xs font-medium">
            {group.label}
          </p>
          <ul aria-label={group.label} className="flex flex-wrap gap-2">
            {group.items.map((item) => {
              /* No mark means the label alone. A wrong logo is worse than a
                 name, and four of these brands have no mark in the set. */
              const icon = item.icon ? ICONS[item.icon] : undefined;
              return (
                <li key={item.name}>
                  <Badge
                    variant="outline"
                    className="gap-1.5 px-2.5 py-1 text-sm font-normal"
                  >
                    {icon && <Mark path={icon.path} />}
                    {item.name}
                  </Badge>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </div>
  );
}
