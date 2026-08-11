import { describe, expect, it } from 'vitest'
import { todosCursorLabel, todosHeadline, windowTodos } from './todoWindow.js'

describe('windowTodos', () => {
  const todos = [
    { content: '审校第1章（通过）', status: 'completed' },
    { content: '审校第2章（通过）', status: 'completed' },
    { content: '审校第3章（未通过 · 待处理）', status: 'in_progress' },
    ...Array.from({ length: 17 }, (_, i) => ({
      content: `审校第${i + 4}章（待审）`,
      status: 'pending',
    })),
  ]

  it('keeps last completed + current, counts pending', () => {
    const win = windowTodos(todos, { keepCompleted: 2 })
    expect(win.done).toBe(2)
    expect(win.total).toBe(20)
    expect(win.pendingCount).toBe(17)
    expect(win.rows).toHaveLength(3)
    expect(win.rows.map((r) => r.item.content)).toEqual([
      '审校第1章（通过）',
      '审校第2章（通过）',
      '审校第3章（未通过 · 待处理）',
    ])
  })

  it('headline for work-card header', () => {
    expect(todosHeadline(todos)).toBe('2/20 · 审校第3章（未通过 · 待处理）')
  })

  it('Chinese todo fold label', () => {
    expect(todosCursorLabel(todos)).toBe('待办 2/20 · 审校第3章')
    expect(todosCursorLabel([
      { content: 'a', status: 'completed' },
      { content: 'b', status: 'completed' },
    ])).toBe('待办已完成（2/2）')
  })
})
