#!/usr/bin/env node
/**
 * NovelX reading desk (Node). Serves /api preview + web/dist (or Vite).
 * Writing stays in the dsh preset — this process does not run Host continue_writing.
 */

import { createServer } from 'node:http'
import { existsSync, readFileSync, rmSync, statSync } from 'node:fs'
import { extname, join, normalize, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import {
  buildPreview,
  chapterPayload,
  deskFocusEvent,
  libraryItems,
  libraryOne,
  safeProjectName,
} from './src/deskPreview.js'

const here = fileURLToPath(new URL('.', import.meta.url))
const webRoot = here
const repoRoot = process.env.NOVELX_ROOT || resolve(webRoot, '..')
const BIND = parseBind(process.argv)
const deskBus = new Set()

function parseBind(argv) {
  const fromEnv = process.env.NOVELX_BIND
  const i = argv.indexOf('--bind')
  const raw = (i >= 0 && argv[i + 1]) || fromEnv || '127.0.0.1:8765'
  const [host, port] = raw.includes(':') ? raw.split(':') : ['127.0.0.1', raw]
  return { host: host || '127.0.0.1', port: Number(port) || 8765, raw: `${host || '127.0.0.1'}:${Number(port) || 8765}` }
}

function json(res, status, body) {
  const data = JSON.stringify(body)
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'cache-control': 'no-store',
  })
  res.end(data)
}

function readBody(req) {
  return new Promise((resolveBody, reject) => {
    const chunks = []
    req.on('data', (c) => chunks.push(c))
    req.on('end', () => {
      const raw = Buffer.concat(chunks).toString('utf8')
      if (!raw.trim()) {
        resolveBody({})
        return
      }
      try {
        resolveBody(JSON.parse(raw))
      } catch (e) {
        reject(e)
      }
    })
    req.on('error', reject)
  })
}

function publishDesk(ev) {
  const line = `data: ${JSON.stringify(ev)}\n\n`
  for (const res of deskBus) {
    try {
      res.write(line)
    } catch {
      deskBus.delete(res)
    }
  }
}

function mime(file) {
  switch (extname(file)) {
    case '.html': return 'text/html; charset=utf-8'
    case '.js': return 'text/javascript; charset=utf-8'
    case '.css': return 'text/css; charset=utf-8'
    case '.json': return 'application/json; charset=utf-8'
    case '.svg': return 'image/svg+xml'
    case '.png': return 'image/png'
    case '.ico': return 'image/x-icon'
    case '.woff2': return 'font/woff2'
    default: return 'application/octet-stream'
  }
}

function serveDist(req, res, url) {
  const dist = join(webRoot, 'dist')
  if (!existsSync(join(dist, 'index.html'))) return false
  let rel = decodeURIComponent(url.pathname)
  if (rel === '/') rel = '/index.html'
  const file = normalize(join(dist, rel))
  const root = resolve(dist)
  if (relative(root, file).startsWith('..')) {
    json(res, 400, { error: 'bad path' })
    return true
  }
  const target = existsSync(file) && statSync(file).isFile() ? file : join(dist, 'index.html')
  res.writeHead(200, { 'content-type': mime(target) })
  res.end(readFileSync(target))
  return true
}

