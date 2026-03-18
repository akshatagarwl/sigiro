import { useCallback, useRef, useSyncExternalStore } from 'react'

const prefersReducedMotion =
  typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches

// Subscribe to document mousemove, returns [x, y] offset from an element center.
// maxOffset clamps the displacement. divisor controls sensitivity (higher = less movement).
export function useMouseOffset(
  ref: React.RefObject<Element | null>,
  maxOffset: number,
  divisor: number,
): [number, number] {
  const pos = useRef<[number, number]>([0, 0])
  return useSyncExternalStore(
    (notify) => {
      if (prefersReducedMotion) return () => {}
      function onMove(e: MouseEvent) {
        const el = ref.current
        if (!el) return
        const rect = el.getBoundingClientRect()
        const cx = rect.left + rect.width / 2
        const cy = rect.top + rect.height / 2
        const dx = e.clientX - cx
        const dy = e.clientY - cy
        const dist = Math.sqrt(dx * dx + dy * dy)
        const scale = Math.min(maxOffset, dist / divisor)
        const angle = Math.atan2(dy, dx)
        const next: [number, number] = [Math.cos(angle) * scale, Math.sin(angle) * scale]
        if (next[0] !== pos.current[0] || next[1] !== pos.current[1]) {
          pos.current = next
          notify()
        }
      }
      document.addEventListener('mousemove', onMove)
      return () => document.removeEventListener('mousemove', onMove)
    },
    () => pos.current,
  )
}

// Tick an interval, incrementing a counter each time. Returns counter value.
// Useful for cycling through snapshots. Does nothing if prefersReducedMotion.
export function useInterval(ms: number): number {
  const tick = useRef(0)
  return useSyncExternalStore(
    (notify) => {
      if (prefersReducedMotion) return () => {}
      const iv = setInterval(() => {
        tick.current += 1
        notify()
      }, ms)
      return () => clearInterval(iv)
    },
    () => tick.current,
  )
}

// Progress through timed phases once `active` becomes true.
// stages: array of { value, delay } sorted by ascending delay.
// Returns the value of the latest stage whose delay has elapsed.
export function useDelayedPhase<T>(stages: { value: T; delay: number }[], active: boolean): T {
  const startedAt = useRef<number>(0)
  const phase = useRef<T>(stages[0].value)
  const activeRef = useRef(active)

  if (active && !activeRef.current) {
    startedAt.current = Date.now()
  }
  activeRef.current = active

  return useSyncExternalStore(
    (notify) => {
      if (!activeRef.current || prefersReducedMotion) {
        if (prefersReducedMotion && activeRef.current) {
          phase.current = stages[stages.length - 1].value
        }
        return () => {}
      }

      const timers = stages.slice(1).map((s) =>
        setTimeout(() => {
          phase.current = s.value
          notify()
        }, s.delay),
      )
      return () => timers.forEach(clearTimeout)
    },
    () => phase.current,
  )
}

// Cycle through frames with variable durations. Returns current frame index
// and a start() function for on-demand triggering.
export function useFrameSequence(
  durations: number[],
  options: { playing: boolean; loop?: boolean },
): [number, () => void] {
  const idx = useRef(0)
  const timer = useRef<ReturnType<typeof setTimeout>>()
  const playingRef = useRef(options.playing)
  playingRef.current = options.playing
  const durRef = useRef(durations)
  durRef.current = durations
  const loop = options.loop ?? true

  const start = useCallback(() => {
    idx.current = 0
    playingRef.current = true
  }, [])

  const frameIdx = useSyncExternalStore(
    (notify) => {
      function scheduleNext() {
        if (!playingRef.current || durRef.current.length === 0) return
        timer.current = setTimeout(() => {
          const next = idx.current + 1
          idx.current = next < durRef.current.length ? next : loop ? 0 : idx.current
          notify()
          scheduleNext()
        }, durRef.current[idx.current])
      }
      scheduleNext()
      return () => {
        if (timer.current) clearTimeout(timer.current)
      }
    },
    () => idx.current,
  )

  return [frameIdx, start]
}
