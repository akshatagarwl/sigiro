import { motion } from 'motion/react'
import { useRef } from 'react'

export type ChopperExpression =
  | 'normal'
  | 'sparkle'
  | 'heart'
  | 'glare'
  | 'cry'
  | 'dizzy'
  | 'happy'
  | 'sleepy'

const spring = { type: 'spring' as const, stiffness: 400, damping: 22 }

// Chopper's eye anatomy (viewBox 0 0 40 40, center 20,20):
// - Outer sclera: perfect circle r=17
// - Iris: massive, r=13 (~76% of eye), fills almost everything
// - Main catchlight: large oval upper-right (the "life" of the eye)
// - Secondary catchlight: small circle lower-left
// - Everything is ROUND and SOFT — zero angular features

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

  if (expression === 'happy') {
    // ^_^ closed-eye smile — two thick upward arcs
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
    // x_x or @_@ — proper spiral inside the eye
    return (
      <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
        <circle cx="20" cy="20" r="17" stroke="currentColor" strokeWidth="2" />
        {/* true spiral: starts from center, winds outward */}
        <motion.path
          d="M20 20 A2 2 0 0 1 22 18 A4 4 0 0 1 24 22 A6 6 0 0 1 18 26 A8 8 0 0 1 12 18 A10 10 0 0 1 22 10 A12 12 0 0 1 32 22"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.5 }}
          stroke="currentColor"
          strokeWidth="2.5"
          strokeLinecap="round"
          fill="none"
        />
      </svg>
    )
  }

  // All remaining expressions share the round-eye base
  return (
    <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
      <defs>
        <clipPath id={id}>
          {expression === 'sleepy' ? (
            // half-closed: only show bottom 60% of eye
            <rect x="0" y="16" width="40" height="24" />
          ) : expression === 'glare' ? (
            // angry: top half open, bottom squished flat
            <path d="M3 14 A17 17 0 0 1 37 14 L37 26 Q20 22 3 26 Z" />
          ) : (
            <circle cx="20" cy="20" r="18" />
          )}
        </clipPath>
      </defs>

      {/* outer sclera circle */}
      <circle
        cx="20" cy="20" r="17"
        stroke="currentColor"
        strokeWidth="2"
      />

      {/* everything inside the eye, clipped */}
      <g clipPath={`url(#${id})`}>
        {/* iris */}
        <motion.circle
          cx="20" cy="20"
          animate={{
            r: expression === 'glare' ? 12
              : expression === 'sparkle' ? 14
              : 13,
          }}
          transition={spring}
          fill="currentColor"
        />

        {/* --- per-expression catchlights --- */}

        {expression === 'normal' && (
          <>
            <motion.ellipse
              cx="24" cy="14"
              initial={{ rx: 0, ry: 0 }}
              animate={{ rx: 4, ry: 5.5 }}
              transition={spring}
              fill={bg}
            />
            <motion.circle
              cx="14" cy="24"
              initial={{ r: 0 }}
              animate={{ r: 2 }}
              transition={{ ...spring, delay: 0.05 }}
              fill={bg}
            />
          </>
        )}

        {expression === 'sparkle' && (
          // big clean 4-point star + small 4-point star
          <>
            <motion.path
              d="M24 9 L25.5 15 L31 16 L25.5 17.5 L24 23 L22.5 17.5 L17 16 L22.5 15 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ ...spring, delay: 0.05 }}
              fill={bg}
            />
            <motion.path
              d="M13 23 L14 25.5 L16.5 26 L14 27 L13 29.5 L12 27 L9.5 26 L12 25.5 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ ...spring, delay: 0.12 }}
              fill={bg}
            />
          </>
        )}

        {expression === 'heart' && (
          // big centered heart — unmistakable
          <>
            <motion.path
              d="M20 14 C18 10 12 10 12 15 C12 20 20 26 20 26 C20 26 28 20 28 15 C28 10 22 10 20 14 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={spring}
              fill={bg}
            />
          </>
        )}

        {expression === 'glare' && (
          // tiny sharp catchlight — concentrated, intense
          <>
            <motion.ellipse
              cx="23" cy="15"
              initial={{ rx: 0, ry: 0 }}
              animate={{ rx: 2, ry: 2.5 }}
              transition={spring}
              fill={bg}
            />
          </>
        )}

        {expression === 'cry' && (
          // shimmering catchlights — pulsing between sizes
          <>
            <motion.ellipse
              cx="24" cy="14"
              animate={{ rx: [3.5, 5, 3.5], ry: [4.5, 6.5, 4.5] }}
              transition={{
                repeat: Infinity,
                duration: 1.2,
                ease: 'easeInOut',
              }}
              fill={bg}
            />
            <motion.circle
              cx="14" cy="24"
              animate={{ r: [1.5, 2.5, 1.5] }}
              transition={{
                repeat: Infinity,
                duration: 1,
                ease: 'easeInOut',
              }}
              fill={bg}
            />
          </>
        )}

        {expression === 'sleepy' && (
          <>
            <ellipse cx="24" cy="19" rx="3" ry="3.5" fill={bg} />
            <circle cx="14" cy="25" r="1.5" fill={bg} />
          </>
        )}
      </g>

      {/* --- overlays outside the clip --- */}

      {/* glare: thick angry eyebrow */}
      {expression === 'glare' && (
        <motion.path
          d="M6 8 L34 14"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.2 }}
          stroke="currentColor"
          strokeWidth="3.5"
          strokeLinecap="round"
          fill="none"
        />
      )}

      {/* cry: visible teardrop shapes, not just lines */}
      {expression === 'cry' && (
        <>
          {/* big tear */}
          <motion.path
            d="M33 30 Q34 35 33 38 Q32 39 31 38 Q30 35 33 30 Z"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 0.9, y: 0 }}
            transition={{ duration: 0.4, delay: 0.2 }}
            fill="currentColor"
          />
          {/* small tear */}
          <motion.path
            d="M36 28 Q37 32 36 34 Q35.5 35 35 34 Q34 32 36 28 Z"
            initial={{ opacity: 0, y: -3 }}
            animate={{ opacity: 0.6, y: 0 }}
            transition={{ duration: 0.4, delay: 0.35 }}
            fill="currentColor"
          />
        </>
      )}

      {/* sleepy: heavy eyelid drooping over top */}
      {expression === 'sleepy' && (
        <>
          {/* thick lid line */}
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
          {/* fill above the lid to cover the top of the sclera */}
          <path
            d="M0 20 Q20 12 40 20 L40 0 L0 0 Z"
            fill={bg}
          />
        </>
      )}
    </svg>
  )
}
