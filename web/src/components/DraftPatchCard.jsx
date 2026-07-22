export default function DraftPatchCard({ item }) {
  return (
    <div className={`nx-card nx-patch nx-status-${item.status || 'completed'}`}>
      <div className="nx-card-head">
        <span className="nx-card-kind">Patch</span>
        <span className="nx-card-title">
          {item.project} · 第{item.chapter}章 · 第{item.start_para}
          {item.end_para !== item.start_para ? `–${item.end_para}` : ''}段
        </span>
      </div>
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
    </div>
  )
}
