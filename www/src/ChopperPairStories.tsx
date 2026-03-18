import { ChopperEye, type ChopperExpression } from './ChopperEyes'
import { useFrameSequence } from './hooks'

type Frame = {
  left: ChopperExpression
  right: ChopperExpression
  duration: number // ms to hold this frame
}

function AnimatedPair({
  frames,
  size = 100,
  label,
  trigger = 'auto',
}: {
  frames: Frame[]
  size?: number
  label?: string
  trigger?: 'auto' | 'hover' | 'click'
}) {
  const durations = frames.map((f) => f.duration)
  const [frameIndex, startSequence] = useFrameSequence(durations, { playing: trigger === 'auto' })
  const frame = frames[frameIndex]
  const gap = size * 0.12

  return (
    <div
      style={{ textAlign: 'center', cursor: trigger !== 'auto' ? 'pointer' : undefined }}
      onMouseEnter={trigger === 'hover' ? startSequence : undefined}
      onClick={trigger === 'click' ? startSequence : undefined}
    >
      <div style={{ display: 'flex', gap, alignItems: 'center', justifyContent: 'center' }}>
        <ChopperEye size={size} expression={frame.left} />
        <ChopperEye size={size} expression={frame.right} />
      </div>
      {label && (
        <div style={{ marginTop: 8, fontSize: 13, opacity: 0.5, fontFamily: 'monospace' }}>
          {label}
        </div>
      )}
    </div>
  )
}

// --- Animation sequences ---

const winkSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 1200 },
  { left: 'happy', right: 'normal', duration: 400 },
  { left: 'normal', right: 'normal', duration: 800 },
  { left: 'normal', right: 'happy', duration: 400 },
  { left: 'normal', right: 'normal', duration: 1000 },
]

const loveAtFirstSight: Frame[] = [
  { left: 'normal', right: 'normal', duration: 1000 },
  { left: 'sparkle', right: 'sparkle', duration: 600 },
  { left: 'heart', right: 'heart', duration: 1200 },
  { left: 'happy', right: 'happy', duration: 800 },
  { left: 'normal', right: 'normal', duration: 600 },
]

const confusedSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 800 },
  { left: 'normal', right: 'dizzy', duration: 600 },
  { left: 'dizzy', right: 'normal', duration: 600 },
  { left: 'dizzy', right: 'dizzy', duration: 800 },
  { left: 'normal', right: 'normal', duration: 500 },
]

const mischievousSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 800 },
  { left: 'happy', right: 'normal', duration: 500 },
  { left: 'glare', right: 'glare', duration: 800 },
  { left: 'sparkle', right: 'sparkle', duration: 600 },
  { left: 'happy', right: 'happy', duration: 700 },
  { left: 'normal', right: 'normal', duration: 500 },
]

const morningSequence: Frame[] = [
  { left: 'sleepy', right: 'sleepy', duration: 1200 },
  { left: 'sleepy', right: 'normal', duration: 600 },
  { left: 'normal', right: 'normal', duration: 500 },
  { left: 'normal', right: 'sparkle', duration: 400 },
  { left: 'sparkle', right: 'sparkle', duration: 800 },
  { left: 'happy', right: 'happy', duration: 700 },
]

const moodSwingSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 400 },
  { left: 'happy', right: 'happy', duration: 350 },
  { left: 'heart', right: 'heart', duration: 350 },
  { left: 'cry', right: 'cry', duration: 400 },
  { left: 'glare', right: 'glare', duration: 350 },
  { left: 'sparkle', right: 'sparkle', duration: 350 },
  { left: 'dizzy', right: 'dizzy', duration: 400 },
  { left: 'sleepy', right: 'sleepy', duration: 500 },
  { left: 'normal', right: 'normal', duration: 300 },
]

const suspiciousSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 900 },
  { left: 'normal', right: 'sleepy', duration: 500 },
  { left: 'sleepy', right: 'normal', duration: 500 },
  { left: 'glare', right: 'normal', duration: 600 },
  { left: 'glare', right: 'glare', duration: 800 },
  { left: 'normal', right: 'normal', duration: 400 },
]

const dramaticCrySequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 800 },
  { left: 'normal', right: 'cry', duration: 500 },
  { left: 'cry', right: 'cry', duration: 1000 },
  { left: 'happy', right: 'cry', duration: 600 },
  { left: 'happy', right: 'happy', duration: 800 },
  { left: 'normal', right: 'normal', duration: 400 },
]

const malfunctionSequence: Frame[] = [
  { left: 'normal', right: 'normal', duration: 700 },
  { left: 'sparkle', right: 'glare', duration: 300 },
  { left: 'heart', right: 'dizzy', duration: 300 },
  { left: 'cry', right: 'happy', duration: 300 },
  { left: 'glare', right: 'sparkle', duration: 300 },
  { left: 'dizzy', right: 'heart', duration: 300 },
  { left: 'dizzy', right: 'dizzy', duration: 600 },
  { left: 'normal', right: 'normal', duration: 500 },
]

export function AllAnimatedPairs() {
  const pairs = [
    { frames: winkSequence, label: 'wink' },
    { frames: loveAtFirstSight, label: 'love at first sight' },
    { frames: confusedSequence, label: 'confused' },
    { frames: mischievousSequence, label: 'plotting something' },
    { frames: morningSequence, label: 'waking up' },
    { frames: moodSwingSequence, label: 'mood swing' },
    { frames: suspiciousSequence, label: 'suspicious' },
    { frames: dramaticCrySequence, label: 'drama queen' },
    { frames: malfunctionSequence, label: 'malfunction' },
  ]

  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        gap: 40,
        alignItems: 'flex-start',
        maxWidth: 900,
      }}
    >
      {pairs.map(({ frames, label }) => (
        <AnimatedPair key={label} frames={frames} size={80} label={label} />
      ))}
    </div>
  )
}

export function HoverAnimatedPairs() {
  const pairs = [
    { frames: winkSequence, label: 'wink' },
    { frames: loveAtFirstSight, label: 'love at first sight' },
    { frames: confusedSequence, label: 'confused' },
    { frames: mischievousSequence, label: 'plotting something' },
    { frames: morningSequence, label: 'waking up' },
    { frames: suspiciousSequence, label: 'suspicious' },
    { frames: dramaticCrySequence, label: 'drama queen' },
    { frames: malfunctionSequence, label: 'malfunction' },
  ]

  return (
    <div
      style={{
        display: 'flex',
        flexWrap: 'wrap',
        gap: 40,
        alignItems: 'flex-start',
        maxWidth: 900,
      }}
    >
      {pairs.map(({ frames, label }) => (
        <AnimatedPair key={label} frames={frames} size={80} label={label} trigger="hover" />
      ))}
    </div>
  )
}

export { AnimatedPair }
export type { Frame }
