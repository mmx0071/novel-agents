/**
 * Deterministic NovelX checks for the dsh preset (schema / names / length / phase).
 */
import { existsSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { defineTool } from './defineTool.ts'

function looksLikeRepo(root: string): boolean {
  return existsSync(join(root, 'web/src/deskCheck.js')) && existsSync(join(root, 'projects'))
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

const checkOutput = {
  schema: { type: 'string' },
  render: (_args: unknown, value: unknown) => [{ type: 'text', text: String(value) }],
}

export const name = 'novelx-check'
export const inject = ['tools']

export function apply(ctx: { tools: { register: (def: unknown) => void } }) {
  ctx.tools.register(
    defineTool({
      name: 'novelx_check',
      description:
        '确定性检查 projects/<目录>/：schema、禁名、字数、setup/卷相位、跳章。写设定或写章落盘后调用。有 blocker 则不要推进 next_chapter。不要把正文贴进聊天。',
      parameters: {
        project: { type: 'string', description: 'projects/ 下的目录名' },
        chapter: { type: 'integer', description: '章号；省略则用 state.next_chapter' },
        scope: {
          type: 'string',
          description: 'all / chapter / draft / outline / setting / phase，默认 all',
        },
      },
      output: checkOutput,
      timeoutMs: 20_000,
      async execute(args) {
        const repo = repoRoot()
        const mod = await import(pathToFileURL(join(repo, 'web/src/deskCheck.js')).href)
        const result = mod.runCheck({
          repoRoot: repo,
          project: String(args.project || '').trim(),
          chapter: Number(args.chapter) || 0,
          scope: String(args.scope || 'all').trim() || 'all',
        })
        return mod.formatCheckReport(result)
      },
    }),
  )
  ctx.tools.register(
    defineTool({
      name: 'novelx_migrate',
      description:
        '把旧落盘迁到当前阅读格式：outline.md→outline.json、扁卷纲→arc_outlines/、同步摘要折进当前状态。不发明情节。打开旧项目或 check 提示旧格式时调用。',
      parameters: {
        project: { type: 'string', description: 'projects/ 下的目录名' },
      },
      output: checkOutput,
      timeoutMs: 20_000,
      async execute(args) {
        const repo = repoRoot()
        const mod = await import(pathToFileURL(join(repo, 'web/src/deskMigrate.js')).href)
        const result = mod.migrateProject(repo, String(args.project || '').trim())
        return mod.formatMigrateReport(result)
      },
    }),
  )
}
