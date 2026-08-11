/**
 * Word targets for writing-desk UI.
 * Longform mirrors config/chapter.yaml; short_drama mirrors config/script.yaml.
 * Keep in sync when those YAML defaults change (no API surface yet).
 */

export const CHAPTER_WORD_MIN = 5000
export const CHAPTER_WORD_MAX = 6000
export const CHAPTER_WORD_HARD_MIN = 4500

export const SCRIPT_WORD_MIN = 800
export const SCRIPT_WORD_MAX = 3500
export const SCRIPT_WORD_HARD_MIN = 400

/** @returns {{ min: number, max: number, hardMin: number }} */
export function wordTargetsForMode(projectMode) {
  if (projectMode === 'short_drama') {
    return {
      min: SCRIPT_WORD_MIN,
      max: SCRIPT_WORD_MAX,
      hardMin: SCRIPT_WORD_HARD_MIN,
    }
  }
  return {
    min: CHAPTER_WORD_MIN,
    max: CHAPTER_WORD_MAX,
    hardMin: CHAPTER_WORD_HARD_MIN,
  }
}

export function wordProgressTone(chars, projectMode) {
  const { min, max, hardMin } = wordTargetsForMode(projectMode)
  const n = Number(chars) || 0
  if (n <= 0) return 'empty'
  if (n < hardMin) return 'short'
  if (n < min) return 'soft'
  if (n > max) return 'long'
  return 'ok'
}

export function wordProgressLabel(chars, projectMode) {
  const { min, max, hardMin } = wordTargetsForMode(projectMode)
  const n = Number(chars) || 0
  const tone = wordProgressTone(n, projectMode)
  if (tone === 'empty') return '尚未写入'
  if (tone === 'short') return `未达发布字数（至少 ${hardMin}）`
  if (tone === 'soft') return `偏短（目标 ${min}+）`
  if (tone === 'long') return `超过建议上限 ${max}`
  return '篇幅达标'
}

/**
 * Author-facing unit title without 章/集 duplication.
 * e.g. short_drama + title "第6集 · 章 · 追认" → "第6集 · 追认"
 */
export function formatUnitTitle(number, rawTitle, projectMode) {
  const n = Number(number) || 0
  const unit = projectMode === 'short_drama' ? '集' : '章'
  const head = n > 0 ? `第${n}${unit}` : ''
  let title = String(rawTitle || '').trim()
  title = title.replace(/^#\s*/, '')
  // Drop leading 「第N章/集」 (same or any number).
  title = title.replace(/^第\s*\d+\s*[章节集]\s*/u, '')
  title = title.replace(/^[·•\-—|｜]\s*/u, '')
  if (projectMode === 'short_drama') {
    // script.yaml strip_title_noise: standalone 「章」
    title = title.replace(/(?:^|[·•\-—|｜\s]+)章(?=[·•\-—|｜\s]|$)/gu, ' · ')
  }
  title = title
    .replace(/[·•\-—|｜]\s*[·•\-—|｜]/gu, ' · ')
    .replace(/^[·•\-—|｜\s]+|[·•\-—|｜\s]+$/gu, '')
    .replace(/\s{2,}/g, ' ')
    .trim()
  if (!head) return title
  return title ? `${head} · ${title}` : head
}
