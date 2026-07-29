import { describe, expect, it } from 'vitest'
import { collectAuditState, isReviseAppliedItem, parseAuditChecklist } from './auditParse.js'

const SAMPLE_BRIEF = `## 第17章审校未通过

阻断 **2** · 建议 **1** · 共 3 条（全文见审校工具卡）

### 问题清单
1. \`p0-meta-1\` [P0/META] 正文出现元叙述「第17章」（约第3行）
2. \`p0-timeline-2\` [P0/TIMELINE] 时段回跳：先夜晚后凌晨（约第9行）
3. \`p1-style-3\` [P1/STYLE] 句式重复偏多
`

describe('parseAuditChecklist', () => {
  it('returns null for unrelated text', () => {
    expect(parseAuditChecklist('继续创作第3章')).toBeNull()
  })

  it('parses issue lines with severity type message and location', () => {
    const parsed = parseAuditChecklist(SAMPLE_BRIEF)
    expect(parsed).not.toBeNull()
    expect(parsed.chapter).toBe(17)
    expect(parsed.failed).toBe(true)
    expect(parsed.passed).toBe(false)
    expect(parsed.issues).toHaveLength(3)
    expect(parsed.issues[0]).toMatchObject({
      id: 'p0-meta-1',
      severity: 'P0',
      type: 'META',
      location: '约第3行',
    })
    expect(parsed.issues[0].message).toContain('元叙述')
    expect(parsed.issues[2].severity).toBe('P1')
  })

  it('detects passed reports without failing issues', () => {
    const parsed = parseAuditChecklist('## 第3章审校通过\n\n无阻断问题。')
    expect(parsed.chapter).toBe(3)
    expect(parsed.passed).toBe(true)
    expect(parsed.failed).toBe(false)
    expect(parsed.issues).toEqual([])
  })
})

describe('collectAuditState', () => {
  it('collects latest checklist from agent messages and todos', () => {
    const state = collectAuditState(
      [
        {
          id: 't1',
          items: [{ type: 'agent_message', text: SAMPLE_BRIEF }],
          approval: {
            prompt: '请选择',
            options: [{ id: '1', label: '修正本章' }],
          },
        },
      ],
      [{ content: '审校第17章（未通过 · 待处理）', status: 'in_progress' }],
    )
    expect(state.latest.chapter).toBe(17)
    expect(state.latest.issues).toHaveLength(3)
    expect(state.todos).toHaveLength(1)
    expect(state.openApproval.options[0].label).toBe('修正本章')
  })

  it('keeps one report per chapter (latest wins)', () => {
    const state = collectAuditState([
      {
        id: 't1',
        items: [{
          type: 'agent_message',
          text: '## 第2章审校未通过\n\n### 问题清单\n1. `p0-a` [P0/META] 旧问题',
        }],
      },
      {
        id: 't2',
        items: [{
          type: 'audit_report',
          chapter: 2,
          passed: false,
          report: '## 第2章审校未通过\n\n### 问题清单\n1. `p0-b` [P0/META] 新问题',
        }],
      },
    ], [])
    expect(state.reports).toHaveLength(1)
    expect(state.reports[0].issues[0].id).toBe('p0-b')
  })

  it('handles empty turns', () => {
    const state = collectAuditState([], [])
    expect(state.latest).toBeNull()
    expect(state.reports).toEqual([])
    expect(state.openApproval).toBeNull()
    expect(state.reviseAppliedAfterFail).toBe(false)
  })

  it('marks reviseAppliedAfterFail when apply follows a failed checklist', () => {
    const state = collectAuditState([
      {
        id: 't1',
        items: [{ type: 'agent_message', text: SAMPLE_BRIEF }],
      },
      {
        id: 't2',
        items: [{
          type: 'tool_call',
          name: 'revise_chapter',
          status: 'completed',
          args: 'apply=true · chapter=17',
          output: '已应用第17章局部修订（3 处）。',
        }],
      },
    ], [])
    expect(state.reviseAppliedAfterFail).toBe(true)
    expect(state.reviseChapter).toBe(17)
    expect(isReviseAppliedItem({
      type: 'tool_call',
      name: 'revise_chapter',
      status: 'completed',
      output: '已应用第17章局部修订（3 处）。',
    })).toBe(true)
  })
})
