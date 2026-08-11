import { memo } from 'react'
import ToolCallCard from './ToolCallCard'
import DraftPatchCard from './DraftPatchCard'
import MutationPreviewCard from './MutationPreviewCard'
import ApprovalOptions from './ApprovalOptions'
import MarkdownView from './MarkdownView'
import WorkedForGroup from './WorkedForGroup'
import { stripToolMarkup } from './toolMarkup'
import { skillLabelZh, stepLabelZh, toolVerbZh } from './toolLabels'
import {
  looksLikeAuditQueueLaunchProse,
  orderTurnItemsForDisplay,
  prepareChatAgentProse,
} from '../chatProse'
import { effectiveToolDisplayStatus } from '../auditQueueStatus.js'
import { groupTurnItemsForWorked } from '../workedFor.js'
import { pickTodoHostSegment, workItemsHostTodos } from '../flowLayout.js'
import { resolveWorkTodos } from '../auditTodos.js'

export default function TurnTimeline({
  turns,
  loading,
  project,
  liveTurnId,
  todos = [],
  onReaderJump,
  onOpenPatchInDesk,
  onOpenSubAgent,
  compactDraftPatches = false,
  onPickOption,
  otherText,
  setOtherText,
  showOther,
  setShowOther,
  onOtherSubmit,
}) {
  if (!turns?.length) {
    return (
      <div className="empty tiny nx-empty-hint">
        <div>说说你想写什么，进度和正文摘要会出现在这里。</div>
        <div className="nx-muted">
          {project
            ? '试试：写下一章 · 或点「常用动作」快速开始'
            : '先在顶部选择或新建小说，再说「我想写一本小说」'}
        </div>
      </div>
    )
  }

  return (
    <div className="nx-turns">
      {turns.map((turn, idx) => {
        // Prefer sticky liveTurnId so mid-stream status flaps don't blink「进行中」.
        // Never keep「进行中」on a finished turn even if loading/liveTurnId lag.
        const turnFinished = turn.status === 'complete' || turn.status === 'aborted'
        const isLiveTurn = !turnFinished && (liveTurnId
          ? turn.id === liveTurnId
          : (idx === turns.length - 1
            && (turn.status === 'running' || turn.status === 'awaiting')))
        const turnAwaiting = turn.status === 'awaiting' || !!turn.approval
        const running = isLiveTurn
          && !turnAwaiting
          && (turn.status === 'running' || loading)
        const hasToolOrSkill = (turn.items || []).some(
          (it) => it.type === 'tool_call' || it.type === 'skill_load' || it.type === 'pipeline_step',
        )
        const visibleItems = (turn.items || []).filter((it) => itemHasVisibleBody(it))
        const showPending = running && !visibleItems.length && !turn.approval
        const displayItems = orderTurnItemsForDisplay(turn.items || [])
        const turnActive = isLiveTurn && !turnAwaiting && (turn.status === 'running' || loading)
        const segments = groupTurnItemsForWorked(displayItems)
        const hostSegIdx = pickHostSegIdx(segments, todos, isLiveTurn || idx === turns.length - 1)
        const hostSeg = hostSegIdx >= 0 ? segments[hostSegIdx] : null
        const hostTodos = hostSeg
          ? resolveWorkTodos(todos, hostSeg.items)
          : (todos?.length ? todos : [])
        const showHostTodos = !!(hostTodos && hostTodos.length
          && (isLiveTurn || idx === turns.length - 1))
        const embeddedApproval = !!(showHostTodos && turn.approval?.options?.length)
        const turnHasTodos = showHostTodos || !!(todos?.length)
        return (
        <div key={turn.id || `turn-${idx}`} className={`nx-turn nx-turn-${turn.status || 'running'}`}>
          <TurnSep
            index={idx}
            running={running}
            awaiting={turnAwaiting && isLiveTurn}
            aborted={turn.status === 'aborted'}
          />
          <div className="nx-turn-items">
            {segments.map((seg, segIdx) => {
              if (seg.kind === 'worked') {
                const isHost = showHostTodos && segIdx === hostSegIdx
                const nestApproval = !!(isHost && embeddedApproval)
                return (
                  <WorkedForGroup
                    key={`worked:${turn.id || idx}:${segIdx}:${seg.indices[0]}`}
                    items={seg.items}
                    running={seg.running && !turnAwaiting}
                    durationMs={seg.durationMs}
                    turnActive={turnActive && seg.running && !turnAwaiting}
                    awaiting={turnAwaiting && isHost}
                    todos={isHost ? hostTodos : null}
                    approval={nestApproval ? turn.approval : null}
                    approvalDisabled={loading && !turn.approval?.options?.length}
                    onPickOption={onPickOption}
                    otherText={otherText}
                    setOtherText={setOtherText}
                    showOther={showOther}
                    setShowOther={setShowOther}
                    onOtherSubmit={onOtherSubmit}
                  >
                    {isHost ? null : seg.items.map((item, j) => {
                      const itemIdx = seg.indices[j]
                      const progressHost = item?.type === 'tool_call'
                        && (
                          item.name === 'audit_chapters'
                          || item.name === 'audit_chapter'
                          || item.name === 'continue_writing_batch'
                          || item.name === 'steer_run'
                        )
                      return (
                        <TurnItemView
                          key={item.id || item._key || `${turn.id}:w:${itemIdx}`}
                          item={item}
                          turnItems={displayItems}
                          itemIndex={itemIdx}
                          project={project}
                          todos={todos}
                          omitQueueProgress={progressHost}
                          compactTool
                          hideStructuredProgress={progressHost && turnHasTodos}
                          onReaderJump={onReaderJump}
                          onOpenPatchInDesk={onOpenPatchInDesk}
                          onOpenSubAgent={onOpenSubAgent}
                          compactDraftPatches={compactDraftPatches}
                          turnActive={turnActive}
                          suppressCaret={hasToolOrSkill}
                          isStreamTip={isLiveTurn && isAgentStreamTip(displayItems, itemIdx)}
                          hideAsPrefix={
                            isAgentPrefixOfLater(displayItems, itemIdx)
                            || isMirroredProgressWithTools(displayItems, itemIdx)
                          }
                        />
                      )
                    })}
                  </WorkedForGroup>
                )
              }
              const item = seg.item
              const itemIdx = seg.index
              return (
                <TurnItemView
                  key={item.id || item._key || `${turn.id}:${itemIdx}`}
                  item={item}
                  turnItems={displayItems}
                  itemIndex={itemIdx}
                  project={project}
                  todos={todos}
                  onReaderJump={onReaderJump}
                  onOpenPatchInDesk={onOpenPatchInDesk}
                  onOpenSubAgent={onOpenSubAgent}
                  compactDraftPatches={compactDraftPatches}
                  turnActive={turnActive}
                  suppressCaret={hasToolOrSkill}
                  isStreamTip={isLiveTurn && isAgentStreamTip(displayItems, itemIdx)}
                  hideAsPrefix={
                    isAgentPrefixOfLater(displayItems, itemIdx)
                    || isMirroredProgressWithTools(displayItems, itemIdx)
                    || (turnHasTodos && looksLikeAuditQueueLaunchProse(item.type === 'agent_message' ? item.text : ''))
                    || (turnAwaiting && item.type === 'agent_message'
                      && agentDuplicatesApproval(item.text, turn.approval?.prompt))
                  }
                />
              )
            })}
            {showPending ? (
              <div className="nx-turn-pending" aria-live="polite">
                <span className="nx-toolcell-spin" aria-hidden="true" />
                <span>正在处理…</span>
              </div>
            ) : null}
            {turn.approval && !embeddedApproval ? (
              <ApprovalOptions
                prompt={turn.approval.prompt}
                options={turn.approval.options}
                disabled={loading && !turn.approval?.options?.length}
                onPick={onPickOption}
                otherText={otherText}
                setOtherText={setOtherText}
                showOther={showOther}
                setShowOther={setShowOther}
                onOtherSubmit={onOtherSubmit}
              />
            ) : null}
          </div>
        </div>
        )
      })}
    </div>
  )
}

