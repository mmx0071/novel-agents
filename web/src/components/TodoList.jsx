import { useEffect, useRef, useState } from 'react'

/**
 * Progressive audit todo list:
 * pending · in_progress · completed — exactly one in_progress while active.
 * Collapsible so long audit queues don't bury the chat.
 */
export default function TodoList({ todos }) {
  const list = Array.isArray(todos) ? todos : []
  const active = list.some((t) => t.status === 'in_progress' || t.status === 'pending')
  const done = list.filter((t) => t.status === 'completed').length
  const current = list.find((t) => t.status === 'in_progress')
  const queueKey = list.map((t) => t.content).join('\0')
  const prevQueueKey = useRef('')

  // Long queues start collapsed; short ones open. Re-collapse only when the queue itself changes.
  const [collapsed, setCollapsed] = useState(() => list.length > 5)

  useEffect(() => {
    if (!queueKey || queueKey === prevQueueKey.current) return
    prevQueueKey.current = queueKey
    setCollapsed(list.length > 5)
  }, [queueKey, list.length])

  if (!list.length) return null
  if (!active && list.every((t) => t.status === 'completed')) {
    // Keep a compact completed summary briefly useful.
  }

  const summary = current
    ? `${done}/${list.length} · ${current.content}`
    : `${done}/${list.length}`

  return (
    <div className={`nx-todo${collapsed ? ' is-collapsed' : ''}`} aria-label="审阅待办">
      <button
        type="button"
        className="nx-todo-head"
        onClick={() => setCollapsed((v) => !v)}
        aria-expanded={!collapsed}
        title={collapsed ? '展开待办' : '收起待办'}
      >
        <span className="nx-todo-chevron" aria-hidden="true">{collapsed ? '▸' : '▾'}</span>
        <span className="nx-todo-title">审阅清单</span>
        <span className="nx-todo-summary">{summary}</span>
      </button>
      {!collapsed && (
        <ul className="nx-todo-list">
          {list.map((t, i) => (
            <li key={`${t.content}-${i}`} className={`nx-todo-item nx-todo-${t.status || 'pending'}`}>
              <span className="nx-todo-mark" aria-hidden="true">
                {markFor(t.status)}
              </span>
              <span className="nx-todo-text">{t.content}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  )
}

function markFor(status) {
  if (status === 'completed') return '✓'
  if (status === 'in_progress') return '▶'
  return '○'
}
