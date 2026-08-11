import { describe, expect, it } from 'vitest'
import { formatArgsSummary } from './toolMarkup.js'

describe('formatArgsSummary', () => {
  it('uses Chinese labels and chapter phrasing', () => {
    expect(formatArgsSummary({
      project: 'sample-novel',
      chapter_number: 3,
      user_instructions: '加强冲突',
    })).toBe('作品 sample-novel · 第3章 · 要求 加强冲突')
  })

  it('hides noisy internal ids and translates role', () => {
    const s = formatArgsSummary({
      project_id: 'hidden',
      mutation_id: 'm1',
      chapter: 2,
      role: 'literary_editor',
    })
    expect(s).toContain('第2章')
    expect(s).toContain('润色文笔')
    expect(s).not.toContain('project_id')
    expect(s).not.toContain('mutation_id')
    expect(s).not.toContain('hidden')
    expect(s).not.toContain('literary_editor')
  })
})
