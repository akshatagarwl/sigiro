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
  return (
    <div className="docs-layout">
      <nav className="docs-sidebar">
        <a href="/" className="docs-logo">
          <svg
            width="18"
            height="18"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="12" cy="12" r="10" />
            <circle cx="12" cy="12" r="3" />
          </svg>
          sigiro
        </a>
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
