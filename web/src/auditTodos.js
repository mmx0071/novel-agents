/**
 * Build progressive To-dos for audit_chapters when the wire TodoList
 * event has not arrived yet (or was cleared). Genre-neutral.
 */

/**
 * @param {{ name?: string, arguments?: object, output?: string }|null} toolItem
 * @returns {Array<{ content: string, status: 'pending'|'in_progress'|'completed' }>}
 */
export function synthesizeAuditTodos(toolItem) {
  if (!toolItem || (toolItem.name !== 'audit_chapters' && toolItem.name !== 'audit_chapter')) {
    return []
  }
  const args = toolItem.arguments && typeof toolItem.arguments === 'object'
    ? toolItem.arguments
    : {}
  let from = Number(args.from ?? args.chapter ?? 0)
  let to = Number(args.to ?? args.chapter ?? 0)
  const output = String(toolItem.output || '')

  // Recover range from queue headers if args missing.
  if (!(from > 0 && to >= from)) {
    const range = output.match(/审阅队列[：:].*?[（(](\d+)\s*\/\s*(\d+)[）)]/)
    if (range) {
      from = 1
      to = Number(range[2]) || 0
    }
  }
  if (!(from > 0 && to >= from) || to - from > 200) return []

  let current = from
  const passed = new Set()
  const failed = new Set()

  for (const m of output.matchAll(/审阅队列[：:]\s*第(\d+)章/g)) {
    current = Number(m[1]) || current
  }
  for (const m of output.matchAll(/##\s*审阅队列.+?[（(](\d+)\s*\/\s*(\d+)[）)]/g)) {
    current = Number(m[1]) || current
  }
  for (const m of output.matchAll(/▶\s*第(\d+)章/g)) {
    current = Number(m[1]) || current
  }
  for (const m of output.matchAll(/✓\s*第(\d+)章/g)) {
    passed.add(Number(m[1]))
  }
  for (const m of output.matchAll(/第(\d+)章未通过/g)) {
    failed.add(Number(m[1]))
  }

  const todos = []
  for (let ch = from; ch <= to; ch += 1) {
    let status = 'pending'
    let note = '待审'
    if (ch < current || (passed.has(ch) && ch !== current)) {
      status = 'completed'
      note = failed.has(ch) ? '未通过' : '通过'
    } else if (ch === current) {
      status = 'in_progress'
      note = failed.has(ch) ? '未通过 · 待处理' : '进行中'
    }
    todos.push({ content: `审校第${ch}章（${note}）`, status })
  }
  return todos
}

/** Prefer wire todos; else synthesize from the audit host tool in a worked segment. */
export function resolveWorkTodos(wireTodos, workedItems) {
  const wire = Array.isArray(wireTodos) ? wireTodos : []
  if (wire.length) return wire
  const items = Array.isArray(workedItems) ? workedItems : []
  const host = items.find(
    (it) => it?.type === 'tool_call'
      && (it.name === 'audit_chapters' || it.name === 'audit_chapter'),
  )
  return host ? synthesizeAuditTodos(host) : []
}
