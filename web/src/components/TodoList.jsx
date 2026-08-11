import { useState } from 'react'
import { normalizeTodoStatus, todosHeadline, windowTodos } from '../todoWindow.js'

/**
 * Progressive To-dos (macro queue).
 *
 * variant:
 *   - default: standalone card
 *   - embed: inside「处理中」unified work card — collapsed by default, windowed list
 */
export default function TodoList({ todos, variant = 'default' }) {
  const list = Array.isArray(todos) ? todos : []
  const embed = variant === 'embed' || variant === 'external' || variant === 'nested'
  const [collapsed, setCollapsed] = useState(true)

  if (!list.length) return null

  const win = windowTodos(list, { keepCompleted: embed ? 2 : list.length })
  const headline = todosHeadline(list)
  const rows = embed ? win.rows : list.map((item, index) => ({
    item,
    index,
    status: normalizeTodoStatus(item?.status),
  }))

  return (
    <div
      className={[
        'nx-todo',
        embed ? 'nx-todo-embed' : '',
        collapsed ? 'is-collapsed' : '',
        win.done === win.total ? 'is-done' : '',
      ].filter(Boolean).join(' ')}
      aria-label="待办清单"
    >
      <button
        type="button"
        className="nx-todo-head"
        onClick={() => setCollapsed((v) => !v)}
        aria-expanded={!collapsed}
        title={collapsed ? '展开待办' : '收起待办'}
      >
        <span className="nx-todo-chevron" aria-hidden="true">{collapsed ? '▸' : '▾'}</span>
        <span className="nx-todo-title">待办</span>
        <span className="nx-todo-summary">{headline}</span>
      </button>
      {!collapsed && (
        <ul className="nx-todo-list">
          {rows.map((row) => (
            <li
              key={`${row.item.content}-${row.index}`}
              className={`nx-todo-item nx-todo-${row.status}`}
            >
              <span className="nx-todo-mark" aria-hidden="true">
                {markFor(row.status)}
              </span>
              <span className="nx-todo-text">{row.item.content}</span>
            </li>
          ))}
          {embed && win.pendingCount > 0 ? (
            <li className="nx-todo-item nx-todo-more">
              <span className="nx-todo-mark" aria-hidden="true">…</span>
              <span className="nx-todo-text">还有 {win.pendingCount} 章待审</span>
            </li>
          ) : null}
        </ul>
      )}
    </div>
  )
}

function markFor(status) {
  if (status === 'completed') return '✓'
  if (status === 'in_progress') return '●'
  return '○'
}
