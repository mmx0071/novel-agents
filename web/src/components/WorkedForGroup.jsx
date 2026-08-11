import { useEffect, useMemo, useState } from 'react'
import TodoTaskList from './TodoTaskList'
import ApprovalOptions from './ApprovalOptions'
import { toolLabelZh } from './toolLabels'
import {
  formatWorkedDuration,
  workedForSummary,
  workedForTitle,
} from '../workedFor.js'
import { parseToolProgress, summarizeToolProgress } from '../toolProgress.js'
import { todosStillOpen } from '../auditQueueStatus.js'
import { workItemsHostTodos } from '../flowLayout.js'
import { buildTodoTasks } from '../todoTasks.js'

/**
 * Cursor-like hierarchy:
 *   ▾ 待办 4/20 · …
 *       ✓ / ● rows (+ micro steps / gate under current)
 *   ▾ 进行中… / 用时 1 分 24 秒
 */
export default function WorkedForGroup({
  items,
  running,
  durationMs,
  turnActive = false,
  awaiting = false,
  todos = null,
  approval = null,
  approvalDisabled = false,
  onPickOption,
  otherText,
  setOtherText,
  showOther,
  setShowOther,
  onOtherSubmit,
  children,
}) {
  const todosList = Array.isArray(todos) && todos.length ? todos : null
  const todosOpen = !!(todosList && todosStillOpen(todosList))
  const auditHost = workItemsHostTodos(items)
  const awaitingChoice = !!awaiting
  const openWork = !!(running || turnActive || todosOpen || awaitingChoice)
  const spinning = openWork && !awaitingChoice && !!(running || turnActive || todosOpen)
  const [open, setOpen] = useState(openWork)

  useEffect(() => {
    if (openWork) setOpen(true)
    else setOpen(false)
  }, [openWork])

  const hostOutput = useMemo(() => pickHostToolOutput(items), [items])
  const todoTasks = useMemo(
    () => (todosList ? buildTodoTasks(todosList, hostOutput) : null),
    [todosList, hostOutput],
  )

  const title = workedForTitle({
    running: spinning,
    awaiting: awaitingChoice,
    durationMs,
  })
  const workTip = workedForSummary(items, {
    toolLabelZh,
    parseToolProgress: (out) => parseToolProgress(out, {
      omitQueue: !!(todosList || auditHost),
    }),
    summarizeToolProgress,
  })
  const durLabel = !openWork && durationMs > 0 ? formatWorkedDuration(durationMs) : ''

  const decision = (awaitingChoice && approval?.options?.length) ? (
    <ApprovalOptions
      variant="embedded"
      prompt={approval.prompt}
      options={approval.options}
      disabled={approvalDisabled}
      onPick={onPickOption}
      otherText={otherText}
      setOtherText={setOtherText}
      showOther={showOther}
      setShowOther={setShowOther}
      onOtherSubmit={onOtherSubmit}
    />
  ) : null

  // Todos fold owns macro + micro; Worked line is a slim status (Cursor sibling).
  if (todoTasks) {
    return (
      <div className={`nx-worked has-todos${spinning ? ' is-live' : ''}${awaitingChoice ? ' is-awaiting' : ''}`}>
        <TodoTaskList
          tasks={todoTasks.tasks}
          live={spinning}
          awaiting={awaitingChoice}
          decision={decision}
          defaultOpen
        />
        <div className="nx-worked-status" aria-live="polite">
          <span className="nx-worked-title">{title}</span>
          {workTip && (spinning || awaitingChoice) ? (
            <span className="nx-worked-summary">{workTip}</span>
          ) : null}
          {spinning ? <span className="nx-toolcell-spin" aria-label="进行中" /> : null}
          {!openWork && durLabel ? (
            <span className="nx-worked-dur">{durLabel}</span>
          ) : null}
        </div>
      </div>
    )
  }

  return (
    <div className={`nx-worked${spinning ? ' is-live' : ''}${awaitingChoice ? ' is-awaiting' : ''}${open ? ' is-open' : ''}`}>
      <button
        type="button"
        className="nx-worked-head"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <span className="nx-worked-chevron" aria-hidden="true">{open ? '▾' : '▸'}</span>
        <span className="nx-worked-title">{title}</span>
        {workTip ? (
          <span className="nx-worked-summary">{workTip}</span>
        ) : null}
        {spinning ? <span className="nx-toolcell-spin" aria-label="进行中" /> : null}
        {!openWork && durLabel && title.indexOf(durLabel) < 0 ? (
          <span className="nx-worked-dur">{durLabel}</span>
        ) : null}
      </button>
      {open ? (
        <div className="nx-worked-body">
          <div className="nx-worked-items">
            {children}
          </div>
        </div>
      ) : null}
    </div>
  )
}

function pickHostToolOutput(items) {
  const list = Array.isArray(items) ? items : []
  const host = list.find(
    (it) => it?.type === 'tool_call'
      && (
        it.name === 'audit_chapters'
        || it.name === 'audit_chapter'
        || it.name === 'continue_writing_batch'
        || it.name === 'steer_run'
      ),
  ) || list.find((it) => it?.type === 'tool_call' && it.output)
  return host?.output || ''
}
