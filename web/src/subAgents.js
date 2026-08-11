/** Normalize AgentStatusChanged / list_agents payloads into a stable SubAgent record. */

export function normalizeLifecycle(raw) {
  const v = String(raw || '').toLowerCase()
  if (v === 'spawned') return 'spawned'
  if (v === 'running') return 'running'
  if (v === 'completed') return 'completed'
  if (v === 'failed') return 'failed'
  if (v === 'interrupted') return 'interrupted'
  return v || 'running'
}

export function isAgentActive(lifecycle) {
  const life = normalizeLifecycle(lifecycle)
  return life === 'spawned' || life === 'running'
}

export function lifecycleLabelZh(lifecycle) {
  switch (normalizeLifecycle(lifecycle)) {
    case 'spawned':
      return '已启动'
    case 'running':
      return '运行中'
    case 'completed':
      return '已完成'
    case 'failed':
      return '失败'
    case 'interrupted':
      return '已中断'
    default:
      return String(lifecycle || '')
  }
}

/** Build / merge a SubAgent entry from wire status (camelCase or snake_case). */
export function normalizeSubAgent(raw) {
  if (!raw || typeof raw !== 'object') return null
  const status = raw.status && typeof raw.status === 'object' ? raw.status : raw
  const threadId = status.threadId || status.thread_id || ''
  if (!threadId) return null
  return {
    threadId,
    role: status.role || '',
    agentPath: status.agentPath || status.agent_path || '',
    parentThreadId: status.parentThreadId || status.parent_thread_id || '',
    lifecycle: normalizeLifecycle(status.lifecycle),
    summary: status.summary != null ? String(status.summary) : '',
    updatedAt: Date.now(),
  }
}

/** Upsert into list; keep insertion order; prefer richer summary. */
export function upsertSubAgent(list, incoming) {
  const next = normalizeSubAgent(incoming)
  if (!next) return Array.isArray(list) ? list : []
  const prev = Array.isArray(list) ? list : []
  const idx = prev.findIndex((a) => a.threadId === next.threadId)
  if (idx < 0) return [...prev, next]
  const old = prev[idx]
  const merged = {
    ...old,
    ...next,
    summary: next.summary || old.summary || '',
    role: next.role || old.role || '',
    agentPath: next.agentPath || old.agentPath || '',
    parentThreadId: next.parentThreadId || old.parentThreadId || '',
  }
  const out = [...prev]
  out[idx] = merged
  return out
}

export function mergeSubAgentList(prev, incomingList) {
  let out = Array.isArray(prev) ? [...prev] : []
  for (const raw of incomingList || []) {
    out = upsertSubAgent(out, raw)
  }
  return out
}

/** Recent agents first for rail; active ones pinned to top. */
export function sortSubAgentsForRail(list) {
  const arr = Array.isArray(list) ? [...list] : []
  arr.sort((a, b) => {
    const aa = isAgentActive(a.lifecycle) ? 0 : 1
    const ba = isAgentActive(b.lifecycle) ? 0 : 1
    if (aa !== ba) return aa - ba
    return (b.updatedAt || 0) - (a.updatedAt || 0)
  })
  return arr
}
