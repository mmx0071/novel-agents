/**
 * Assemble reading-desk preview JSON from projects/<name>/ on disk.
 * Genre-neutral: no baked-in work titles.
 */

import { readdirSync, readFileSync, existsSync, statSync } from 'node:fs'
import { join } from 'node:path'
import {
  formatBodyStateBoard,
  formatBodyStateForCharacter,
} from './deskBodyState.js'
import {
  displayArcOutline,
  displayBible,
  displayChapterOutline,
  displayDraft,
  displayEntityCard,
  displayEntityGaps,
  displayMasterOutline,
  displayPlotCardBody,
  draftBodyChars,
  parseChapterOutline,
  splitFm,
} from './deskDisplay.js'

const STUB_BODY_CHARS = 400

export function safeProjectName(name) {
  const t = String(name || '').trim()
  return Boolean(
    t
    && t !== '.'
    && t !== '..'
    && !t.includes('/')
    && !t.includes('\\')
    && !t.includes('\0'),
  )
}

export function projectDir(repoRoot, name) {
  return join(repoRoot, 'projects', name)
}

function readText(path) {
  try {
    return readFileSync(path, 'utf8')
  } catch {
    return ''
  }
}

function readJson(path) {
  const raw = readText(path)
  if (!raw.trim()) return null
  try {
    return JSON.parse(raw)
  } catch {
    return null
  }
}

export function isShortDrama(dir) {
  const meta = readJson(join(dir, 'meta.json')) || {}
  const state = readJson(join(dir, 'state.json')) || {}
  const mode = String(meta.project_mode || state.meta?.project_mode || '').trim()
  return mode === 'short_drama'
}

export function loadProjectState(dir) {
  return readJson(join(dir, 'state.json')) || {}
}

export function listProjects(repoRoot) {
  const root = join(repoRoot, 'projects')
  if (!existsSync(root)) return []
  return readdirSync(root)
    .filter((n) => !n.startsWith('.') && existsSync(join(root, n)) && statSync(join(root, n)).isDirectory())
    .sort()
}

export function listChapterNumbers(dir) {
  const roots = isShortDrama(dir) ? [join(dir, 'episodes'), join(dir, 'chapters')] : [join(dir, 'chapters')]
  const nums = []
  for (const root of roots) {
    if (!existsSync(root)) continue
    for (const name of readdirSync(root)) {
      const n = Number(name)
      if (Number.isInteger(n) && n > 0 && statSync(join(root, name)).isDirectory()) nums.push(n)
    }
  }
  return [...new Set(nums)].sort((a, b) => a - b)
}

export function chapterDir(dir, n) {
  const folder = isShortDrama(dir) ? 'episodes' : 'chapters'
  return join(dir, folder, String(n).padStart(3, '0'))
}

export function readChapterDraft(dir, n) {
  const unit = chapterDir(dir, n)
  const body = isShortDrama(dir) ? 'script.md' : 'draft.md'
  return readText(join(unit, body))
}

export function readChapterOutlineRaw(dir, n) {
  const unit = chapterDir(dir, n)
  const json = readText(join(unit, 'outline.json'))
  if (json.trim()) return json
  return readText(join(unit, 'outline.md'))
}

function cardComplete(meta, body) {
  const c = String(meta.complete || '').toLowerCase()
  if (['true', 'yes', '1'].includes(c)) return true
  if (['false', 'no', '0'].includes(c)) return false
  if (meta.source === 'volume_sync') return false
  const chars = [...body].length
  if ((body.includes('## 卷末同步摘要') || body.includes('## 剧情同步摘要')) && chars < STUB_BODY_CHARS) {
    return false
  }
  return chars > 80
}

export function loadMarkdownCards(dir, kind) {
  if (!existsSync(dir)) return []
  const files = readdirSync(dir).filter((n) => n.endsWith('.md')).sort()
  return files.map((file) => {
    const text = readText(join(dir, file))
    const slug = file.replace(/\.md$/, '')
    const { meta, body } = splitFm(text)
    const name = meta.name || meta.title || slug
    return {
      id: meta.id || slug,
      slug,
      name,
      title: meta.title || name,
      category: meta.category || kind,
      markdown: body.trim() ? body : text,
      complete: cardComplete(meta, body),
      meta,
      full: text,
    }
  })
}

function cardToPreview(card, markdown) {
  const obj = {
    id: card.id,
    slug: card.slug,
    name: card.name,
    title: card.title,
    category: card.category,
    markdown,
    complete: card.complete,
    gaps: [],
  }
  for (const key of [
    'scope', 'arc', 'plot_type', 'status', 'holdings', 'needs_bridge',
    'next_plot', 'volume_index', 'arc_index', 'portrait',
  ]) {
    if (card.meta[key] != null && card.meta[key] !== '') obj[key] = card.meta[key]
  }
  return obj
}