async function handleApi(req, res, url) {
  const path = url.pathname
  const method = req.method || 'GET'

  if (path === '/api/health' && method === 'GET') {
    res.writeHead(200, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('ok')
    return
  }

  if (path === '/api/config/studio_flags' && method === 'GET') {
    json(res, 200, {
      ok: true,
      flags: { 'studio.assistant_runtime_dsh': true },
    })
    return
  }

  if (path === '/api/library' && method === 'GET') {
    json(res, 200, { novels: libraryItems(repoRoot) })
    return
  }

  const libOne = path.match(/^\/api\/library\/([^/]+)$/)
  if (libOne && method === 'GET') {
    const name = decodeURIComponent(libOne[1])
    const data = libraryOne(repoRoot, name)
    if (data.error) {
      json(res, 404, data)
      return
    }
    json(res, 200, data)
    return
  }
  if (libOne && method === 'DELETE') {
    const name = decodeURIComponent(libOne[1])
    if (!safeProjectName(name)) {
      json(res, 400, { ok: false, error: '非法项目名', deleted: name })
      return
    }
    const dir = join(repoRoot, 'projects', name)
    if (existsSync(dir)) rmSync(dir, { recursive: true, force: true })
    json(res, 200, { ok: true, deleted: name })
    return
  }

  const preview = path.match(/^\/api\/projects\/([^/]+)\/preview$/)
  if (preview && method === 'GET') {
    const name = decodeURIComponent(preview[1])
    const data = buildPreview(repoRoot, name)
    if (data.error) {
      json(res, 404, data)
      return
    }
    json(res, 200, data)
    return
  }

  const chapter = path.match(/^\/api\/projects\/([^/]+)\/chapters\/(\d+)$/)
  if (chapter && method === 'GET') {
    const data = chapterPayload(repoRoot, decodeURIComponent(chapter[1]), chapter[2])
    json(res, data.ok ? 200 : 404, data)
    return
  }

  if (path.match(/^\/api\/projects\/[^/]+\/loop/) && method === 'GET') {
    json(res, 200, { ok: true, hasJob: false })
    return
  }
  if (path.match(/^\/api\/projects\/[^/]+\/loop\/arm$/) && method === 'POST') {
    json(res, 200, { ok: true, armed: false, loop: { ok: true, hasJob: false } })
    return
  }
  if (path.match(/^\/api\/projects\/[^/]+\/version_nodes/) && method === 'GET') {
    json(res, 200, { ok: true, nodes: [] })
    return
  }
  if (path.match(/^\/api\/projects\/[^/]+\/content$/) && method === 'PUT') {
    json(res, 400, { ok: false, error: '阅读台只读，请在 dsh 里改稿' })
    return
  }

  if (path === '/api/host/v1/desk/focus' && method === 'POST') {
    let body
    try {
      body = await readBody(req)
    } catch {
      json(res, 400, { error: 'invalid json' })
      return
    }
    const project = String(body.project || '').trim()
    if (!safeProjectName(project)) {
      json(res, 400, { error: 'project must be a projects/ directory name' })
      return
    }
    const chapter = Number(body.chapter) || 0
    const tab = String(body.reader_tab || body.readerTab || 'draft').trim() || 'draft'
    const ev = deskFocusEvent(project, chapter > 0 ? chapter : null, tab)
    publishDesk(ev)
    json(res, 200, { ok: true, project, chapter: chapter || null, reader_tab: tab })
    return
  }

  if (path === '/api/host/v1/events' && method === 'GET') {
    const filter = String(url.searchParams.get('project') || '').trim()
    res.writeHead(200, {
      'content-type': 'text/event-stream; charset=utf-8',
      'cache-control': 'no-cache',
      connection: 'keep-alive',
    })
    res.write(': ok\n\n')
    const wrapped = {
      write(chunk) {
        if (filter) {
          try {
            const ev = JSON.parse(String(chunk).replace(/^data:\s*/, '').trim())
            if (ev.project && ev.project !== filter) return
          } catch {
            /* keep */
          }
        }
        res.write(chunk)
      },
    }
    deskBus.add(wrapped)
    const ping = setInterval(() => {
      try {
        res.write(': ping\n\n')
      } catch {
        clearInterval(ping)
        deskBus.delete(wrapped)
      }
    }, 15000)
    req.on('close', () => {
      clearInterval(ping)
      deskBus.delete(wrapped)
    })
    return
  }

  json(res, 404, { error: `no route ${method} ${path}` })
}

async function loadVite() {
  const viteEntry = join(webRoot, 'node_modules/vite/dist/node/index.js')
  if (!existsSync(viteEntry)) return null
  const vite = await import(pathToFileURL(viteEntry).href)
  return vite.createServer({
    root: webRoot,
    appType: 'spa',
    server: { middlewareMode: true },
  })
}

const vite = existsSync(join(webRoot, 'dist', 'index.html')) ? null : await loadVite()

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url || '/', `http://${BIND.host}:${BIND.port}`)
    if (url.pathname.startsWith('/api/')) {
      await handleApi(req, res, url)
      return
    }
    if (serveDist(req, res, url)) return
    if (vite) {
      vite.middlewares(req, res, () => {
        json(res, 404, { error: 'not found' })
      })
      return
    }
    res.writeHead(503, { 'content-type': 'text/plain; charset=utf-8' })
    res.end('阅读台前端未构建。在 web/ 执行 npm install && npm run build 后重试。')
  } catch (e) {
    if (!res.headersSent) {
      json(res, 500, { error: String(e?.message || e) })
    }
  }
})

server.listen(BIND.port, BIND.host, () => {
  console.log(`NovelX desk ${BIND.raw}  repo=${repoRoot}`)
})
