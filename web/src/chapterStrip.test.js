import { describe, expect, it } from 'vitest'
import {
  CHAPTER_STRIP_PAGE_SIZE,
  buildChapterStripPage,
  neighborChapter,
  pageIndexForChapter,
  snapChapterNumber,
} from './chapterStrip.js'

describe('buildChapterStripPage', () => {
  it('defaults to 5 chapters per page', () => {
    expect(CHAPTER_STRIP_PAGE_SIZE).toBe(5)
    const chapters = Array.from({ length: 23 }, (_, i) => ({ number: i + 1 }))
    const p0 = buildChapterStripPage(chapters, 1, 0)
    expect(p0.visible).toEqual([1, 2, 3, 4, 5])
    expect(p0.pageCount).toBe(5)
    expect(p0.selectedOnPage).toBe(true)

    const p2 = buildChapterStripPage(chapters, 12, 2)
    expect(p2.visible).toEqual([11, 12, 13, 14, 15])
    expect(p2.page).toBe(2)
  })

  it('marks when selection is off the current page', () => {
    const chapters = Array.from({ length: 20 }, (_, i) => ({ number: i + 1 }))
    const page = buildChapterStripPage(chapters, 18, 0)
    expect(page.visible).toEqual([1, 2, 3, 4, 5])
    expect(page.selectedOnPage).toBe(false)
  })
})

describe('pageIndexForChapter', () => {
  it('maps selection to page', () => {
    const nums = Array.from({ length: 20 }, (_, i) => i + 1)
    expect(pageIndexForChapter(nums, 1)).toBe(0)
    expect(pageIndexForChapter(nums, 5)).toBe(0)
    expect(pageIndexForChapter(nums, 6)).toBe(1)
    expect(pageIndexForChapter(nums, 20)).toBe(3)
  })
})

describe('neighborChapter', () => {
  it('walks prev/next in sorted list', () => {
    expect(neighborChapter([1, 2, 5], 2, -1)).toBe(1)
    expect(neighborChapter([1, 2, 5], 2, 1)).toBe(5)
    expect(neighborChapter([1, 2, 5], 1, -1)).toBe(0)
  })
})

describe('snapChapterNumber', () => {
  it('snaps to nearest existing chapter', () => {
    expect(snapChapterNumber([1, 5, 10], 6)).toBe(5)
    expect(snapChapterNumber([1, 5, 10], 10)).toBe(10)
  })
})
