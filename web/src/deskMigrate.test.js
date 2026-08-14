import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { formatBodyStateBoard } from './deskBodyState.js'
import { migrateProject } from './deskMigrate.js'

const temps = []

function write(path, text) {
  mkdirSync(join(path, '..'), { recursive: true })
  writeFileSync(path, text)
}

afterEach(() => {
  while (temps.length) rmSync(temps.pop(), { recursive: true, force: true })
})

describe('deskMigrate', () => {
  it('pads outline.json and moves flat 卷纲', () => {
    const root = mkdtempSync(join(tmpdir(), 'novelx-mig-'))
    temps.push(root)
    const dir = join(root, 'projects', 'sample-novel')
    mkdirSync(join(dir, 'chapters', '001'), { recursive: true })
    mkdirSync(join(dir, 'artifacts'), { recursive: true })
    write(join(dir, 'chapters', '001', 'outline.json'), JSON.stringify({
      title: '开端',
      pov: '主角',
      key_events: ['相遇', '交物'],
    }))
    write(join(dir, 'artifacts', 'arc_outline.md'), '# 第1卷 · 样例小区\n\n## 终止条件\n拿到信物。\n')
    const r = migrateProject(root, 'sample-novel')
    expect(r.ok).toBe(true)
    expect(r.outlines_padded).toContain(1)
    const padded = JSON.parse(readFileSync(join(dir, 'chapters', '001', 'outline.json'), 'utf8'))
    expect(padded.items).toEqual([])
    expect(padded.locations).toEqual([])
    expect(existsSync(join(dir, 'artifacts', 'arc_outlines', '01.md'))).toBe(true)
    expect(existsSync(join(dir, 'artifacts', 'arc_outline.md'))).toBe(false)
  })

  it('folds 卷末同步摘要 into Current status', () => {
    const root = mkdtempSync(join(tmpdir(), 'novelx-mig-'))
    temps.push(root)
    const dir = join(root, 'projects', 'sample-novel')
    mkdirSync(join(dir, 'entities', 'characters'), { recursive: true })
    write(join(dir, 'entities', 'characters', 'zhujue.md'), [
      '---',
      'name: 主角',
      '---',
      '',
      '# 主角',
      '',
      '## History',
      '住过样例小区。',
      '',
      '## 卷末同步摘要',
      '信物在手里。',
      '',
    ].join('\n'))
    const r = migrateProject(root, 'sample-novel')
    expect(r.entities_stub_folded.some((x) => x.includes('zhujue'))).toBe(true)
    const text = readFileSync(join(dir, 'entities', 'characters', 'zhujue.md'), 'utf8')
    expect(text).toContain('## Current status')
    expect(text).toContain('信物在手里')
    expect(text).not.toContain('## 卷末同步摘要')
  })
})

describe('deskBodyState', () => {
  it('reads injuries from prior summary.json', () => {
    const root = mkdtempSync(join(tmpdir(), 'novelx-bs-'))
    temps.push(root)
    const dir = join(root, 'projects', 'sample-novel')
    mkdirSync(join(dir, 'chapters', '001'), { recursive: true })
    write(join(dir, 'chapters', '001', 'summary.json'), JSON.stringify({
      event_summary: '见面',
      body_state: { injuries: ['主角左臂擦伤'], ability_loci: [] },
    }))
    const board = formatBodyStateBoard(dir, 2)
    expect(board).toContain('第1章')
    expect(board).toContain('伤：主角左臂擦伤')
  })
})
