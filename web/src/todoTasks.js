/**
 * Progressive To-dos → N major tasks (one per todo).
 * Micro steps attach only to the current in_progress task.
 * Genre-neutral: optional unit align via「第N章」when both todos and progress share it.
 */

import { normalizeTodoStatus } from './todoWindow.js'
import { parseToolProgress } from './toolProgress.js'

/**
 * @param {Array<{ content?: string, status?: string }>} todos
 * @param {string} [toolOutput]
 * @returns {{
 *   tasks: Array<{ todo: object, status: string, steps: object[], index: number }>,
 *   currentIndex: number,
 * }}
 */
export function buildTodoTasks(todos, toolOutput = '') {
  const list = Array.isArray(todos) ? todos : []
  const output = String(toolOutput || '')
  const unitKeys = list.map((t) => unitKeyFromText(t?.content))
  const canAlign = canAlignUnits(unitKeys, output)

  const stepsByUnit = canAlign ? segmentStepsByUnit(output) : null
  const tipSteps = canAlign
    ? []
    : parseTipSteps(output)

  let currentIndex = list.findIndex((t) => normalizeTodoStatus(t?.status) === 'in_progress')
  if (currentIndex < 0 && canAlign) {
    // Prefer the unit that still has live/running steps.
    for (let i = 0; i < list.length; i += 1) {
      const key = unitKeys[i]
      if (!key) continue
      const steps = stepsByUnit.get(key) || []
      if (steps.some((e) => e.status === 'running' || e.status === 'paused')) {
        currentIndex = i
        break
      }
    }
  }

  const tasks = list.map((todo, index) => {
    const status = normalizeTodoStatus(todo?.status)
    let steps = []
    if (status === 'in_progress' || index === currentIndex) {
      if (canAlign && unitKeys[index]) {
        steps = stepsByUnit.get(unitKeys[index]) || []
      } else if (status === 'in_progress') {
        steps = tipSteps
      }
    }
    return {
      todo,
      status: index === currentIndex && status === 'pending' ? 'in_progress' : status,
      steps,
      index,
    }
  })

  return { tasks, currentIndex }
}

export function unitKeyFromText(text) {
  const m = String(text || '').match(/第(\d+)章/)
  return m ? `第${m[1]}章` : null
}

function canAlignUnits(unitKeys, output) {
  const keyed = unitKeys.filter(Boolean).length
  if (keyed < 1) return false
  if (keyed >= 2) return /审阅队列|▶\s*第\d+章|第\d+章/.test(output) || !output
  return /审阅队列|▶\s*第\d+章/.test(output)
}

/** Split raw tool output into per-unit line buffers, then parse each. */
export function segmentStepsByUnit(output) {
  const text = String(output || '')
  const map = new Map()
  let current = null
  let buf = []

  const flush = () => {
    if (!current) {
      buf = []
      return
    }
    const prev = map.get(current) || []
    map.set(current, prev.concat(buf))
    buf = []
  }

  for (const raw of text.split(/\n/)) {
    const t = raw.trim()
    const q = t.match(/^审阅队列[：:]\s*第(\d+)章/)
      || t.match(/^##\s*审阅队列.+?[（(](\d+)\s*\/\s*\d+[）)]/)
      || t.match(/^▶\s*第(\d+)章/)
    if (q) {
      flush()
      current = `第${q[1]}章`
      continue
    }
    if (/^✓\s*第(\d+)章/.test(t)) {
      // Milestone close — stays on current unit; do not start a new bucket.
      continue
    }
    if (current) buf.push(raw)
  }
  flush()

  const out = new Map()
  for (const [unit, lines] of map) {
    const chunk = lines.join('\n')
    const { entries } = parseToolProgress(chunk, {
      omitQueue: true,
      compactHistory: false,
    })
    out.set(
      unit,
      entries.filter((e) => e.kind === 'step' || e.kind === 'note'),
    )
  }
  return out
}

function parseTipSteps(output) {
  const { entries } = parseToolProgress(output, {
    omitQueue: true,
    compactHistory: true,
  })
  return entries.filter((e) => e.kind === 'step' || e.kind === 'note')
}
