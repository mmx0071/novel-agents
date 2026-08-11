import { describe, expect, it } from 'vitest'
import { parseToolProgress, summarizeToolProgress } from './toolProgress.js'

const SAMPLE = `
——
审阅队列：第1章（1/20）
## 审阅队列《样例》（1/20）
当前：第1章
▶ 第1章 · 待审
· 第2章 · 待审
· 第20章 · 待审

▶ 一致性审计
  调用模型中…
⏸ 第1章未通过（2 条阻断），请按问题选择处理项
✓ 一致性审计: 一致性未通过

【评审团】已自动选择修订阻断项，正在执行局部修订…
评审团自动修订中…（第1轮 · 整章）
▶ script_writer
  … 生成中 · 25字
↻ draft · 80
  正文已写入 · 80字
↻ draft · 1710
✓ script_writer: 正文已写入（1711 字）

▶ 节奏审查
  调用模型中…
✓ 节奏审查: 节奏审查完成

▶ 章节摘要
  调用模型中…
`

describe('parseToolProgress', () => {
  it('lifts pipeline steps and compresses queue checklist + draft spam', () => {
    // Audit streams omit queue by default — keep raw rows only when asked.
    const { entries, structured } = parseToolProgress(SAMPLE, { omitQueue: false })
    expect(structured).toBe(true)
    const queues = entries.filter((e) => e.kind === 'queue')
    expect(queues.length).toBe(1)
    expect(queues.some((q) => /样例/.test(q.name))).toBe(false)
    expect(entries.filter((e) => e.kind === 'step' && e.name === '一致性审计').length).toBe(1)
    const writer = entries.find((e) => e.name === 'script_writer')
    expect(writer?.mark).toBe('✓')
    expect(writer?.summary).toContain('1711')
    // draft ticks collapsed into live, not one row each
    expect(entries.filter((e) => /draft/i.test(e.name || '')).length).toBe(0)
    const tip = entries.find((e) => e.name === '章节摘要')
    expect(tip?.status).toBe('running')
    expect(tip?.live).toContain('调用模型')
  })

  it('summarizes running tip for collapsed headers', () => {
    const { entries } = parseToolProgress(SAMPLE)
    const s = summarizeToolProgress(entries)
    expect(s).toMatch(/章节摘要|调用模型/)
  })

  it('omits queue checklist for audit streams by default', () => {
    const { entries } = parseToolProgress(SAMPLE)
    expect(entries.every((e) => e.kind !== 'queue')).toBe(true)
    expect(entries.some((e) => e.name === 'script_writer')).toBe(true)
  })

  it('omits chapter milestone steps for audit streams', () => {
    const { entries } = parseToolProgress(
      `
▶ 第3章 · 待审
▶ 一致性审计
  仍在等待首包… 30s
✓ 一致性审计: 一致性通过
✓ 第3章 · 通过
`,
    )
    expect(entries.every((e) => !/^第\d+章/.test(e.name || ''))).toBe(true)
    expect(entries.some((e) => e.name === '一致性审计')).toBe(true)
  })

  it('collapses prior-chapter micro steps when advancing', () => {
    const { entries } = parseToolProgress(`
审阅队列：第1章（1/20）
▶ 一致性审计
✓ 一致性审计: 一致性通过
✓ 节奏审查: 节奏审查完成
审阅队列：第2章（2/20）
▶ 一致性审计
✓ 一致性审计: 一致性通过
审阅队列：第3章（3/20）
▶ 一致性审计
  调用模型中…
`)
    const rolled = entries.find((e) => /第1–2章|已审过/.test(`${e.name}${e.summary || ''}`))
    expect(rolled).toBeTruthy()
    const running = entries.filter((e) => e.status === 'running')
    expect(running).toHaveLength(1)
    expect(running[0].name).toBe('一致性审计')
    // Prior chapter's pacing row should be gone after collapse.
    expect(entries.some((e) => e.name === '节奏审查')).toBe(false)
  })

  it('shortens gate pause lines to point at ApprovalOptions outside', () => {
    const { entries } = parseToolProgress(
      '⏸ 第1章未通过（3 条阻断），请按问题选择处理项（1. 修：TIMELINE · 2. 修：POV）\n',
    )
    const pause = entries.find((e) => e.mark === '⏸')
    expect(pause?.name).toMatch(/见下方选项/)
    expect(pause?.name.length).toBeLessThan(80)
  })

  it('handles ⏸ on its own line before the gate text', () => {
    const { entries } = parseToolProgress(
      '⏸\n第3章未通过（1 条阻断），请按问题选择处理项（1. 修：TIMELINE · 2. 接受）\n',
    )
    const pause = entries.find((e) => e.mark === '⏸')
    expect(pause?.name).toMatch(/见下方选项/)
    expect(pause?.name).not.toMatch(/TIMELINE/)
  })

  it('keeps only the tip step running; chapter milestones never spin', () => {
    const { entries } = parseToolProgress(`
▶ 第2章 · 待审
▶ 一致性审计
  调用模型中…
✓ 一致性审计: 一致性通过
✓ 第2章 · 通过 — 一致性通过
▶ 第3章 · 待审
▶ 一致性审计
  调用模型中…
`, { omitQueue: false, compactHistory: false })
    const ch2 = entries.find((e) => /第2章/.test(e.name))
    expect(ch2?.mark).toBe('✓')
    expect(ch2?.status).not.toBe('running')
    const ch3 = entries.find((e) => /第3章/.test(e.name))
    expect(ch3?.status).toBe('note')
    const running = entries.filter((e) => e.status === 'running')
    expect(running).toHaveLength(1)
    expect(running[0].name).toBe('一致性审计')
  })
})
