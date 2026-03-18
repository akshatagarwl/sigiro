import type { Meta, StoryObj } from '@storybook/react-vite'
import { ChopperEye, type ChopperExpression } from './ChopperEyes'
import { Logo, type EyeExpression } from './Logo'
import { AllAnimatedPairs, HoverAnimatedPairs } from './ChopperPairStories'

const meta = {
  title: 'Chopper Eyes',
  component: ChopperEye,
  parameters: {
    layout: 'centered',
    backgrounds: { default: 'dark' },
  },
  decorators: [
    (Story) => (
      <div style={{ color: '#e5e5e5', padding: 40 }}>
        <Story />
      </div>
    ),
  ],
} satisfies Meta<typeof ChopperEye>

export default meta
type Story = StoryObj<typeof meta>

export const Normal: Story = {
  args: { size: 200, expression: 'normal' },
}

export const Sparkle: Story = {
  args: { size: 200, expression: 'sparkle' },
}

export const Heart: Story = {
  args: { size: 200, expression: 'heart' },
}

export const Glare: Story = {
  args: { size: 200, expression: 'glare' },
}

export const Cry: Story = {
  args: { size: 200, expression: 'cry' },
}

export const Dizzy: Story = {
  args: { size: 200, expression: 'dizzy' },
}

export const Happy: Story = {
  args: { size: 200, expression: 'happy' },
}

export const Sleepy: Story = {
  args: { size: 200, expression: 'sleepy' },
}

const allExpressions: ChopperExpression[] = [
  'normal', 'sparkle', 'heart', 'glare', 'cry', 'dizzy', 'happy', 'sleepy',
]

export const AllExpressions: Story = {
  args: { size: 150 },
  render: () => (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 24, alignItems: 'flex-start', maxWidth: 700 }}>
      {allExpressions.map((expr) => (
        <div key={expr} style={{ textAlign: 'center' }}>
          <ChopperEye size={150} expression={expr} />
          <div style={{ marginTop: 4, fontSize: 13, opacity: 0.5, fontFamily: 'monospace' }}>{expr}</div>
        </div>
      ))}
    </div>
  ),
}

// --- Two-Eye Variations ---

function TwoChopperEyes({ size, expression }: { size: number; expression: ChopperExpression }) {
  const gap = size * 0.15
  return (
    <div style={{ display: 'flex', gap, alignItems: 'center' }}>
      <ChopperEye size={size} expression={expression} />
      <ChopperEye size={size} expression={expression} />
    </div>
  )
}

export const TwoEyesNormal: Story = {
  name: 'Two Eyes — Normal',
  args: { size: 120 },
  render: () => <TwoChopperEyes size={120} expression="normal" />,
}

export const TwoEyesHeart: Story = {
  name: 'Two Eyes — Heart',
  args: { size: 120 },
  render: () => <TwoChopperEyes size={120} expression="heart" />,
}

export const TwoEyesSparkle: Story = {
  name: 'Two Eyes — Sparkle',
  args: { size: 120 },
  render: () => <TwoChopperEyes size={120} expression="sparkle" />,
}

export const TwoEyesAllExpressions: Story = {
  name: 'Two Eyes — All',
  args: { size: 80 },
  render: () => (
    <div style={{ display: 'flex', flexWrap: 'wrap', gap: 32, alignItems: 'flex-start', maxWidth: 800 }}>
      {allExpressions.map((expr) => (
        <div key={expr} style={{ textAlign: 'center' }}>
          <TwoChopperEyes size={80} expression={expr} />
          <div style={{ marginTop: 8, fontSize: 13, opacity: 0.5, fontFamily: 'monospace' }}>{expr}</div>
        </div>
      ))}
    </div>
  ),
}

// --- Animated Pair Sequences ---

export const AnimatedSequences: Story = {
  name: 'Animated — Auto-playing',
  args: { size: 80 },
  render: () => <AllAnimatedPairs />,
}

export const HoverSequences: Story = {
  name: 'Animated — Hover to play',
  args: { size: 80 },
  render: () => <HoverAnimatedPairs />,
}

// --- Current Logo (single eye) for comparison ---

const logoExpressions: EyeExpression[] = ['normal', 'happy', 'surprised', 'wonder']

export const CurrentLogoComparison: Story = {
  name: 'Current Logo (comparison)',
  args: { size: 120 },
  render: () => (
    <div style={{ display: 'flex', gap: 32, alignItems: 'flex-start' }}>
      {logoExpressions.map((expr) => (
        <div key={expr} style={{ textAlign: 'center' }}>
          <Logo size={120} expression={expr} />
          <div style={{ marginTop: 8, fontSize: 13, opacity: 0.5, fontFamily: 'monospace' }}>{expr}</div>
        </div>
      ))}
    </div>
  ),
}