/** Isolated so tool-output re-renders don't repaint the separator text. */
const TurnSep = memo(function TurnSep({ index, running, awaiting = false, aborted }) {
  return (
    <div className="nx-turn-sep">
      <span className="nx-turn-sep-line" />
      <span className="nx-turn-sep-label">
        本轮 {index + 1}
        {awaiting ? ' · 待选择' : running ? ' · 进行中' : aborted ? ' · 已中断' : ''}
      </span>
      <span className="nx-turn-sep-line" />
    </div>
  )
})

function itemHasVisibleBody(item) {
  if (!item) return false
  if (item.type === 'user_message') return !!String(item.text || '').trim()
  if (item.type === 'agent_message' || item.type === 'reasoning') {
    return !!String(item.text || '').trim()
  }
  if (
    item.type === 'tool_call'
    || item.type === 'skill_load'
    || item.type === 'pipeline_step'
    || item.type === 'agent_spawn'
  ) {
    return true
  }
  if (item.type === 'draft_patch' || item.type === 'mutation_preview' || item.type === 'audit_report') {
    return true
  }
  return false
}

/** Caret only on the live tip: last agent bubble with nothing after it (no tool/skill yet). */
function isAgentStreamTip(items, index) {
  const cur = items?.[index]
  if (!cur || cur.type !== 'agent_message') return false
  for (let j = index + 1; j < (items?.length || 0); j += 1) {
    const t = items[j]?.type
    if (t === 'tool_call' || t === 'skill_load' || t === 'pipeline_step' || t === 'agent_message') {
      return false
    }
  }
  return true
}

