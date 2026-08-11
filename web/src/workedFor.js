/**
 * Group consecutive "work" timeline items into Cursor-style Worked-for blocks.
 * User / agent prose and decision cards stay outside the fold.
 */

const WORK_TYPES = new Set([
  'tool_call',
  'skill_load',
  'pipeline_step',
  'reasoning',
  'agent_spawn',
])

export function isWorkTimelineItem(item) {
  return !!(item && WORK_TYPES.has(item.type))
}

/**
 * @returns {Array<
 *   | { kind: 'item', item: object, index: number }
 *   | { kind: 'worked', items: object[], indices: number[], running: boolean, durationMs: number }
 * >}
 */
export function groupTurnItemsForWorked(items) {
  const list = Array.isArray(items) ? items : []
  const out = []
  let buf = []
  let bufIdx = []

  const flush = () => {
    if (!buf.length) return
    out.push({
      kind: 'worked',
      items: buf,
      indices: bufIdx,
      running: buf.some((it) => workItemRunning(it)),
      durationMs: workedDurationMs(buf),
    })
    buf = []
    bufIdx = []
  }

  list.forEach((item, index) => {
    if (isWorkTimelineItem(item)) {
      buf.push(item)
      bufIdx.push(index)
      return
    }
    flush()
    out.push({ kind: 'item', item, index })
  })
  flush()
  return out
}

export function workItemRunning(item) {
  if (!item) return false
  const s = item.status
  return s == null
    || s === ''
    || s === 'in_progress'
    || s === 'inProgress'
    || s === 'InProgress'
}

/** Prefer wall-ish total: sum of tool durations; fall back to max. */
export function workedDurationMs(items) {
  const list = Array.isArray(items) ? items : []
  let sum = 0
  let max = 0
  let n = 0
  for (const it of list) {
    const ms = Number(it?.duration_ms)
    if (!Number.isFinite(ms) || ms < 0) continue
    sum += ms
    if (ms > max) max = ms
    n += 1
  }
  if (!n) return 0
  // Parallel tools inflate sum; use max when sum is much larger than max.
  if (n > 1 && sum > max * 1.6) return max
  return sum
}

/** Consumer duration: `1 分 24 秒` / `24 秒` / `不到 1 秒` */
export function formatWorkedDuration(ms) {
  const n = Number(ms) || 0
  if (n <= 0) return ''
  if (n < 1000) return '不到 1 秒'
  const sec = Math.round(n / 1000)
  if (sec < 60) return `${sec} 秒`
  const m = Math.floor(sec / 60)
  const s = sec % 60
  return s > 0 ? `${m} 分 ${s} 秒` : `${m} 分钟`
}

/** Primary header — Chinese consumer wording. */
export function workedForTitle({ running, awaiting = false, durationMs }) {
  if (awaiting) return '等待你选择…'
  if (running) return '进行中…'
  const dur = formatWorkedDuration(durationMs)
  return dur ? `用时 ${dur}` : '已完成'
}

/**
 * Secondary summary like Cursor「Explored 3 files, 2 searches」.
 * Chinese, genre-neutral. Prefer live pipeline tip from fat tool outputs.
 */
export function workedForSummary(items, { toolLabelZh, parseToolProgress, summarizeToolProgress } = {}) {
  const list = Array.isArray(items) ? items : []
  const tools = list.filter((it) => it.type === 'tool_call')
  const skills = list.filter((it) => it.type === 'skill_load')
  const thoughts = list.filter((it) => it.type === 'reasoning')
  const steps = list.filter((it) => it.type === 'pipeline_step')
  const spawns = list.filter((it) => it.type === 'agent_spawn')

  // Cursor mid-line: surface nested ▶/✓ tip when a single fat tool dominates.
  if (
    tools.length === 1
    && typeof parseToolProgress === 'function'
    && typeof summarizeToolProgress === 'function'
  ) {
    const parsed = parseToolProgress(tools[0].output)
    if (parsed.structured) {
      const tip = summarizeToolProgress(parsed.entries)
      const label = typeof toolLabelZh === 'function'
        ? toolLabelZh(tools[0].name)
        : (tools[0].name || '工具')
      if (tip) return tip.includes(label) ? tip : `${label} · ${tip}`
    }
  }

  const parts = []
  if (tools.length) {
    const labels = []
    const seen = new Set()
    for (const t of tools) {
      const label = typeof toolLabelZh === 'function'
        ? toolLabelZh(t.name)
        : (t.name || '工具')
      if (!label || seen.has(label)) continue
      seen.add(label)
      labels.push(label)
      if (labels.length >= 2) break
    }
    if (labels.length === 1 && tools.length === 1) {
      parts.push(labels[0])
    } else if (labels.length) {
      parts.push(`${tools.length} 项操作 · ${labels.join('、')}`)
    } else {
      parts.push(`${tools.length} 项操作`)
    }
  }
  if (skills.length) parts.push(`查阅 ${skills.length} 份写作指南`)
  if (thoughts.length) {
    parts.push(thoughts.length === 1 ? '思考' : `思考 ${thoughts.length} 次`)
  }
  if (steps.length) parts.push(`${steps.length} 个步骤`)
  if (spawns.length) parts.push(`启动 ${spawns.length} 个协作角色`)
  return parts.join('，') || '处理中'
}
