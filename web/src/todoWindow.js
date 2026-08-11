/**
 * Compact To-dos window for audit/batch queues:
 * last few completed + current + pending count (not all 20 ○ rows).
 */

export function normalizeTodoStatus(status) {
  if (status === 'completed' || status === 'Completed') return 'completed'
  if (status === 'in_progress' || status === 'inProgress' || status === 'InProgress') {
    return 'in_progress'
  }
  return 'pending'
}

/**
 * @returns {{
 *   rows: Array<{ item: object, status: string, index: number }>,
 *   pendingCount: number,
 *   done: number,
 *   total: number,
 *   current: object|null,
 * }}
 */
export function windowTodos(todos, { keepCompleted = 2 } = {}) {
  const list = Array.isArray(todos) ? todos : []
  const total = list.length
  const annotated = list.map((item, index) => ({
    item,
    index,
    status: normalizeTodoStatus(item?.status),
  }))
  const done = annotated.filter((t) => t.status === 'completed').length
  const current = annotated.find((t) => t.status === 'in_progress') || null
  const completed = annotated.filter((t) => t.status === 'completed')
  const pending = annotated.filter((t) => t.status === 'pending')
  const keep = Math.max(0, keepCompleted)
  const rows = [
    ...completed.slice(-keep),
    ...(current ? [current] : []),
  ]
  return {
    rows,
    pendingCount: pending.length,
    done,
    total,
    current: current?.item || null,
  }
}

export function todosHeadline(todos) {
  const { done, total, current } = windowTodos(todos)
  if (!total) return ''
  if (done === total) return `${done}/${total} 已完成`
  if (current) {
    const label = String(current.content || '')
      .replace(/（[^）]*）\s*$/, '')
      .trim()
    const note = String(current.content || '').match(/（([^）]*)）\s*$/)
    const tip = note ? note[1] : ''
    return tip ? `${done}/${total} · ${label}（${tip}）` : `${done}/${total} · ${label}`
  }
  return `${done}/${total} 待办`
}

/** Fold label:「待办 4/20 · …」/「待办已完成」 */
export function todosCursorLabel(todos) {
  const { done, total, current } = windowTodos(todos)
  if (!total) return ''
  if (done === total) return `待办已完成（${done}/${total}）`
  if (current) {
    const label = String(current.content || '')
      .replace(/（[^）]*）\s*$/, '')
      .trim()
    return label
      ? `待办 ${done}/${total} · ${label}`
      : `待办 ${done}/${total}`
  }
  return `待办 ${done}/${total}`
}
