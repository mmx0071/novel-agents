/**
 * Split display markdown into H1 title + H2 sections (reader desks).
 * Does not mutate disk formats — parse only.
 */

export function splitMarkdownByH2(text) {
  const src = String(text || '').replace(/\r\n/g, '\n').trim()
  if (!src) return { title: '', preamble: '', sections: [] }

  const lines = src.split('\n')
  let title = ''
  let i = 0
  if (lines[0]?.startsWith('# ') && !lines[0].startsWith('## ')) {
    title = lines[0].replace(/^#\s+/, '').trim()
    i = 1
    while (i < lines.length && !lines[i].trim()) i += 1
  }

  const preambleLines = []
  const sections = []
  let cur = null

  for (; i < lines.length; i += 1) {
    const line = lines[i]
    const h2 = line.match(/^##\s+(.+)$/)
    if (h2) {
      if (cur) sections.push(cur)
      cur = { heading: h2[1].trim(), body: '' }
      continue
    }
    if (!cur) {
      preambleLines.push(line)
      continue
    }
    cur.body = cur.body ? `${cur.body}\n${line}` : line
  }
  if (cur) sections.push(cur)

  return {
    title,
    preamble: preambleLines.join('\n').trim(),
    sections: sections.map((s) => ({
      heading: s.heading,
      body: String(s.body || '').trim(),
    })),
  }
}

/** Pick first section whose heading matches any alias (exact, then includes). */
export function pickSection(sections, aliases = []) {
  const list = Array.isArray(sections) ? sections : []
  const norms = aliases.map((a) => String(a || '').trim().toLowerCase()).filter(Boolean)
  if (!norms.length) return null
  for (const s of list) {
    const h = String(s.heading || '').trim().toLowerCase()
    if (norms.some((a) => h === a)) return s
  }
  for (const s of list) {
    const h = String(s.heading || '').trim().toLowerCase()
    if (norms.some((a) => h.includes(a) || a.includes(h))) return s
  }
  return null
}

export function sectionBody(sections, aliases, fallback = '') {
  const s = pickSection(sections, aliases)
  return s?.body || fallback
}

/** Bullet / numbered lines → string[]. */
export function listItemsFromBody(body) {
  const text = String(body || '').trim()
  if (!text) return []
  const items = []
  for (const line of text.split('\n')) {
    const t = line.trim()
    if (!t) continue
    const m = t.match(/^(?:[-*+]|\d+[.)、])\s+(.+)$/)
    if (m) {
      const v = m[1].trim()
      if (v && v !== '（无）' && v !== '（未标注）') items.push(v)
    } else if (!t.startsWith('#') && t !== '（无）' && t !== '（未标注）') {
      items.push(t)
    }
  }
  return items
}

function metaLine(preamble, label) {
  const re = new RegExp(`^${label}[：:]\\s*(.+)$`, 'm')
  const m = String(preamble || '').match(re)
  return m ? m[1].trim() : ''
}

/**
 * Parse chapter-outline display markdown from display_chapter_outline.
 * Falls back to null fields when structure is unexpected.
 */
export function parseChapterOutlineDisplay(text) {
  const { title, preamble, sections } = splitMarkdownByH2(text)
  if (!title && !sections.length && !preamble) {
    return null
  }
  const goal = sectionBody(sections, ['目标'])
  const conflict = sectionBody(sections, ['冲突'])
  const emotion = sectionBody(sections, ['情绪曲线', '情绪'])
  const includes = listItemsFromBody(sectionBody(sections, ['本章纳入', '纳入']))
  const defers = listItemsFromBody(sectionBody(sections, ['顺延后章', '顺延']))
  const events = listItemsFromBody(sectionBody(sections, ['关键事件']))
  const characters = listItemsFromBody(sectionBody(sections, ['出场人物', '人物']))
  const items = listItemsFromBody(sectionBody(sections, ['本章物品', '物品']))
  const locations = listItemsFromBody(sectionBody(sections, ['本章地点', '地点']))
  const tags = listItemsFromBody(sectionBody(sections, ['场景标签', '标签']))
  const cliffhanger = sectionBody(sections, ['章末钩子', '钩子'])
  const lore = listItemsFromBody(sectionBody(sections, ['lore 查询', 'Lore 查询', 'Lore']))

  const used = new Set(
    ['目标', '冲突', '情绪曲线', '情绪', '本章纳入', '纳入', '顺延后章', '顺延',
      '关键事件', '出场人物', '人物', '本章物品', '物品', '本章地点', '地点',
      '场景标签', '标签', '章末钩子', '钩子', 'lore 查询', 'Lore 查询', 'Lore']
      .map((x) => x.toLowerCase()),
  )
  const extra = sections.filter((s) => {
    const h = String(s.heading || '').trim().toLowerCase()
    return !used.has(h) && !Array.from(used).some((u) => h.includes(u))
  })

  return {
    title,
    pov: metaLine(preamble, '视角'),
    timeLocation: metaLine(preamble, '时空'),
    goal,
    conflict,
    emotion,
    includes,
    defers,
    events,
    characters,
    items,
    locations,
    tags,
    cliffhanger,
    lore,
    extra,
    raw: text,
  }
}

/** Stable 0..1 hash from string (layout seeds). */
export function stableUnit(str) {
  const s = String(str || '')
  let h = 2166136261
  for (let i = 0; i < s.length; i += 1) {
    h ^= s.charCodeAt(i)
    h = Math.imul(h, 16777619)
  }
  return (h >>> 0) / 4294967296
}

/** Parse ### 子区 blocks under overview for map child nodes. */
export function parseLocationSubzones(overviewBody) {
  const text = String(overviewBody || '').replace(/\r\n/g, '\n')
  if (!text.trim()) return []
  const lines = text.split('\n')
  const out = []
  let cur = null
  for (const line of lines) {
    const m = line.match(/^###\s+(.+)$/)
    if (m) {
      if (cur) out.push(cur)
      cur = { name: m[1].trim(), body: '' }
      continue
    }
    if (cur) cur.body = cur.body ? `${cur.body}\n${line}` : line
  }
  if (cur) out.push(cur)
  return out
    .map((z) => ({ name: z.name, body: z.body.trim() }))
    .filter((z) => z.name)
}
