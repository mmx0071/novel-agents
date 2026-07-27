/** Strip model-emitted pseudo tool markup (defense in depth). */
export function stripToolMarkup(text) {
  if (!text) return ''
  let t = String(text)
  t = t.replace(/<function_calls>[\s\S]*?<\/function_calls>/gi, '')
  t = t.replace(/<invoke\s+name=["'][^"']+["'][\s\S]*?<\/invoke>/gi, '')
  t = t.replace(/```(?:xml|tool_call)?\s*<function_calls>[\s\S]*?```/gi, '')
  return t.replace(/\n{3,}/g, '\n\n').trim()
}

/** Best-effort parse of XML invokes for client-side recovery cards. */
export function parseXmlToolCalls(text) {
  if (!text || !text.includes('<invoke')) return []
  const calls = []
  const invokeRe = /<invoke\s+name=["']([^"']+)["']\s*>([\s\S]*?)<\/invoke>/gi
  let m
  let i = 0
  while ((m = invokeRe.exec(text)) !== null) {
    const name = m[1]
    const body = m[2]
    const args = {}
    const paramRe = /<parameter\s+name=["']([^"']+)["']\s*>([\s\S]*?)<\/parameter>/gi
    let p
    while ((p = paramRe.exec(body)) !== null) {
      const key = normalizeArgKey(p[1].trim())
      const raw = p[2].trim()
      const n = Number(raw)
      args[key] = Number.isFinite(n) && String(n) === raw ? n : raw
    }
    calls.push({
      id: `client_xml_${i++}`,
      type: 'tool_call',
      name,
      arguments: args,
      output: null,
      status: 'in_progress',
      duration_ms: null,
      _clientRecovered: true,
    })
  }
  return calls
}

function normalizeArgKey(key) {
  if (['project_id', 'project_name', 'novel', 'name'].includes(key)) return 'project'
  if (['chapter_number', 'chapter_num', 'ch', 'n'].includes(key)) return 'chapter'
  if (['user_instructions', 'instruction', 'msg', 'message'].includes(key)) return 'instructions'
  return key
}

export function formatArgsSummary(args) {
  if (!args) return ''
  const obj = typeof args === 'string' ? safeParse(args) : args
  if (!obj || typeof obj !== 'object') return String(args).slice(0, 120)
  const parts = Object.entries(obj).map(([k, v]) => `${k}=${typeof v === 'string' ? v : JSON.stringify(v)}`)
  return parts.join(' · ').slice(0, 160)
}

/** Tools that dump large context — chat shows a short “正在阅读…” line instead. */
const BULK_CONTEXT_TOOLS = new Set([
  'read_chapter',
  'query_lore',
  'query_memory',
  'list_entities',
  'list_plots',
  'list_expected_events',
  'review_expected_events',
  'get_project_status',
])

export function isBulkContextTool(name) {
  return BULK_CONTEXT_TOOLS.has(name)
}

/** Strip legacy “详见右侧…” suffixes from server/cache lines. */
function stripReaderHint(s) {
  return String(s || '')
    .replace(/[·•]\s*详见右侧阅读区/g, '')
    .replace(/详见右侧阅读区/g, '')
    .trim()
}

/**
 * Structured nav for bulk-read tool cards:
 *   { prefix, links: [{ label, readerTab, chapter? }] }
 * Clickable labels jump the reader panel.
 */
