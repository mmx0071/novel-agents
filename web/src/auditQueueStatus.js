/**
 * Display status for audit_* tool cards: wire ItemCompleted / client doneHint can
 * fire while Decision Council nested revise/reaudit is still streaming into the
 * same card. Keep the parent "running" while the queue job is logically in flight.
 */

const FOLLOW_ON_TOOLS = new Set([
  'steer_run',
  'audit_chapter',
  'audit_chapters',
  'revise_chapter',
  'revise_episode',
])

function isInProgressStatus(s) {
  return s == null
    || s === ''
    || s === 'in_progress'
    || s === 'inProgress'
    || s === 'InProgress'
}

function isFailedOrCancelled(s) {
  return s === 'failed'
    || s === 'Failed'
    || s === 'cancelled'
    || s === 'Cancelled'
}

/** Progressive To-dos still have open work. */
export function todosStillOpen(todos) {
  const list = Array.isArray(todos) ? todos : []
  if (!list.length) return false
  return list.some((t) => {
    const s = t?.status
    return s === 'pending'
      || s === 'Pending'
      || isInProgressStatus(s)
  })
}

/** A follow-on revise/steer/audit tool is still running in this turn. */
export function hasFollowOnRunning(turnItems, parentItemId) {
  const items = Array.isArray(turnItems) ? turnItems : []
  return items.some((it) => {
    if (it?.type !== 'tool_call') return false
    if (parentItemId && it.id === parentItemId) return false
    if (!FOLLOW_ON_TOOLS.has(it.name)) return false
    return isInProgressStatus(it.status)
  })
}

function isLiveProgressLine(line) {
  return line.includes('调用模型中')
    || line.includes('等待首包')
    || line.includes('生成中')
    || line.includes('评审团自动修订中')
    || line.includes('正在接续审阅队列')
    || line.includes('正在复审')
    || line.includes('正在执行局部修订')
    || /^▶\s/.test(line)
}

function isSettledProgressLine(line) {
  return /^[✓✕]/.test(line)
    || line.includes('审阅队列已全部完成')
    || line.includes('审阅队列已结束')
    || line.includes('审阅结束：')
    || line.includes('请在下方')
    || line.includes('（审校未通过')
    || /(^|\s)⏸\s/.test(line)
}

/**
 * Output tip still shows an in-flight model/pipeline step.
 * Scan from the end: live line → running; settled coda/✓ → not live.
 */
export function looksLikeLiveToolProgress(output) {
  const t = String(output || '')
  if (!t) return false
  const tip = t
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .slice(-12)
  for (let i = tip.length - 1; i >= 0; i -= 1) {
    const line = tip[i]
    if (isLiveProgressLine(line)) return true
    if (isSettledProgressLine(line)) return false
  }
  return false
}

/**
 * Effective status shown on audit_chapter(s) cards.
 * Wire `completed` is overridden while todos, follow-on tools, or live progress
 * in the card output indicate the job is still running.
 */
export function effectiveToolDisplayStatus(item, { turnItems, todos } = {}) {
  if (!item || item.type !== 'tool_call') return item?.status
  if (item.name !== 'audit_chapters' && item.name !== 'audit_chapter') {
    return item.status
  }
  const wire = item.status
  if (isFailedOrCancelled(wire)) return wire === 'Failed' ? 'failed'
    : wire === 'Cancelled' ? 'cancelled'
      : wire
  if (isInProgressStatus(wire)) return 'in_progress'
  if (todosStillOpen(todos)) return 'in_progress'
  if (hasFollowOnRunning(turnItems, item.id)) return 'in_progress'
  if (looksLikeLiveToolProgress(item.output)) return 'in_progress'
  return 'completed'
}
