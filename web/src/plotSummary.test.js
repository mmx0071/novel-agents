import { describe, expect, it } from 'vitest'
import { pickActivePlot, summarizePlotForDesk } from './plotSummary.js'

describe('pickActivePlot', () => {
  it('returns null for empty list', () => {
    expect(pickActivePlot([])).toBeNull()
    expect(pickActivePlot(null)).toBeNull()
  })

  it('prefers in_progress over planned and completed', () => {
    const picked = pickActivePlot([
      { title: '甲', status: 'completed' },
      { title: '乙', status: 'planned' },
      { title: '丙', status: 'in_progress' },
    ])
    expect(picked.title).toBe('丙')
  })

  it('prefers bridging after in_progress', () => {
    const picked = pickActivePlot([
      { title: '甲', status: 'bridging' },
      { title: '乙', status: 'planned' },
    ])
    expect(picked.title).toBe('甲')
  })
})

describe('summarizePlotForDesk', () => {
  it('uses overview field when present', () => {
    const s = summarizePlotForDesk({
      title: '城中线索',
      status: 'in_progress',
      overview: '主角在城中寻找线索。',
    })
    expect(s.title).toBe('城中线索')
    expect(s.statusLabel).toBe('进行中')
    expect(s.body).toContain('寻找线索')
  })

  it('extracts 概览 section from markdown', () => {
    const s = summarizePlotForDesk({
      title: 'demo',
      status: 'planned',
      markdown: '# demo\n\n## 概览\n\n这是概览段落。\n\n## 剧情走向\n\n走向内容。\n',
    })
    expect(s.body).toContain('这是概览段落')
    expect(s.statusLabel).toBe('计划中')
  })

  it('truncates long body', () => {
    const long = '字'.repeat(200)
    const s = summarizePlotForDesk({ title: 'demo', overview: long }, 40)
    expect(s.body.length).toBeLessThanOrEqual(41)
    expect(s.body.endsWith('…')).toBe(true)
  })
})
