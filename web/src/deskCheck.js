/**
 * Deterministic checks for the dsh NovelX path (schema / names / length / phase).
 * Does not write projects/. Genre-neutral.
 */

import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'
import {
  draftBodyChars,
  hasH2,
  parseChapterOutline,
  splitFm,
} from './deskDisplay.js'
import {
  chapterDir,
  isShortDrama,
  listChapterNumbers,
  loadProjectState,
  projectDir,
  readChapterDraft,
  readChapterOutlineRaw,
  safeProjectName,
} from './deskPreview.js'

const MIN_DRAFT_SHAPE = 800
const ENTITY_REQUIRED = {
  characters: [
    ['History', ['History', '经历', '历史']],
    ['Personality', ['Personality', '性格']],
    ['Core events', ['Core events', 'Core Events', '核心事件']],
    ['Current status', ['Current status', 'Current Status', '当前状态', '现状']],
  ],
  items: [
    ['Origin', ['Origin', '产出地', '来源']],
    ['Usage', ['Usage', '用途']],
    ['Current status', ['Current status', 'Current Status', '当前状态', '现状']],
  ],
  locations: [
    ['Overview', ['Overview', '概述']],
    ['Factions', ['Factions', '此地势力', '势力']],
    ['Production', ['Production', '产出资源', '产出']],
  ],
}
const PLOT_FM = ['title', 'scope', 'plot_type', 'status', 'needs_bridge']
const PLOT_H2 = [
  ['概览', ['概览']],
  ['剧情走向', ['剧情走向', '剧情走向（叙事节点，无章号）']],
  ['冲突与赌注', ['冲突与赌注', '冲突/赌注', '赌注']],
  ['出场人物', ['出场人物']],
  ['收束条件', ['收束条件', '落点条件', 'exit_condition']],
]
const OUTLINE_STRINGS = [
  'title', 'pov', 'time_location', 'goal', 'conflict', 'emotion_curve', 'cliffhanger',
]
const OUTLINE_ARRAYS = [
  'key_events', 'characters', 'scene_tags', 'lore_queries',
  'plot_includes', 'plot_defers', 'items', 'locations',
]
const BIBLE_H2 = [
  [0, '一句话世界'],
  [1, '时代与叙事框架'],
  [2, '全局势力与阵营'],
  [7, '开放问题'],
]

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

function yamlBool(text, key, fallback = true) {
  const re = new RegExp(`^\\s*${key.replace('.', '\\.')}:\\s*(true|false)\\s*$`, 'm')
  const m = String(text || '').match(re)
  if (!m) return fallback
  return m[1] === 'true'
}

function yamlInt(text, key, fallback) {
  const re = new RegExp(`^${key}:\\s*(-?\\d+)\\s*$`, 'm')
  const m = String(text || '').match(re)
  return m ? Number(m[1]) : fallback
}

