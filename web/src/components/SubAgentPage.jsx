import { useCallback, useEffect, useRef, useState } from 'react'
import TodoList from './TodoList'
import TurnTimeline from './TurnTimeline'
import SubAgentRail from './SubAgentRail'
import MarkdownView from './MarkdownView'
import { stepLabelZh } from './toolLabels'
import {
  isAgentActive,
  lifecycleLabelZh,
  normalizeLifecycle,
  normalizeSubAgent,
} from '../subAgents'

const API = '/api'

function wsUrl(path) {
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}${path}`
}

function normalizeItem(raw) {
  if (!raw) return null
  const type = raw.type
  const status = normalizeWireStatus(raw.status)
  const key = raw.id || raw._key || `${type}:${raw.name || raw.agent || 'x'}`
  return { ...raw, type, status, _key: key }
}

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

function finishOpenItems(items) {
  return (items || []).map((it) => {
    if (!it) return it
    if (
      it.type === 'tool_call'
      || it.type === 'agent_message'
      || it.type === 'skill_load'
      || it.type === 'pipeline_step'
      || it.type === 'reasoning'
    ) {
      if (isTerminalStatus(it.status)) return it
      return { ...it, status: 'completed' }
    }
    return it
  })
}

/**
 * Dedicated SubAgent page: left task brief + right live/replay timeline.
 * Observation + interrupt only — user input stays on root Studio chat.
 */
export default function SubAgentPage({
  agent,
  agents,
  onClose,
  onSelectAgent,
  onAgentUpdate,
}) {
  const threadId = agent?.threadId || ''
  const [lifecycle, setLifecycle] = useState(() => normalizeLifecycle(agent?.lifecycle))
  const [summary, setSummary] = useState(() => agent?.summary || '')
  const [todos, setTodos] = useState([])
  const [turns, setTurns] = useState([])
  const [liveTurnId, setLiveTurnId] = useState('')
  const [loading, setLoading] = useState(false)
  const [wsState, setWsState] = useState('idle')
  const [activity, setActivity] = useState('')
  const wsRef = useRef(null)
  const activeTurnRef = useRef('')
  const turnsRef = useRef([])
  turnsRef.current = turns
  const handleEventRef = useRef(() => {})

  useEffect(() => {
    setLifecycle(normalizeLifecycle(agent?.lifecycle))
    setSummary(agent?.summary || '')
  }, [agent?.threadId, agent?.lifecycle, agent?.summary])

  const upsertTurn = useCallback((turnId, fn) => {
    if (!turnId) return
    setTurns((prev) => {
      const i = prev.findIndex((t) => t.id === turnId)
      if (i < 0) {
        const created = fn({ id: turnId, status: 'running', items: [], approval: null })
        return [...prev, created]
      }
      const next = [...prev]
      next[i] = fn(prev[i])
      return next
    })
  }, [])

  const publishAgentPatch = useCallback((patch) => {
    if (!threadId || typeof onAgentUpdate !== 'function') return
    onAgentUpdate(normalizeSubAgent({
      threadId,
      role: agent?.role,
      agentPath: agent?.agentPath,
      parentThreadId: agent?.parentThreadId,
      ...patch,
    }))
  }, [agent?.agentPath, agent?.parentThreadId, agent?.role, onAgentUpdate, threadId])

  const handleEvent = useCallback((ev) => {
    if (!ev?.type) return
    switch (ev.type) {
      case 'agent_status_changed': {
        const st = normalizeSubAgent(ev)
        if (!st || st.threadId !== threadId) break
        setLifecycle(st.lifecycle)
        if (st.summary) setSummary(st.summary)
        publishAgentPatch(st)
        if (!isAgentActive(st.lifecycle)) {
          setLoading(false)
          setLiveTurnId('')
          activeTurnRef.current = ''
        }
        break
      }
      case 'session_phase_changed': {
        const phase = ev.phase || 'idle'
        setLoading(phase === 'working')
        break
      }
      case 'turn_started': {
        activeTurnRef.current = ev.turn_id
        setLiveTurnId(ev.turn_id)
        setLoading(true)
        setActivity('运行中…')
        setTurns((prev) => {
          if (prev.some((t) => t.id === ev.turn_id)) {
            return prev.map((t) => (t.id === ev.turn_id ? { ...t, status: 'running' } : t))
          }
          return [...prev, { id: ev.turn_id, status: 'running', items: [], approval: null }]
        })
        break
      }
      case 'turn_complete':
      case 'turn_aborted': {
        const aborted = ev.type === 'turn_aborted'
        setTurns((prev) => prev.map((t) => (
          t.id === ev.turn_id
            ? {
              ...t,
              status: aborted ? 'aborted' : 'complete',
              items: finishOpenItems(t.items),
            }
            : t
        )))
        if (activeTurnRef.current === ev.turn_id) {
          activeTurnRef.current = ''
          setLiveTurnId('')
        }
        if (aborted) setActivity('已中断')
        break
      }
      case 'item_started':
      case 'item_completed': {
        const item = normalizeItem(ev.item)
        if (!item) break
        // SubAgent page is observation-only — skip approval cards.
        if (item.type === 'user_message' || item.type === 'agent_message'
          || item.type === 'reasoning' || item.type === 'tool_call'
          || item.type === 'skill_load' || item.type === 'pipeline_step'
          || item.type === 'draft_patch' || item.type === 'audit_report'
          || item.type === 'agent_spawn') {
          upsertTurn(ev.turn_id, (t) => {
            const items = [...(t.items || [])]
            const i = items.findIndex((x) => x.id === item.id)
            if (i >= 0) {
              const prev = items[i]
              if (
                item.type === 'tool_call'
                && prev.type === 'tool_call'
                && String(prev.output || '').length > String(item.output || '').length
              ) {
                items[i] = { ...prev, ...item, output: prev.output }
              } else if (
                item.type === 'agent_message'
                && prev.type === 'agent_message'
                && String(prev.text || '').length > String(item.text || '').length
              ) {
                items[i] = { ...prev, ...item, text: prev.text }
              } else {
                items[i] = { ...prev, ...item }
              }
            } else {
              items.push(item)
            }
            return { ...t, items }
          })
        }
        break
      }
      case 'agent_message_content_delta': {
        const delta = ev.delta || ''
        if (!delta) break
        upsertTurn(ev.turn_id, (t) => {
          const items = [...(t.items || [])]
          const i = items.findIndex((x) => x.id === ev.item_id)
          if (i >= 0 && items[i].type === 'agent_message') {
            items[i] = {
              ...items[i],
              text: `${items[i].text || ''}${delta}`,
              status: 'in_progress',
            }
          } else if (i < 0) {
            items.push({
              type: 'agent_message',
              id: ev.item_id,
              text: delta,
              status: 'in_progress',
              _key: ev.item_id,
            })
          }
          return { ...t, status: 'running', items }
        })
        break
      }
      case 'reasoning_content_delta': {
        const delta = ev.delta || ''
        if (!delta) break
        upsertTurn(ev.turn_id, (t) => {
          const items = [...(t.items || [])]
          const i = items.findIndex((x) => x.id === ev.item_id)
          if (i >= 0 && items[i].type === 'reasoning') {
            items[i] = {
              ...items[i],
              text: `${items[i].text || ''}${delta}`,
              status: 'in_progress',
            }
          } else if (i < 0) {
            items.push({
              type: 'reasoning',
              id: ev.item_id,
              text: delta,
              status: 'in_progress',
              _key: ev.item_id,
            })
          }
          return { ...t, status: 'running', items }
        })
        break
      }
      case 'tool_call_output_delta': {
        const delta = ev.delta || ''
        if (!delta) break
        upsertTurn(ev.turn_id, (t) => {
          const items = [...(t.items || [])]
          const i = items.findIndex((x) => x.id === ev.item_id)
          if (i >= 0 && items[i].type === 'tool_call') {
            items[i] = {
              ...items[i],
              output: `${items[i].output || ''}${delta}`,
              status: items[i].status || 'in_progress',
            }
          } else if (i < 0) {
            items.push({
              type: 'tool_call',
              id: ev.item_id,
              name: 'tool',
              arguments: {},
              output: delta,
              status: 'in_progress',
              _key: ev.item_id,
            })
          }
          return { ...t, status: 'running', items }
        })
        break
      }
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
        break
      }
      case 'error':
        setActivity(ev.message || 'error')
        break
      default:
        break
    }
  }, [publishAgentPatch, threadId, upsertTurn])

  handleEventRef.current = handleEvent

  // Hydrate turns from snapshot, then subscribe to child WS.
  useEffect(() => {
    if (!threadId) return undefined
    let cancelled = false
    setTurns([])
    setTodos([])
    setLiveTurnId('')
    activeTurnRef.current = ''
    setActivity('加载中…')

    const hydrate = async () => {
      try {
        const res = await fetch(`${API}/thread/${encodeURIComponent(threadId)}/snapshot`)
        const data = await res.json().catch(() => ({}))
        if (cancelled) return
        const serverTurns = Array.isArray(data.turns) ? data.turns : []
        setTurns(serverTurns.map((t) => ({
          ...t,
          items: (t.items || []).map((it) => normalizeItem(it)).filter(Boolean),
        })))
        if (data.active_turn_id || data.activeTurnId) {
          const tid = data.active_turn_id || data.activeTurnId
          activeTurnRef.current = tid
          setLiveTurnId(tid)
        }
        const phase = data.composer_phase || data.composerPhase
        setLoading(phase === 'working' || data.turn_active === true)
        setActivity('')
      } catch {
        if (!cancelled) setActivity('快照加载失败，尝试实时连接…')
      }
    }

    hydrate()

    const ws = new WebSocket(wsUrl(`/ws/thread/${threadId}`))
    wsRef.current = ws
    setWsState('connecting')
    ws.onopen = () => {
      if (!cancelled) setWsState('open')
    }
    ws.onerror = () => {
      if (!cancelled) setWsState('error')
    }
    ws.onclose = () => {
      if (!cancelled) setWsState('closed')
    }
    ws.onmessage = (msg) => {
      try {
        const ev = JSON.parse(msg.data)
        handleEventRef.current?.(ev)
      } catch {
        /* ignore */
      }
    }

    return () => {
      cancelled = true
      try { ws.close() } catch { /* ignore */ }
      if (wsRef.current === ws) wsRef.current = null
    }
  }, [threadId])

  const handleInterrupt = async () => {
    if (!threadId) return
    const turnId = activeTurnRef.current || liveTurnId || ''
    const body = { thread_id: threadId, turn_id: turnId || null }
    try {
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify({
          op: 'interrupt_turn',
          thread_id: threadId,
          turn_id: turnId || null,
        }))
      } else {
        await fetch(`${API}/turn/interrupt`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(body),
        })
      }
      setActivity('已请求中断')
    } catch {
      setActivity('中断失败')
    }
  }

  const roleLabel = stepLabelZh(agent?.role) || agent?.role || '协作角色'
  const active = isAgentActive(lifecycle)
  const connLabel = wsState === 'open' ? '已连接'
    : wsState === 'connecting' ? '连接中'
      : '离线'
  const connTone = wsState === 'open' ? 'ok'
    : wsState === 'connecting' ? 'pending'
      : 'idle'

  return (
    <div className="subagent-page" aria-label={`${roleLabel} 专用页`}>
      <header className="subagent-page-bar">
        <div className="subagent-page-bar-left">
          <button
            type="button"
            className="btn-ghost btn-inline"
            onClick={onClose}
            title="返回写作台"
          >
            ← 返回写作台
          </button>
          <h2 className="subagent-page-title">{roleLabel}</h2>
          <span className={`status-chip status-${active ? 'running' : 'idle'}`}>
            {lifecycleLabelZh(lifecycle)}
          </span>
          <span
            className={`conn-light conn-${connTone}`}
            role="status"
            aria-label={connLabel}
            title={connLabel}
          />
        </div>
        <div className="subagent-page-bar-actions">
          {active ? (
            <button
              type="button"
              className="btn-ghost btn-inline"
              onClick={handleInterrupt}
            >
              中断
            </button>
          ) : null}
        </div>
      </header>

      <div className="subagent-page-body">
        <section className="panel subagent-brief" aria-label="任务文档">
          <h2>任务</h2>
          {agent?.summary ? null : (
            <p className="subagent-brief-meta muted">协作角色后台任务</p>
          )}
          <div className="subagent-brief-task">
            {summary ? (
              <MarkdownView className="nx-msg-text" source={summary} variant="chat" />
            ) : (
              <p className="empty tiny">暂无任务说明</p>
            )}
          </div>
          <TodoList todos={todos} />
          <SubAgentRail
            agents={agents}
            openThreadId={threadId}
            onOpen={onSelectAgent}
          />
          <p className="subagent-brief-foot">
            观察模式：在此查看执行过程；继续吩咐请返回写作台右侧「创作助手」。
          </p>
        </section>

        <section className="panel subagent-session" aria-label="执行过程">
          <div className="subagent-session-head">
            <h2>执行</h2>
            {loading ? <span className="status-chip status-running">进行中</span> : null}
          </div>
          <div className="subagent-session-messages">
            <TurnTimeline
              turns={turns}
              loading={loading}
              project=""
              liveTurnId={liveTurnId}
              onPickOption={() => {}}
              otherText=""
              setOtherText={() => {}}
              showOther={false}
              setShowOther={() => {}}
              onOtherSubmit={() => {}}
            />
          </div>
          {activity ? (
            <div className="nx-activity" aria-live="polite">
              <span className={`nx-activity-dot${loading ? ' pulse' : ''}`} />
              {activity}
            </div>
          ) : null}
          <div className="subagent-session-compose-hint">
            此处仅查看进度 · 继续吩咐请返回创作助手
          </div>
        </section>
      </div>
    </div>
  )
}
