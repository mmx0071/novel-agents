/**
 * Paginated chapter / episode strip for the writing desk.
 * Default: 5 chapters per page + chapter nav + page nav + jump.
 */

export const CHAPTER_STRIP_PAGE_SIZE = 5

/** Sorted unique chapter numbers from preview list. */
export function chapterNumbers(chapters) {
  return [...new Set(
    (Array.isArray(chapters) ? chapters : [])
      .map((c) => (typeof c === 'number' ? c : Number(c?.number)))
      .filter((n) => Number.isFinite(n) && n > 0),
  )].sort((a, b) => a - b)
}

/**
 * @returns {{
 *   numbers: number[],
 *   total: number,
 *   pageSize: number,
 *   pageCount: number,
 *   page: number,
 *   visible: number[],
 *   selected: number,
 *   selectedOnPage: boolean,
 * }}
 */
export function buildChapterStripPage(chapters, selected, pageIndex, opts = {}) {
  const pageSize = Math.max(1, opts.pageSize ?? CHAPTER_STRIP_PAGE_SIZE)
  const numbers = chapterNumbers(chapters)
  const total = numbers.length
  if (!total) {
    return {
      numbers: [],
      total: 0,
      pageSize,
      pageCount: 1,
      page: 0,
      visible: [],
      selected: 0,
      selectedOnPage: false,
    }
  }

  const selectedN = numbers.includes(Number(selected))
    ? Number(selected)
    : numbers[numbers.length - 1]
  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  const page = Math.max(0, Math.min(pageCount - 1, Number(pageIndex) || 0))
  const start = page * pageSize
  const visible = numbers.slice(start, start + pageSize)

  return {
    numbers,
    total,
    pageSize,
    pageCount,
    page,
    visible,
    selected: selectedN,
    selectedOnPage: visible.includes(selectedN),
  }
}

export function pageIndexForChapter(numbers, selected, pageSize = CHAPTER_STRIP_PAGE_SIZE) {
  const nums = Array.isArray(numbers) ? numbers : []
  const size = Math.max(1, pageSize)
  if (!nums.length) return 0
  const idx = nums.indexOf(Number(selected))
  if (idx < 0) return 0
  return Math.floor(idx / size)
}

/** Neighbor chapter number in sorted list, or 0 if none. */
export function neighborChapter(numbers, selected, delta) {
  const nums = Array.isArray(numbers) ? numbers : []
  if (!nums.length) return 0
  const sel = Number(selected)
  let idx = nums.indexOf(sel)
  if (idx < 0) {
    if (delta < 0) {
      for (let i = nums.length - 1; i >= 0; i--) {
        if (nums[i] < sel) return nums[i]
      }
      return 0
    }
    for (let i = 0; i < nums.length; i++) {
      if (nums[i] > sel) return nums[i]
    }
    return 0
  }
  const next = idx + delta
  if (next < 0 || next >= nums.length) return 0
  return nums[next]
}

/** Snap typed number to nearest existing chapter. */
export function snapChapterNumber(numbers, raw) {
  const nums = Array.isArray(numbers) ? numbers : []
  const n = Number(raw)
  if (!nums.length || !Number.isFinite(n) || n <= 0) return 0
  if (nums.includes(n)) return n
  let best = nums[0]
  let bestDist = Math.abs(best - n)
  for (const x of nums) {
    const d = Math.abs(x - n)
    if (d < bestDist) {
      best = x
      bestDist = d
    }
  }
  return best
}
