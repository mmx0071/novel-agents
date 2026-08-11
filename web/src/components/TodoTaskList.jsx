import { useEffect, useMemo, useRef, useState } from 'react'
import ToolProgressList from './ToolProgressList'
import { todosCursorLabel } from '../todoWindow.js'

/**
 * Todo fold:
 *   ▾ ✓ 待办 4/20 · 审校第5章
 *     ✓ …
 *     ● current (+ micro / decision)
 *     … N pending
 */
export default function TodoTaskList({
  tasks = [],
  live = false,
  awaiting = false,
  decision = null,
  defaultOpen = true,
}) {
  const list = Array.isArray(tasks) ? tasks : []
  const currentRef = useRef(null)
  const [expandPending, setExpandPending] = useState(false)
  const allDone = list.length > 0 && list.every((t) => t.status === 'completed')
  const [open, setOpen] = useState(() => (allDone ? false : defaultOpen))

  useEffect(() => {
    if (live || awaiting) setOpen(true)
    else if (allDone) setOpen(false)
  }, [live, awaiting, allDone])

  const todosForLabel = useMemo(
    () => list.map((t) => t.todo || { content: '', status: t.status }),
    [list],
  )
  const headLabel = todosCursorLabel(todosForLabel)

  const { focus, pending, pendingCount } = useMemo(() => {
    const focusRows = []
    const pendingRows = []
    for (const task of list) {
      if (task.status === 'pending') pendingRows.push(task)
      else focusRows.push(task)
    }
    return {
      focus: focusRows,
      pending: pendingRows,
      pendingCount: pendingRows.length,
    }
  }, [list])

  const currentKey = list.find((t) => t.status === 'in_progress')?.index

  useEffect(() => {
    if (!open) return
    const el = currentRef.current
    if (!el || typeof el.scrollIntoView !== 'function') return
    el.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  }, [currentKey, list.length, expandPending, !!decision, open])

  if (!list.length) return null

  const showPendingRows = expandPending || pendingCount <= 2

  return (
    <div className={`nx-todofold${allDone ? ' is-done' : ''}${open ? ' is-open' : ''}${awaiting ? ' is-awaiting' : ''}`}>
      <button
        type="button"
        className="nx-todofold-head"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className="nx-todofold-chevron" aria-hidden="true">{open ? '▾' : '▸'}</span>
        {allDone ? (
          <span className="nx-todofold-check" aria-hidden="true">✓</span>
        ) : null}
        <span className="nx-todofold-title">{headLabel}</span>
      </button>
      {open ? (
        <ul className="nx-todotasks" aria-label="待办">
          {focus.map((task) => (
            <TaskRow
              key={`${task.index}:${task.todo?.content || ''}`}
              task={task}
              live={live}
              awaiting={awaiting}
              decision={task.status === 'in_progress' ? decision : null}
              rowRef={task.status === 'in_progress' ? currentRef : null}
            />
          ))}
          {showPendingRows
            ? pending.map((task) => (
              <TaskRow
                key={`${task.index}:${task.todo?.content || ''}`}
                task={task}
                live={false}
                awaiting={false}
              />
            ))
            : (
              <li className="nx-todotask nx-todotask-more">
                <button
                  type="button"
                  className="nx-todotask-more-btn"
                  onClick={() => setExpandPending(true)}
                >
                  <span className="nx-todotask-mark" aria-hidden="true">…</span>
                  <span className="nx-todotask-text">还有 {pendingCount} 项</span>
                </button>
              </li>
            )}
          {expandPending && pendingCount > 2 ? (
            <li className="nx-todotask nx-todotask-more">
              <button
                type="button"
                className="nx-todotask-more-btn"
                onClick={() => setExpandPending(false)}
              >
                <span className="nx-todotask-mark" aria-hidden="true">▴</span>
                <span className="nx-todotask-text">收起</span>
              </button>
            </li>
          ) : null}
        </ul>
      ) : null}
    </div>
  )
}

function TaskRow({ task, live, awaiting, decision = null, rowRef }) {
  const active = task.status === 'in_progress'
  const hasSteps = active && !decision && Array.isArray(task.steps) && task.steps.length > 0
  return (
    <li
      ref={rowRef}
      className={`nx-todotask nx-todotask-${task.status}${active ? ' is-current' : ''}`}
    >
      <div className="nx-todotask-row">
        <span className="nx-todotask-mark" aria-hidden="true">
          {markFor(task.status, awaiting && active)}
        </span>
        <span className="nx-todotask-text">{task.todo?.content || '未命名步骤'}</span>
      </div>
      {hasSteps ? (
        <div className="nx-todotask-steps">
          <ToolProgressList entries={task.steps} live={live && active && !awaiting} />
        </div>
      ) : null}
      {decision ? (
        <div className="nx-todotask-decision">
          {decision}
        </div>
      ) : null}
    </li>
  )
}

function markFor(status, paused) {
  if (status === 'completed') return '✓'
  if (status === 'in_progress') return paused ? '⏸' : '●'
  return '○'
}
