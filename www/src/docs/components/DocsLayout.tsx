import { useState } from 'react'
import { NavLink, Outlet } from 'react-router'

const sections = [
  {
    title: 'Getting Started',
    links: [
      { to: '/docs', label: 'Introduction' },
      { to: '/docs/quickstart', label: 'Quickstart' },
      { to: '/docs/auth', label: 'Authentication' },
    ],
  },
  {
    title: 'Architecture',
    links: [
      { to: '/docs/architecture/overview', label: 'Overview' },
      { to: '/docs/architecture/write-path', label: 'Write Path' },
      { to: '/docs/architecture/investigation', label: 'Investigation Engine' },
      { to: '/docs/architecture/hosted-mode', label: 'Hosted Mode' },
    ],
  },
  {
    title: 'Integrations',
    links: [
      { to: '/docs/integrations/mcp', label: 'MCP (Claude/Cursor)' },
      { to: '/docs/integrations/vercel', label: 'Vercel Marketplace' },
      { to: '/docs/integrations/otel', label: 'OpenTelemetry' },
    ],
  },
  {
    title: 'Reference',
    links: [
      { to: '/docs/reference/api', label: 'HTTP API' },
      { to: '/docs/reference/cli', label: 'CLI' },
      { to: '/docs/reference/schema', label: 'Data Schema' },
    ],
  },
]

export function DocsLayout() {
  const [sidebarOpen, setSidebarOpen] = useState(false)

  return (
    <div className="docs-layout">
      <div className="docs-mobile-header">
        <a href="/" className="docs-mobile-logo">sigiro</a>
        <button className="docs-hamburger" onClick={() => setSidebarOpen(!sidebarOpen)} aria-label="Toggle menu">
          <svg width="18" height="18" viewBox="0 0 18 18" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
            <line x1="3" y1="5" x2="15" y2="5" />
            <line x1="3" y1="9" x2="15" y2="9" />
            <line x1="3" y1="13" x2="15" y2="13" />
          </svg>
        </button>
      </div>
      <div className={`docs-backdrop${sidebarOpen ? ' open' : ''}`} onClick={() => setSidebarOpen(false)} />
      <nav className={`docs-sidebar${sidebarOpen ? ' open' : ''}`}>
        <a href="/" className="docs-logo">sigiro</a>
        {sections.map((section) => (
          <div key={section.title} className="docs-section">
            <h3>{section.title}</h3>
            <ul>
              {section.links.map((link) => (
                <li key={link.to}>
                  <NavLink
                    to={link.to}
                    end={link.to === '/docs'}
                    className={({ isActive }) => (isActive ? 'docs-link active' : 'docs-link')}
                    onClick={() => setSidebarOpen(false)}
                  >
                    {link.label}
                  </NavLink>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </nav>
      <main className="docs-content">
        <article className="docs-article">
          <Outlet />
        </article>
      </main>
    </div>
  )
}