export function getBulkReadNav(name, args, output, projectFallback = '') {
  if (!isBulkContextTool(name)) return null
  const a = typeof args === 'string' ? safeParse(args) || {} : (args || {})
  const out = stripReaderHint(String(output || '').split('\n')[0])
  if (/^⛔|BLOCKER|格式不合规|必填/.test(out.trim())) return null
  const title = a.project || projectFallback || '当前小说'
  const book = `《${title}》`

  if (name === 'read_chapter') {
    const ch = Number(a.chapter)
    const n = Number.isFinite(ch) && ch > 0 ? ch : null
    const chLabel = n ? `第${n}章` : '本章'
    return {
      prefix: `正在阅读${book}`,
      links: [
        { label: `${chLabel}·正文`, readerTab: 'draft', chapter: n || undefined },
        { label: '章纲', readerTab: 'outline', chapter: n || undefined },
      ],
    }
  }
  if (name === 'query_lore') {
    const surface = loreSurfaceFromQuery(a.query)
    const tab = loreReaderTab(a.query)
    const ch = Number(a.chapter)
    const n = Number.isFinite(ch) && ch > 0 ? ch : null
    return {
      prefix: `正在阅读${book}`,
      links: [{ label: surface, readerTab: tab, chapter: n || undefined }],
    }
  }
  if (name === 'query_memory') {
    return { prefix: `正在查阅${book}`, links: [{ label: '记忆摘要', readerTab: 'master' }] }
  }
  if (name === 'list_entities') {
    const kind = a.kind || 'all'
    if (kind === 'character') {
      return { prefix: `正在浏览${book}`, links: [{ label: '人物', readerTab: 'ent:characters' }] }
    }
    if (kind === 'item') {
      return { prefix: `正在浏览${book}`, links: [{ label: '物品', readerTab: 'ent:items' }] }
    }
    if (kind === 'location') {
      return { prefix: `正在浏览${book}`, links: [{ label: '地点', readerTab: 'ent:locations' }] }
    }
    return {
      prefix: `正在浏览${book}`,
      links: [
        { label: '人物', readerTab: 'ent:characters' },
        { label: '物品', readerTab: 'ent:items' },
        { label: '地点', readerTab: 'ent:locations' },
      ],
    }
  }
  if (name === 'list_plots') {
    return { prefix: `正在核对${book}`, links: [{ label: '卷纲·剧情', readerTab: 'arcs' }] }
  }
  if (name === 'list_expected_events' || name === 'review_expected_events') {
    return { prefix: `正在查阅${book}`, links: [{ label: '预处理预期', readerTab: 'expected' }] }
  }
  if (name === 'get_project_status') {
    return { prefix: `正在查看${book}`, links: [{ label: '总纲', readerTab: 'master' }] }
  }
  return { prefix: `正在查阅${book}`, links: [] }
}

/**
 * Compact plain-text label (activity bar / restored turns).
 * Jumpable surfaces are rendered separately in ToolCallCard.
 */
export function formatBulkReadSummary(name, args, output, projectFallback = '') {
  const nav = getBulkReadNav(name, args, output, projectFallback)
  if (!nav) {
    const out = stripReaderHint(String(output || '').split('\n')[0])
    if (/^正在阅读|^正在查阅|^正在浏览|^正在查看/.test(out)) return out.slice(0, 200)
    return null
  }
  const labels = (nav.links || []).map((l) => l.label).join(' / ')
  return labels ? `${nav.prefix} ${labels}` : nav.prefix
}

export function readerTabForBulkTool(name, args) {
  const a = typeof args === 'string' ? safeParse(args) || {} : (args || {})
  if (name === 'read_chapter') return 'draft'
  if (name === 'list_plots') return 'arcs'
  if (name === 'list_expected_events' || name === 'review_expected_events') return 'expected'
  if (name === 'list_entities') {
    if (a.kind === 'character') return 'ent:characters'
    if (a.kind === 'item') return 'ent:items'
    if (a.kind === 'location') return 'ent:locations'
    return 'ent:characters'
  }
  if (name === 'query_lore') return loreReaderTab(a.query)
  return null
}

export function chapterForBulkTool(name, args) {
  if (name !== 'read_chapter' && name !== 'query_lore') return null
  const a = typeof args === 'string' ? safeParse(args) || {} : (args || {})
  const n = Number(a.chapter)
  return Number.isFinite(n) && n > 0 ? n : null
}

function loreSurfaceFromQuery(query) {
  const q = String(query || '').toLowerCase()
  if (/总纲|master/.test(q)) return '总纲'
  if (/卷纲|arc/.test(q)) return '卷纲'
  if (/世界观|bible/.test(q)) return '世界观'
  if (/剧情|plot/.test(q)) return '卷纲·剧情'
  if (/人物|character/.test(q)) return '人物'
  if (/物品|item/.test(q)) return '物品'
  if (/地点|location/.test(q)) return '地点'
  if (/设定|名词/.test(q)) return '设定'
  return '设定与上下文'
}

function loreReaderTab(query) {
  const q = String(query || '').toLowerCase()
  if (/总纲|master/.test(q)) return 'master'
  if (/卷纲|arc/.test(q)) return 'arcs'
  if (/世界观|bible/.test(q)) return 'art:bible'
  if (/剧情|plot/.test(q)) return 'arcs'
  if (/人物|character/.test(q)) return 'ent:characters'
  if (/物品|item/.test(q)) return 'ent:items'
  if (/地点|location/.test(q)) return 'ent:locations'
  return 'master'
}

function safeParse(s) {
  try {
    return JSON.parse(s)
  } catch {
    return null
  }
}
