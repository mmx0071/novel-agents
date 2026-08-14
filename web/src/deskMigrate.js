/**
 * One-shot reader-format migrate. Does not invent plot.
 */

import { existsSync, mkdirSync, readdirSync, readFileSync, unlinkSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { hasH2, joinFm, parseChapterOutline, splitFm } from './deskDisplay.js'
import { listChapterNumbers, projectDir, safeProjectName } from './deskPreview.js'

function readText(path) {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

function writeText(path, text) {
  mkdirSync(join(path, '..'), { recursive: true })
  writeFileSync(path, text)
}

const SYNC_STUBS = ['卷末同步摘要', '章后同步摘要', '剧情同步摘要']

function foldSyncStubs(text) {
  const { meta, body } = splitFm(text)
  const lines = body.split('\n')
  const out = []
  const stubs = []
  let i = 0
  let folded = false
  while (i < lines.length) {
    const t = lines[i].trim()
    if (t.startsWith('## ') && SYNC_STUBS.includes(t.slice(3).trim())) {
      folded = true
      i += 1
      const chunk = []
      while (i < lines.length) {
        const lt = lines[i].trim()
        if (lt.startsWith('## ')) break
        if (lt.startsWith('### ')) {
          i += 1
          continue
        }
        if (lt) chunk.push(lt)
        i += 1
      }
      if (chunk.length) stubs.push(chunk.join('\n'))
      continue
    }
    out.push(lines[i])
    i += 1
  }
  if (!folded) return { text, changed: false }
  let next = out.join('\n')
  const blob = stubs.join('\n\n').trim()
  if (blob) {
    if (hasH2(next, ['Current status', 'Current Status', '当前状态', '现状'])) {
      next = `${next.trimEnd()}\n\n### 同步终态\n\n${blob}\n`
    } else {
      next = `${next.trimEnd()}\n\n## Current status\n\n${blob}\n`
    }
  }
  if (!meta.status) meta.status = 'active'
  return { text: joinFm(meta, next.trim()), changed: true }
}

function migrateOutlines(dir, report) {
  const root = join(dir, 'chapters')
  if (!existsSync(root)) return
  for (const n of listChapterNumbers(dir)) {
    const unit = join(root, String(n).padStart(3, '0'))
    const jsonPath = join(unit, 'outline.json')
    if (!existsSync(jsonPath)) {
      const md = readText(join(unit, 'outline.md'))
      if (!md.trim()) continue
      const parsed = parseChapterOutline(md)
      if (parsed) {
        writeText(jsonPath, `${JSON.stringify(parsed, null, 2)}\n`)
        report.outlines_migrated.push(n)
      } else {
        report.notes.push(`第${n}章 outline.md 不是 JSON，未自动迁（请改成 outline.json）`)
      }
      continue
    }
    try {
      const v = JSON.parse(readText(jsonPath))
      if (!v || typeof v !== 'object') continue
      let changed = false
      for (const key of ['items', 'locations', 'plot_includes', 'plot_defers']) {
        if (v[key] == null) {
          v[key] = []
          changed = true
        }
      }
      if (changed) {
        writeText(jsonPath, `${JSON.stringify(v, null, 2)}\n`)
        report.outlines_padded.push(n)
      }
    } catch {
      report.notes.push(`第${n}章 outline.json 解析失败`)
    }
  }
}

function migrateArc(dir, report) {
  const destDir = join(dir, 'artifacts', 'arc_outlines')
  const legacy = join(dir, 'artifacts', 'arc_outline.md')
  const planner = join(dir, 'artifacts', 'arc_planner.md')
  for (const src of [legacy, planner]) {
    const text = readText(src)
    if ([...text.trim()].length < 20) continue
    mkdirSync(destDir, { recursive: true })
    const dest = join(destDir, '01.md')
    if (!existsSync(dest)) writeText(dest, text)
    try {
      unlinkSync(src)
    } catch {
      /* keep */
    }
    report.notes.push(`卷纲 ${src.split('/').pop()} → artifacts/arc_outlines/01.md`)
  }
}

function migrateEntities(dir, report) {
  for (const group of ['characters', 'items', 'locations']) {
    const root = join(dir, 'entities', group)
    if (!existsSync(root)) continue
    for (const name of readdirSync(root).filter((n) => n.endsWith('.md'))) {
      const path = join(root, name)
      const raw = readText(path)
      const { text, changed } = foldSyncStubs(raw)
      if (changed) {
        writeText(path, text)
        report.entities_stub_folded.push(`${group}/${name}`)
      }
    }
  }
}

export function migrateProject(repoRoot, name) {
  const report = {
    ok: true,
    project: name,
    outlines_migrated: [],
    outlines_padded: [],
    entities_stub_folded: [],
    notes: [],
  }
  if (!safeProjectName(name)) {
    return { ...report, ok: false, notes: ['非法项目名'] }
  }
  const dir = projectDir(repoRoot, name)
  if (!existsSync(dir)) {
    return { ...report, ok: false, notes: [`项目「${name}」不存在`] }
  }
  migrateOutlines(dir, report)
  migrateArc(dir, report)
  migrateEntities(dir, report)
  return report
}

export function formatMigrateReport(report) {
  const lines = [
    `status: ${report.ok ? 'ok' : 'error'}`,
    `project: ${report.project}`,
  ]
  if (report.outlines_migrated?.length) {
    lines.push(`outlines_migrated: ${report.outlines_migrated.join(', ')}`)
  }
  if (report.outlines_padded?.length) {
    lines.push(`outlines_padded: ${report.outlines_padded.join(', ')}`)
  }
  if (report.entities_stub_folded?.length) {
    lines.push(`entities_stub_folded: ${report.entities_stub_folded.join(', ')}`)
  }
  for (const n of report.notes || []) lines.push(`- ${n}`)
  if (lines.length === 2) lines.push('没有需要迁移的旧格式。')
  return lines.join('\n')
}
