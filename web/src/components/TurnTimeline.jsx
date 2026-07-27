import { memo } from 'react'
import ToolCallCard from './ToolCallCard'
import DraftPatchCard from './DraftPatchCard'
import MutationPreviewCard from './MutationPreviewCard'
import ApprovalOptions from './ApprovalOptions'
import MarkdownView from './MarkdownView'
import { stripToolMarkup } from './toolMarkup'
import { stepLabelZh, toolVerbZh } from './toolLabels'

export default function TurnTimeline({
  turns,
  loading,
  project,
  liveTurnId,
  onReaderJump,
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
        <div>向 NovelX 下指令，Turn 内会按 Codex 样式展示 Skill / Tool / 正文。</div>
        <div className="nx-muted">试试：审阅第1–5章 · 或输入 $ 选择 skill</div>
      </div>
    )
  }

  return (
    <div className="nx-turns">
      {turns.map((turn, idx) => {
        // Prefer sticky liveTurnId so mid-stream status flaps don't blink「进行中」.
        const isLiveTurn = liveTurnId
          ? turn.id === liveTurnId
          : (idx === turns.length - 1
            && (turn.status === 'running' || turn.status === 'awaiting'))
        const running = isLiveTurn
          && (turn.status === 'running' || turn.status === 'awaiting' || loading)
        const hasToolOrSkill = (turn.items || []).some(
          (it) => it.type === 'tool_call' || it.type === 'skill_load' || it.type === 'pipeline_step',
        )
        const visibleItems = (turn.items || []).filter((it) => itemHasVisibleBody(it))
        const showPending = running && !visibleItems.length && !turn.approval
        return (
        <div key={turn.id || `turn-${idx}`} className={`nx-turn nx-turn-${turn.status || 'running'}`}>
          <TurnSep
            index={idx}
            running={running}
            aborted={turn.status === 'aborted'}
          />
          <div className="nx-turn-items">
            {(turn.items || []).map((item, itemIdx) => (
              <TurnItemView
                key={item.id || item._key || `${turn.id}:${itemIdx}`}
                item={item}
                turnItems={turn.items}
                itemIndex={itemIdx}
                project={project}
                onReaderJump={onReaderJump}
                turnActive={isLiveTurn && (turn.status === 'running' || loading)}
                suppressCaret={hasToolOrSkill}
                isStreamTip={isLiveTurn && isAgentStreamTip(turn.items, itemIdx)}
                hideAsPrefix={isAgentPrefixOfLater(turn.items, itemIdx)}
              />
            ))}
            {showPending ? (
              <div className="nx-turn-pending" aria-live="polite">
                <span className="nx-toolcell-spin" aria-hidden="true" />
                <span>正在处理…</span>
              </div>
            ) : null}
            {turn.approval && (
              <ApprovalOptions
                prompt={turn.approval.prompt}
                options={turn.approval.options}
                disabled={loading}
                onPick={onPickOption}
                otherText={otherText}
                setOtherText={setOtherText}
                showOther={showOther}
                setShowOther={setShowOther}
                onOtherSubmit={onOtherSubmit}
              />
            )}
          </div>
        </div>
        )
      })}
    </div>
  )
}

/** Isolated so tool-output re-renders don't repaint the separator text. */
const TurnSep = memo(function TurnSep({ index, running, aborted }) {
  return (
    <div className="nx-turn-sep">
      <span className="nx-turn-sep-line" />
      <span className="nx-turn-sep-label">
        回合 {index + 1}
        {running ? ' · 进行中' : aborted ? ' · 已中断' : ''}
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
  if (item.type === 'tool_call' || item.type === 'skill_load' || item.type === 'pipeline_step') {
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
  onReaderJump,
  turnActive,
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
      const text = dedupeAgentTextAgainstTools(
        turnItems,
        itemIndex,
        stripToolMarkup(item.text),
      )
      if (!text) return null
      // Never leave carets on ack/progress once a tool card follows / turn paused.
      const streaming = turnActive
        && !suppressCaret
        && item.status === 'in_progress'
        && isStreamTip
        && !looksLikeMirroredProgress(text)
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
      return (
        <details className="nx-card nx-reasoning" open={item.status === 'in_progress'}>
          <summary className="nx-card-head">
            <span className="nx-card-kind">思考</span>
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
            {item.status === 'completed' ? '已加载技能' : '加载技能中'}
          </span>
          <span className="nx-skillcell-name">${item.name}</span>
          {item.path ? <span className="nx-toolcell-args">· {shortPath(item.path)}</span> : null}
        </div>
      )
    case 'tool_call':
      return <ToolCallCard item={item} project={project} onReaderJump={onReaderJump} />
    case 'draft_patch':
      return <DraftPatchCard item={item} />
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

function shortPath(p) {
  if (!p) return ''
  const parts = String(p).split(/[/\\]/)
  return parts.slice(-3).join('/')
}
