import {
  Timeline,
  TimelineContent,
  TimelineHeader,
  TimelineIndicator,
  TimelineItem,
  TimelineSeparator,
  TimelineTitle,
} from '@/components/reui/timeline';

/**
 * What the loop looks like from the agent's side: six turns, each a real call
 * against a real endpoint. Server-rendered — reui's Timeline is built on Base UI
 * primitives and needs no client state for a static sequence, so this costs no
 * JavaScript and every step is in the HTML.
 */
const STEPS = [
  ['GET /v1/anomalies', 'Two rows share incident_id 1. Largest shift first, so it is already ranked.'],
  ['POST /v1/investigate', '{"service":"checkout"} — one call, one evidence block for the window.'],
  ['read findings[0]', 'critical · incident · 2 correlated shifts · deploy 9f2c1ab landed 2 minutes before.'],
  ['read error_breakdown', '246 timeouts, 6 HTTP 500. The failure mode, not just the failure rate.'],
  ['POST /v1/query', 'Runs the drill_down_sql that came attached to that row. No schema guessing.'],
  ['open a pull request', 'Not us. Your coding agent, holding evidence instead of a screenshot.'],
] as const;

export function AgentLoop() {
  return (
    <Timeline defaultValue={STEPS.length}>
      {STEPS.map(([call, note], i) => (
        <TimelineItem key={call} step={i + 1} className="pb-6 last:pb-0">
          <TimelineHeader>
            <TimelineSeparator className="bg-border" />
            <TimelineIndicator />
            <TimelineTitle className="font-mono text-sm">{call}</TimelineTitle>
          </TimelineHeader>
          <TimelineContent className="text-sm text-muted-foreground">{note}</TimelineContent>
        </TimelineItem>
      ))}
    </Timeline>
  );
}
