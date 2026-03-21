import { Routes, Route } from 'react-router'
import { DocsLayout } from './components/DocsLayout'
import './docs.css'

import Index from './content/index.mdx'
import Quickstart from './content/quickstart.mdx'
import Auth from './content/auth.mdx'

function Placeholder({ title }: { title: string }) {
  return (
    <div>
      <h1>{title}</h1>
      <p>This page is coming soon.</p>
    </div>
  )
}

export default function DocsRoutes() {
  return (
    <Routes>
      <Route element={<DocsLayout />}>
        <Route index element={<Index />} />
        <Route path="quickstart" element={<Quickstart />} />
        <Route path="auth" element={<Auth />} />
        <Route path="architecture/overview" element={<Placeholder title="Architecture Overview" />} />
        <Route path="architecture/write-path" element={<Placeholder title="Write Path" />} />
        <Route path="architecture/investigation" element={<Placeholder title="Investigation Engine" />} />
        <Route path="architecture/hosted-mode" element={<Placeholder title="Hosted Mode" />} />
        <Route path="integrations/mcp" element={<Placeholder title="MCP Integration" />} />
        <Route path="integrations/vercel" element={<Placeholder title="Vercel Marketplace" />} />
        <Route path="integrations/otel" element={<Placeholder title="OpenTelemetry" />} />
        <Route path="reference/api" element={<Placeholder title="HTTP API Reference" />} />
        <Route path="reference/cli" element={<Placeholder title="CLI Reference" />} />
        <Route path="reference/schema" element={<Placeholder title="Data Schema" />} />
      </Route>
    </Routes>
  )
}
