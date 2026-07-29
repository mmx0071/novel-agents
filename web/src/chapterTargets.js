/**
 * Mirrors config/chapter.yaml word targets for writing-desk UI.
 * Keep in sync when chapter.yaml defaults change (no API surface yet).
 */
export const CHAPTER_WORD_MIN = 5000
export const CHAPTER_WORD_MAX = 6000
export const CHAPTER_WORD_HARD_MIN = 4500

export function wordProgressTone(chars) {
  const n = Number(chars) || 0
  if (n <= 0) return 'empty'
  if (n < CHAPTER_WORD_HARD_MIN) return 'short'
  if (n < CHAPTER_WORD_MIN) return 'soft'
  if (n > CHAPTER_WORD_MAX) return 'long'
  return 'ok'
}

export function wordProgressLabel(chars) {
  const n = Number(chars) || 0
  const tone = wordProgressTone(n)
  if (tone === 'empty') return '尚未落盘'
  if (tone === 'short') return `低于硬门 ${CHAPTER_WORD_HARD_MIN}`
  if (tone === 'soft') return `偏短（目标 ${CHAPTER_WORD_MIN}+）`
  if (tone === 'long') return `超过建议上限 ${CHAPTER_WORD_MAX}`
  return '篇幅达标'
}
