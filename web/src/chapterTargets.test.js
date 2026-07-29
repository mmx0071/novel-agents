import { describe, expect, it } from 'vitest'
import {
  CHAPTER_WORD_HARD_MIN,
  CHAPTER_WORD_MAX,
  CHAPTER_WORD_MIN,
  wordProgressLabel,
  wordProgressTone,
} from './chapterTargets.js'

describe('chapterTargets', () => {
  it('mirrors chapter.yaml defaults', () => {
    expect(CHAPTER_WORD_MIN).toBe(5000)
    expect(CHAPTER_WORD_MAX).toBe(6000)
    expect(CHAPTER_WORD_HARD_MIN).toBe(4500)
  })

  it('classifies word progress tones', () => {
    expect(wordProgressTone(0)).toBe('empty')
    expect(wordProgressTone(4000)).toBe('short')
    expect(wordProgressTone(4600)).toBe('soft')
    expect(wordProgressTone(5200)).toBe('ok')
    expect(wordProgressTone(7000)).toBe('long')
  })

  it('labels tones for desk UI', () => {
    expect(wordProgressLabel(0)).toContain('尚未')
    expect(wordProgressLabel(4000)).toContain('硬门')
    expect(wordProgressLabel(5200)).toContain('达标')
  })
})
