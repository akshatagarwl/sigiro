import { useEffect, useRef, useState } from 'react'
import { motion, useSpring, useMotionValue } from 'motion/react'

const prefersReducedMotion =
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches

// hover cycles: normal → happy → surprised → wonder → normal
export type EyeExpression = 'normal' | 'happy' | 'surprised' | 'wonder'
const eyeExpressions: EyeExpression[] = ['normal', 'happy', 'surprised', 'wonder']

export function Logo({
  size = 24,
  expression: controlledExpression,
}: {
  size?: number
  expression?: EyeExpression
}) {
  const svgRef = useRef<SVGSVGElement>(null)
  const x = useMotionValue(0)
  const y = useMotionValue(0)
  const springX = useSpring(x, { stiffness: 300, damping: 30 })
  const springY = useSpring(y, { stiffness: 300, damping: 30 })
  const [internalExpression, setInternalExpression] = useState<EyeExpression>('normal')
  const expressionIndex = useRef(0)

  const expression = controlledExpression ?? internalExpression

  useEffect(() => {
    if (prefersReducedMotion) return
    const svg = svgRef.current
    if (!svg) return
    const maxOffset = 2.5
    function onMouseMove(e: MouseEvent) {
      const rect = svg!.getBoundingClientRect()
      const cx = rect.left + rect.width / 2
      const cy = rect.top + rect.height / 2
      const dx = e.clientX - cx
      const dy = e.clientY - cy
      const dist = Math.sqrt(dx * dx + dy * dy)
      const scale = Math.min(maxOffset, dist / 80)
      const angle = Math.atan2(dy, dx)
      x.set(Math.cos(angle) * scale)
      y.set(Math.sin(angle) * scale)
    }
    document.addEventListener('mousemove', onMouseMove)
    return () => document.removeEventListener('mousemove', onMouseMove)
  }, [x, y])

  function cycleExpression() {
    if (controlledExpression != null) return
    expressionIndex.current = (expressionIndex.current + 1) % eyeExpressions.length
    setInternalExpression(eyeExpressions[expressionIndex.current])
  }

  const clipIdRef = useRef(`eye-${size}-${Math.random().toString(36).slice(2, 6)}`)
  const clipId = clipIdRef.current
  const spring = { type: 'spring' as const, stiffness: 400, damping: 20 }

  return (
    <svg
      ref={svgRef}
      className="logo-eye"
      viewBox="0 0 34 32"
      width={size}
      height={size * 0.916}
      fill="none"
      aria-hidden="true"
      onMouseEnter={cycleExpression}
      style={{ cursor: 'pointer' }}
    >
      {expression === 'happy' ? (
        <>
          <motion.path
            d="M5 18 Q17 6 29 18"
            initial={{ pathLength: 0 }}
            animate={{ pathLength: 1 }}
            transition={{ duration: 0.25 }}
            stroke="currentColor"
            strokeWidth="2.5"
            strokeLinecap="round"
            fill="none"
          />
          <motion.path
            d="M10 19 Q17 23 24 19"
            initial={{ pathLength: 0 }}
            animate={{ pathLength: 1 }}
            transition={{ duration: 0.25, delay: 0.05 }}
            stroke="currentColor"
            strokeWidth="1.5"
            strokeLinecap="round"
            fill="none"
          />
        </>
      ) : expression === 'surprised' ? (
        <>
          <motion.circle
            cx="17" cy="16"
            initial={{ r: 9 }}
            animate={{ r: 13 }}
            transition={spring}
            stroke="currentColor"
            strokeWidth="2"
          />
          <defs>
            <clipPath id={clipId}>
              <circle cx="17" cy="16" r="12" />
            </clipPath>
          </defs>
          <motion.g style={{ x: springX, y: springY }} clipPath={`url(#${clipId})`}>
            <motion.circle
              cx="17" cy="16"
              initial={{ r: 7 }}
              animate={{ r: 3 }}
              transition={spring}
              fill="currentColor"
            />
            <circle cx="18" cy="15" r="1" fill="var(--bg, #0A0A0A)" />
          </motion.g>
        </>
      ) : (
        <>
          <defs>
            <clipPath id={clipId}>
              <ellipse cx="17" cy="16" rx="14" ry="10" />
            </clipPath>
          </defs>
          <ellipse
            cx="17" cy="16" rx="13" ry="9"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
          <motion.g style={{ x: springX, y: springY }} clipPath={`url(#${clipId})`}>
            <motion.circle
              cx="17" cy="16"
              animate={{ r: expression === 'wonder' ? 9 : 7 }}
              transition={spring}
              fill="currentColor"
            />
            <motion.circle
              cx="19.5" cy="13.5"
              animate={{ r: expression === 'wonder' ? 3 : 2.5 }}
              transition={spring}
              fill="var(--bg, #0A0A0A)"
            />
            {expression === 'wonder' && (
              <>
                <motion.circle
                  cx="14" cy="14"
                  initial={{ r: 0, opacity: 0 }}
                  animate={{ r: 1.5, opacity: 1 }}
                  transition={{ ...spring, delay: 0.1 }}
                  fill="var(--bg, #0A0A0A)"
                />
                <motion.circle
                  cx="19" cy="18"
                  initial={{ r: 0, opacity: 0 }}
                  animate={{ r: 1, opacity: 1 }}
                  transition={{ ...spring, delay: 0.15 }}
                  fill="var(--bg, #0A0A0A)"
                />
              </>
            )}
          </motion.g>
        </>
      )}
    </svg>
  )
}
