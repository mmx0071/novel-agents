import MarkdownView from './MarkdownView'

/**
 * variant:
 *   - default: standalone decision card (non-todo turns)
 *   - embedded: nested under current todo inside「处理中/待选择」work card
 */
export default function ApprovalOptions({
  prompt,
  options,
  disabled,
  onPick,
  otherText,
  setOtherText,
  onOtherSubmit,
  showOther,
  setShowOther,
  variant = 'default',
}) {
  const opts = options || []
  const embedded = variant === 'embedded'
  return (
    <div className={embedded ? 'nx-approval nx-approval-embed' : 'nx-card nx-approval'}>
      {prompt ? (
        <MarkdownView
          className="nx-approval-prompt"
          source={prompt}
          variant="chat"
        />
      ) : null}
      <div className="chat-option-list" role="group" aria-label="请选择下一步">
        {!embedded ? (
          <div className="chat-option-hint">请选择下一步（也可输入序号）</div>
        ) : null}
        {opts.map((opt, i) => (
          <button
            key={opt.id || i}
            type="button"
            className="chat-option-btn"
            disabled={disabled}
            onClick={() => onPick(opt, i + 1)}
          >
            <span className="choice-num">{i + 1}.</span>
            {opt.label}
          </button>
        ))}
        <button
          type="button"
          className="chat-option-btn chat-option-other"
          disabled={disabled}
          onClick={() => setShowOther(true)}
        >
          <span className="choice-num">{opts.length + 1}.</span>
          补充要求
        </button>
        {showOther && (
          <div className="chat-other-row">
            <input
              type="text"
              className="choice-other-input"
              placeholder="填写你的具体要求…"
              value={otherText}
              disabled={disabled}
              autoFocus
              onChange={(e) => setOtherText(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  onOtherSubmit()
                }
              }}
            />
            <button
              type="button"
              className="btn-primary btn-inline"
              disabled={disabled || !otherText.trim()}
              onClick={onOtherSubmit}
            >
              发送
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