function yamlList(text, key) {
  const lines = String(text || '').replace(/\r\n/g, '\n').split('\n')
  const out = []
  let inKey = false
  for (const line of lines) {
    if (/^\S/.test(line) && !line.startsWith(' ') && !line.startsWith('\t')) {
      inKey = line.startsWith(`${key}:`)
      continue
    }
    if (!inKey) continue
    const m = line.match(/^\s+-\s+(.+)$/)
    if (m) out.push(m[1].trim().replace(/^['"]|['"]$/g, ''))
  }
  return out
}

function loadChapterLimits(repoRoot) {
  const raw = readText(join(repoRoot, 'config', 'chapter.yaml'))
  return {
    word_min: yamlInt(raw, 'word_min', 5000),
    word_max: yamlInt(raw, 'word_max', 6000),
    word_hard_min: yamlInt(raw, 'word_hard_min', 4500),
    word_hard_max: yamlInt(raw, 'word_hard_max', 11000),
  }
}

function loadForbiddenNames(repoRoot, projectDirPath) {
  const sys = yamlList(readText(join(repoRoot, 'config', 'naming_rules.yaml')), 'forbidden_names')
  const lore = readJson(join(projectDirPath, 'lore', 'nomenclature.json')) || {}
  const extra = Array.isArray(lore.forbidden_names) ? lore.forbidden_names.map((x) => String(x || '').trim()) : []
  return [...new Set([...sys, ...extra].filter((n) => [...n].length >= 2))]
    .sort((a, b) => [...b].length - [...a].length)
}

function loadContentRules(repoRoot) {
  const raw = readText(join(repoRoot, 'config', 'content_rules.yaml'))
  const block = (id) => ({
    enabled: ruleFlag(raw, id, 'enabled', true),
    blocking: ruleFlag(raw, id, 'blocking', true),
  })
  return {
    raw,
    banned_name: block('banned_name'),
    contradiction_marker: {
      ...block('contradiction_marker'),
      marker: ruleScalar(raw, 'contradiction_marker', 'marker') || '[CONTRADICTION]',
      message: ruleScalar(raw, 'contradiction_marker', 'message') || '正文残留管道内部标记：{marker}',
    },
    too_many_new_facts: {
      ...block('too_many_new_facts'),
      max_count: Number(ruleScalar(raw, 'too_many_new_facts', 'max_count')) || 8,
      pattern: ruleScalar(raw, 'too_many_new_facts', 'pattern') || '\\[NEW_FACT:[^\\]]+\\]',
      message: ruleScalar(raw, 'too_many_new_facts', 'message') || '[NEW_FACT] 过多（{count}）',
    },
    meta_chapter_ref: {
      ...block('meta_chapter_ref'),
      pattern: ruleScalar(raw, 'meta_chapter_ref', 'pattern') || '第\\s*[一二三四五六七八九十百千零〇两\\d]+\\s*章',
      allow_heading: ruleFlag(raw, 'meta_chapter_ref', 'allow_heading', true),
      message: ruleScalar(raw, 'meta_chapter_ref', 'message') || '正文出现章号元叙述「{match}」',
    },
    pipeline_meta_leak: {
      ...block('pipeline_meta_leak'),
      patterns: ruleList(raw, 'pipeline_meta_leak', 'patterns'),
      message: ruleScalar(raw, 'pipeline_meta_leak', 'message') || '正文出现管线元叙述「{match}」',
    },
  }
}

function ruleBlock(raw, id) {
  const lines = String(raw || '').split('\n')
  const start = lines.findIndex((l) => l.match(new RegExp(`^\\s{2}${id}:\\s*$`)))
  if (start < 0) return ''
  const chunk = [lines[start]]
  for (let i = start + 1; i < lines.length; i += 1) {
    if (/^\s{2}[a-z_]+:\s*$/.test(lines[i]) && i !== start) break
    if (/^[a-z_]+:/.test(lines[i])) break
    chunk.push(lines[i])
  }
  return chunk.join('\n')
}

function ruleFlag(raw, id, key, fallback) {
  const block = ruleBlock(raw, id)
  return yamlBool(block, key, fallback)
}

function ruleScalar(raw, id, key) {
  const block = ruleBlock(raw, id)
  const m = block.match(new RegExp(`^\\s+${key}:\\s*(.+)$`, 'm'))
  if (!m) return ''
  return m[1].trim().replace(/^["']|["']$/g, '')
}

function ruleList(raw, id, key) {
  return yamlList(ruleBlock(raw, id), key)
}

function loadFeatures(repoRoot) {
  const raw = readText(join(repoRoot, 'config', 'features.yaml'))
  return {
    enforce_setup_gate: yamlBool(raw, 'studio.enforce_setup_gate', true),
    enforce_volume_phase: yamlBool(raw, 'studio.enforce_volume_phase', true),
    enforce_chapter_order: yamlBool(raw, 'studio.enforce_chapter_order', true),
  }
}

function issue(level, id, message, extra = {}) {
  return { level, id, message, ...extra }
}

function resolveSetupPhase(dir) {
  const meta = readJson(join(dir, 'meta.json')) || {}
  const st = loadProjectState(dir)
  if (!existsSync(join(dir, 'meta.json')) && !existsSync(join(dir, 'state.json'))) return 'ready'
  if (Number(st.published_count) >= 1) return 'ready'
  const p = String(meta.setup_phase || st.meta?.setup_phase || '').trim()
  if (p) return p
  const hasMaster = ['master_outline.md', 'master_planner.md', 'series_outline.md']
    .some((f) => readText(join(dir, 'artifacts', f)).trim().length > 20)
  const hasArc = existsSync(join(dir, 'artifacts', 'arc_outlines'))
    && readdirSync(join(dir, 'artifacts', 'arc_outlines')).some((n) => n.endsWith('.md'))
  if (hasMaster && (isShortDrama(dir) || hasArc)) return 'awaiting_confirm'
  return 'collecting'
}

function resolveVolumePhase(dir) {
  if (isShortDrama(dir)) return 'drafting_volume'
  const st = loadProjectState(dir)
  const p = String(st.meta?.volume_phase || '').trim()
  if (p) return p
  const plotsDir = join(dir, 'plots')
  if (existsSync(plotsDir)) {
    for (const name of readdirSync(plotsDir).filter((n) => n.endsWith('.md'))) {
      const { meta } = splitFm(readText(join(plotsDir, name)))
      if (['in_progress', 'bridging'].includes(String(meta.status || ''))) return 'drafting_volume'
    }
  }
  return 'drafting_volume'
}

function draftTitleOk(text, chapter, short) {
  const first = String(text || '').trim().split('\n')[0]?.trim() || ''
  const unit = short ? '集' : '章'
  const re = new RegExp(`^#\\s*第\\s*${chapter}\\s*${unit}(?:\\s|$)`)
  return re.test(first)
}

function findNamesInText(text, names) {
  const hits = []
  const src = String(text || '')
  for (const name of names) {
    if (src.includes(name)) hits.push(name)
  }
  return hits
}

function scanContentRules(draft, rules, names) {
  const issues = []
  const lines = String(draft || '').replace(/\r\n/g, '\n').split('\n')
  if (rules.banned_name.enabled) {
    const hits = findNamesInText(draft, names)
    if (hits.length) {
      issues.push(issue(
        rules.banned_name.blocking ? 'blocker' : 'warning',
        'banned_name',
        `正文命中禁名：${hits.slice(0, 8).join('、')}`,
      ))
    }
  }
  if (rules.contradiction_marker.enabled && draft.includes(rules.contradiction_marker.marker)) {
    issues.push(issue(
      rules.contradiction_marker.blocking ? 'blocker' : 'warning',
      'contradiction_marker',
      rules.contradiction_marker.message.replace('{marker}', rules.contradiction_marker.marker),
    ))
  }
  if (rules.too_many_new_facts.enabled) {
    let count = 0
    try {
      const re = new RegExp(rules.too_many_new_facts.pattern, 'g')
      count = (draft.match(re) || []).length
    } catch {
      count = 0
    }
    if (count > rules.too_many_new_facts.max_count) {
      issues.push(issue(
        rules.too_many_new_facts.blocking ? 'blocker' : 'warning',
        'too_many_new_facts',
        rules.too_many_new_facts.message.replace('{count}', String(count)),
      ))
    }
  }
  if (rules.meta_chapter_ref.enabled) {
    try {
      const re = new RegExp(rules.meta_chapter_ref.pattern, 'g')
      for (let i = 0; i < lines.length; i += 1) {
        if (rules.meta_chapter_ref.allow_heading && i === 0 && lines[i].trim().startsWith('#')) continue
        const m = lines[i].match(re)
        if (m) {
          issues.push(issue(
            rules.meta_chapter_ref.blocking ? 'blocker' : 'warning',
            'meta_chapter_ref',
            rules.meta_chapter_ref.message
              .replace('{match}', m[0])
              .replace('{line}', String(i + 1)),
          ))
          break
        }
      }
    } catch { /* ignore bad pattern */ }
  }
  if (rules.pipeline_meta_leak.enabled) {
    for (let i = 0; i < lines.length; i += 1) {
      const hit = rules.pipeline_meta_leak.patterns.find((p) => p && lines[i].includes(p))
      if (hit) {
        issues.push(issue(
          rules.pipeline_meta_leak.blocking ? 'blocker' : 'warning',
          'pipeline_meta_leak',
          rules.pipeline_meta_leak.message
            .replace('{match}', hit)
            .replace('{line}', String(i + 1)),
        ))
        break
      }
    }
  }
  return issues
}

function checkOutline(raw) {
  const issues = []
  const parsed = parseChapterOutline(raw)
  if (!parsed) {
    issues.push(issue('blocker', 'schema.outline', '章纲须为 outline.json 对象（不要用 Markdown）'))
    return issues
  }
  const missing = []
  for (const k of OUTLINE_STRINGS) {
    if (!String(parsed[k] || '').trim()) missing.push(k)
  }
  for (const k of OUTLINE_ARRAYS) {
    if (parsed[k] == null) missing.push(`${k}（缺）`)
    else if (!Array.isArray(parsed[k])) missing.push(`${k}（须为数组）`)
  }
  if (missing.length) {
    issues.push(issue('blocker', 'schema.outline', `章纲缺少或非法字段：${missing.join('、')}`))
  }
  const events = Array.isArray(parsed.key_events) ? parsed.key_events : []
  if (events.length < 2) {
    issues.push(issue('blocker', 'schema.outline', `key_events 至少 2 条，当前 ${events.length}`))
  } else if (events.length > 4) {
    issues.push(issue('warning', 'schema.outline', `key_events 至多 4 条（单章预算），当前 ${events.length}`))
  }
  return issues
}

function checkDraft(text, chapter, short, limits) {
  const issues = []
  const trimmed = String(text || '').trim()
  if (!trimmed) {
    issues.push(issue('blocker', 'schema.draft', '正文不能为空'))
    return issues
  }
  if (!draftTitleOk(trimmed, chapter, short)) {
    const unit = short ? '集' : '章'
    issues.push(issue('blocker', 'schema.draft', `首行须为「# 第${chapter}${unit} …」`))
  }
  const chars = draftBodyChars(trimmed)
  if (chars < MIN_DRAFT_SHAPE) {
    issues.push(issue('blocker', 'schema.draft', `正文（标题行之后）至少 ${MIN_DRAFT_SHAPE} 字，当前 ${chars}`))
  }
  if (chars < limits.word_hard_min) {
    issues.push(issue('blocker', 'words', `正文不足硬门 ${limits.word_hard_min} 字，当前 ${chars}`))
  } else if (chars < limits.word_min) {
    issues.push(issue('warning', 'words', `正文偏短（目标 ${limits.word_min}–${limits.word_max}，当前 ${chars}）`))
  } else if (chars > limits.word_hard_max) {
    issues.push(issue('blocker', 'words', `正文超过硬门 ${limits.word_hard_max} 字，当前 ${chars}`))
  } else if (chars > limits.word_max) {
    issues.push(issue('warning', 'words', `正文偏长（目标 ${limits.word_min}–${limits.word_max}，当前 ${chars}）`))
  }
  return issues
}

function checkBible(text) {
  const issues = []
  const t = String(text || '').trim()
  if (!t) {
    issues.push(issue('blocker', 'schema.bible', '世界观不能为空'))
    return issues
  }
  const h1 = t.split('\n').find((l) => l.startsWith('# ') && !l.startsWith('## '))
    ?.replace(/^#\s+/, '').trim() || ''
  if (!h1.startsWith('世界观') && !/^world bible$/i.test(h1) && !/^bible$/i.test(h1)) {
    issues.push(issue('blocker', 'schema.bible', `首个 H1 须为「# 世界观」，当前：${h1 || '（缺失）'}`))
  }
  const missing = []
  for (const [num, label] of BIBLE_H2) {
    if (!hasH2(t, [`${num}. ${label}`, `${num}.${label}`, label])) {
      missing.push(`## ${num}. ${label}`)
    }
  }
  if (missing.length) {
    issues.push(issue('blocker', 'schema.bible', `缺少必填节：${missing.join('、')}`))
  }
  return issues
}

function checkMaster(text) {
  const issues = []
  const t = String(text || '').trim()
  if (!t) {
    issues.push(issue('blocker', 'schema.master', '总纲不能为空'))
    return issues
  }
  const missing = []
  if (!hasH2(t, ['一句话卖点', 'Logline', 'logline', '卖点'])) missing.push('## 一句话卖点')
  if (!hasH2(t, ['三幕结构', '分卷', '分卷结构', '分集骨架', '分集', '分季', '分季结构'])) {
    missing.push('## 三幕结构 或 ## 分卷')
  }
  if (!hasH2(t, ['主角弧', '主角弧光'])) missing.push('## 主角弧')
  if (!hasH2(t, ['主线冲突', '核心冲突', '主冲突'])) missing.push('## 主线冲突')
  if (missing.length) {
    issues.push(issue('blocker', 'schema.master', `缺少必填节：${missing.join('、')}`))
  }
  return issues
}

function checkEntity(group, text, file) {
  const issues = []
  const { meta, body } = splitFm(text)
  if (!String(text || '').trimStart().startsWith('---') || !meta.name) {
    issues.push(issue('blocker', 'schema.entity', `${file} 须含 frontmatter，且有 name`))
  }
  const status = String(meta.status || 'active')
  if (!['active', 'background', 'exited', 'consumed', 'Active', 'Background'].includes(status)) {
    issues.push(issue('blocker', 'schema.entity', `${file} status 非法：${status}`))
  }
  const missing = []
  for (const [canon, aliases] of (ENTITY_REQUIRED[group] || [])) {
    if (!hasH2(body, aliases)) missing.push(`## ${canon}`)
  }
  if (missing.length) {
    issues.push(issue('blocker', 'schema.entity', `${file} 缺少必填节：${missing.join('、')}`))
  }
  return issues
}

function checkPlot(text, file) {
  const issues = []
  const { meta, body } = splitFm(text)
  const missingFm = PLOT_FM.filter((k) => !String(meta[k] || '').trim())
  if (missingFm.length) {
    issues.push(issue('blocker', 'schema.plot', `${file} frontmatter 缺少：${missingFm.join('、')}`))
  }
  const missing = []
  for (const [canon, aliases] of PLOT_H2) {
    if (!hasH2(body, aliases)) missing.push(`## ${canon}`)
  }
  if (missing.length) {
    issues.push(issue('blocker', 'schema.plot', `${file} 缺少必填节：${missing.join('、')}`))
  }
  return issues
}

function listMd(dir) {
  if (!existsSync(dir)) return []
  return readdirSync(dir).filter((n) => n.endsWith('.md') && statSync(join(dir, n)).isFile()).sort()
}

/**
 * @param {{ repoRoot: string, project: string, chapter?: number, scope?: string }} opts
 */
export function runCheck(opts) {
  const repoRoot = opts.repoRoot
  const project = String(opts.project || '').trim()
  const scope = String(opts.scope || 'all').trim() || 'all'
  const issues = []
  if (!safeProjectName(project)) {
    return {
      ok: false,
      project,
      chapter: 0,
      setup_phase: '',
      volume_phase: '',
      issues: [issue('blocker', 'project', 'project 须为 projects/ 下的目录名')],
    }
  }
  const dir = projectDir(repoRoot, project)
  if (!existsSync(dir)) {
    return {
      ok: false,
      project,
      chapter: 0,
      setup_phase: '',
      volume_phase: '',
      issues: [issue('blocker', 'project', `项目「${project}」不存在`)],
    }
  }

  const st = loadProjectState(dir)
  const short = isShortDrama(dir)
  const setup = resolveSetupPhase(dir)
  const volume = resolveVolumePhase(dir)
  const features = loadFeatures(repoRoot)
  const limits = loadChapterLimits(repoRoot)
  const names = loadForbiddenNames(repoRoot, dir)
  const rules = loadContentRules(repoRoot)
  const nums = listChapterNumbers(dir)
  const chapter = Number(opts.chapter) || Number(st.next_chapter) || (nums.at(-1) || 0)

  if (scope === 'all' || scope === 'phase') {
    if (features.enforce_setup_gate && setup !== 'ready' && (scope === 'all' || opts.chapter)) {
      issues.push(issue('blocker', 'phase.setup', `setup_phase=${setup}，未确认设定前不要写新章`))
    }
    if (features.enforce_volume_phase && !short && volume !== 'drafting_volume' && opts.chapter) {
      issues.push(issue('blocker', 'phase.volume', `volume_phase=${volume}，当前不能 continue 写新章`))
    }
    if (features.enforce_chapter_order && chapter > 1) {
      const prev = chapter - 1
      if (!nums.includes(prev)) {
        issues.push(issue('blocker', 'phase.order', `缺第${prev}章目录，不能跳写第${chapter}章`))
      }
    }
  }

  if (scope === 'all' || scope === 'chapter' || scope === 'draft' || scope === 'outline') {
    if (chapter >= 1) {
      const unit = chapterDir(dir, chapter)
      const draft = readChapterDraft(dir, chapter)
      const outline = readChapterOutlineRaw(dir, chapter)
      if (scope !== 'draft' && (existsSync(join(unit, 'outline.json')) || existsSync(join(unit, 'outline.md')) || outline.trim())) {
        issues.push(...checkOutline(outline))
      } else if (scope === 'outline') {
        issues.push(issue('blocker', 'schema.outline', `第${chapter}章没有 outline.json`))
      }
      if (scope !== 'outline') {
        if (draft.trim()) {
          issues.push(...checkDraft(draft, chapter, short, limits))
          issues.push(...scanContentRules(draft, rules, names))
        } else if (scope === 'draft' || scope === 'chapter') {
          issues.push(issue('blocker', 'schema.draft', `第${chapter}章没有正文`))
        }
      }
    }
  }

  if (scope === 'all' || scope === 'setting') {
    const bible = readText(join(dir, 'artifacts', 'bible.md'))
    if (bible.trim()) issues.push(...checkBible(bible))
    const master = readText(join(dir, 'artifacts', 'master_outline.md'))
      || readText(join(dir, 'artifacts', 'series_outline.md'))
      || readText(join(dir, 'artifacts', 'master_planner.md'))
    if (master.trim()) issues.push(...checkMaster(master))
    for (const group of ['characters', 'items', 'locations']) {
      for (const file of listMd(join(dir, 'entities', group))) {
        const text = readText(join(dir, 'entities', group, file))
        issues.push(...checkEntity(group, text, `${group}/${file}`))
        const { meta } = splitFm(text)
        const cardName = String(meta.name || '').trim()
        if (cardName && names.includes(cardName)) {
          issues.push(issue('blocker', 'banned_name', `设定卡「${cardName}」命中禁名（${group}/${file}）`))
        }
      }
    }
    for (const file of listMd(join(dir, 'plots'))) {
      issues.push(...checkPlot(readText(join(dir, 'plots', file)), `plots/${file}`))
    }
  }

  const blockers = issues.filter((x) => x.level === 'blocker')
  return {
    ok: blockers.length === 0,
    project,
    chapter,
    setup_phase: setup,
    volume_phase: volume,
    issues,
  }
}

export function formatCheckReport(result) {
  const lines = [
    `status: ${result.ok ? 'ok' : 'blocked'}`,
    `project: ${result.project}`,
    result.chapter > 0 ? `chapter: ${result.chapter}` : '',
    result.setup_phase ? `setup_phase: ${result.setup_phase}` : '',
    result.volume_phase ? `volume_phase: ${result.volume_phase}` : '',
  ].filter(Boolean)
  const blockers = (result.issues || []).filter((x) => x.level === 'blocker')
  const warnings = (result.issues || []).filter((x) => x.level === 'warning')
  if (blockers.length) {
    lines.push('blockers:')
    for (const i of blockers) lines.push(`- [${i.id}] ${i.message}`)
  }
  if (warnings.length) {
    lines.push('warnings:')
    for (const i of warnings) lines.push(`- [${i.id}] ${i.message}`)
  }
  if (!blockers.length && !warnings.length) {
    lines.push('没有结构 / 禁名 / 字数 / 相位硬伤。')
  }
  if (blockers.length) {
    lines.push('有 blocker：不要推进 next_chapter / 不要当章已完成。先改 projects/ 再检查。')
  }
  return lines.join('\n')
}