/** Hide short ack bubbles once a later NovelX reply already starts with the same text. */
function isAgentPrefixOfLater(items, index) {
  const cur = items?.[index]
  if (!cur || cur.type !== 'agent_message') return false
  const curText = stripToolMarkup(cur.text || '').trim()
  if (curText.length < 8) return false
  for (let j = index + 1; j < (items?.length || 0); j += 1) {
    if (items[j]?.type !== 'agent_message') continue
    const later = stripToolMarkup(items[j].text || '').trim()
    if (later.length > curText.length && later.startsWith(curText)) return true
  }
  return false
}

/** Hide pipeline heartbeats mirrored into NovelX bubbles when tool cards already show them. */
function isMirroredProgressWithTools(items, index) {
  const cur = items?.[index]
  if (!cur || cur.type !== 'agent_message') return false
  const text = stripToolMarkup(cur.text || '')
  if (!looksLikeMirroredProgress(text)) return false
  return (items || []).some((it) => it?.type === 'tool_call')
}

/**
 * Drop agent-bubble lines that already appear as a sibling tool_call output
 * (legacy: mutation apply echoed「已写入设定卡 …」in both places).
 */
function dedupeAgentTextAgainstTools(items, index, text) {
  const raw = String(text || '')
  if (!raw.trim() || !items?.length) return raw
  const toolOutputs = items
    .filter((it, i) => i !== index && it?.type === 'tool_call' && it.output)
    .map((it) => String(it.output).trim())
    .filter(Boolean)
  if (!toolOutputs.length) return raw
  const kept = raw.split('\n').filter((line) => {
    const t = line.trim()
    if (!t) return true
    if (t.startsWith('已确认，正在应用')) return true
    return !toolOutputs.some((out) => out === t || out.includes(t))
  })
  return kept.join('\n').replace(/\n{3,}/g, '\n\n').trim()
}

