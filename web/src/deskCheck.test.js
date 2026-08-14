import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { formatCheckReport, runCheck } from './deskCheck.js'

const temps = []

function write(path, text) {
  mkdirSync(join(path, '..'), { recursive: true })
  writeFileSync(path, text)
}

function makeRepo(extra = {}) {
  const root = mkdtempSync(join(tmpdir(), 'novelx-check-'))
  temps.push(root)
  write(join(root, 'config', 'chapter.yaml'), [
    'word_min: 40',
    'word_max: 2000',
    'word_hard_min: 20',
    'word_hard_max: 5000',
    '',
  ].join('\n'))
  write(join(root, 'config', 'naming_rules.yaml'), [
    'forbidden_names:',
    '  - 林远',
    '  - 天元宗',
    '',
  ].join('\n'))
  write(join(root, 'config', 'features.yaml'), [
    'features:',
    '  studio.enforce_setup_gate: true',
    '  studio.enforce_volume_phase: true',
    '  studio.enforce_chapter_order: true',
    '',
  ].join('\n'))
  write(join(root, 'config', 'content_rules.yaml'), [
    'rules:',
    '  banned_name:',
    '    enabled: true',
    '    blocking: true',
    '  contradiction_marker:',
    '    enabled: true',
    '    blocking: true',
    '    marker: "[CONTRADICTION]"',
    '    message: "正文残留管道内部标记：{marker}"',
    '  too_many_new_facts:',
    '    enabled: true',
    '    blocking: true',
    '    max_count: 8',
    '    pattern: "\\\\[NEW_FACT:[^\\\\]]+\\\\]"',
    '  meta_chapter_ref:',
    '    enabled: true',
    '    blocking: true',
    '    pattern: "第\\\\s*[一二三四五六七八九十百千零〇两\\\\d]+\\\\s*章"',
    '    allow_heading: true',
    '    message: "正文出现章号元叙述「{match}」（约第{line}行）"',
    '  pipeline_meta_leak:',
    '    enabled: true',
    '    blocking: true',
    '    patterns:',
    '      - 按章纲',
    '    message: "正文出现管线元叙述「{match}」（约第{line}行）"',
    '',
  ].join('\n'))
  const dir = join(root, 'projects', 'sample-novel')
  if (!extra.skipChapter) mkdirSync(join(dir, 'chapters', '001'), { recursive: true })
  else mkdirSync(join(dir, 'chapters'), { recursive: true })
  mkdirSync(join(dir, 'entities', 'characters'), { recursive: true })
  mkdirSync(join(dir, 'artifacts'), { recursive: true })
  mkdirSync(join(dir, 'plots'), { recursive: true })
  write(join(dir, 'meta.json'), JSON.stringify({
    name: '样例长篇',
    setup_phase: extra.setup_phase || 'ready',
    project_mode: 'longform',
  }, null, 2))
  write(join(dir, 'state.json'), JSON.stringify({
    name: '样例长篇',
    next_chapter: extra.next_chapter ?? 1,
    published_count: extra.published_count ?? 0,
    meta: { volume_phase: extra.volume_phase || 'drafting_volume' },
  }, null, 2))
  return { root, dir }
}

function longBody(n = 30) {
  return Array.from({ length: n }, () => '主角在样例小区门口停住，甲把信物递过来。').join('')
}

afterEach(() => {
  while (temps.length) rmSync(temps.pop(), { recursive: true, force: true })
})

describe('deskCheck', () => {
  it('blocks a missing chapter title and forbidden name', () => {
    const { root, dir } = makeRepo({ setup_phase: 'ready' })
    writeFileSync(join(dir, 'chapters', '001', 'draft.md'), [
      '没有标题',
      '',
      `${longBody()}林远走过来。`,
      '',
    ].join('\n'))
    const r = runCheck({ repoRoot: root, project: 'sample-novel', chapter: 1, scope: 'draft' })
    expect(r.ok).toBe(false)
    const ids = r.issues.map((i) => i.id)
    expect(ids).toContain('schema.draft')
    expect(ids).toContain('banned_name')
  })

  it('passes a shaped draft and reports ok', () => {
    const { root, dir } = makeRepo({ setup_phase: 'ready', published_count: 1 })
    writeFileSync(join(dir, 'chapters', '001', 'draft.md'), [
      '# 第1章 开端',
      '',
      longBody(40),
      '',
    ].join('\n'))
    writeFileSync(join(dir, 'chapters', '001', 'outline.json'), JSON.stringify({
      title: '开端',
      pov: '主角',
      time_location: '样例小区',
      goal: '拿到信物',
      conflict: '甲不肯松口',
      emotion_curve: '紧到松',
      plot_includes: ['见面'],
      plot_defers: ['后话'],
      key_events: ['相遇', '交物'],
      characters: ['主角', '甲'],
      items: ['信物'],
      locations: ['样例小区'],
      scene_tags: ['对峙'],
      cliffhanger: '门开了',
      lore_queries: [],
    }, null, 2))
    const r = runCheck({ repoRoot: root, project: 'sample-novel', chapter: 1, scope: 'chapter' })
    expect(r.ok).toBe(true)
    expect(formatCheckReport(r)).toContain('status: ok')
  })

  it('blocks new chapter when setup is still collecting', () => {
    const { root, dir } = makeRepo({ setup_phase: 'collecting', published_count: 0 })
    writeFileSync(join(dir, 'chapters', '001', 'draft.md'), `# 第1章 开端\n\n${longBody(40)}\n`)
    const r = runCheck({ repoRoot: root, project: 'sample-novel', chapter: 1, scope: 'phase' })
    expect(r.ok).toBe(false)
    expect(r.issues.some((i) => i.id === 'phase.setup')).toBe(true)
  })

  it('blocks jumping chapter 2 when chapter 1 is missing', () => {
    const { root } = makeRepo({
      setup_phase: 'ready',
      published_count: 1,
      next_chapter: 2,
      skipChapter: true,
    })
    const r = runCheck({ repoRoot: root, project: 'sample-novel', chapter: 2, scope: 'phase' })
    expect(r.ok).toBe(false)
    expect(r.issues.some((i) => i.id === 'phase.order')).toBe(true)
  })

  it('rejects unsafe project names', () => {
    const { root } = makeRepo()
    const r = runCheck({ repoRoot: root, project: '../etc', scope: 'phase' })
    expect(r.ok).toBe(false)
    expect(r.issues[0].id).toBe('project')
  })
})
