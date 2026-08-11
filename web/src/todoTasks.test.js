import { describe, expect, it } from 'vitest'
import { buildTodoTasks, segmentStepsByUnit } from './todoTasks.js'

describe('buildTodoTasks', () => {
  it('maps N audit todos to N tasks; only current unit keeps micro steps', () => {
    const todos = Array.from({ length: 20 }, (_, i) => {
      const ch = i + 1
      if (ch < 3) return { content: `审校第${ch}章（通过）`, status: 'completed' }
      if (ch === 3) return { content: `审校第${ch}章（进行中）`, status: 'in_progress' }
      return { content: `审校第${ch}章（待审）`, status: 'pending' }
    })
    const output = `
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
`
    const { tasks, currentIndex } = buildTodoTasks(todos, output)
    expect(tasks).toHaveLength(20)
    expect(currentIndex).toBe(2)
    expect(tasks[0].steps).toHaveLength(0)
    expect(tasks[1].steps).toHaveLength(0)
    expect(tasks[2].steps.some((s) => s.name === '一致性审计')).toBe(true)
    expect(tasks[2].steps.some((s) => s.status === 'running')).toBe(true)
    expect(tasks[3].steps).toHaveLength(0)
  })

  it('non-chapter todos: tip steps only on in_progress', () => {
    const todos = [
      { content: '规划本批章纲', status: 'completed' },
      { content: '撰写第1章正文', status: 'in_progress' },
      { content: '撰写第2章正文', status: 'pending' },
    ]
    const output = `
▶ writer
  生成中 · 120字
✓ lore_librarian: 已检索
▶ writer
  调用模型中…
`
    const { tasks } = buildTodoTasks(todos, output)
    expect(tasks).toHaveLength(3)
    expect(tasks[0].steps).toHaveLength(0)
    expect(tasks[1].steps.length).toBeGreaterThan(0)
    expect(tasks[1].steps.some((s) => s.status === 'running')).toBe(true)
    expect(tasks[2].steps).toHaveLength(0)
  })
})

describe('segmentStepsByUnit', () => {
  it('buckets steps by 第N章 markers', () => {
    const map = segmentStepsByUnit(`
审阅队列：第1章（1/3）
▶ 一致性审计
✓ 一致性审计: 一致性通过
审阅队列：第2章（2/3）
▶ 节奏审查
  调用模型中…
`)
    expect(map.get('第1章')?.some((s) => s.name === '一致性审计')).toBe(true)
    expect(map.get('第2章')?.some((s) => s.name === '节奏审查')).toBe(true)
  })
})
