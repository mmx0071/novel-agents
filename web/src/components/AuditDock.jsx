export default function AuditDock({
  open,
  onToggle,
  auditState,
  selectedChapter,
  onJumpChapter,
  onSendChat,
  onPickOption,
}) {
  const todos = auditState?.todos || []
  const latest = (() => {
    const list = auditState?.reports || []
    if (!list.length) return auditState?.latest || null
    if (selectedChapter > 0) {
      const hit = list.find((r) => Number(r.chapter) === Number(selectedChapter))
      if (hit) return hit
    }
    return list[list.length - 1]
  })()
  const approval = auditState?.openApproval
  const openCount = todos.filter((t) => t.status !== 'completed').length
    + (latest?.failed ? (latest.issues?.length || 1) : 0)
  const hasSignal = !!(latest || todos.length || approval)
  const statusHint = latest?.failed
    ? '未通过'
    : latest?.passed
      ? '通过'
      : approval
        ? '待选择'
        : openCount > 0
          ? '进行中'
          : '空闲'

  return (
    <div className={`audit-dock${open ? ' is-open' : ''}${hasSignal ? ' has-signal' : ''}`}>
      <button
        type="button"
        className="audit-dock-toggle"
        onClick={onToggle}
        aria-expanded={open}
        title={open ? '收起审校附栏' : '展开审校附栏'}
      >
        <span>审校</span>
        <span className={`audit-dock-status status-${latest?.failed ? 'fail' : latest?.passed ? 'pass' : hasSignal ? 'busy' : 'idle'}`}>
          {statusHint}
        </span>
        {openCount > 0 ? <span className="audit-dock-badge">{openCount}</span> : null}
        <span className="audit-dock-chevron">{open ? '▾' : '▸'}</span>
      </button>
      {open ? (
        <div className="audit-dock-body" role="region" aria-label="审校附栏">
          {!hasSignal ? (
            <p className="audit-dock-empty">
              暂无审校结果。可对创作助手说「审校第{selectedChapter || 'N'}章」。
            </p>
          ) : null}
          {todos.length > 0 ? (
            <div className="audit-dock-section">
              <h3>审阅队列</h3>
              <ul className="audit-dock-todos">
                {todos.map((t, i) => (
                  <li key={`${t.content}-${i}`} className={`audit-todo-${t.status || 'pending'}`}>
                    {t.content}
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
          {latest ? (
            <div className="audit-dock-section">
              <div className="audit-dock-section-head">
                <h3>
                  第{latest.chapter || selectedChapter || '?'}章
                  <span className={`audit-result ${latest.failed ? 'fail' : latest.passed ? 'pass' : ''}`}>
                    {latest.failed ? '未通过' : latest.passed ? '通过' : '报告'}
                  </span>
                </h3>
                {latest.chapter > 0 ? (
                  <button
                    type="button"
                    className="btn-ghost btn-inline"
                    onClick={() => onJumpChapter?.(latest.chapter)}
                  >
                    打开正文
                  </button>
                ) : null}
              </div>
              {latest.issues?.length ? (
                <ul className="audit-issue-list">
                  {latest.issues.map((issue) => (
                    <li key={issue.id || issue.index} className={`sev-${String(issue.severity || '').toLowerCase()}`}>
                      <div className="audit-issue-top">
                        <span className="audit-sev">{issue.severity}</span>
                        {issue.type ? <span className="audit-type">{issue.type}</span> : null}
                        <code className="audit-id">{issue.id}</code>
                      </div>
                      <div className="audit-issue-msg">{issue.message}</div>
                      {issue.location ? (
                        <div className="audit-issue-loc">{issue.location}</div>
                      ) : null}
                      {latest.failed && typeof onSendChat === 'function' ? (
                        <button
                          type="button"
                          className="btn-ghost btn-inline"
                          onClick={() => onSendChat(
                            `按审校局部修订第${latest.chapter}章：${issue.id} ${issue.message}`,
                          )}
                        >
                          按此条修订
                        </button>
                      ) : null}
                    </li>
                  ))}
                </ul>
              ) : (
                <p className="audit-dock-summary">{latest.summary || '见创作助手中的审校卡片'}</p>
              )}
            </div>
          ) : null}
          {approval?.options?.length ? (
            <div className="audit-dock-section">
              <h3>待你选择</h3>
              {approval.prompt ? (
                <p className="audit-dock-prompt">{String(approval.prompt)}</p>
              ) : null}
              <div className="audit-dock-options">
                {approval.options.map((opt, i) => (
                  <button
                    key={opt.id || i}
                    type="button"
                    className="btn-primary btn-inline"
                    onClick={() => onPickOption?.(opt)}
                  >
                    {opt.label || opt.id}
                  </button>
                ))}
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
