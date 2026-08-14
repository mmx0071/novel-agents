import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { displayEntityCard, displayPlotCardBody, draftBodyChars } from './deskDisplay.js'
import {
  buildPreview,
  chapterPayload,
  deskFocusEvent,
  libraryItems,
  safeProjectName,
} from './deskPreview.js'

const temps = []

function makeRepo() {
  const root = mkdtempSync(join(tmpdir(), 'novelx-desk-'))
  temps.push(root)
  const dir = join(root, 'projects', 'sample-novel')
  mkdirSync(join(dir, 'chapters', '001'), { recursive: true })
  mkdirSync(join(dir, 'entities', 'characters'), { recursive: true })
  mkdirSync(join(dir, 'entities', 'items'), { recursive: true })
  mkdirSync(join(dir, 'entities', 'locations'), { recursive: true })
  mkdirSync(join(dir, 'plots'), { recursive: true })
  mkdirSync(join(dir, 'artifacts'), { recursive: true })
  writeFileSync(join(dir, 'meta.json'), JSON.stringify({
    name: '样例长篇',
    genre: '悬疑',
    setup_phase: 'ready',
    project_mode: 'longform',
  }, null, 2))
  writeFileSync(join(dir, 'state.json'), JSON.stringify({
    name: '样例长篇',
    genre: '悬疑',
    next_chapter: 2,
    published_count: 1,
  }, null, 2))
  writeFileSync(join(dir, 'chapters', '001', 'draft.md'), [
    '# 第1章 开端',
    '',
    '主角在样例小区门口停住。甲把信物递过来。',
    '',
  ].join('\n'))
  writeFileSync(join(dir, 'chapters', '001', 'outline.json'), JSON.stringify({
    title: '开端',
    pov: '主角',
    time_location: '样例小区 · 傍晚',
    goal: '拿到信物',
    conflict: '甲不肯松口',
    emotion_curve: '紧到松',
    plot_includes: ['见面'],
    plot_defers: [],
    key_events: ['相遇', '交物'],
    characters: ['主角', '甲'],
    items: ['信物'],
    locations: ['样例小区'],
    scene_tags: ['对峙'],
    cliffhanger: '门开了',
    lore_queries: [],
  }, null, 2))
  writeFileSync(join(dir, 'entities', 'characters', 'zhujue.md'), [
    '---',
    'name: 主角',
    'status: active',
    '---',
    '',
    '# 主角',
    '',
    '## History',
    '在样例小区住过。',
    '',
    '## Personality',
    '少话。',
    '',
    '## Core events',
    '- 接到信物',
    '',
    '## Current status',
    '在场。',
    '',
  ].join('\n'))
  writeFileSync(join(dir, 'entities', 'items', 'xinyu.md'), [
    '---',
    'name: 信物',
    'status: active',
    '---',
    '',
    '# 信物',
    '',
    '## Origin',
    '甲交出。',
    '',
    '## Usage',
    '开门。',
    '',
    '## Current status',
    '在主角手里。',
    '',
  ].join('\n'))
  writeFileSync(join(dir, 'plots', 'meet.md'), [
    '---',
    'title: 见面',
    'scope: local',
    'plot_type: main',
    'status: active',
    'needs_bridge: false',
    '---',
    '',
    '# 见面',
    '',
    '## 概览',
    '主角与甲见面。',
    '',
    '## 剧情走向',
    '交信物。',
    '',
    '## 冲突与赌注',
    '信物真伪。',
    '',
    '## 出场人物',
    '- 主角',
    '- 甲',
    '',
    '## 收束条件',
    '信物易手。',
    '',
  ].join('\n'))
  writeFileSync(join(dir, 'artifacts', 'bible.md'), [
    '# 世界观',
    '',
    '## 0. 一句话世界',
    '样例小区里的一桩旧事。',
    '',
  ].join('\n'))
  writeFileSync(join(dir, 'artifacts', 'master_outline.md'), [
    '# 总纲',
    '',
    '## Logline',
    '主角要回信物。',
    '',
    '## 分卷',
    '第一卷在样例小区。',
    '',
    '## 主角弧',
    '从躲到面对。',
    '',
    '## 核心冲突',
    '信物归属。',
    '',
  ].join('\n'))
  return root
}

afterEach(() => {
  while (temps.length) {
    rmSync(temps.pop(), { recursive: true, force: true })
  }
})

describe('deskDisplay', () => {
  it('strips frontmatter and shows Chinese H2 on entity cards', () => {
    const shown = displayEntityCard('characters', [
      '---',
      'name: 主角',
      '---',
      '',
      '# 主角',
      '',
      '## History',
      '住过样例小区。',
    ].join('\n'))
    expect(shown).not.toContain('---')
    expect(shown).toContain('## 经历')
    expect(shown).not.toContain('## History')
  })

  it('counts draft body after the title line', () => {
    expect(draftBodyChars('# 第1章 开端\n\n甲把信物递过来。\n')).toBe('甲把信物递过来。'.length)
  })

  it('keeps plot body without frontmatter', () => {
    const shown = displayPlotCardBody('---\ntitle: 见面\n---\n\n## 概览\n主角与甲见面。\n')
    expect(shown).toContain('## 概览')
    expect(shown).not.toContain('title:')
  })
})

describe('deskPreview', () => {
  it('rejects unsafe project names', () => {
    expect(safeProjectName('../etc')).toBe(false)
    expect(safeProjectName('sample-novel')).toBe(true)
  })

  it('lists library and assembles preview without embedding drafts', () => {
    const root = makeRepo()
    const novels = libraryItems(root)
    expect(novels).toEqual([expect.objectContaining({
      id: 'sample-novel',
      display_title: '样例长篇',
      project_mode: 'longform',
    })])
    const preview = buildPreview(root, 'sample-novel')
    expect(preview.error).toBeUndefined()
    expect(preview.chapters).toHaveLength(1)
    expect(preview.chapters[0].has_draft).toBe(true)
    expect(preview.chapters[0].draft).toBe('')
    expect(preview.entities.characters[0].name).toBe('主角')
    expect(preview.entities.characters[0].markdown).toContain('## 经历')
    expect(preview.entities.items[0].name).toBe('信物')
    expect(preview.artifacts.bible).toContain('一句话世界')
    expect(preview.artifacts.master_outline).toContain('## 一句话卖点')
    expect(preview.plots[0].title).toBe('见面')
  })

  it('loads chapter body on demand', () => {
    const root = makeRepo()
    const ch = chapterPayload(root, 'sample-novel', 1)
    expect(ch.ok).toBe(true)
    expect(ch.draft).toContain('信物')
    expect(ch.outline).toContain('## 目标')
    expect(ch.outline).toContain('拿到信物')
  })

  it('builds a genre-neutral desk focus event', () => {
    expect(deskFocusEvent('sample-novel', 2, 'bible')).toEqual({
      kind: 'focus',
      project: 'sample-novel',
      chapter: 2,
      readerTab: 'bible',
      tool: 'desk_focus',
    })
  })
})
