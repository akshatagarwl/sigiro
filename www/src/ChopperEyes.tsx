import { motion, useMotionValue, useSpring } from 'motion/react'
import { useRef } from 'react'
import { useMouseOffset } from './hooks'

export type ChopperExpression =
  | 'normal'
  | 'sparkle'
  | 'heart'
  | 'glare'
  | 'cry'
  | 'dizzy'
  | 'happy'
  | 'sleepy'

const anim = { type: 'spring' as const, stiffness: 400, damping: 22 }
const lidAnim = { type: 'spring' as const, stiffness: 300, damping: 26 }

const PINK = '#FF6B9D'
const GOLD = '#FFD700'
const BLUE = '#6BB5FF'
const PURPLE = '#B07CFF'
const RED = '#FF4D4D'

export function ChopperEye({
  size = 200,
  expression = 'normal',
}: {
  size?: number
  expression?: ChopperExpression
}) {
  const idRef = useRef(`ch-${Math.random().toString(36).slice(2, 6)}`)
  const id = idRef.current
  const bg = 'var(--bg, #0A0A0A)'
  const svgRef = useRef<SVGSVGElement>(null)

  // Mouse-tracking for iris movement via external store (no useEffect)
  const [rawX, rawY] = useMouseOffset(svgRef, 3, 60)
  const mx = useMotionValue(rawX)
  const my = useMotionValue(rawY)
  mx.set(rawX)
  my.set(rawY)
  const springX = useSpring(mx, { stiffness: 300, damping: 30 })
  const springY = useSpring(my, { stiffness: 300, damping: 30 })

  // --- Shape-replacement expressions (no eyeball at all) ---

  if (expression === 'heart') {
    return (
      <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
        <motion.path
          d="M20 36 C20 36 4 24 4 14 C4 8 8 4 14 4 C17 4 19 6 20 8 C21 6 23 4 26 4 C32 4 36 8 36 14 C36 24 20 36 20 36 Z"
          initial={{ scale: 0 }}
          animate={{ scale: 1 }}
          transition={anim}
          fill={PINK}
          stroke={PINK}
          strokeWidth="1"
        />
        <motion.ellipse
          cx="12"
          cy="12"
          initial={{ rx: 0, ry: 0, opacity: 0 }}
          animate={{ rx: 3, ry: 4, opacity: 0.5 }}
          transition={{ ...anim, delay: 0.1 }}
          fill="#FFF"
        />
      </svg>
    )
  }

  if (expression === 'sparkle') {
    return (
      <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
        <motion.path
          d="M20 2 L24 15 L38 20 L24 25 L20 38 L16 25 L2 20 L16 15 Z"
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={anim}
          fill={GOLD}
          stroke={GOLD}
          strokeWidth="0.5"
          strokeLinejoin="round"
        />
        <motion.ellipse
          cx="18"
          cy="15"
          initial={{ rx: 0, ry: 0, opacity: 0 }}
          animate={{ rx: 2.5, ry: 3.5, opacity: 0.5 }}
          transition={{ ...anim, delay: 0.1 }}
          fill="#FFF"
        />
        <motion.circle
          cx="23"
          cy="24"
          initial={{ r: 0, opacity: 0 }}
          animate={{ r: 1.5, opacity: 0.4 }}
          transition={{ ...anim, delay: 0.15 }}
          fill="#FFF"
        />
      </svg>
    )
  }

  if (expression === 'dizzy') {
    return (
      <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
        <motion.path
          d="M20 20 A2 2 0 0 1 22 18 A4 4 0 0 1 24 22 A6 6 0 0 1 18 26 A8 8 0 0 1 12 18 A10 10 0 0 1 22 10 A12 12 0 0 1 32 22 A14 14 0 0 1 16 34"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.6 }}
          stroke={PURPLE}
          strokeWidth="3"
          strokeLinecap="round"
          fill="none"
        />
      </svg>
    )
  }

  // --- Unified eye with animated eyelid ---
  // All remaining expressions (normal, happy, sleepy, glare, cry) share
  // one SVG. An eyelid overlay animates to close/open the eye naturally.

  const isOpen = expression === 'normal' || expression === 'cry'
  const isHappy = expression === 'happy'
  const isSleepy = expression === 'sleepy'
  const isGlare = expression === 'glare'

  // Eyelid coverage: how far down the bg-colored lid covers the eye
  // Expressed as the y position of the bottom edge of the lid cover
  // Higher = more covered. Eye center is at y=20, sclera radius=17.
  // y=3  → fully open (lid at very top of sclera, nothing covered)
  // y=19 → sleepy (half the eye covered)
  // y=14 → glare (narrow squint opening)
  // y=37 → fully closed (entire eye covered)
  const lidBottom = isOpen ? 3 : isSleepy ? 19 : isGlare ? 14 : 37

  // Iris color
  const irisColor = expression === 'cry' ? BLUE : isGlare ? RED : 'currentColor'

  // Mouse tracking only when eye is open
  const hasTracking = isOpen

  return (
    <svg ref={svgRef} viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
      <defs>
        {/* Clip to sclera bounds */}
        <clipPath id={id}>
          <circle cx="20" cy="20" r="17" />
        </clipPath>
      </defs>

      {/* Sclera outline — always visible */}
      <circle cx="20" cy="20" r="17" stroke="currentColor" strokeWidth="2" />

      {/* Iris group — inside sclera clip, tracks cursor */}
      <g clipPath={`url(#${id})`}>
        <motion.g style={hasTracking ? { x: springX, y: springY } : undefined}>
          {/* Iris fill */}
          <motion.circle
            cx="20"
            cy="20"
            animate={{ r: isGlare ? 12 : 13 }}
            transition={anim}
            fill={irisColor}
          />

          {/* Catchlights — normal */}
          <motion.ellipse
            cx="24"
            cy="14"
            animate={{
              rx: isOpen ? 4 : 0,
              ry: isOpen ? 5.5 : 0,
              opacity: isOpen ? 1 : 0,
            }}
            transition={anim}
            fill={bg}
          />
          <motion.circle
            cx="14"
            cy="24"
            animate={{
              r: isOpen ? 2 : 0,
              opacity: isOpen ? 1 : 0,
            }}
            transition={anim}
            fill={bg}
          />

          {/* Catchlight — glare (smaller, more intense) */}
          <motion.ellipse
            cx="23"
            cy="15"
            animate={{
              rx: isGlare ? 2 : 0,
              ry: isGlare ? 2.5 : 0,
              opacity: isGlare ? 1 : 0,
            }}
            transition={anim}
            fill={bg}
          />

          {/* Catchlights — cry (pulsing) */}
          {expression === 'cry' && (
            <>
              <motion.ellipse
                cx="24"
                cy="14"
                animate={{ rx: [3.5, 5, 3.5], ry: [4.5, 6.5, 4.5] }}
                transition={{ repeat: Infinity, duration: 1.2, ease: 'easeInOut' }}
                fill="#FFF"
              />
              <motion.circle
                cx="14"
                cy="24"
                animate={{ r: [1.5, 2.5, 1.5] }}
                transition={{ repeat: Infinity, duration: 1, ease: 'easeInOut' }}
                fill="#FFF"
              />
            </>
          )}

          {/* Catchlights — sleepy */}
          <motion.ellipse
            cx="24"
            cy="19"
            animate={{
              rx: isSleepy ? 3 : 0,
              ry: isSleepy ? 3.5 : 0,
              opacity: isSleepy ? 1 : 0,
            }}
            transition={anim}
            fill={bg}
          />
          <motion.circle
            cx="14"
            cy="25"
            animate={{
              r: isSleepy ? 1.5 : 0,
              opacity: isSleepy ? 1 : 0,
            }}
            transition={anim}
            fill={bg}
          />
        </motion.g>

        {/* Eyelid — bg-colored shape that slides down to cover the iris */}
        {/* Uses a curved bottom edge for natural eyelid look */}
        <motion.path
          animate={{
            d: `M0 0 L40 0 L40 ${lidBottom} Q20 ${lidBottom - 6} 0 ${lidBottom} Z`,
          }}
          transition={lidAnim}
          fill={bg}
        />
      </g>

      {/* --- Overlays on top of sclera --- */}

      {/* Eyelid crease line — visible when eye is partially or fully closed */}
      <motion.path
        animate={{
          d: isHappy
            ? 'M5 22 Q20 8 35 22'
            : isSleepy
              ? 'M3 20 Q20 12 37 20'
              : isGlare
                ? 'M3 14 Q20 8 37 14'
                : 'M3 3 Q20 3 37 3',
          opacity: isHappy || isSleepy || isGlare ? 1 : 0,
        }}
        transition={lidAnim}
        stroke="currentColor"
        strokeWidth="2.5"
        strokeLinecap="round"
        fill="none"
      />

      {/* Happy — lower arc (the smile part of ^_^) */}
      <motion.path
        d="M11 24 Q20 30 29 24"
        animate={{ opacity: isHappy ? 1 : 0 }}
        transition={lidAnim}
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        fill="none"
      />

      {/* Glare — angry eyebrow slash */}
      <motion.path
        d="M6 8 L34 14"
        animate={{ opacity: isGlare ? 1 : 0 }}
        transition={{ duration: 0.2 }}
        stroke={RED}
        strokeWidth="3.5"
        strokeLinecap="round"
        fill="none"
      />

      {/* Cry — teardrops */}
      <motion.path
        d="M33 30 Q34 35 33 38 Q32 39 31 38 Q30 35 33 30 Z"
        animate={{ opacity: expression === 'cry' ? 0.9 : 0, y: expression === 'cry' ? 0 : -4 }}
        transition={{ duration: 0.4, delay: 0.2 }}
        fill={BLUE}
      />
      <motion.path
        d="M36 28 Q37 32 36 34 Q35.5 35 35 34 Q34 32 36 28 Z"
        animate={{ opacity: expression === 'cry' ? 0.6 : 0, y: expression === 'cry' ? 0 : -3 }}
        transition={{ duration: 0.4, delay: 0.35 }}
        fill={BLUE}
      />
    </svg>
  )
}
