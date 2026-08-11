import { describe, expect, it } from 'vitest'
import {
  CHAPTER_WORD_HARD_MIN,
  CHAPTER_WORD_MAX,
  CHAPTER_WORD_MIN,
  SCRIPT_WORD_HARD_MIN,
  SCRIPT_WORD_MAX,
  SCRIPT_WORD_MIN,
  formatUnitTitle,
  wordProgressLabel,
  wordProgressTone,
  wordTargetsForMode,
} from './chapterTargets.js'

describe('chapterTargets', () => {
  it('mirrors chapter.yaml defaults', () => {
    expect(CHAPTER_WORD_MIN).toBe(5000)
    expect(CHAPTER_WORD_MAX).toBe(6000)
    expect(CHAPTER_WORD_HARD_MIN).toBe(4500)
  })

  it('mirrors script.yaml defaults for short_drama', () => {
    expect(SCRIPT_WORD_MIN).toBe(800)
    expect(SCRIPT_WORD_MAX).toBe(3500)
    expect(SCRIPT_WORD_HARD_MIN).toBe(400)
    expect(wordTargetsForMode('short_drama')).toEqual({
      min: 800,
      max: 3500,
      hardMin: 400,
    })
  })

  it('classifies word progress tones', () => {
    expect(wordProgressTone(0)).toBe('empty')
    expect(wordProgressTone(4000)).toBe('short')
    expect(wordProgressTone(4600)).toBe('soft')
    expect(wordProgressTone(5200)).toBe('ok')
    expect(wordProgressTone(7000)).toBe('long')
  })

  it('uses script band for short_drama tones', () => {
    expect(wordProgressTone(3628, 'short_drama')).toBe('long')
    expect(wordProgressTone(300, 'short_drama')).toBe('short')
    expect(wordProgressTone(1000, 'short_drama')).toBe('ok')
  })

  it('labels tones for desk UI', () => {
    expect(wordProgressLabel(0)).toContain('尚未')
    expect(wordProgressLabel(4000)).toContain('未达发布字数')
    expect(wordProgressLabel(5200)).toContain('达标')
  })

  it('formats unit titles without 章/集 duplication', () => {
    expect(formatUnitTitle(6, '第6集 · 章 · 追认', 'short_drama')).toBe('第6集 · 追认')
    expect(formatUnitTitle(6, '章 · 追认', 'short_drama')).toBe('第6集 · 追认')
    expect(formatUnitTitle(3, '开端', 'short_drama')).toBe('第3集 · 开端')
    expect(formatUnitTitle(3, '第3章 · 开端', 'longform')).toBe('第3章 · 开端')
  })
})
