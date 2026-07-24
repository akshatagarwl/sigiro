'use client';

import { motion } from 'motion/react';
import type { ReactNode } from 'react';

/**
 * Transform-only entrance. Deliberately does NOT animate opacity: motion
 * server-renders its initial styles, so an opacity-0 start leaves the page
 * blank for anyone without JS. A 12px offset is invisible when JS never runs
 * and costs no layout shift when it does.
 */
export function Reveal({
  children,
  className,
  delay = 0,
}: {
  children: ReactNode;
  className?: string;
  delay?: number;
}) {
  return (
    <motion.div
      className={className}
      initial={{ y: 12 }}
      whileInView={{ y: 0 }}
      viewport={{ once: true, margin: '-48px' }}
      transition={{ type: 'spring', stiffness: 120, damping: 18, delay }}
    >
      {children}
    </motion.div>
  );
}
