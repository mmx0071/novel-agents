/**
 * Continuity board from chapter summary.json (lookback 8). Genre-neutral.
 */

import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { chapterDir, listChapterNumbers } from './deskPreview.js'

const LOOKBACK = 8

function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, 'utf8'))
  } catch {
    return null
  }
}

function factsFromSummary(summary) {
  if (!summary || typeof summary !== 'object') return []
  const bs = summary.body_state && typeof summary.body_state === 'object' ? summary.body_state : {}
  const out = []
  for (const [key, label] of [
    ['injuries', '伤'],
    ['ability_loci', '位点'],
  ]) {
    const raw = bs[key]
    const items = Array.isArray(raw) ? raw : raw ? [raw] : []
    for (const item of items) {
      const t = String(item || '').trim()
      if (t) out.push(`${label}：${t}`)
    }
  }
  return out
}

export function collectBodyStateLines(dir, chapter) {
  const nums = listChapterNumbers(dir).filter((n) => n < chapter).slice(-LOOKBACK)
  const lines = []
  const seen = new Set()
  for (const n of nums) {
    const summary = readJson(join(chapterDir(dir, n), 'summary.json'))
    for (const fact of factsFromSummary(summary)) {
      const sig = `${n}:${fact}`
      if (seen.has(sig)) continue
      seen.add(sig)
      lines.push({ chapter: n, fact })
    }
  }
  return lines
}

export function formatBodyStateBoard(dir, chapter) {
  const lines = collectBodyStateLines(dir, chapter)
  if (!lines.length) return ''
  return lines.map((x) => `- 第${x.chapter}章 ${x.fact}`).join('\n')
}

export function formatBodyStateForCharacter(dir, chapter, selfKeys, allKeys, isDefaultOwner) {
  const self = [...new Set((selfKeys || []).map((s) => String(s || '').trim()).filter(Boolean))]
    .sort((a, b) => [...b].length - [...a].length)
  const all = [...new Set((allKeys || []).map((s) => String(s || '').trim()).filter(Boolean))]
    .sort((a, b) => [...b].length - [...a].length)
  if (!self.length && !isDefaultOwner) return ''
  const owned = []
  for (const { chapter: n, fact } of collectBodyStateLines(dir, chapter)) {
    const mentioned = all.filter((k) => fact.includes(k))
    const mine = mentioned.some((k) => self.includes(k))
    if (mentioned.length ? mine : isDefaultOwner) owned.push({ chapter: n, fact })
  }
  if (!owned.length) return ''
  return owned.map((x) => `- 第${x.chapter}章 ${x.fact}`).join('\n')
}
