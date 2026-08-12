import { describe, expect, it } from 'vitest'
import { textHunkDiffs } from './textHunkDiffs'

describe('textHunkDiffs', () => {
  it('highlights changed middle line', () => {
    const diffs = textHunkDiffs('a\nb\nold line\nc\n', 'a\nb\nnew line with 如曼德拉去世\nc\n')
    expect(diffs).toHaveLength(1)
    expect(diffs[0].before).toContain('old line')
    expect(diffs[0].after).toContain('如曼德拉去世')
    expect(diffs[0].before).not.toContain('如曼德拉去世')
  })

  it('treats empty before as pure addition', () => {
    const diffs = textHunkDiffs('', '# 世界观\n\n## 0. x\n')
    expect(diffs).toHaveLength(1)
    expect(diffs[0].before).toBe('')
    expect(diffs[0].after).toContain('世界观')
  })

  it('returns empty when unchanged', () => {
    expect(textHunkDiffs('same\n', 'same\n')).toEqual([])
  })
})
