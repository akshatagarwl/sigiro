import type { Meta, StoryObj } from '@storybook/react-vite'
import { Logo } from './Logo'

const meta = {
  title: 'Logo',
  component: Logo,
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
} satisfies Meta<typeof Logo>

export default meta
type Story = StoryObj<typeof meta>

export const Normal: Story = {
  args: { size: 200, expression: 'normal' },
}

export const Happy: Story = {
  args: { size: 200, expression: 'happy' },
}

export const Surprised: Story = {
  args: { size: 200, expression: 'surprised' },
}

export const Wonder: Story = {
  args: { size: 200, expression: 'wonder' },
}

export const NavSize: Story = {
  args: { size: 24 },
  name: 'Nav Size (24px)',
}

export const AllExpressions: Story = {
  args: { size: 120 },
  render: () => (
    <div style={{ display: 'flex', gap: 32, alignItems: 'center' }}>
      {(['normal', 'happy', 'surprised', 'wonder'] as const).map((expr) => (
        <div key={expr} style={{ textAlign: 'center' }}>
          <Logo size={120} expression={expr} />
          <div style={{ marginTop: 8, fontSize: 12, opacity: 0.6 }}>{expr}</div>
        </div>
      ))}
    </div>
  ),
}

export const Interactive: Story = {
  args: { size: 200 },
  name: 'Interactive (hover to cycle)',
}
