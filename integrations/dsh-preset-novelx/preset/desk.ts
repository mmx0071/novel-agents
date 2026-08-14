/**
 * Reading-desk tools for the NovelX dsh preset.
 * Starts the Node desk on 127.0.0.1:8765, opens the browser, and focuses a project.
 */
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { defineTool } from './defineTool.ts'

const HOST = (process.env.NOVELX_HOST || 'http://127.0.0.1:8765').replace(/\/$/, '')
const BIND = process.env.NOVELX_BIND || '127.0.0.1:8765'

function looksLikeRepo(root: string): boolean {
  return existsSync(join(root, 'web/desk-server.mjs')) && existsSync(join(root, 'projects'))
}

function repoRoot(): string {
  const candidates = [
    process.env.NOVELX_ROOT,
    process.cwd(),
    join(process.cwd(), 'novel-agents'),
  ].filter((p): p is string => Boolean(p))
  const hit = candidates.find(looksLikeRepo)
  if (!hit) {
    throw new Error('找不到 NovelX 仓库。请设 NOVELX_ROOT，或在仓库目录启动 dsh。')
  }
  return hit
}

async function healthOk(): Promise<boolean> {
  try {
    const res = await fetch(`${HOST}/api/health`, { signal: AbortSignal.timeout(1500) })
    return res.ok
  } catch {
    return false
  }
}

function startWeb(): void {
  const repo = repoRoot()
  const script = join(repo, 'web/desk-server.mjs')
  spawn(process.execPath, [script, '--bind', BIND], {
    cwd: repo,
    detached: true,
    stdio: 'ignore',
    env: { ...process.env, NOVELX_ROOT: repo, NOVELX_BIND: BIND },
  }).unref()
}

async function waitUntilUp(ms = 20000): Promise<boolean> {
  const t0 = Date.now()
  while (Date.now() - t0 < ms) {
    if (await healthOk()) return true
    await new Promise((r) => setTimeout(r, 400))
  }
  return healthOk()
}

function openBrowser(url: string): void {
  const opener = process.platform === 'darwin' ? 'open' : process.platform === 'win32' ? 'start' : 'xdg-open'
  spawn(opener, [url], { detached: true, stdio: 'ignore' }).unref()
}

function deskPageUrl(project: string, chapter: number, tab: string): string {
  const q = new URLSearchParams()
  if (project) q.set('project', project)
  if (chapter > 0) q.set('chapter', String(chapter))
  if (tab) q.set('tab', tab)
  const s = q.toString()
  return s ? `${HOST}/?${s}` : `${HOST}/`
}

async function ensureUp(): Promise<boolean> {
  if (await healthOk()) return false
  startWeb()
  if (!(await waitUntilUp())) {
    throw new Error(
      `已尝试启动阅读台，但 ${HOST} 仍无响应。请在仓库执行：cd web && npm install && npm run build && node desk-server.mjs`,
    )
  }
  return true
}

async function focusDesk(project: string, chapter: number, tab: string): Promise<void> {
  if (!project) return
  try {
    await fetch(`${HOST}/api/host/v1/desk/focus`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        project,
        ...(chapter > 0 ? { chapter } : {}),
        ...(tab ? { reader_tab: tab } : {}),
      }),
      signal: AbortSignal.timeout(4000),
    })
  } catch {
    // Desk may still be booting; the query URL is enough for a fresh tab.
  }
}

const deskOutput = {
  schema: { type: 'string' },
  render: (_args: unknown, value: unknown) => [{ type: 'text', text: String(value) }],
}

export const name = 'novelx-desk'
export const inject = ['tools']

export function apply(ctx: { tools: { register: (def: unknown) => void } }) {
  ctx.tools.register(
    defineTool({
      name: 'novelx_open_desk',
      description:
        '打开或跟随 NovelX 阅读台（本机 8765）。写设定/写章时默认调用：传入 project 与 chapter，浏览器会切到该作品并刷新正文。不要把章节正文贴进聊天。',
      parameters: {
        project: { type: 'string', description: 'projects/ 下的目录名' },
        chapter: { type: 'integer', description: '章号；省略则保持当前章或最新章' },
        tab: {
          type: 'string',
          description: '阅读页：draft / outline / master / volume / arcs / bible / characters',
        },
        open_browser: { type: 'boolean', description: '是否打开浏览器，默认 true' },
      },
      output: deskOutput,
      timeoutMs: 45_000,
      async execute(args) {
        const started = await ensureUp()
        const project = String(args.project || '').trim()
        const chapter = Number(args.chapter) || 0
        const tab = String(args.tab || '').trim()
        const url = deskPageUrl(project, chapter, tab)
        await focusDesk(project, chapter, tab)
        if (args.open_browser !== false) openBrowser(url)
        return [
          'status: ok',
          started ? 'action: started' : 'action: already_running',
          `url: ${url}`,
          project ? `project: ${project}` : '',
          chapter > 0 ? `chapter: ${chapter}` : '',
          '阅读台已跟随当前作品。看稿用浏览器翻页，不要把正文贴进聊天。',
        ]
          .filter(Boolean)
          .join('\n')
      },
    }),
  )
}
