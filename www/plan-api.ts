/**
 * Vite dev server middleware: serves docs/plan/ files as a REST API.
 * GET  /api/plan           → { canvas, steps: { [id]: frontmatter } }
 * PATCH /api/plan/steps/:id → update frontmatter on disk
 *
 * This is the bridge between the web UI and files on disk.
 * Files on disk are the ONLY source of truth.
 */

import type { Plugin } from 'vite'
import { readFileSync, writeFileSync, readdirSync } from 'node:fs'
import { join, basename } from 'node:path'

const PLAN_DIR = join(import.meta.dirname, '..', 'docs', 'plan')
const STEPS_DIR = join(PLAN_DIR, 'steps')
const CANVAS_FILE = join(PLAN_DIR, 'plan.canvas')

/** Parse YAML frontmatter from markdown string (minimal parser, no deps) */
function parseFrontmatter(content: string): { data: Record<string, unknown>; body: string } {
  const match = content.match(/^---\n([\s\S]*?)\n---\n([\s\S]*)$/)
  if (!match) return { data: {}, body: content }

  const data: Record<string, unknown> = {}
  for (const line of match[1].split('\n')) {
    const idx = line.indexOf(':')
    if (idx === -1) continue
    const key = line.slice(0, idx).trim()
    let val: unknown = line.slice(idx + 1).trim()
    // Strip quotes
    if (typeof val === 'string' && val.startsWith('"') && val.endsWith('"')) {
      val = val.slice(1, -1)
    }
    // Parse booleans and numbers
    if (val === 'true') val = true
    else if (val === 'false') val = false
    else if (typeof val === 'string' && /^\d+$/.test(val)) val = Number.parseInt(val, 10)
    data[key] = val
  }
  // Parse concerns from JSON if present
  if (typeof data.concerns === 'string') {
    try {
      data.concerns = JSON.parse(data.concerns)
    } catch {
      data.concerns = []
    }
  }
  return { data, body: match[2] }
}

/** Serialize frontmatter back to YAML string */
function serializeFrontmatter(data: Record<string, unknown>): string {
  const lines: string[] = []
  for (const [k, v] of Object.entries(data)) {
    if (k === 'concerns' && Array.isArray(v)) {
      lines.push(`${k}: "${JSON.stringify(v).replace(/"/g, '\\"')}"`)
    } else if (typeof v === 'string') {
      lines.push(`${k}: "${v}"`)
    } else {
      lines.push(`${k}: ${String(v)}`)
    }
  }
  return lines.join('\n')
}

/** Color mapping for canvas node status sync */
const STATUS_TO_COLOR: Record<string, string> = {
  pending: '0',
  active: '3',
  done: '4',
  cut: '1',
  failed: '6',
}

export function planApiPlugin(): Plugin {
  return {
    name: 'plan-api',
    configureServer(server) {
      server.middlewares.use((req, res, next) => {
        if (!req.url?.startsWith('/api/plan')) return next()

        // GET /api/plan — return canvas + all step frontmatter
        if (req.method === 'GET' && req.url === '/api/plan') {
          try {
            const canvas = JSON.parse(readFileSync(CANVAS_FILE, 'utf-8'))
            const steps: Record<string, unknown> = {}

            for (const file of readdirSync(STEPS_DIR)) {
              if (!file.endsWith('.md')) continue
              const id = basename(file, '.md')
              const content = readFileSync(join(STEPS_DIR, file), 'utf-8')
              const { data, body } = parseFrontmatter(content)
              steps[id] = { ...data, body: body.trim() }
            }

            res.setHeader('Content-Type', 'application/json')
            res.end(JSON.stringify({ canvas, steps }))
          } catch (e) {
            res.statusCode = 500
            res.end(JSON.stringify({ error: String(e) }))
          }
          return
        }

        // PATCH /api/plan/steps/:id — update step frontmatter on disk
        const patchMatch = req.url?.match(/^\/api\/plan\/steps\/([a-z0-9-]+)$/)
        if (req.method === 'PATCH' && patchMatch) {
          const id = patchMatch[1]
          const filePath = join(STEPS_DIR, `${id}.md`)

          let body = ''
          req.on('data', (chunk: Buffer) => {
            body += chunk.toString()
          })
          req.on('end', () => {
            try {
              const updates = JSON.parse(body)
              const content = readFileSync(filePath, 'utf-8')
              const { data, body: mdBody } = parseFrontmatter(content)

              // Merge updates into frontmatter
              const merged = { ...data, ...updates }
              const newContent = `---\n${serializeFrontmatter(merged)}\n---\n${mdBody}`
              writeFileSync(filePath, newContent)

              // Also update canvas node color to match status
              if (updates.status) {
                try {
                  const canvasContent = JSON.parse(readFileSync(CANVAS_FILE, 'utf-8'))
                  const node = canvasContent.nodes.find((n: { id: string }) => n.id === id)
                  if (node) {
                    node.color = STATUS_TO_COLOR[updates.status] ?? '0'
                    writeFileSync(CANVAS_FILE, JSON.stringify(canvasContent, null, 2))
                  }
                } catch {
                  // Canvas update is best-effort
                }
              }

              res.setHeader('Content-Type', 'application/json')
              res.end(JSON.stringify({ ok: true, id, ...merged }))
            } catch (e) {
              res.statusCode = 400
              res.end(JSON.stringify({ error: String(e) }))
            }
          })
          return
        }

        res.statusCode = 404
        res.end(JSON.stringify({ error: 'not found' }))
      })
    },
  }
}
