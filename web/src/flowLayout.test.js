import { describe, expect, it } from 'vitest'
import {
  pickTodoHostSegment,
  todosFlowSummary,
  workItemsHostTodos,
} from './flowLayout.js'

describe('flowLayout', () => {
  it('detects audit queue as todo host', () => {
    expect(workItemsHostTodos([
      { type: 'tool_call', name: 'audit_chapters' },
    ])).toBe(true)
    expect(workItemsHostTodos([
      { type: 'tool_call', name: 'query_lore' },
    ])).toBe(false)
  })

  it('picks live host segment for todos', () => {
    const segments = [
      { kind: 'item' },
      {
        kind: 'worked',
        running: false,
        items: [{ type: 'tool_call', name: 'query_lore' }],
      },
      {
        kind: 'worked',
        running: true,
        items: [{ type: 'tool_call', name: 'audit_chapters', status: 'in_progress' }],
      },
    ]
    expect(pickTodoHostSegment(segments, [{ content: '审校第1章', status: 'in_progress' }]))
      .toBe(2)
  })

  it('summarizes todos for worked header', () => {
    expect(todosFlowSummary([
      { content: '审校第1章（通过）', status: 'completed' },
      { content: '审校第2章（进行中）', status: 'in_progress' },
      { content: '审校第3章（待审）', status: 'pending' },
    ])).toBe('1/3 · 审校第2章')
  })
})
