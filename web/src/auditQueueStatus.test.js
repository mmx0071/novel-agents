import { describe, expect, it } from 'vitest'
import {
  effectiveToolDisplayStatus,
  hasFollowOnRunning,
  looksLikeLiveToolProgress,
  todosStillOpen,
} from './auditQueueStatus.js'

describe('todosStillOpen', () => {
  it('is false for empty or all completed', () => {
    expect(todosStillOpen([])).toBe(false)
    expect(todosStillOpen([{ content: '第1章', status: 'completed' }])).toBe(false)
  })

  it('is true when any pending or in_progress', () => {
    expect(todosStillOpen([
      { content: '第1章', status: 'completed' },
      { content: '第2章', status: 'in_progress' },
    ])).toBe(true)
    expect(todosStillOpen([{ content: '第3章', status: 'pending' }])).toBe(true)
  })
})

describe('hasFollowOnRunning', () => {
  it('detects sibling steer_run still in progress', () => {
    const items = [
      { id: 'a', type: 'tool_call', name: 'audit_chapters', status: 'completed' },
      { id: 'b', type: 'tool_call', name: 'steer_run', status: 'in_progress' },
    ]
    expect(hasFollowOnRunning(items, 'a')).toBe(true)
    expect(hasFollowOnRunning(items, 'b')).toBe(false)
  })
})

describe('effectiveToolDisplayStatus', () => {
  it('keeps audit_chapters running while todos open even if wire completed', () => {
    const item = {
      id: 'q',
      type: 'tool_call',
      name: 'audit_chapters',
      status: 'completed',
    }
    expect(effectiveToolDisplayStatus(item, {
      todos: [{ content: '第1章', status: 'in_progress' }],
      turnItems: [item],
    })).toBe('in_progress')
  })

  it('keeps audit_chapters running while follow-on steer is live', () => {
    const item = {
      id: 'q',
      type: 'tool_call',
      name: 'audit_chapters',
      status: 'completed',
    }
    const turnItems = [
      item,
      { id: 's', type: 'tool_call', name: 'steer_run', status: 'in_progress' },
    ]
    expect(effectiveToolDisplayStatus(item, { todos: [], turnItems })).toBe('in_progress')
  })

  it('shows completed when queue todos done and no follow-on', () => {
    const item = {
      id: 'q',
      type: 'tool_call',
      name: 'audit_chapters',
      status: 'completed',
    }
    expect(effectiveToolDisplayStatus(item, {
      todos: [
        { content: '第1章', status: 'completed' },
        { content: '第2章', status: 'completed' },
      ],
      turnItems: [item],
    })).toBe('completed')
  })

  it('does not override non-audit tools', () => {
    const item = {
      id: 'w',
      type: 'tool_call',
      name: 'continue_writing',
      status: 'completed',
    }
    expect(effectiveToolDisplayStatus(item, {
      todos: [{ content: 'x', status: 'pending' }],
      turnItems: [item],
    })).toBe('completed')
  })

  it('keeps audit_chapters running when tip still shows model calls', () => {
    const item = {
      id: 'q',
      type: 'tool_call',
      name: 'audit_chapters',
      status: 'completed',
      output: [
        '（审校未通过 — 请在下方决策卡选择）',
        '评审团自动修订中…（第1轮）',
        '▶ 一致性审计',
        '调用模型中…',
        '⚙ 自动修复: 发现 5 条 P0/P1',
        '… 生成中 · 3068字',
        '调用模型中…',
      ].join('\n'),
    }
    expect(looksLikeLiveToolProgress(item.output)).toBe(true)
    expect(effectiveToolDisplayStatus(item, { todos: [], turnItems: [item] }))
      .toBe('in_progress')
  })

  it('does not treat settled tip as live progress', () => {
    const out = [
      '调用模型中…',
      '✓ 一致性审计: 一致性通过',
      '审阅队列已全部完成。',
    ].join('\n')
    expect(looksLikeLiveToolProgress(out)).toBe(false)
  })
})
