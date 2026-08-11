import { describe, expect, it } from 'vitest'
import { looksLikeToolGateDone } from './toolGateDone.js'

describe('looksLikeToolGateDone', () => {
  it('does not treat mid-queue progress as done', () => {
    expect(looksLikeToolGateDone(
      '\n——\n审阅队列：第1章（1/20）\n## 审阅队列《样例》（1/20）\n当前：第1章\n',
    )).toBe(false)
    expect(looksLikeToolGateDone(
      '▶ 一致性审计\n  调用模型中…\n✓ 节奏审查: 节奏审查完成\n',
    )).toBe(false)
  })

  it('treats real gate / finish codas as done', () => {
    expect(looksLikeToolGateDone(
      '\n——\n第1章一致性未通过（详见上方流式输出）\n\n## 审阅队列《样例》（1/20）\n\n（审校未通过 — 请在下方决策卡选择）',
    )).toBe(true)
    expect(looksLikeToolGateDone('⏸ 第1章未通过（2 条阻断），请按问题选择处理项')).toBe(true)
    expect(looksLikeToolGateDone('审阅结束：通过 18 · 未通过 0 · 已修订 2')).toBe(true)
    expect(looksLikeToolGateDone('审阅队列已全部完成。')).toBe(true)
  })

  it('does not mark done while progressive todos are still open', () => {
    expect(looksLikeToolGateDone('（审校未通过 — 请在下方决策卡选择）', { todosOpen: true }))
      .toBe(false)
    expect(looksLikeToolGateDone('审阅队列已全部完成。', { todosOpen: true })).toBe(false)
  })
})
