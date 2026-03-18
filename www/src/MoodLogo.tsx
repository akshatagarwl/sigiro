import { useEyeMood } from './EyeMood'
import { ChopperEye } from './ChopperEyes'

export function MoodLogo({ size = 24 }: { size?: number }) {
  const { expression, trigger } = useEyeMood()

  return (
    <span
      onMouseEnter={() => trigger('action:hover-logo')}
      style={{ cursor: 'pointer', display: 'inline-flex', verticalAlign: 'middle' }}
    >
      <ChopperEye size={size} expression={expression} />
    </span>
  )
}
