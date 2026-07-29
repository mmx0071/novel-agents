import { summarizePlotForDesk } from '../plotSummary'

function plotStatusLabel(status) {
  const s = String(status || '').toLowerCase()
  if (s === 'in_progress' || s === '进行中') return '进行中'
  if (s === 'bridging' || s === '衔接中') return '衔接中'
  if (s === 'completed' || s === '已完成') return '已完成'
  if (s === 'planned' || s === '计划中') return '计划中'
  if (s === 'abandoned') return '已放弃'
  return status || '剧情'
}

export default function VolumeWorkspace({
  group,
  nextChapter,
  publishedCount,
  onOpenArc,
  onOpenPlot,
  onWriteNext,
  onOpenChapter,
  writeBusy = false,
}) {
  if (!group) {
    return (
      <div className="volume-workspace empty">
        <p>尚无分卷。完成立项并生成卷纲后，这里会成为本卷工作台。</p>
      </div>
    )
  }

  const arcExcerpt = String(group.arc?.markdown || '')
    .replace(/^#+\s*.+\n+/, '')
    .replace(/\s+/g, ' ')
    .trim()
    .slice(0, 220)

  return (
    <div className="volume-workspace" aria-label="当前卷工作区">
      <header className="volume-workspace-head">
        <div>
          <h3>{group.title || group.shortLabel}</h3>
          <p className="volume-workspace-meta">
            已发布 {publishedCount || 0} 章
            {nextChapter ? ` · 下一章第 ${nextChapter} 章` : ''}
            {group.plots?.length ? ` · ${group.plots.length} 张剧情卡` : ''}
          </p>
        </div>
        <div className="volume-workspace-actions">
          <button type="button" className="btn-ghost btn-inline" onClick={onOpenArc}>
            看卷纲
          </button>
          {typeof onWriteNext === 'function' ? (
            <button
              type="button"
              className="btn-primary btn-inline"
              onClick={onWriteNext}
              disabled={writeBusy}
              aria-busy={writeBusy}
            >
              {writeBusy ? '进行中…' : `写第${nextChapter}章`}
            </button>
          ) : null}
        </div>
      </header>

      <section className="volume-workspace-section">
        <h4>卷纲摘要</h4>
        <p>{arcExcerpt || '（尚无卷纲正文，可点「看卷纲」或让助手生成）'}</p>
      </section>

      <section className="volume-workspace-section">
        <h4>剧情链</h4>
        {group.plots?.length ? (
          <ol className="volume-plot-chain">
            {group.plots.map((p) => {
              const summary = summarizePlotForDesk(p, 96)
              const key = p.slug || p.id || p.title
              return (
                <li key={key} className={`plot-status-${String(p.status || 'planned').toLowerCase()}`}>
                  <button
                    type="button"
                    className="volume-plot-item"
                    onClick={() => onOpenPlot?.(p)}
                  >
                    <span className="volume-plot-title">{p.title || key}</span>
                    <span className="volume-plot-badge">{plotStatusLabel(p.status)}</span>
                    {summary?.body ? (
                      <span className="volume-plot-brief">{summary.body}</span>
                    ) : null}
                  </button>
                </li>
              )
            })}
          </ol>
        ) : (
          <p className="volume-workspace-muted">本卷尚无剧情卡。可在创作助手中设计并激活主线卡。</p>
        )}
      </section>

      {typeof onOpenChapter === 'function' && nextChapter > 0 ? (
        <section className="volume-workspace-section">
          <h4>本章入口</h4>
          <div className="volume-chapter-entry">
            <button
              type="button"
              className="btn-ghost btn-inline"
              onClick={() => onOpenChapter(nextChapter, 'outline')}
            >
              第{nextChapter}章 · 章纲
            </button>
            <button
              type="button"
              className="btn-ghost btn-inline"
              onClick={() => onOpenChapter(nextChapter, 'draft')}
            >
              第{nextChapter}章 · 正文
            </button>
          </div>
        </section>
      ) : null}
    </div>
  )
}
