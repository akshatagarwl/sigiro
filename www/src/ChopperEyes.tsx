import { motion, useMotionValue, useSpring } from 'motion/react'
import { useEffect, useRef } from 'react'

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

const PINK = '#FF6B9D'
const GOLD = '#FFD700'
const BLUE = '#6BB5FF'
const PURPLE = '#B07CFF'
const RED = '#FF4D4D'

const prefersReducedMotion =
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches

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

  // Mouse-tracking for iris movement
  const mx = useMotionValue(0)
  const my = useMotionValue(0)
  const springX = useSpring(mx, { stiffness: 300, damping: 30 })
  const springY = useSpring(my, { stiffness: 300, damping: 30 })

  // Max offset in viewBox units (40x40) — iris can shift ~3 units
  const maxOffset = 3

  useEffect(() => {
    if (prefersReducedMotion) return
    const svg = svgRef.current
    if (!svg) return

    function onMouseMove(e: MouseEvent) {
      const rect = svg!.getBoundingClientRect()
      const cx = rect.left + rect.width / 2
      const cy = rect.top + rect.height / 2
      const dx = e.clientX - cx
      const dy = e.clientY - cy
      const dist = Math.sqrt(dx * dx + dy * dy)
      const scale = Math.min(maxOffset, dist / 60)
      const angle = Math.atan2(dy, dx)
      mx.set(Math.cos(angle) * scale)
      my.set(Math.sin(angle) * scale)
    }

    document.addEventListener('mousemove', onMouseMove)
    return () => document.removeEventListener('mousemove', onMouseMove)
  }, [mx, my])

  // Expressions that replace the entire eye shape — no tracking needed

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
          cx="12" cy="12"
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
          cx="18" cy="15"
          initial={{ rx: 0, ry: 0, opacity: 0 }}
          animate={{ rx: 2.5, ry: 3.5, opacity: 0.5 }}
          transition={{ ...anim, delay: 0.1 }}
          fill="#FFF"
        />
        <motion.circle
          cx="23" cy="24"
          initial={{ r: 0, opacity: 0 }}
          animate={{ r: 1.5, opacity: 0.4 }}
          transition={{ ...anim, delay: 0.15 }}
          fill="#FFF"
        />
      </svg>
    )
  }

  if (expression === 'happy') {
    return (
      <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
        <motion.path
          d="M5 22 Q20 6 35 22"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.3 }}
          stroke="currentColor"
          strokeWidth="3"
          strokeLinecap="round"
          fill="none"
        />
        <motion.path
          d="M11 24 Q20 30 29 24"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.3, delay: 0.05 }}
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          fill="none"
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

  // Round-eye-based expressions: normal, glare, cry, sleepy
  // Iris + catchlights track the mouse cursor
  const hasTracking = expression === 'normal' || expression === 'cry'

  return (
    <svg ref={svgRef} viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
      <defs>
        <clipPath id={id}>
          {expression === 'sleepy' ? (
            <rect x="0" y="16" width="40" height="24" />
          ) : expression === 'glare' ? (
            <path d="M3 14 A17 17 0 0 1 37 14 L37 26 Q20 22 3 26 Z" />
          ) : (
            <circle cx="20" cy="20" r="18" />
          )}
        </clipPath>
      </defs>

      {/* outer sclera */}
      <circle cx="20" cy="20" r="17" stroke="currentColor" strokeWidth="2" />

      <g clipPath={`url(#${id})`}>
        {/* iris group — moves with cursor for normal/cry */}
        <motion.g style={hasTracking ? { x: springX, y: springY } : undefined}>
          <motion.circle
            cx="20" cy="20"
            animate={{ r: expression === 'glare' ? 12 : 13 }}
            transition={anim}
            fill={expression === 'cry' ? BLUE : expression === 'glare' ? RED : 'currentColor'}
          />

          {expression === 'normal' && (
            <>
              <motion.ellipse
                cx="24" cy="14"
                initial={{ rx: 0, ry: 0 }}
                animate={{ rx: 4, ry: 5.5 }}
                transition={anim}
                fill={bg}
              />
              <motion.circle
                cx="14" cy="24"
                initial={{ r: 0 }}
                animate={{ r: 2 }}
                transition={{ ...anim, delay: 0.05 }}
                fill={bg}
              />
            </>
          )}

          {expression === 'glare' && (
            <motion.ellipse
              cx="23" cy="15"
              initial={{ rx: 0, ry: 0 }}
              animate={{ rx: 2, ry: 2.5 }}
              transition={anim}
              fill={bg}
            />
          )}

          {expression === 'cry' && (
            <>
              <motion.ellipse
                cx="24" cy="14"
                animate={{ rx: [3.5, 5, 3.5], ry: [4.5, 6.5, 4.5] }}
                transition={{ repeat: Infinity, duration: 1.2, ease: 'easeInOut' }}
                fill="#FFF"
              />
              <motion.circle
                cx="14" cy="24"
                animate={{ r: [1.5, 2.5, 1.5] }}
                transition={{ repeat: Infinity, duration: 1, ease: 'easeInOut' }}
                fill="#FFF"
              />
            </>
          )}

          {expression === 'sleepy' && (
            <>
              <ellipse cx="24" cy="19" rx="3" ry="3.5" fill={bg} />
              <circle cx="14" cy="25" r="1.5" fill={bg} />
            </>
          )}
        </motion.g>
      </g>

      {/* overlays (outside tracking group — stay fixed) */}

      {expression === 'glare' && (
        <motion.path
          d="M6 8 L34 14"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.2 }}
          stroke={RED}
          strokeWidth="3.5"
          strokeLinecap="round"
          fill="none"
        />
      )}

      {expression === 'cry' && (
        <>
          <motion.path
            d="M33 30 Q34 35 33 38 Q32 39 31 38 Q30 35 33 30 Z"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 0.9, y: 0 }}
            transition={{ duration: 0.4, delay: 0.2 }}
            fill={BLUE}
          />
          <motion.path
            d="M36 28 Q37 32 36 34 Q35.5 35 35 34 Q34 32 36 28 Z"
            initial={{ opacity: 0, y: -3 }}
            animate={{ opacity: 0.6, y: 0 }}
            transition={{ duration: 0.4, delay: 0.35 }}
            fill={BLUE}
          />
        </>
      )}

      {expression === 'sleepy' && (
        <>
          <motion.path
            d="M3 20 Q20 12 37 20"
            initial={{ pathLength: 0 }}
            animate={{ pathLength: 1 }}
            transition={{ duration: 0.3 }}
            stroke="currentColor"
            strokeWidth="3"
            strokeLinecap="round"
            fill="none"
          />
          <path d="M0 20 Q20 12 40 20 L40 0 L0 0 Z" fill={bg} />
        </>
      )}
    </svg>
  )
}
