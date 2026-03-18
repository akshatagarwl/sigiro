import { createContext, useContext, useState, useCallback, useRef, useEffect } from 'react'
import type { ChopperExpression } from './ChopperEyes'

// --- Mood Rules ---
// Maps app events to eye expressions with priority and duration.
// Higher priority overrides lower. Duration = how long before reverting to idle.

type MoodRule = {
  expression: ChopperExpression
  priority: number
  duration: number // ms, 0 = until manually cleared
}

const RULES = {
  // --- Page / Navigation ---
  'page:home':        { expression: 'normal',  priority: 0, duration: 0 } as MoodRule,
  'page:pricing':     { expression: 'sparkle', priority: 1, duration: 0 } as MoodRule,
  'page:roadmap':     { expression: 'normal',  priority: 0, duration: 0 } as MoodRule,

  // --- User Actions ---
  'action:waitlist':  { expression: 'heart',   priority: 5, duration: 2000 } as MoodRule,
  'action:scroll-pricing': { expression: 'sparkle', priority: 2, duration: 0 } as MoodRule,
  'action:hover-logo': { expression: 'happy',  priority: 3, duration: 800 } as MoodRule,

  // --- Content Sections (scroll-triggered) ---
  'section:hero':         { expression: 'normal',  priority: 0, duration: 0 } as MoodRule,
  'section:problem':      { expression: 'glare',   priority: 1, duration: 0 } as MoodRule,
  'section:dashboards':   { expression: 'dizzy',   priority: 1, duration: 0 } as MoodRule,
  'section:solution':     { expression: 'sparkle', priority: 1, duration: 0 } as MoodRule,
  'section:how-it-works': { expression: 'normal',  priority: 1, duration: 0 } as MoodRule,
  'section:pricing':      { expression: 'sparkle', priority: 1, duration: 0 } as MoodRule,
  'section:waitlist':     { expression: 'heart',   priority: 1, duration: 0 } as MoodRule,

  // --- App State (for future product usage) ---
  'state:incident':    { expression: 'cry',    priority: 8, duration: 0 } as MoodRule,
  'state:alert':       { expression: 'glare',  priority: 7, duration: 3000 } as MoodRule,
  'state:resolved':    { expression: 'happy',  priority: 6, duration: 2000 } as MoodRule,
  'state:loading':     { expression: 'normal', priority: 0, duration: 0 } as MoodRule,
  'state:idle':        { expression: 'sleepy', priority: 1, duration: 0 } as MoodRule,
  'state:error':       { expression: 'glare',  priority: 6, duration: 3000 } as MoodRule,
  'state:success':     { expression: 'sparkle', priority: 5, duration: 1500 } as MoodRule,

  // --- Easter Eggs ---
  'easter:konami':     { expression: 'sparkle', priority: 10, duration: 3000 } as MoodRule,
  'easter:rage-click': { expression: 'dizzy',   priority: 9, duration: 2000 } as MoodRule,
} as const

type MoodEvent = keyof typeof RULES

type EyeMoodContextType = {
  expression: ChopperExpression
  trigger: (event: MoodEvent) => void
  clear: () => void
}

const EyeMoodContext = createContext<EyeMoodContextType>({
  expression: 'normal',
  trigger: () => {},
  clear: () => {},
})

export function EyeMoodProvider({ children }: { children: React.ReactNode }) {
  const [expression, setExpression] = useState<ChopperExpression>('normal')
  const currentPriority = useRef(0)
  const revertTimer = useRef<ReturnType<typeof setTimeout>>()

  const trigger = useCallback((event: MoodEvent) => {
    const rule = RULES[event]
    if (rule.priority >= currentPriority.current) {
      setExpression(rule.expression)
      currentPriority.current = rule.priority

      if (revertTimer.current) clearTimeout(revertTimer.current)

      if (rule.duration > 0) {
        revertTimer.current = setTimeout(() => {
          setExpression('normal')
          currentPriority.current = 0
        }, rule.duration)
      }
    }
  }, [])

  const clear = useCallback(() => {
    if (revertTimer.current) clearTimeout(revertTimer.current)
    setExpression('normal')
    currentPriority.current = 0
  }, [])

  useEffect(() => {
    return () => {
      if (revertTimer.current) clearTimeout(revertTimer.current)
    }
  }, [])

  return (
    <EyeMoodContext.Provider value={{ expression, trigger, clear }}>
      {children}
    </EyeMoodContext.Provider>
  )
}

export function useEyeMood() {
  return useContext(EyeMoodContext)
}

export { RULES }
export type { MoodEvent }
