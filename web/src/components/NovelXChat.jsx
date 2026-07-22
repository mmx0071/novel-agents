import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import SkillPopup from './SkillPopup'
import TodoList from './TodoList'
import TurnTimeline from './TurnTimeline'
import {
  chapterForBulkTool,
  formatBulkReadSummary,
  isBulkContextTool,
  parseXmlToolCalls,
  readerTabForBulkTool,
  stripToolMarkup,
} from './toolMarkup'

const API = '/api'

function wsUrl(path) {
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}${path}`
}

function normalizeItem(raw) {
  if (!raw) return null
  const type = raw.type
  const status = normalizeWireStatus(raw.status)
  return { ...raw, type, status, _key: raw.id || `${type}-${Math.random()}` }
}

/** Wire may send camelCase (inProgress) or PascalCase (Completed). */
function normalizeWireStatus(s) {
  if (s == null || s === '') return s
  const v = String(s)
  if (v === 'completed' || v === 'Completed') return 'completed'
  if (v === 'failed' || v === 'Failed') return 'failed'
  if (v === 'cancelled' || v === 'Cancelled') return 'cancelled'
  if (v === 'in_progress' || v === 'inProgress' || v === 'InProgress') return 'in_progress'
  return v
}

function isTerminalStatus(s) {
  const st = normalizeWireStatus(s)
  return st === 'completed' || st === 'failed' || st === 'cancelled'
}

/** When the turn is waiting on the user, stop all spinning tool / agent cards. */
function finishOpenItems(items) {
  return (items || []).map((it) => {
    if (!it) return it
    if (it.type === 'tool_call' || it.type === 'agent_message' || it.type === 'skill_load'
      || it.type === 'pipeline_step' || it.type === 'reasoning') {
      if (isTerminalStatus(it.status)) return it
      return { ...it, status: 'completed' }
    }
    return it
  })
}

/** Audit/revise often stream a gate line before ItemCompleted arrives. */
function looksLikeToolGateDone(output) {
  if (!output) return false
  const t = String(output)
  // Do NOT treat mid-stream heartbeats like「（模型返回完成，N 字）」as tool done —
  // that froze the card until the final assistant summary appeared.
  return t.includes('请在下方选项')
    || t.includes('请选择下一步')
    || t.includes('（审校未通过')
    || t.includes('\n⏸ ')
    || t.includes('审阅队列')
    || t.includes('已完成发布')
    || t.includes('流水线完成')
    || t.includes('复审通过，本轮已正常结束')
    || /✓\s+plot_acceptor\b/i.test(t)
}

function isAuditToolName(name) {
  return name === 'audit_chapter' || name === 'audit_chapters' || name === 'steer_run'
}

/** Tools that write project files — refresh reader when they finish (don't wait for turn_complete). */
const PREVIEW_MUTATING_TOOLS = new Set([
  'continue_writing',
  'revise_chapter',
  'design_plot',
  'update_plot',
  'design_entity',
  'delete_entity',
  'design_arc_outline',
  'design_master_outline',
  'upsert_setting',
  'supplement_setting',
  'sync_volume',
  'create_novel',
  'init_novel',
  'steer_run',
  'apply_draft_patch',
  'audit_volume',
  'audit_chapter',
  'audit_chapters',
  'audit_setting',
  'lock_brief',
  'confirm_setup',
])

/** Pipeline progress lines that mean readable files likely changed mid-tool. */
function looksLikePreviewStepDone(delta) {
  return !!readerHintFromPipelineDelta(delta)
}

/** Map pipeline progress → reader surface. Only writer/planner drive live reader sync. */
function readerHintFromPipelineDelta(delta) {
  if (!delta) return null
  const t = String(delta)
  // 章纲：仅 chapter_planner 落盘后刷新（审校/润色流式不碰阅读区）
  if (/✓\s+chapter_planner\b/i.test(t)) {
    return { focusLatestChapter: true, readerTab: 'outline' }
  }
  // 正文：仅 writer 间歇落盘 / writer 步骤完成。literary / 审校 / 专改只在工具卡流式。
  if (/正文已写入/i.test(t) || /↻\s*draft/i.test(t) || /✓\s+writer\b/i.test(t)) {
    return { focusLatestChapter: true, readerTab: 'draft' }
  }
  return null
}

function finishAuditTools(items) {
  return (items || []).map((it) => {
    if (!it || it.type !== 'tool_call') return it
    if (!isAuditToolName(it.name)) return it
    if (isTerminalStatus(it.status)) return it
    return { ...it, status: 'completed' }
  })
}

/** Normalize server open_gate / turn.approval into ApprovalOptions props. */
function normalizeServerGate(gate) {
  if (!gate || typeof gate !== 'object') return null
  const options = Array.isArray(gate.options) ? gate.options : []
  if (!options.length && !gate.prompt) return null
  return {
    prompt: gate.prompt || '',
    options: options.map((o) => ({
      id: o.id || o.label,
      label: o.label || o.id || '',
    })),
  }
}

/** Prefer approval already on the last turn (from gates.yaml via server). */
function approvalFromTurns(turns) {
  if (!Array.isArray(turns) || !turns.length) return null
  const last = turns[turns.length - 1]
  return normalizeServerGate(last?.approval)
}

function queueToTodos(q) {
  if (!q?.chapters?.length) return []
  const results = q.results || []
  const finished = (q.index ?? 0) >= q.chapters.length
  return results.map((r, i) => {
    let status = 'pending'
    let note = '待审'
    if (finished || i < (q.index ?? 0)) {
      status = 'completed'
      note = r.status === 'skipped' ? '已跳过'
        : r.status === 'revised' ? '已修订'
          : r.status === 'failed' ? '未通过' : '通过'
    } else if (i === (q.index ?? 0)) {
      status = 'in_progress'
      note = r.status === 'failed' ? '未通过 · 待处理' : '进行中'
    }
    return { content: `审校第${r.chapter}章（${note}）`, status }
  })
}

function newLocalId(prefix) {
  return `${prefix}_${Math.random().toString(16).slice(2, 10)}`
}

/** Optimistic client turns use this prefix so they never collide with server `turn_<uuid>`. */
function newOptimisticTurnId() {
  return `local_${Math.random().toString(16).slice(2, 10)}`
}

function isOptimisticTurnId(id) {
  return String(id || '').startsWith('local_')
}

function chatCacheKey(project) {
  return `novelx-chat-v1:${project || '__draft__'}`
}

function loadChatCache(project) {
  try {
    const raw = localStorage.getItem(chatCacheKey(project))
    if (!raw) return null
    const data = JSON.parse(raw)
    if (!data || typeof data !== 'object') return null
    return {
      threadId: data.threadId || '',
      turns: Array.isArray(data.turns) ? data.turns : [],
    }
  } catch {
    return null
  }
}

function saveChatCache(project, threadId, turns) {
  try {
    localStorage.setItem(
      chatCacheKey(project),
      JSON.stringify({
        threadId: threadId || '',
        turns: (turns || []).slice(-80),
        savedAt: Date.now(),
      }),
    )
  } catch {
    /* quota */
  }
}

function clearChatCache(project) {
  try {
    localStorage.removeItem(chatCacheKey(project))
  } catch {
    /* ignore */
  }
}

function messagesToTurns(messages) {
  if (!Array.isArray(messages) || !messages.length) return []
  const turns = []
  let buf = null
  const flush = () => {
    if (buf) turns.push(buf)
    buf = null
  }
  const toolOutById = new Map()
  messages.forEach((m) => {
    if (m?.role === 'tool' && m.tool_call_id) {
      toolOutById.set(m.tool_call_id, m.content || '')
    }
  })
  messages.forEach((m, i) => {
    if (!m || m.role === 'tool') return
    if (m.role === 'user') {
      flush()
      buf = {
        id: `restored-${i}`,
        status: 'complete',
        items: [{ type: 'user_message', id: `u-${i}`, text: m.content || '' }],
      }
    } else if (m.role === 'assistant') {
      const text = (m.content || '').trim()
      const calls = Array.isArray(m.tool_calls) ? m.tool_calls : []
      if (!text && !calls.length) return
      if (!buf) {
        buf = { id: `restored-${i}`, status: 'complete', items: [] }
      }
      if (text) {
        buf.items.push({
          type: 'agent_message',
          id: `a-${i}`,
          text,
          status: 'completed',
        })
      }
      calls.forEach((tc, j) => {
        const fn = tc?.function || tc
        const name = fn?.name || tc?.name || 'tool'
        let args = {}
        try {
          args = typeof fn?.arguments === 'string'
            ? JSON.parse(fn.arguments || '{}')
            : (fn?.arguments || tc?.arguments || {})
        } catch {
          args = { raw: fn?.arguments }
        }
        const id = tc?.id || `tc-${i}-${j}`
        const rawOut = toolOutById.get(id) || ''
        const compact = formatBulkReadSummary(name, args, rawOut)
        buf.items.push({
          type: 'tool_call',
          id,
          name,
          arguments: args,
          output: isBulkContextTool(name)
            ? (compact || formatBulkReadSummary(name, args, '') || '正在查阅…')
            : rawOut,
          status: 'completed',
        })
      })
    }
  })
  flush()
  return turns
}

/** Prefer short UI line for bulk-read tools (full text stays server-side for the model). */
function compactToolItem(item, project) {
  if (!item || item.type !== 'tool_call' || !isBulkContextTool(item.name)) return item
  const summary = formatBulkReadSummary(item.name, item.arguments, item.output, project)
  if (!summary) return item
  return { ...item, output: summary }
}

/**
 * Codex-style NovelX chat:
 * - Turn timeline with separators
 * - Tool / Skill cells (Ran / Loaded)
 * - StartTurn via WebSocket Op (same connection as events)
 */
export default function NovelXChat({ project, onPreviewRefresh, sendRef, onSetupGateChange }) {
  const [threadId, setThreadId] = useState('')
  const [turns, setTurns] = useState([])
  const [input, setInput] = useState('')
  const [loading, setLoading] = useState(false)
  const [wsState, setWsState] = useState('idle')
  const [skills, setSkills] = useState([])
  const [skillOpen, setSkillOpen] = useState(false)
  const [skillQuery, setSkillQuery] = useState('')
  const [otherText, setOtherText] = useState('')
  const [showOther, setShowOther] = useState(false)
  const [activity, setActivity] = useState('')
  const [todos, setTodos] = useState([])
  const [setupGateOpen, setSetupGateOpen] = useState(false)
  const wsRef = useRef(null)
  const endRef = useRef(null)
  const activeTurnRef = useRef('')
  const readyRef = useRef(false)
  const loadingRef = useRef(false)
  loadingRef.current = loading
  const previewRefreshAtRef = useRef(0)
  const previewRefreshTimerRef = useRef(null)

  useEffect(() => () => {
    if (previewRefreshTimerRef.current) clearTimeout(previewRefreshTimerRef.current)
  }, [])

  /** Throttled mid-turn preview sync; pipeline ✓ can jump reader to 章纲/正文. */
  const syncPreview = useCallback((opts = {}) => {
    if (!project || !onPreviewRefresh) return
    const keepSelection = opts.keepSelection !== false
    const minGap = opts.immediate ? 0 : 1200
    const now = Date.now()
    const wait = Math.max(0, minGap - (now - previewRefreshAtRef.current))
    const run = () => {
      previewRefreshAtRef.current = Date.now()
      previewRefreshTimerRef.current = null
      onPreviewRefresh(project, {
        keepSelection,
        focusLatestChapter: !!opts.focusLatestChapter,
        readerTab: opts.readerTab || undefined,
        chapter: opts.chapter || undefined,
      })
    }
    if (previewRefreshTimerRef.current) {
      clearTimeout(previewRefreshTimerRef.current)
      previewRefreshTimerRef.current = null
    }
    if (wait === 0) run()
    else previewRefreshTimerRef.current = setTimeout(run, wait)
  }, [onPreviewRefresh, project])

  const upsertTurn = useCallback((turnId, mutator) => {
    setTurns((prev) => {
      const idx = prev.findIndex((t) => t.id === turnId)
      if (idx < 0) {
        const created = mutator({
          id: turnId,
          status: 'running',
          items: [],
          approval: null,
        })
        return [...prev, created]
      }
      const next = [...prev]
      next[idx] = mutator({ ...next[idx], items: [...(next[idx].items || [])] })
      return next
    })
  }, [])

  const handleEvent = useCallback(
    (ev) => {
      if (!ev || !ev.type) return
      switch (ev.type) {
        case 'session_configured':
          setThreadId(ev.thread_id)
          break
        case 'turn_started': {
          const serverTurn = ev.turn_id
          setLoading(true)
          setActivity('working…')
          // Merge optimistic `local_*` into the server turn. Never drop an existing
          // approval on the server turn (RequestUserInput may have already arrived).
          setTurns((prev) => {
            const localIdx = prev.findIndex(
              (t) => t.status === 'running' && isOptimisticTurnId(t.id),
            )
            const serverIdx = prev.findIndex((t) => t.id === serverTurn)
            activeTurnRef.current = serverTurn

            if (localIdx >= 0 && serverIdx >= 0 && localIdx !== serverIdx) {
              const local = prev[localIdx]
              const server = prev[serverIdx]
              const userItems = (local.items || []).filter((i) => i.type === 'user_message')
              const serverItems = server.items || []
              const merged = {
                ...server,
                status: 'running',
                approval: server.approval || local.approval || null,
                items: [
                  ...userItems,
                  ...serverItems.filter((i) => i.type !== 'user_message'),
                ],
              }
              return prev
                .filter((_, i) => i !== localIdx)
                .map((t) => (t.id === serverTurn ? merged : t))
            }

            if (localIdx >= 0 && prev[localIdx].id !== serverTurn) {
              const next = [...prev]
              next[localIdx] = {
                ...next[localIdx],
                id: serverTurn,
                status: 'running',
                approval: next[localIdx].approval ?? null,
              }
              return next
            }

            if (serverIdx >= 0) return prev
            return [...prev, { id: serverTurn, status: 'running', items: [], approval: null }]
          })
          break
        }
        case 'turn_complete':
          // Force-finish spinning tools/agents across turns (late deltas / id mismatch).
          setTurns((prev) => prev.map((t) => {
            const matched = t.id === ev.turn_id
            return {
              ...t,
              status: matched
                ? 'complete'
                : (t.status === 'running' ? 'complete' : t.status),
              approval: t.approval,
              items: finishOpenItems(t.items),
            }
          }))
          setLoading(false)
          setActivity('')
          syncPreview({ keepSelection: false, immediate: true })
          break
        case 'turn_aborted':
          upsertTurn(ev.turn_id, (t) => ({
            ...t,
            status: 'aborted',
            items: finishOpenItems(t.items),
          }))
          setLoading(false)
          setActivity('interrupted')
          break
        case 'item_started':
        case 'item_completed': {
          const item = normalizeItem(ev.item)
          if (!item) break
          if (item.type === 'tool_call') {
            const compact = compactToolItem(item, project)
            const label = isBulkContextTool(item.name) && compact.output
              ? compact.output
              : item.name
            setActivity(
              item.status === 'completed'
                ? (isBulkContextTool(item.name) ? label : `✓ ${item.name}`)
                : (isBulkContextTool(item.name) ? `Reading…` : `Running ${item.name}…`),
            )
          } else if (item.type === 'skill_load') {
            setActivity(
              item.status === 'completed' ? `Loaded $${item.name}` : `Loading $${item.name}…`,
            )
          }
          // Skip duplicate user_message if we already optimistic-inserted
          if (item.type === 'user_message') {
            upsertTurn(ev.turn_id, (t) => {
              if (t.items.some((x) => x.type === 'user_message' && x.text === item.text)) return t
              return { ...t, items: [...t.items, item] }
            })
            break
          }
          upsertTurn(ev.turn_id, (t) => {
            let items = [...t.items]
            // Tool / skill start ⇒ stop blinking carets on prior agent bubbles.
            if (
              ev.type === 'item_started'
              && (item.type === 'tool_call' || item.type === 'skill_load')
            ) {
              items = items.map((x) => (
                x.type === 'agent_message' && x.status === 'in_progress'
                  ? { ...x, status: 'completed' }
                  : x
              ))
            }
            const stored = item.type === 'tool_call' ? compactToolItem(item, project) : item
            const i = items.findIndex((x) => x.id === stored.id)
            if (i >= 0) {
              const prev = items[i]
              // Gate/steer mirrors ▶ / （调用模型…） into the agent bubble; ItemCompleted
              // may carry a short closing — never shrink away the streamed process.
              if (
                stored.type === 'agent_message'
                && prev.type === 'agent_message'
                && String(prev.text || '').length > String(stored.text || '').length
              ) {
                items[i] = {
                  ...prev,
                  ...stored,
                  text: prev.text,
                  status: stored.status || prev.status,
                }
              } else {
                items[i] = { ...prev, ...stored }
              }
            } else items.push(stored)
            // Promote client-recovered placeholder with same tool name
            if (item.type === 'tool_call') {
              for (let j = items.length - 1; j >= 0; j -= 1) {
                if (items[j]._clientRecovered && items[j].name === item.name && items[j].id !== item.id) {
                  items.splice(j, 1)
                }
              }
              // ItemCompleted for audit may miss id match — finish all audit spinners by name.
              if (
                ev.type === 'item_completed'
                && isTerminalStatus(item.status)
                && isAuditToolName(item.name)
              ) {
                items = finishAuditTools(items)
              }
            }
            return { ...t, items }
          })
          if (
            ev.type === 'item_completed'
            && item.type === 'tool_call'
            && isTerminalStatus(item.status)
          ) {
            if (PREVIEW_MUTATING_TOOLS.has(item.name)) {
              const writing = item.name === 'continue_writing' || item.name === 'revise_chapter'
              syncPreview({
                keepSelection: !writing,
                focusLatestChapter: writing,
                readerTab: writing ? 'draft' : undefined,
                immediate: writing,
              })
            } else if (isBulkContextTool(item.name)) {
              const tab = readerTabForBulkTool(item.name, item.arguments)
              const ch = chapterForBulkTool(item.name, item.arguments)
              syncPreview({
                keepSelection: ch == null,
                chapter: ch || undefined,
                focusLatestChapter: false,
                readerTab: tab || undefined,
                immediate: true,
              })
            }
          }
          break
        }
        case 'agent_message_content_delta': {
          const delta = stripToolMarkup(ev.delta || '')
          const recovered = parseXmlToolCalls(ev.delta || '')
          if (recovered.length) {
            upsertTurn(ev.turn_id, (t) => {
              // Don't spawn ghost Running cards after the turn already paused/finished.
              if (t.status === 'awaiting' || t.status === 'complete' || t.status === 'aborted') {
                return t
              }
              let items = t.items.map((x) => (
                x.type === 'agent_message' && x.status === 'in_progress'
                  ? { ...x, status: 'completed' }
                  : x
              ))
              for (const tc of recovered) {
                if (!items.some((x) => x.name === tc.name && (
                  x._clientRecovered
                  || JSON.stringify(x.arguments) === JSON.stringify(tc.arguments)
                ))) {
                  items = [...items, tc]
                }
              }
              return { ...t, items }
            })
          }
          if (!delta) break
          upsertTurn(ev.turn_id, (t) => {
            const turnDone = t.status === 'awaiting' || t.status === 'complete' || t.status === 'aborted'
            const items = [...t.items]
            const i = items.findIndex((x) => x.id === ev.item_id)
            if (i >= 0) {
              const prevStatus = items[i].status
              items[i] = {
                ...items[i],
                type: 'agent_message',
                text: stripToolMarkup(`${items[i].text || ''}${delta}`),
                // After tool rounds the bubble may be marked completed; reopen while turn is live.
                // Once paused/complete, never revive the caret from late deltas.
                status: turnDone
                  ? (isTerminalStatus(prevStatus) ? prevStatus : 'completed')
                  : 'in_progress',
              }
            } else if (!turnDone) {
              items.push({
                type: 'agent_message',
                id: ev.item_id,
                text: delta,
                status: 'in_progress',
              })
            }
            return { ...t, items }
          })
          break
        }
        case 'tool_call_output_delta':
          upsertTurn(ev.turn_id, (t) => {
            const items = [...t.items]
            const i = items.findIndex((x) => x.id === ev.item_id)
            if (i >= 0) {
              const prevStatus = items[i].status
              const bulk = isBulkContextTool(items[i].name)
              // Bulk-read: replace with short line (never append multi-KB context into React state).
              const output = bulk
                ? (formatBulkReadSummary(items[i].name, items[i].arguments, ev.delta, project)
                  || formatBulkReadSummary(items[i].name, items[i].arguments, '', project)
                  || String(ev.delta || '').slice(0, 200))
                : `${items[i].output || ''}${ev.delta || ''}`
              // Pipeline may finish (report / ⏸ gate) while ItemCompleted is delayed by WS backpressure.
              const doneHint = !bulk && looksLikeToolGateDone(output)
              items[i] = {
                ...items[i],
                output,
                status: isTerminalStatus(prevStatus)
                  ? prevStatus
                  : (doneHint ? 'completed' : (prevStatus || 'in_progress')),
              }
            }
            return { ...t, items }
          })
          {
            const d = String(ev.delta || '')
            const step = d.match(/▶\s*([a-zA-Z0-9_]+)/)
            const done = d.match(/✓\s*([a-zA-Z0-9_]+)/)
            const wait = /调用模型|生成中|等待首包|流式生成中/.test(d)
            if (step) setActivity(`Running ${step[1]}…`)
            else if (done) setActivity(`✓ ${done[1]}`)
            else if (wait) setActivity((prev) => prev || 'streaming…')
            else setActivity((prev) => (prev && prev.startsWith('Running') ? prev : 'streaming…'))
          }
          // Mid-pipeline: chapter/card files land before the whole tool finishes.
          // Jump reader to latest chapter + 章纲/正文 so creation is visible live.
          const hint = readerHintFromPipelineDelta(ev.delta)
          if (hint) {
            const liveFlush = /正文已写入|↻\s*draft/i.test(String(ev.delta || ''))
            syncPreview({
              keepSelection: false,
              focusLatestChapter: !!hint.focusLatestChapter,
              readerTab: hint.readerTab,
              // Step ✓: refresh ASAP; intermittent draft flush: throttle (~1.2s).
              immediate: !liveFlush,
            })
          }
          break
        case 'todo_updated': {
          const list = Array.isArray(ev.todos) ? ev.todos : []
          setTodos(list.map((t) => ({
            content: t.content || '',
            status: t.status === 'in_progress' || t.status === 'inProgress'
              ? 'in_progress'
              : t.status === 'completed' || t.status === 'Completed'
                ? 'completed'
                : 'pending',
          })))
          // Only dismiss gates when the queue is fully done — NOT on empty todos
          // from a single-chapter audit (that was wiping RequestUserInput options).
          const queueDone = list.length > 0
            && list.every((t) => t.status === 'completed' || t.status === 'Completed')
          if (queueDone) {
            setTurns((prev) => prev.map((t) => ({
              ...t,
              approval: null,
              items: finishAuditTools(finishOpenItems(t.items)),
              status: (t.status === 'running' || t.status === 'awaiting') ? 'complete' : t.status,
            })))
            setLoading(false)
          }
          break
        }
        case 'chat_history_reset': {
          const summary = ev.summary || '对话历史已清理。'
          const keepId = ev.keep_turn_id || ''
          setTurns((prev) => {
            let next
            if (keepId) {
              // New-chapter start: drop prior turns, keep the in-flight write turn.
              const kept = prev.filter((t) => (
                t.id === keepId || isOptimisticTurnId(t.id)
              ))
              next = kept.length
                ? kept.map((t) => (
                  t.id === keepId || isOptimisticTurnId(t.id)
                    ? { ...t, approval: null, status: t.status === 'complete' ? 'complete' : 'running' }
                    : t
                ))
                : [{
                  id: keepId,
                  status: 'running',
                  approval: null,
                  items: [{
                    id: newLocalId('item_reset'),
                    type: 'agent_message',
                    text: summary,
                    status: 'completed',
                    _key: newLocalId('key'),
                  }],
                }]
            } else {
              next = [{
                id: newLocalId('turn_reset'),
                status: 'complete',
                approval: null,
                items: [{
                  id: newLocalId('item_reset'),
                  type: 'agent_message',
                  text: summary,
                  status: 'completed',
                  _key: newLocalId('key'),
                }],
              }]
            }
            const tid = ev.thread_id || ''
            if (tid) saveChatCache(project, tid, next)
            return next
          })
          setActivity(keepId ? '新章开写 · 已清理旧对话' : '历史已清理')
          break
        }
        case 'request_user_input': {
          const opts = ev.options || []
          // Empty options = queue finished / dismiss all gate cards.
          if (!opts.length) {
            setSetupGateOpen(false)
            setTurns((prev) => prev.map((t) => ({
              ...t,
              approval: null,
              status: t.status === 'awaiting' || t.status === 'running' ? 'complete' : t.status,
              items: finishAuditTools(finishOpenItems(t.items)),
            })))
            setLoading(false)
            setActivity(ev.prompt || '')
            break
          }
          const isSetupGate = opts.some((o) => o.id === 'sc_approve' || o.id === 'sc_revise'
            || o.label === '确认定稿' || o.label === '修改再生成')
          if (isSetupGate) setSetupGateOpen(true)
          // Attach gate to matching turn; if turn_id missed, prefer last turn that has content
          // (never spawn an empty approval-only turn that makes the chat look wiped).
          setTurns((prev) => {
            const gate = { prompt: ev.prompt, options: opts }
            const hasMatch = prev.some((t) => t.id === ev.turn_id)
            const lastWithItems = [...prev].reverse().find((t) => (t.items || []).length > 0)
            const targetId = hasMatch
              ? ev.turn_id
              : (lastWithItems?.id || prev[prev.length - 1]?.id || ev.turn_id)
            if (!prev.length) {
              return [{
                id: targetId,
                status: 'awaiting',
                approval: gate,
                items: [{
                  id: `gate-${Date.now()}`,
                  type: 'agent_message',
                  text: ev.prompt || '请选择下一步',
                  status: 'completed',
                }],
              }]
            }
            return prev.map((t) => {
              const matched = t.id === targetId
              const items = finishAuditTools(finishOpenItems(t.items))
              // If matched turn is empty, keep a prompt line so dialogue does not vanish.
              const nextItems = matched && !(items || []).length
                ? [{
                  id: `gate-${Date.now()}`,
                  type: 'agent_message',
                  text: ev.prompt || '请选择下一步',
                  status: 'completed',
                }]
                : items
              return {
                ...t,
                status: matched
                  ? (t.status === 'complete' ? 'complete' : 'awaiting')
                  : (t.status === 'running' ? 'awaiting' : t.status),
                approval: matched ? gate : null,
                items: nextItems,
              }
            })
          })
          setLoading(false)
          setActivity('waiting for input')
          // Gates often follow a write (plot/chapter); sync reader before user decides.
          syncPreview({ keepSelection: true })
          break
        }
        case 'error':
          setActivity(ev.message || 'error')
          setLoading(false)
          break
        default:
          break
      }
    },
    [project, syncPreview, upsertTurn],
  )

  const connectWs = useCallback(
    (tid) => new Promise((resolve, reject) => {
      if (!tid) {
        reject(new Error('no thread'))
        return
      }
      if (wsRef.current) {
        try { wsRef.current.close() } catch { /* ignore */ }
      }
      setWsState('connecting')
      const ws = new WebSocket(wsUrl(`/ws/thread/${tid}`))
      wsRef.current = ws
      ws.onopen = () => {
        setWsState('open')
        readyRef.current = true
        resolve(ws)
      }
      ws.onerror = () => {
        setWsState('error')
        reject(new Error('ws error'))
      }
      ws.onclose = () => {
        setWsState('closed')
        readyRef.current = false
      }
      // Drain many events per frame. Critical control events bypass the queue
      // so ItemCompleted / RequestUserInput are never stuck behind token spam.
      const pending = []
      let raf = 0
      const BATCH = 128
      const CRITICAL = new Set([
        'item_completed',
        'item_started',
        'request_user_input',
        'todo_updated',
        'chat_history_reset',
        'turn_complete',
        'turn_aborted',
        'turn_started',
        'error',
      ])
      const flush = () => {
        raf = 0
        if (!pending.length) return
        const n = Math.min(BATCH, pending.length)
        let i = 0
        while (i < n) {
          const ev = pending[i]
          if (ev?.type === 'tool_call_output_delta') {
            let merged = { ...ev, delta: ev.delta || '' }
            i += 1
            while (
              i < n
              && pending[i]?.type === 'tool_call_output_delta'
              && pending[i].item_id === merged.item_id
              && pending[i].turn_id === merged.turn_id
            ) {
              merged = {
                ...merged,
                delta: `${merged.delta || ''}${pending[i].delta || ''}`,
              }
              i += 1
            }
            handleEvent(merged)
          } else {
            handleEvent(ev)
            i += 1
          }
        }
        pending.splice(0, n)
        if (pending.length) raf = requestAnimationFrame(flush)
      }
      ws.onmessage = (msg) => {
        try {
          const ev = JSON.parse(msg.data)
          if (CRITICAL.has(ev?.type)) {
            handleEvent(ev)
            return
          }
          pending.push(ev)
          if (!raf) raf = requestAnimationFrame(flush)
        } catch {
          /* ignore */
        }
      }
    }),
    [handleEvent],
  )

  useEffect(() => {
    let cancelled = false
    ;(async () => {
      const skillsRes = await fetch(`${API}/skills/list`)
        .then((r) => r.json())
        .catch(() => ({ skills: [] }))
      if (!cancelled) setSkills(skillsRes.skills || [])

      // Local cache first (survives refresh even if server restarted mid-turn).
      const cached = loadChatCache(project)
      if (cached?.turns?.length && !cancelled) {
        setTurns(cached.turns)
      }

      const started = await fetch(`${API}/thread/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ project: project || null }),
      }).then((r) => r.json()).catch(() => ({}))
      if (cancelled) return
      const tid = started.threadId || started.thread_id
      if (tid) {
        setThreadId(tid)
        const serverTurns = Array.isArray(started.turns) ? started.turns : []
        const fromMessages = Array.isArray(started.messages) && started.messages.length
          ? messagesToTurns(started.messages)
          : []
        let nextTurns = null
        if (serverTurns.length && fromMessages.length > serverTurns.length) {
          // ui_turns lagged behind (e.g. revise finished but client overwrote with old audit card).
          nextTurns = fromMessages
        } else if (serverTurns.length) {
          nextTurns = serverTurns
        } else if (fromMessages.length && !cached?.turns?.length) {
          nextTurns = fromMessages
        } else if (cached?.turns?.length) {
          nextTurns = cached.turns
        }
        const hasQueue = !!(started.pending_audit_queue?.chapters?.length)
        setTodos(hasQueue ? queueToTodos(started.pending_audit_queue) : [])
        // gates.yaml via server open_gate / turn.approval — never invent buttons from Chinese scrape.
        const restoredGate = normalizeServerGate(started.open_gate)
          || approvalFromTurns(serverTurns)
          || approvalFromTurns(nextTurns)
        const hasHumanGate = !!(
          restoredGate
          || started.pending_audit
          || started.pending_volume_sync
          || started.pending_setup
          || started.pending_chapter_next
          || started.pending_volume_audit
        )
        setSetupGateOpen(!!started.pending_setup)
        if (nextTurns?.length && restoredGate) {
          nextTurns = nextTurns.map((t, idx) => {
            const last = idx === nextTurns.length - 1
            return {
              ...t,
              status: last ? 'awaiting' : (t.status === 'running' ? 'awaiting' : t.status),
              items: finishAuditTools(finishOpenItems(t.items)),
              approval: last ? restoredGate : null,
            }
          })
          setLoading(false)
          setActivity('waiting for input')
        } else if (nextTurns?.length && !hasHumanGate) {
          // Drop stale approval cards; also clear「调用模型中」if the turn already ended.
          const turnActive = started.turn_active === true
          nextTurns = nextTurns.map((t) => {
            const stuckRunning = !turnActive && t.status === 'running'
            return {
              ...t,
              approval: null,
              status: stuckRunning ? 'complete' : t.status,
              items: stuckRunning
                ? finishOpenItems(finishAuditTools(t.items || []))
                : finishAuditTools(t.items || []),
            }
          })
          if (!turnActive) {
            setLoading(false)
            setActivity('')
          }
        }
        if (nextTurns?.length) {
          setTurns(nextTurns)
          saveChatCache(project, tid, nextTurns)
          // Never push stale Running timelines back while a gate is open.
          if (!serverTurns.length && cached?.turns?.length && !hasHumanGate) {
            fetch(`${API}/thread/turns`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ thread_id: tid, turns: nextTurns }),
            }).catch(() => {})
          }
        }
        try {
          await connectWs(tid)
        } catch {
          /* HTTP fallback still available */
        }
      }
    })()
    return () => {
      cancelled = true
      wsRef.current?.close()
    }
  }, [project, connectWs])

  // Persist chat timeline locally + to disk-backed API (debounced).
  // Never POST while a turn is loading — that revived Running audit cards after the server finished.
  useEffect(() => {
    if (!threadId || !turns.length) return
    saveChatCache(project, threadId, turns)
    if (loadingRef.current) return undefined
    const t = setTimeout(() => {
      if (loadingRef.current) return
      fetch(`${API}/thread/turns`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId, turns }),
      }).catch(() => {})
    }, 600)
    return () => clearTimeout(t)
  }, [project, threadId, turns, loading])

  // Keep the latest turn / tool progress in view (outer chat scroller).
  const turnsSig = turns.map((t) => (
    `${t.id}:${t.status}:${(t.items || []).map((it) => `${it.id}:${it.status}:${String(it.output || '').length}`).join('|')}`
  )).join(';')
  useEffect(() => {
    endRef.current?.scrollIntoView({ behavior: 'smooth', block: 'end' })
  }, [turnsSig, activity])

  const detectSkillTrigger = (value, caret) => {
    const before = value.slice(0, caret)
    const m = before.match(/\$([a-zA-Z0-9_-]*)$/)
    if (m) {
      setSkillOpen(true)
      setSkillQuery(m[1] || '')
    } else {
      setSkillOpen(false)
      setSkillQuery('')
    }
  }

  const sendText = async (text) => {
    const msg = (text || '').trim()
    if (!msg || !threadId) return
    // Prevent double-click / concurrent start_turn (was re-triggering audit queues).
    if (loadingRef.current) return
    loadingRef.current = true
    setLoading(true)
    setShowOther(false)
    setOtherText('')
    setActivity('sending…')
    // Choosing a next step dismisses any pending approval cards.
    setSetupGateOpen(false)
    setTurns((prev) => prev.map((t) => (t.approval ? { ...t, approval: null } : t)))

    // Optimistic local turn (Codex shows user input immediately)
    const localTurnId = newOptimisticTurnId()
    activeTurnRef.current = localTurnId
    upsertTurn(localTurnId, (t) => ({
      ...t,
      id: localTurnId,
      status: 'running',
      approval: null,
      items: [{
        type: 'user_message',
        id: newLocalId('item'),
        text: msg,
      }],
    }))

    const op = {
      op: 'start_turn',
      thread_id: threadId,
      text: msg,
      skills: [],
    }

    const ws = wsRef.current
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(op))
      return
    }

    // Fallback: reconnect then HTTP
    try {
      await connectWs(threadId)
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify(op))
        return
      }
    } catch {
      /* fall through */
    }
    await fetch(`${API}/turn/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        thread_id: threadId,
        text: msg,
        skills: [],
      }),
    }).catch(() => {})
  }

  const handleSend = () => {
    const msg = input
    setInput('')
    setSkillOpen(false)
    sendText(msg)
  }

  const handlePickOption = (opt) => {
    // Prefer option id (gates.yaml); labels are aliases for typed replies only.
    sendText(String(opt.id || opt.label || ''))
  }

  const handleOtherSubmit = () => {
    if (!otherText.trim()) return
    sendText(otherText.trim())
  }

  // Parent (reader 总纲栏) can send gate tokens: sc_approve / sc_revise.
  useEffect(() => {
    if (!sendRef) return undefined
    sendRef.current = (text) => {
      if (!text) return
      sendText(String(text))
    }
    return () => {
      if (sendRef.current) sendRef.current = null
    }
  }, [sendRef, sendText])

  // Mirror setup gate to parent so reader「确认定稿」stays in sync with chat ApprovalOptions.
  useEffect(() => {
    const fromTurns = turns.some((t) => {
      const opts = t.approval?.options || []
      return opts.some((o) => o.id === 'sc_approve' || o.id === 'sc_revise'
        || o.label === '确认定稿' || o.label === '修改再生成')
    })
    const open = fromTurns || setupGateOpen
    onSetupGateChange?.(open)
  }, [turns, setupGateOpen, onSetupGateChange])

  const handleStop = async () => {
    const turnId = activeTurnRef.current
    if (!threadId || !turnId) return
    const op = {
      op: 'interrupt_turn',
      thread_id: threadId,
      turn_id: turnId,
    }
    if (wsRef.current?.readyState === WebSocket.OPEN) {
      wsRef.current.send(JSON.stringify(op))
    } else {
      await fetch(`${API}/turn/interrupt`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(op),
      }).catch(() => {})
    }
    setLoading(false)
    setActivity('interrupted')
  }

  const handleNewTask = async () => {
    const label = project || '当前会话'
    if (!window.confirm(
      `清理「${label}」的对话并新开任务？\n\n不影响已写大纲、正文与项目文件。`,
    )) {
      return
    }
    if (loading && threadId && activeTurnRef.current) {
      await handleStop()
    }
    clearChatCache(project)
    setTurns([])
    setInput('')
    setOtherText('')
    setShowOther(false)
    setSkillOpen(false)
    setActivity('新开任务…')
    setLoading(false)
    activeTurnRef.current = ''
    try {
      wsRef.current?.close()
    } catch {
      /* ignore */
    }
    const started = await fetch(`${API}/thread/new`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ project: project || null }),
    }).then((r) => r.json()).catch((e) => ({ error: e.message }))
    if (started?.error) {
      setActivity(`新开失败：${started.error}`)
      return
    }
    const tid = started.threadId || started.thread_id
    if (!tid) {
      setActivity('新开失败：无 thread')
      return
    }
    setThreadId(tid)
    saveChatCache(project, tid, [])
    try {
      await connectWs(tid)
      setActivity('已新开任务')
    } catch {
      setActivity('已新开任务（HTTP）')
    }
  }

  const onPickSkill = (s) => {
    setInput((prev) => {
      const replaced = prev.replace(/\$[a-zA-Z0-9_-]*$/, `$${s.name} `)
      return replaced.includes(`$${s.name}`) ? replaced : `${prev}$${s.name} `
    })
    setSkillOpen(false)
  }

  const placeholder = useMemo(
    () => (project
      ? `对《${project}》下指令…  Enter 发送 · $ 选 skill`
      : '描述你想写的小说… Enter 发送 · $ 选 skill'),
    [project],
  )

  const connLabel = wsState === 'open' ? 'live'
    : wsState === 'connecting' ? 'connecting'
      : threadId ? 'http' : '…'

  return (
    <section className="panel agent-chat nx-agent">
      <div className="chat-head">
        <h2>NovelX</h2>
        <div className="chat-head-actions">
          <button
            type="button"
            className="btn-ghost btn-inline btn-clear-context"
            onClick={handleNewTask}
            title="清空对话历史并新开任务（不删项目文件）"
          >
            新开任务
          </button>
          <span className={`status-chip status-${wsState === 'open' ? 'running' : 'idle'}`} title={threadId}>
            {connLabel}
          </span>
          {loading && <span className="status-chip status-running">turn</span>}
        </div>
      </div>
      <TodoList todos={todos} />
      <div className="chat-messages">
        <TurnTimeline
          turns={turns}
          loading={loading}
          project={project}
          onReaderJump={(opts) => syncPreview({
            ...opts,
            immediate: true,
            focusLatestChapter: false,
          })}
          onPickOption={handlePickOption}
          otherText={otherText}
          setOtherText={setOtherText}
          showOther={showOther}
          setShowOther={setShowOther}
          onOtherSubmit={handleOtherSubmit}
        />
        <div ref={endRef} />
      </div>
      {activity ? (
        <div className="nx-activity" aria-live="polite">
          <span className={`nx-activity-dot${loading ? ' pulse' : ''}`} />
          {activity}
        </div>
      ) : null}
      <div className="chat-compose">
        <div className="chat-input-wrap">
          <SkillPopup
            open={skillOpen}
            query={skillQuery}
            skills={skills}
            onPick={onPickSkill}
            onClose={() => setSkillOpen(false)}
          />
          <textarea
            className="chat-input"
            placeholder={placeholder}
            value={input}
            onChange={(e) => {
              const v = e.target.value
              setInput(v)
              detectSkillTrigger(v, e.target.selectionStart ?? v.length)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                handleSend()
              }
            }}
            rows={3}
            disabled={loading || !threadId}
          />
          <div className="chat-input-actions">
            {loading && (
              <button type="button" className="btn-ghost btn-inline" onClick={handleStop}>
                停止
              </button>
            )}
            <button
              type="button"
              className="btn-primary btn-inline"
              onClick={handleSend}
              disabled={loading || !input.trim() || !threadId}
            >
              发送
            </button>
          </div>
        </div>
        <div className="nx-composer-hint">
          Codex 式 Turn · Tool 卡片 · $skills
          {project ? ` · 《${project}》` : ''}
        </div>
      </div>
    </section>
  )
}