const TurnItemView = memo(function TurnItemView({
  item,
  turnItems,
  itemIndex,
  project,
  todos = [],
  omitQueueProgress = false,
  compactTool = false,
  hideStructuredProgress = false,
  onReaderJump,
  onOpenPatchInDesk,
  onOpenSubAgent,
  compactDraftPatches = false,
  turnActive = false,
  suppressCaret,
  isStreamTip,
  hideAsPrefix,
}) {
  switch (item.type) {
    case 'user_message':
      return (
        <div className="nx-msg nx-msg-user">
          <div className="nx-msg-role">你</div>
          <MarkdownView className="nx-msg-text" source={item.text} variant="chat" />
        </div>
      )
    case 'agent_message': {
      if (hideAsPrefix) return null
      const raw = dedupeAgentTextAgainstTools(
        turnItems,
        itemIndex,
        stripToolMarkup(item.text),
      )
      if (!raw) return null
      const reasoningTexts = (turnItems || [])
        .filter((it) => it?.type === 'reasoning')
        .map((it) => it.text)
      // While streaming, keep raw text so the caret doesn't jump; structure on settle.
      const streaming = turnActive
        && !suppressCaret
        && item.status === 'in_progress'
        && isStreamTip
        && !looksLikeMirroredProgress(raw)
      const text = streaming ? raw : prepareChatAgentProse(raw, reasoningTexts)
      if (!text) return null
      return (
        <div className="nx-msg nx-msg-agent">
          <div className="nx-msg-role">NovelX</div>
          {streaming ? (
            <pre className="nx-msg-text chat-streaming">{text}</pre>
          ) : (
            <MarkdownView className="nx-msg-text" source={text} variant="chat" />
          )}
        </div>
      )
    }
    case 'reasoning': {
      const text = stripToolMarkup(item.text)
      if (!text) return null
      const live = item.status === 'in_progress' && turnActive
      const preview = text.replace(/\s+/g, ' ').trim().slice(0, 42)
      return (
        <details className="nx-card nx-reasoning" open={live}>
          <summary className="nx-card-head">
            <span className="nx-card-kind">思考</span>
            {!live && preview ? (
              <span className="nx-reasoning-preview">{preview}{text.length > 42 ? '…' : ''}</span>
            ) : null}
          </summary>
          <MarkdownView className="nx-card-output" source={text} variant="chat" />
        </details>
      )
    }
    case 'skill_load':
      return (
        <div className={`nx-skillcell nx-status-${item.status}`}>
          <span className="nx-toolcell-bullet">{item.status === 'completed' ? '✓' : '•'}</span>
          <span className="nx-toolcell-verb">
            {item.status === 'completed' ? '已加载写作指南' : '加载写作指南'}
          </span>
          <span className="nx-skillcell-name">{skillLabelZh(item.name)}</span>
        </div>
      )
    case 'tool_call': {
      const displayStatus = effectiveToolDisplayStatus(item, { turnItems, todos })
      const displayItem = displayStatus === item.status
        ? item
        : { ...item, status: displayStatus }
      return (
        <ToolCallCard
          item={displayItem}
          project={project}
          onReaderJump={onReaderJump}
          omitQueueProgress={omitQueueProgress}
          compact={compactTool}
          hideStructuredProgress={hideStructuredProgress}
        />
      )
    }
    case 'draft_patch':
      return (
        <DraftPatchCard
          item={item}
          onOpenInDesk={onOpenPatchInDesk}
          // Compact only while writing-desk dock already shows the same diffs.
          compact={!!compactDraftPatches}
        />
      )
    case 'mutation_preview':
      return <MutationPreviewCard item={item} />
    case 'pipeline_step':
      return (
        <div className={`nx-toolcell nx-toolcell-${item.status === 'completed' ? 'completed' : 'in_progress'}`}>
          <div className="nx-toolcell-head static">
            <span className="nx-toolcell-bullet">{item.status === 'completed' ? '✓' : '•'}</span>
            <span className="nx-toolcell-verb">
              {toolVerbZh(item.status === 'completed' ? 'completed' : 'in_progress')}
            </span>
            <span className="nx-toolcell-name">{stepLabelZh(item.agent)}</span>
          </div>
          {item.summary ? (
            <div className="nx-toolcell-body">
              <pre className="nx-toolcell-output">
                <span className="nx-toolcell-branch">└ </span>
                {item.summary}
              </pre>
            </div>
          ) : null}
        </div>
      )
    case 'audit_report':
      return (
        <div className={`nx-card nx-audit ${item.passed ? 'nx-pass' : 'nx-fail'}`}>
          <div className="nx-card-head">
            <span className="nx-card-kind">审校</span>
            <span className="nx-card-title">{item.passed ? '通过' : '待处理'}</span>
          </div>
          <MarkdownView className="nx-card-output" source={item.report} variant="chat" />
        </div>
      )
    case 'agent_spawn': {
      const childId = item.child_thread_id || item.childThreadId || ''
      const role = item.role || ''
      const path = item.agent_path || item.agentPath || ''
      const label = stepLabelZh(role) || '协作角色'
      return (
        <div className={`nx-toolcell nx-agent-spawn nx-toolcell-${item.status === 'completed' ? 'completed' : 'in_progress'}`}>
          <div className="nx-toolcell-head static">
            <span className="nx-toolcell-bullet">✓</span>
            <span className="nx-toolcell-verb">已启动协作</span>
            <span className="nx-toolcell-name">{label}</span>
          </div>
          {childId && typeof onOpenSubAgent === 'function' ? (
            <div className="nx-toolcell-body">
              <button
                type="button"
                className="btn-ghost btn-inline nx-open-subagent"
                onClick={() => onOpenSubAgent({
                  threadId: childId,
                  role,
                  agentPath: path,
                  lifecycle: 'running',
                  summary: '',
                })}
              >
                查看进度
              </button>
            </div>
          ) : null}
        </div>
      )
    }
    default:
      return null
  }
})

function looksLikeMirroredProgress(text) {
  const t = String(text || '')
  return /[▶✓⏸⚙✕]/.test(t)
    || t.includes('调用模型')
    || t.includes('等待首包')
    || t.includes('正在执行')
    || t.includes('已确认，正在应用')
}

function pickHostSegIdx(segments, todos, allowHost) {
  if (!allowHost) return -1
  if (todos?.length) {
    const picked = pickTodoHostSegment(segments, todos)
    if (picked >= 0) return picked
  }
  for (let i = segments.length - 1; i >= 0; i -= 1) {
    if (segments[i]?.kind === 'worked' && workItemsHostTodos(segments[i].items)) {
      return i
    }
  }
  if (todos?.length) {
    for (let i = segments.length - 1; i >= 0; i -= 1) {
      if (segments[i]?.kind === 'worked') return i
    }
  }
  return -1
}

function agentDuplicatesApproval(text, prompt) {
  const a = String(text || '').replace(/\s+/g, ' ').trim()
  const b = String(prompt || '').replace(/\s+/g, ' ').trim()
  if (a.length < 12 || b.length < 12) return false
  const aHead = a.slice(0, 48)
  const bHead = b.slice(0, 48)
  return a.includes(bHead) || b.includes(aHead)
}
