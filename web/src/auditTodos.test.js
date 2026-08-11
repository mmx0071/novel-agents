import { describe, expect, it } from 'vitest'
import { resolveWorkTodos, synthesizeAuditTodos } from './auditTodos.js'

describe('synthesizeAuditTodos', () => {
  it('builds window from args + latest chapter in output', () => {
    const todos = synthesizeAuditTodos({
      name: 'audit_chapters',
      arguments: { action: 'start', from: 1, to: 20, project: '样例' },
      output: `
审阅队列：第1章（1/20）
✓ 第1章 · 通过 — 一致性通过
审阅队列：第2章（2/20）
✓ 第2章 · 通过 — 一致性通过
审阅队列：第3章（3/20）
▶ 第3章 · 待审
▶ 一致性审计
  调用模型中…
`,
    })
    expect(todos).toHaveLength(20)
    expect(todos[0]).toEqual({ content: '审校第1章（通过）', status: 'completed' })
    expect(todos[1]).toEqual({ content: '审校第2章（通过）', status: 'completed' })
    expect(todos[2]).toEqual({ content: '审校第3章（进行中）', status: 'in_progress' })
    expect(todos[3].status).toBe('pending')
  })

  it('marks failed current chapter', () => {
    const todos = synthesizeAuditTodos({
      name: 'audit_chapters',
      arguments: { from: 1, to: 5 },
      output: '审阅队列：第3章（3/5）\n第3章未通过（1 条阻断）\n',
    })
    expect(todos[2]).toEqual({
      content: '审校第3章（未通过 · 待处理）',
      status: 'in_progress',
    })
  })
})

describe('resolveWorkTodos', () => {
  it('prefers wire todos over synthesis', () => {
    const wire = [{ content: '审校第1章（通过）', status: 'completed' }]
    expect(resolveWorkTodos(wire, [{
      type: 'tool_call',
      name: 'audit_chapters',
      arguments: { from: 1, to: 3 },
      output: '',
    }])).toBe(wire)
  })

  it('synthesizes when wire empty', () => {
    const got = resolveWorkTodos([], [{
      type: 'tool_call',
      name: 'audit_chapters',
      arguments: { from: 1, to: 3 },
      output: '审阅队列：第2章（2/3）\n▶ 第2章 · 待审\n',
    }])
    expect(got).toHaveLength(3)
    expect(got[1].status).toBe('in_progress')
  })
})