function collectEntityGaps(dir) {
  const gaps = []
  for (const group of ['characters', 'items', 'locations']) {
    for (const c of loadMarkdownCards(join(dir, 'entities', group), group)) {
      if (!c.complete) {
        gaps.push(`[${group}]「${c.name}」待补全（短摘要/同步 stub 或 complete:false）`)
      }
    }
  }
  return gaps
}

function listArcOutlines(dir) {
  const rows = []
  const arcDir = join(dir, 'artifacts', 'arc_outlines')
  const legacy = readText(join(dir, 'artifacts', 'arc_outline.md'))
  const files = []
  if (existsSync(arcDir)) {
    for (const name of readdirSync(arcDir)) {
      if (!name.endsWith('.md')) continue
      const n = Number(name.replace(/\.md$/, ''))
      if (Number.isInteger(n) && n >= 1) files.push(n)
    }
  }
  files.sort((a, b) => a - b)
  for (const vi of files) {
    const text = readText(join(arcDir, `${String(vi).padStart(2, '0')}.md`))
      || readText(join(arcDir, `${vi}.md`))
    if ([...text.trim()].length < 20) continue
    const title = text.split('\n').find((l) => l.trim().startsWith('#'))
      ?.replace(/^#+\s*/, '')
      .trim() || `第${vi}卷`
    rows.push({ volume: vi, title, markdown: displayArcOutline(text) })
  }
  if (!rows.length && [...legacy.trim()].length >= 20) {
    const title = legacy.split('\n').find((l) => l.trim().startsWith('#'))
      ?.replace(/^#+\s*/, '')
      .trim() || '第1卷'
    rows.push({ volume: 1, title, markdown: displayArcOutline(legacy) })
  }
  return rows
}

function loadArtifacts(dir) {
  const artifacts = {}
  const artDir = join(dir, 'artifacts')
  if (!existsSync(artDir)) return artifacts
  for (const name of readdirSync(artDir)) {
    const path = join(artDir, name)
    if (!statSync(path).isFile()) continue
    const stem = name.replace(/\.(md|txt)$/i, '')
    if (stem === name) continue
    if (stem === 'arc_outline' || stem === 'arc_planner') continue
    const text = readText(path)
    if (!text.trim()) continue
    if (stem === 'master_outline' || stem === 'master_planner' || stem === 'series_outline') {
      artifacts[stem] = displayMasterOutline(text)
    } else if (stem === 'bible') {
      artifacts[stem] = displayBible(text)
    } else {
      artifacts[stem] = text
    }
  }
  return artifacts
}

export function buildPreview(repoRoot, name) {
  if (!safeProjectName(name)) {
    return { error: '非法项目名' }
  }
  const dir = projectDir(repoRoot, name)
  if (!existsSync(dir)) {
    return { error: `项目「${name}」不存在` }
  }
  const st = loadProjectState(dir)
  const meta = readJson(join(dir, 'meta.json')) || {}
  const published = Number(st.published_count) || 0
  const next = Number(st.next_chapter) || 1
  const short = isShortDrama(dir)
  const unitWord = short ? '集' : '章'
  const chapters = listChapterNumbers(dir).map((n) => {
    const draftRaw = readChapterDraft(dir, n)
    const bodyChars = draftRaw.trim() ? draftBodyChars(draftRaw) : 0
    const unit = chapterDir(dir, n)
    const hasOutline = existsSync(join(unit, 'outline.json')) || existsSync(join(unit, 'outline.md'))
    const title = draftRaw
      .split('\n')[0]
      ?.trim()
      .replace(/^#+\s*/, '')
      .trim()
    return {
      number: n,
      title: title && [...title].length < 80 ? title : `第${n}${unitWord}`,
      has_draft: bodyChars > 0,
      has_outline: hasOutline,
      body_chars: bodyChars,
      draft: '',
      outline: '',
    }
  })

  const entities = { characters: [], items: [], locations: [] }
  const charCards = loadMarkdownCards(join(dir, 'entities', 'characters'), 'characters')
  const allCharKeys = charCards.flatMap((c) => [c.name, c.title, c.slug].filter(Boolean))
  const defaultOwners = []
  const pushOwner = (v) => {
    if (typeof v === 'string' && v.trim()) defaultOwners.push(v.trim())
    else if (Array.isArray(v)) v.forEach((x) => typeof x === 'string' && x.trim() && defaultOwners.push(x.trim()))
  }
  pushOwner(meta.protagonist)
  pushOwner(meta.protagonists)
  pushOwner(st.meta?.protagonist)
  pushOwner(st.meta?.protagonists)
  for (const group of ['characters', 'items', 'locations']) {
    const cards = group === 'characters' ? charCards : loadMarkdownCards(join(dir, 'entities', group), group)
    entities[group] = cards.map((c) => {
      const v = cardToPreview(c, displayEntityCard(group, c.full))
      if (group === 'characters') {
        const selfKeys = [c.name, c.title, c.slug]
        const isDefault = selfKeys.some((k) => defaultOwners.some((d) => d === k || k.includes(d) || d.includes(k)))
          || ['protagonist', 'main', 'lead', '主角'].includes(String(c.meta.role || '').toLowerCase())
        const board = formatBodyStateForCharacter(dir, Math.max(next, 1), selfKeys, allCharKeys, isDefault)
        if (board.trim()) v.body_state = board
      }
      return v
    })
  }

  const plots = loadMarkdownCards(join(dir, 'plots'), 'plots')
    .map((c) => cardToPreview(c, displayPlotCardBody(c.full)))
    .sort((a, b) => {
      const av = Number(a.volume_index) || 0
      const bv = Number(b.volume_index) || 0
      if (av !== bv) return av - bv
      return String(a.title || a.name).localeCompare(String(b.title || b.name), 'zh')
    })

  const artifacts = loadArtifacts(dir)
  const storyOutline = readJson(join(dir, 'artifacts', 'story_outline.json')) || {
    markdown: artifacts.master_planner || '',
    acts: [],
  }
  const entityGaps = collectEntityGaps(dir)
  const arcOutlines = listArcOutlines(dir)
  const hasMaster = ['master_outline', 'series_outline', 'master_planner']
    .some((k) => String(artifacts[k] || '').trim().length > 20)
    || String(storyOutline.markdown || '').trim().length > 20
  const setupPhase = String(meta.setup_phase || st.meta?.setup_phase || (hasMaster ? 'ready' : 'collecting'))
  const volumePhase = String(st.meta?.volume_phase || meta.volume_phase || 'drafting_volume')
  const state = { ...st }
  delete state.chapters
  state.published_count = published
  state.next_chapter = next
  if (!state.name) state.name = meta.name || name
  if (state.genre == null) state.genre = meta.genre || ''

  return {
    project: name,
    name,
    chapters,
    project_mode: short ? 'short_drama' : 'longform',
    story_outline: storyOutline,
    plots,
    entities,
    artifacts,
    arc_outlines: arcOutlines,
    entity_gaps: entityGaps,
    entity_gaps_display: displayEntityGaps(entityGaps),
    expected_events: [],
    body_state_board: formatBodyStateBoard(dir, next > 0 ? next : 1),
    published_count: published,
    next_chapter: next,
    state,
    setup_phase: setupPhase,
    volume_phase: volumePhase,
    volume_qa_phase: 'idle',
    foreshadow_phase: 'idle',
    foreshadow_pressure: 0,
    foreshadow_debt_cap: 0,
    has_master_outline: hasMaster,
    has_arc_outline: arcOutlines.length > 0,
    longform_health: {},
    cost_by_agent: [],
    loop: { ok: true, hasJob: false },
  }
}

export function libraryItems(repoRoot) {
  return listProjects(repoRoot).map((name) => {
    const dir = projectDir(repoRoot, name)
    const st = loadProjectState(dir)
    const meta = readJson(join(dir, 'meta.json')) || {}
    const short = isShortDrama(dir)
    const title = st.name || meta.name || name
    return {
      id: name,
      name,
      display_title: title,
      title,
      genre: st.genre || meta.genre || '',
      next_chapter: Number(st.next_chapter) || 1,
      published_count: Number(st.published_count) || 0,
      project_mode: short ? 'short_drama' : 'longform',
      status: 'active',
    }
  })
}

export function libraryOne(repoRoot, name) {
  const preview = buildPreview(repoRoot, name)
  if (preview.error) return preview
  return {
    novel: {
      id: name,
      display_title: preview.state?.name || name,
      genre: preview.state?.genre || '',
      published_count: preview.published_count,
      next_chapter: preview.next_chapter,
      project_mode: preview.project_mode,
      status: 'active',
    },
    preview,
  }
}

export function chapterPayload(repoRoot, name, chapter) {
  if (!safeProjectName(name)) return { ok: false, error: '非法项目名' }
  const dir = projectDir(repoRoot, name)
  if (!existsSync(dir)) return { ok: false, error: `项目「${name}」不存在` }
  const n = Number(chapter) || 0
  if (n < 1) return { ok: false, error: '无效章号' }
  const draftRaw = readChapterDraft(dir, n)
  const outlineRaw = readChapterOutlineRaw(dir, n)
  const parsed = parseChapterOutline(outlineRaw)
  const outline = parsed ? displayChapterOutline(parsed) : outlineRaw
  const titleLine = draftRaw.split('\n')[0]?.trim().replace(/^#+\s*/, '').trim()
  const unitWord = isShortDrama(dir) ? '集' : '章'
  return {
    ok: true,
    number: n,
    title: titleLine && [...titleLine].length < 80 ? titleLine : `第${n}${unitWord}`,
    draft: draftRaw.trim() ? displayDraft(draftRaw) : '',
    outline,
    body_chars: draftBodyChars(draftRaw),
  }
}

export function deskFocusEvent(project, chapter, readerTab) {
  const tab = String(readerTab || '').trim() || 'draft'
  return {
    kind: 'focus',
    project: String(project || '').trim(),
    chapter: chapter || null,
    readerTab: tab,
    tool: 'desk_focus',
  }
}
