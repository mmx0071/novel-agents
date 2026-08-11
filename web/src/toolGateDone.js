/**
 * Heuristic: streamed tool output looks like the tool has reached a human gate
 * or terminal coda (ItemCompleted may lag on WS).
 *
 * Must NOT treat mid-queue progress (「审阅队列：第N章」) as done — that froze
 * the audit_chapters card to ✅ while pacing/consistency were still running.
 *
 * @param {string} output
 * @param {{ todosOpen?: boolean }} [opts] — when progressive To-dos are still open,
 *   never mark the parent audit card completed from a coda heuristic.
 */
export function looksLikeToolGateDone(output, opts = {}) {
  if (!output) return false
  if (opts.todosOpen) return false
  const t = String(output)
  // Do NOT treat mid-stream heartbeats like「（模型返回完成，N 字）」as tool done.
  return t.includes('请在下方选项')
    || t.includes('请选择下一步')
    || t.includes('（审校未通过')
    || /(^|\n)⏸\s/.test(t)
    || t.includes('审阅结束：')
    || t.includes('审阅队列已全部完成')
    || t.includes('审阅队列已结束')
    || t.includes('已完成发布')
    || t.includes('流水线完成')
    || t.includes('复审通过，本轮已正常结束')
    || /✓\s+plot_acceptor\b/i.test(t)
}
