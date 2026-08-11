import { describe, expect, it } from 'vitest'
import {
  mutationKindZh,
  skillLabelZh,
  skillPromptForAuthor,
  stepLabelZh,
  toolLabelZh,
  toolVerbZh,
} from './toolLabels.js'

describe('toolLabelZh', () => {
  it('maps common tools to consumer Chinese', () => {
    expect(toolLabelZh('continue_writing')).toBe('继续创作')
    expect(toolLabelZh('continue_writing_batch')).toBe('批量续写')
    expect(toolLabelZh('apply_draft_patch')).toBe('局部改稿')
    expect(toolLabelZh('steer_run')).toBe('按你的选择继续')
    expect(toolLabelZh('consistency_auditor')).toBe('核对前后文')
    expect(toolLabelZh('replan_volume')).toBe('重排本卷计划')
    expect(toolLabelZh('split_chapter')).toBe('拆成两章')
    expect(toolLabelZh('research_materials')).toBe('搜集素材')
  })

  it('never leaks unknown snake_case ids', () => {
    expect(toolLabelZh('totally_unknown_tool')).toBe('后台步骤')
    expect(stepLabelZh('foo_bar')).toBe('后台步骤')
  })

  it('keeps already-Chinese labels', () => {
    expect(toolLabelZh('核对前后文')).toBe('核对前后文')
    expect(stepLabelZh('规划章纲')).toBe('规划章纲')
  })
})

describe('skillLabelZh / mutationKindZh / toolVerbZh', () => {
  it('gives each skill a distinct Chinese title', () => {
    expect(skillLabelZh('studio')).toBe('创作助手总则')
    expect(skillLabelZh('$novel-draft')).toBe('正文写作要点')
    expect(skillLabelZh('volume-lifecycle')).toBe('卷生命周期')
    expect(skillLabelZh('writer')).toBe('撰写正文')
    expect(skillLabelZh('consistency-auditor')).toBe('核对前后文')
    expect(skillLabelZh('literary-editor')).toBe('润色文笔')
    expect(skillLabelZh('unknown-skill', '追踪某某状态，输出 JSON')).toBe('追踪某某状态，输出 JSON')
    expect(skillLabelZh('weird_skill_id')).toBe('weird skill id')
    expect(skillPromptForAuthor('writer')).toBe('请撰写或续写正文')
    expect(skillPromptForAuthor('unknown-x')).toContain('请参考')
    expect(skillPromptForAuthor('unknown-x')).not.toContain('$')
    expect(mutationKindZh('mutation')).toBe('拟修改设定')
    expect(mutationKindZh('unknown_kind')).toBe('拟修改内容')
    expect(toolVerbZh('in_progress')).toBe('进行中')
    expect(toolVerbZh('completed')).toBe('已完成')
  })
})
