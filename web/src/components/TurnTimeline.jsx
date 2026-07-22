import ToolCallCard from './ToolCallCard'
import DraftPatchCard from './DraftPatchCard'
import ApprovalOptions from './ApprovalOptions'
import { stripToolMarkup } from './toolMarkup'

export default function TurnTimeline({
  turns,
  loading,
  project,
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
      {turns.map((turn, idx) => (
        <div key={turn.id} className={`nx-turn nx-turn-${turn.status || 'running'}`}>
          <div className="nx-turn-sep">
            <span className="nx-turn-sep-line" />
            <span className="nx-turn-sep-label">
              Turn {idx + 1}
              {turn.status === 'running' ? ' · working' : turn.status === 'aborted' ? ' · interrupted' : ''}
            </span>
            <span className="nx-turn-sep-line" />
          </div>
          <div className="nx-turn-items">
            {(turn.items || []).map((item, itemIdx) => (
              <TurnItemView
                key={item.id || item._key}
                item={item}
                project={project}
                onReaderJump={onReaderJump}
                turnActive={turn.status === 'running'}
                isStreamTip={isAgentStreamTip(turn.items, itemIdx)}
              />
            ))}
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
      ))}
    </div>
  )
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

function TurnItemView({ item, project, onReaderJump, turnActive, isStreamTip }) {
  switch (item.type) {
    case 'user_message':
      return (
        <div className="nx-msg nx-msg-user">
          <div className="nx-msg-role">You</div>
          <pre className="nx-msg-text">{item.text}</pre>
        </div>
      )
    case 'agent_message': {
      const text = stripToolMarkup(item.text)
      if (!text) return null
      // Never leave carets on ack text once a tool card follows / turn paused.
      const streaming = turnActive && item.status === 'in_progress' && isStreamTip
      return (
        <div className="nx-msg nx-msg-agent">
          <div className="nx-msg-role">NovelX</div>
          <pre className={`nx-msg-text${streaming ? ' chat-streaming' : ''}`}>
            {text}
          </pre>
        </div>
      )
    }
    case 'reasoning': {
      const text = stripToolMarkup(item.text)
      if (!text) return null
      return (
        <details className="nx-card nx-reasoning" open={item.status === 'in_progress'}>
          <summary className="nx-card-head">
            <span className="nx-card-kind">Thinking</span>
          </summary>
          <pre className="nx-card-output">{text}</pre>
        </details>
      )
    }
    case 'skill_load':
      return (
        <div className={`nx-skillcell nx-status-${item.status}`}>
          <span className="nx-toolcell-bullet">{item.status === 'completed' ? '✓' : '•'}</span>
          <span className="nx-toolcell-verb">
            {item.status === 'completed' ? 'Loaded skill' : 'Loading skill'}
          </span>
          <span className="nx-skillcell-name">${item.name}</span>
          {item.path ? <span className="nx-toolcell-args">· {shortPath(item.path)}</span> : null}
        </div>
      )
    case 'tool_call':
      return <ToolCallCard item={item} project={project} onReaderJump={onReaderJump} />
    case 'draft_patch':
      return <DraftPatchCard item={item} />
    case 'pipeline_step':
      return (
        <div className={`nx-toolcell nx-toolcell-${item.status === 'completed' ? 'completed' : 'in_progress'}`}>
          <div className="nx-toolcell-head static">
            <span className="nx-toolcell-bullet">{item.status === 'completed' ? '✓' : '•'}</span>
            <span className="nx-toolcell-verb">
              {item.status === 'completed' ? 'Agent' : 'Running'}
            </span>
            <span className="nx-toolcell-name">{item.agent}</span>
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
            <span className="nx-card-kind">Audit</span>
            <span className="nx-card-title">{item.passed ? '通过' : '待处理'}</span>
          </div>
          <pre className="nx-card-output">{item.report}</pre>
        </div>
      )
    default:
      return null
  }
}

function shortPath(p) {
  if (!p) return ''
  const parts = String(p).split(/[/\\]/)
  return parts.slice(-3).join('/')
}
