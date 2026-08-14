/**
 * Reader-only display transforms. Do not write these back to disk.
 */

export function normalizeBlankLines(text) {
  const lines = []
  let blanks = 0
  for (const line of String(text || '').replace(/\r\n/g, '\n').split('\n')) {
    const trimmed = line.trimEnd()
    if (!trimmed) {
      blanks += 1
      if (blanks <= 2) lines.push('')
    } else {
      blanks = 0
      lines.push(trimmed)
    }
  }
  let s = lines.join('\n')
  if (s && !s.endsWith('\n')) s += '\n'
  return s
}

export function joinFm(meta, body) {
  const keys = Object.keys(meta || {}).sort()
  let yaml = '---\n'
  for (const k of keys) {
    const v = String(meta[k] ?? '')
    const needsQuote = v.includes(':') || v.includes('#') || v.includes('\n')
    yaml += needsQuote ? `${k}: "${v.replace(/"/g, '\\"')}"\n` : `${k}: ${v}\n`
  }
  yaml += '---\n\n'
  yaml += String(body || '').replace(/^\n+/, '')
  if (yaml && !yaml.endsWith('\n')) yaml += '\n'
  return yaml
}

export function splitFm(text) {
  const raw = String(text || '').replace(/^\uFEFF/, '')
  if (!raw.startsWith('---')) return { meta: {}, body: raw }
  const rest = raw.slice(3).replace(/^\n/, '')
  const end = rest.indexOf('\n---')
  if (end < 0) return { meta: {}, body: raw }
  const yaml = rest.slice(0, end)
  const body = rest.slice(end + 4).replace(/^\n/, '')
  const meta = {}
  for (const line of yaml.split('\n')) {
    const t = line.trim()
    if (!t || t.startsWith('#')) continue
    const i = t.indexOf(':')
    if (i < 0) continue
    const key = t.slice(0, i).trim()
    const val = t.slice(i + 1).trim().replace(/^['"]|['"]$/g, '')
    if (key) meta[key] = val
  }
  return { meta, body }
}

function normalizeHeading(s) {
  return String(s || '')
    .trim()
    .replace(/^[*_]+/, '')
    .replace(/[*_]+$/, '')
    .trim()
}

export function headingEq(got, want) {
  const g = normalizeHeading(got)
  const w = normalizeHeading(want)
  if (g === w) return true
  if (g.toLowerCase() === w.toLowerCase()) return true
  return g.startsWith(`${w}（`) || g.startsWith(`${w}(`) || g.startsWith(`${w} ·`) || g.startsWith(`${w}·`)
}

export function hasH2(text, aliases = []) {
  const titles = String(text || '')
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.startsWith('## ') && !l.startsWith('### '))
    .map((l) => l.slice(3))
  return aliases.some((a) => titles.some((t) => headingEq(t, a)))
}

/** @param {Array<[string, string[]]>} map */
export function rewriteH2Aliases(text, map) {
  const lines = String(text || '').replace(/\r\n/g, '\n').split('\n')
  const out = []
  for (const line of lines) {
    const t = line.trim()
    if (t.startsWith('## ') && !t.startsWith('### ')) {
      const rest = t.slice(3)
      let replaced = false
      for (const [canon, aliases] of map) {
        if (aliases.some((a) => headingEq(rest, a))) {
          out.push(`## ${canon}`)
          replaced = true
          break
        }
      }
      if (!replaced) out.push(line)
    } else {
      out.push(line)
    }
  }
  return normalizeBlankLines(out.join('\n'))
}

export const ENTITY_DISPLAY_ZH = {
  characters: [
    ['经历', ['History', '经历', '历史']],
    ['性格', ['Personality', '性格']],
    ['核心事件', ['Core events', 'Core Events', '核心事件']],
    ['当前状态', ['Current status', 'Current Status', '当前状态', '现状']],
  ],
  items: [
    ['来源', ['Origin', '产出地', '来源']],
    ['用途', ['Usage', '用途']],
    ['当前状态', ['Current status', 'Current Status', '当前状态', '现状']],
  ],
  locations: [
    ['概述', ['Overview', '概述']],
    ['此地势力', ['Factions', '此地势力', '势力']],
    ['产出资源', ['Production', '产出资源', '产出']],
  ],
}

export const PLOT_DISPLAY_H2 = [
  ['概览', ['概览']],
  ['剧情走向', ['剧情走向', '剧情走向（叙事节点，无章号）']],
  ['冲突与赌注', ['冲突与赌注', '冲突/赌注', '赌注']],
  ['出场人物', ['出场人物']],
  ['收束条件', ['收束条件', '落点条件', 'exit_condition']],
]

export const MASTER_DISPLAY_H2 = [
  ['一句话卖点', ['一句话卖点', 'Logline', 'logline', '卖点']],
  ['三幕结构', ['三幕结构', '分卷', '分卷结构', '三幕结构（映射为多卷）']],
  ['主角弧', ['主角弧', '主角弧光', '人物弧（主角）']],
  ['主线冲突', ['主线冲突', '核心冲突', '主冲突']],
]

export function displayEntityCard(group, text) {
  const { body } = splitFm(text)
  const map = ENTITY_DISPLAY_ZH[group] || ENTITY_DISPLAY_ZH.characters
  return rewriteH2Aliases(body.trim(), map)
}

export function displayPlotCardBody(text) {
  const { body } = splitFm(text)
  return rewriteH2Aliases(body.trim(), PLOT_DISPLAY_H2)
}

export function displayBible(text) {
  return normalizeBlankLines(String(text || '').trim())
}

export function displayMasterOutline(text) {
  return rewriteH2Aliases(String(text || '').trim(), MASTER_DISPLAY_H2)
}

export function displayArcOutline(text) {
  return normalizeBlankLines(String(text || '').trim())
}

export function displayDraft(text) {
  return normalizeBlankLines(String(text || '').trim())
}

export function draftBodyChars(text) {
  const trimmed = String(text || '').trim()
  if (!trimmed) return 0
  const lines = trimmed.split('\n')
  lines.shift()
  return lines.join('\n').trim().length
}

export function displayEntityGaps(gaps) {
  if (!gaps.length) return '设定卡与世界观暂无明显缺口。'
  return ['（软提示·不阻断写章）待补全：', '', ...gaps.map((g) => `- ${g}`)].join('\n')
}

function asList(v) {
  if (Array.isArray(v)) return v.map((x) => String(x || '').trim()).filter(Boolean)
  if (typeof v === 'string' && v.trim()) return [v.trim()]
  return []
}

export function displayChapterOutline(o) {
  const title = String(o?.title || '').trim() || '章纲'
  const list = (arr, empty) => {
    const items = asList(arr)
    return items.length ? items.map((e) => `- ${e}`) : [`- ${empty}`]
  }
  const events = asList(o?.key_events)
  const eventLines = events.length
    ? events.map((e, i) => `${i + 1}. ${e}`)
    : ['- （未标注）']
  return [
    `# ${title}`,
    '',
    `视角：${o?.pov || ''}`,
    `时空：${o?.time_location || ''}`,
    '',
    '## 目标',
    o?.goal || '',
    '',
    '## 冲突',
    o?.conflict || '',
    '',
    '## 情绪曲线',
    o?.emotion_curve || '',
    '',
    '## 本章纳入',
    ...list(o?.plot_includes, '（未标注）'),
    '',
    '## 顺延后章',
    ...list(o?.plot_defers, '（未标注）'),
    '',
    '## 关键事件',
    ...eventLines,
    '',
    '## 出场人物',
    ...list(o?.characters, '（无）'),
    '',
    '## 本章物品',
    ...list(o?.items, '（无）'),
    '',
    '## 本章地点',
    ...list(o?.locations, '（无）'),
    '',
    '## 场景标签',
    ...list(o?.scene_tags, '（无）'),
    '',
    '## 章末钩子',
    o?.cliffhanger || '',
    '',
  ].join('\n')
}

export function parseChapterOutline(text) {
  const raw = String(text || '').trim()
  if (!raw) return null
  const unfenced = raw.startsWith('```')
    ? raw.replace(/^```(?:json)?\s*/i, '').replace(/\s*```$/, '')
    : raw
  try {
    const v = JSON.parse(unfenced)
    if (v && typeof v === 'object' && !Array.isArray(v)) return v
  } catch {
    return null
  }
  return null
}
