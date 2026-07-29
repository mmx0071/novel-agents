export default function DraftPatchCard({ item, onOpenInDesk, compact }) {
  const title = (
    <>
      {item.project ? `${item.project} · ` : ''}
      第{item.chapter}章 · 第{item.start_para}
      {item.end_para !== item.start_para ? `–${item.end_para}` : ''}段
    </>
  )

  return (
    <div className={`nx-card nx-patch nx-status-${item.status || 'completed'}`}>
      <div className="nx-card-head">
        <span className="nx-card-kind">修订预览</span>
        <span className="nx-card-title">{title}</span>
        {typeof onOpenInDesk === 'function' ? (
          <button
            type="button"
            className="btn-ghost btn-inline nx-patch-open-desk"
            onClick={() => onOpenInDesk(item)}
            title="在写作台对照查看"
          >
            写作台对照
          </button>
        ) : null}
      </div>
      {!compact ? (
        <div className="nx-patch-diff">
          <div className="nx-patch-before">
            <div className="nx-patch-label">−</div>
            <pre>{item.before}</pre>
          </div>
          <div className="nx-patch-after">
            <div className="nx-patch-label">+</div>
            <pre>{item.after}</pre>
          </div>
        </div>
      ) : (
        <div className="nx-patch-compact-hint">对照已在写作台展示；收起后可在此查看全文，或点上方打开。</div>
      )}
    </div>
  )
}
