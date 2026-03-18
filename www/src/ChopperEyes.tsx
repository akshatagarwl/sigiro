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

// Expression colors
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

  // iris color changes per expression
  const irisColor =
    expression === 'heart' ? PINK
    : expression === 'sparkle' ? GOLD
    : expression === 'cry' ? BLUE
    : expression === 'glare' ? RED
    : 'currentColor'

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
        <circle cx="20" cy="20" r="17" stroke="currentColor" strokeWidth="2" />
        <motion.path
          d="M20 20 A2 2 0 0 1 22 18 A4 4 0 0 1 24 22 A6 6 0 0 1 18 26 A8 8 0 0 1 12 18 A10 10 0 0 1 22 10 A12 12 0 0 1 32 22"
          initial={{ pathLength: 0 }}
          animate={{ pathLength: 1 }}
          transition={{ duration: 0.5 }}
          stroke={PURPLE}
          strokeWidth="2.5"
          strokeLinecap="round"
          fill="none"
        />
      </svg>
    )
  }

  return (
    <svg viewBox="0 0 40 40" width={size} height={size} fill="none" aria-hidden="true">
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
      <circle
        cx="20" cy="20" r="17"
        stroke="currentColor"
        strokeWidth="2"
      />

      <g clipPath={`url(#${id})`}>
        {/* iris — colored per expression */}
        <motion.circle
          cx="20" cy="20"
          animate={{
            r: expression === 'glare' ? 12
              : expression === 'sparkle' ? 14
              : 13,
          }}
          transition={spring}
          fill={irisColor}
        />

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
          // golden iris with white stars
          <>
            <motion.path
              d="M24 9 L25.5 15 L31 16 L25.5 17.5 L24 23 L22.5 17.5 L17 16 L22.5 15 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ ...spring, delay: 0.05 }}
              fill="#FFF"
            />
            <motion.path
              d="M13 23 L14 25.5 L16.5 26 L14 27 L13 29.5 L12 27 L9.5 26 L12 25.5 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={{ ...spring, delay: 0.12 }}
              fill="#FFF"
            />
            {/* tiny extra sparkle dots */}
            <motion.circle
              cx="17" cy="13"
              initial={{ r: 0 }}
              animate={{ r: 1 }}
              transition={{ ...spring, delay: 0.18 }}
              fill="#FFF"
            />
          </>
        )}

        {expression === 'heart' && (
          // pink iris with white heart
          <>
            <motion.path
              d="M20 14 C18 10 12 10 12 15 C12 20 20 26 20 26 C20 26 28 20 28 15 C28 10 22 10 20 14 Z"
              initial={{ scale: 0, opacity: 0 }}
              animate={{ scale: 1, opacity: 1 }}
              transition={spring}
              fill="#FFF"
            />
          </>
        )}

        {expression === 'glare' && (
          // red iris with dark catchlight
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
          // blue iris with white shimmering catchlights
          <>
            <motion.ellipse
              cx="24" cy="14"
              animate={{ rx: [3.5, 5, 3.5], ry: [4.5, 6.5, 4.5] }}
              transition={{
                repeat: Infinity,
                duration: 1.2,
                ease: 'easeInOut',
              }}
              fill="#FFF"
            />
            <motion.circle
              cx="14" cy="24"
              animate={{ r: [1.5, 2.5, 1.5] }}
              transition={{
                repeat: Infinity,
                duration: 1,
                ease: 'easeInOut',
              }}
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
      </g>

      {/* --- overlays --- */}

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
          <path
            d="M0 20 Q20 12 40 20 L40 0 L0 0 Z"
            fill={bg}
          />
        </>
      )}
    </svg>
  )
}
