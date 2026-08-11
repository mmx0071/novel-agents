import { describe, expect, it } from 'vitest'
import {
  isKvLine,
  looksLikeAuditQueueLaunchProse,
  orderTurnItemsForDisplay,
  prepareChatAgentProse,
  stripPlanningPreamble,
  structureStatusProse,
} from './chatProse'

describe('chatProse', () => {
  it('detects audit-queue launch / todo-mirror prose', () => {
    expect(looksLikeAuditQueueLaunchProse(
      '项目共 20 章已发布。现在启动全部章节逐章审阅队列（第 1–20 章）。\n\n审阅队列 To-dos：\n- 审阅第 1 章\n- 审阅第 2 章',
    )).toBe(true)
    expect(looksLikeAuditQueueLaunchProse('复审通过，可继续创作。')).toBe(false)
  })

  it('detects kv lines', () => {
    expect(isKvLine('题材：末世 · 旧土')).toBe(true)
    expect(isKvLine('进度: 已发布 20 章')).toBe(true)
    expect(isKvLine('这不是键值对，只是一句说明。')).toBe(false)
    expect(isKvLine('## 标题')).toBe(false)
  })

  it('strips planning preamble before heading', () => {
    const raw = '用户要求"审视"——按规则第5条：调用 list_plots。可以并行。\n\n## 《样例》审视\n\n题材：末世'
    expect(stripPlanningPreamble(raw)).toMatch(/^## 《样例》审视/)
  })

  it('strips planning glued to heading', () => {
    const raw = '所以我要调用 list_plots。这两个是独立调用，可以并行。## 《样例》审视\n\n进度：第21章'
    const out = prepareChatAgentProse(raw)
    expect(out.startsWith('## 《样例》审视')).toBe(true)
    expect(out).not.toMatch(/所以我要调用/)
    expect(out).toMatch(/\| 进度 \|/)
  })

  it('structures section + kv into table', () => {
    const raw = [
      '## 审视',
      '',
      '项目状态',
      '',
      '题材：末世',
      '进度：已发布 20 章',
      '',
      '可选下一步',
      '',
      '- 继续创作',
    ].join('\n')
    const out = structureStatusProse(raw)
    expect(out).toContain('### 项目状态')
    expect(out).toContain('| 题材 | 末世 |')
    expect(out).toContain('- 继续创作')
  })

  it('strips duplicated reasoning prefix', () => {
    const reasoning = '用户要求审视。按规则第5条调用 list_plots。'
    const raw = `${reasoning}\n\n## 报告\n\n题材：x`
    expect(stripPlanningPreamble(raw, [reasoning])).toMatch(/^## 报告/)
  })

  it('moves trailing reasoning before the last agent message', () => {
    const ordered = orderTurnItemsForDisplay([
      { type: 'reasoning', id: 'r1', text: '先规划' },
      { type: 'tool_call', id: 't1', name: 'list_plots', status: 'completed' },
      { type: 'agent_message', id: 'a1', text: '## 报告\n进度：ok' },
      { type: 'reasoning', id: 'r2', text: '整理汇报内容' },
    ])
    expect(ordered.map((x) => x.id)).toEqual(['r1', 't1', 'r2', 'a1'])
  })

  it('cleans a realistic status report bubble', () => {
    const raw = [
      '用户要求"审视不周山再临"——按规则第5条：调用 list_plots。可以并行。## 《不周山再临》审视',
      '',
      '项目状态',
      '',
      '题材：末世 · 旧土 · 神话',
      '进度：已发布 20 章，下一章第 21 章',
      '',
      '当前主线卡',
      '',
      '第 1 卷「帛书咬合」（进行中）',
      '收束条件：模型通过实测验证',
      '收束后下一卡：天虞山征召',
      '',
      '可选下一步',
      '',
      '- 继续创作：写第 21 章',
    ].join('\n')
    const out = prepareChatAgentProse(raw)
    expect(out).not.toMatch(/按规则第/)
    expect(out).toContain('## 《不周山再临》审视')
    expect(out).toContain('### 项目状态')
    expect(out).toContain('### 当前主线卡')
    expect(out).toContain('| 题材 |')
    expect(out).toContain('| 收束条件 |')
  })
})
