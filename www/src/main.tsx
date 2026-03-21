import { StrictMode, lazy, Suspense } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter, Routes, Route } from 'react-router'
import App from './App'
import './index.css'

const DocsRoutes = lazy(() => import('./docs/routes'))

if ('scrollRestoration' in history) history.scrollRestoration = 'manual'
window.scrollTo(0, 0)

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <BrowserRouter>
      <Routes>
        <Route index element={<App />} />
        <Route
          path="docs/*"
          element={
            <Suspense fallback={<div style={{ background: '#0a0a0a', minHeight: '100vh' }} />}>
              <DocsRoutes />
            </Suspense>
          }
        />
      </Routes>
    </BrowserRouter>
  </StrictMode>,
)
