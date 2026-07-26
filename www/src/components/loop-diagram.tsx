'use client';

import { useRef } from 'react';
import { AnimatedBeam } from '@/components/ui/animated-beam';
import { LOOP } from '@/lib/site';

/**
 * The thesis as a picture: five stages in a ring, beams travelling between them,
 * the two sigiro owns drawn solid and the three it does not drawn grey with their
 * owner named. The beams only animate the direction of flow — the nodes and every
 * label are plain DOM, so the whole argument is still readable with JS disabled
 * and still in the HTML a crawler gets.
 */
export function LoopDiagram() {
  const container = useRef<HTMLDivElement>(null);
  const nodes = [
    useRef<HTMLDivElement>(null),
    useRef<HTMLDivElement>(null),
    useRef<HTMLDivElement>(null),
    useRef<HTMLDivElement>(null),
    useRef<HTMLDivElement>(null),
  ];

  return (
    <div ref={container} className="relative w-full py-6">
      <ol className="relative z-10 grid grid-cols-2 gap-x-6 gap-y-4 sm:grid-cols-5 sm:gap-x-2">
        {LOOP.map((item, i) => (
          <li key={item.step} className="flex flex-col items-center text-center">
            <div
              ref={nodes[i]}
              className={
                item.ours
                  ? 'z-10 rounded-lg border border-foreground bg-background px-3 py-2 font-mono text-sm text-foreground'
                  : 'z-10 rounded-lg border border-border bg-background px-3 py-2 font-mono text-sm text-muted-foreground'
              }
            >
              {item.step.toLowerCase()}
            </div>
            <p className="mt-2 text-xs text-muted-foreground">{item.owner}</p>
            <p className="mt-1 hidden text-xs text-muted-foreground sm:block">{item.body}</p>
          </li>
        ))}
      </ol>

      {nodes.slice(0, -1).map((from, i) => (
        <AnimatedBeam
          key={LOOP[i].step}
          containerRef={container}
          fromRef={from}
          toRef={nodes[i + 1]}
          duration={4}
          delay={i * 0.6}
          curvature={0}
          pathWidth={1.5}
          pathOpacity={0.15}
        />
      ))}
      {/* learn → detect: the step that makes it a loop rather than a pipeline. */}
      <AnimatedBeam
        containerRef={container}
        fromRef={nodes[4]}
        toRef={nodes[0]}
        duration={4}
        delay={3}
        curvature={70}
        pathWidth={1.5}
        pathOpacity={0.15}
      />
    </div>
  );
}
