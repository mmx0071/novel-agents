/**
 * How progressive To-dos attach to Cursor-style「已工作」blocks.
 *
 * Hierarchy (audit / batch):
 *   已工作 Xs
 *     To-dos · N/M · 当前章     ← macro queue
 *     审阅队列 / 工具           ← mid
 *       ▶ 一致性审计 …         ← micro steps
 */

const TODO_HOST_TOOLS = new Set([
  'audit_chapters',
  'audit_chapter',
  'continue_writing_batch',
  'steer_run',
])

export function workItemsHostTodos(items) {
  return (Array.isArray(items) ? items : []).some(
    (it) => it?.type === 'tool_call' && TODO_HOST_TOOLS.has(it.name),
  )
}

/**
 * Pick which worked segment should render the shared todos list.
 * Prefer a live host block; else the last host block in the turn.
 * @returns {number} segment index in `segments`, or -1
 */
export function pickTodoHostSegment(segments, todos) {
  const list = Array.isArray(todos) ? todos : []
  if (!list.length) return -1
  const segs = Array.isArray(segments) ? segments : []
  let lastHost = -1
  let lastWorked = -1
  for (let i = 0; i < segs.length; i += 1) {
    const seg = segs[i]
    if (seg?.kind !== 'worked') continue
    lastWorked = i
    if (!workItemsHostTodos(seg.items)) continue
    lastHost = i
    if (seg.running) return i
  }
  if (lastHost >= 0) return lastHost
  // Todos arrived before the host tool card — park on latest worked block.
  const looksQueue = list.some((t) => /审校第\d+章|第\d+章/.test(String(t.content || '')))
  return looksQueue ? lastWorked : -1
}

/** Compact one-line todos tip for「已工作」header. */
export function todosFlowSummary(todos) {
  const list = Array.isArray(todos) ? todos : []
  if (!list.length) return ''
  const done = list.filter((t) => t.status === 'completed' || t.status === 'Completed').length
  const current = list.find((t) => {
    const s = t.status
    return s === 'in_progress' || s === 'inProgress' || s === 'InProgress'
  })
  const allDone = done === list.length
  if (allDone) return `${done}/${list.length} 待办已完成`
  if (current) {
    const label = String(current.content || '')
      .replace(/（[^）]*）\s*$/, '')
      .trim()
    return `${done}/${list.length} · ${label || '进行中'}`
  }
  return `${done}/${list.length} 待办`
}
