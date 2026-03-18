import type { Meta, StoryObj } from '@storybook/react-vite'
import { ChopperEye, type ChopperExpression } from './ChopperEyes'

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
