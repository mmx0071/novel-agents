import { describe, expect, it } from 'vitest'
import {
  formatWorkedDuration,
  groupTurnItemsForWorked,
  isWorkTimelineItem,
  workedDurationMs,
  workedForSummary,
  workedForTitle,
} from './workedFor.js'

describe('isWorkTimelineItem', () => {
  it('folds tools / skills / reasoning', () => {
    expect(isWorkTimelineItem({ type: 'tool_call' })).toBe(true)
    expect(isWorkTimelineItem({ type: 'reasoning' })).toBe(true)
    expect(isWorkTimelineItem({ type: 'user_message' })).toBe(false)
    expect(isWorkTimelineItem({ type: 'agent_message' })).toBe(false)
  })
})

describe('groupTurnItemsForWorked', () => {
  it('wraps consecutive work items and keeps prose outside', () => {
    const items = [
      { type: 'user_message', text: '审阅' },
      { type: 'reasoning', text: '…', status: 'completed' },
      { type: 'tool_call', name: 'audit_chapters', status: 'completed', duration_ms: 24000 },
      { type: 'agent_message', text: '已提交决策' },
    ]
    const groups = groupTurnItemsForWorked(items)
    expect(groups.map((g) => g.kind)).toEqual(['item', 'worked', 'item'])
    expect(groups[1].items).toHaveLength(2)
    expect(groups[1].durationMs).toBe(24000)
    expect(groups[1].running).toBe(false)
  })

  it('marks worked block running when a tool is in progress', () => {
    const groups = groupTurnItemsForWorked([
      { type: 'tool_call', name: 'audit_chapters', status: 'in_progress' },
    ])
    expect(groups[0].running).toBe(true)
  })
})

describe('workedForTitle / summary', () => {
  it('formats Chinese consumer duration titles', () => {
    expect(formatWorkedDuration(800)).toBe('不到 1 秒')
    expect(formatWorkedDuration(24000)).toBe('24 秒')
    expect(formatWorkedDuration(84000)).toBe('1 分 24 秒')
    expect(workedForTitle({ running: true, durationMs: 0 })).toBe('进行中…')
    expect(workedForTitle({ running: false, durationMs: 24000 })).toBe('用时 24 秒')
    expect(workedForTitle({ running: true, awaiting: true, durationMs: 0 })).toBe('等待你选择…')
  })

  it('summarizes tool labels', () => {
    const s = workedForSummary(
      [
        { type: 'tool_call', name: 'audit_chapters' },
        { type: 'tool_call', name: 'query_lore' },
        { type: 'reasoning' },
      ],
      { toolLabelZh: (n) => (n === 'audit_chapters' ? '审阅队列' : n === 'query_lore' ? '查询设定' : n) },
    )
    expect(s).toContain('2 项操作')
    expect(s).toContain('审阅队列')
    expect(s).toContain('思考')
  })
})

describe('workedDurationMs', () => {
  it('uses max when sum looks parallel-inflated', () => {
    expect(workedDurationMs([
      { duration_ms: 80000 },
      { duration_ms: 70000 },
    ])).toBe(80000)
  })

  it('sums sequential short tools', () => {
    expect(workedDurationMs([
      { duration_ms: 100 },
      { duration_ms: 200 },
    ])).toBe(300)
  })
})
