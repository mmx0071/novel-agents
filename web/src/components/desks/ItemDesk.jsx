import MarkdownView from '../MarkdownView'
import { sectionBody, splitMarkdownByH2 } from '../../mdSections'

const STATUS_ZH = {
  active: '在用',
  background: '背景',
  exited: '退场',
  consumed: '已消耗',
}

export default function ItemDesk({ entity }) {
  if (!entity) {
    return (
      <div className="item-desk desk-empty">
        <p>暂无物品设定卡。</p>
      </div>
    )
  }

  const name = entity.name || '未命名'
  const status = String(entity.status || '').trim()
  const statusLabel = STATUS_ZH[status] || status
  const { sections } = splitMarkdownByH2(entity.markdown || '')
  const origin = sectionBody(sections, ['来源', 'Origin'])
  const usage = sectionBody(sections, ['用途', 'Usage'])
  const current = sectionBody(sections, ['当前状态', 'Current status'])

  return (
    <div className="item-desk" aria-label="物件图鉴">
      <header className="item-desk-head">
        <div className="item-icon" aria-hidden>
          <span>{String(name).slice(0, 1)}</span>
        </div>
        <div>
          <h3>{name}</h3>
          <div className="item-badges">
            {statusLabel ? (
              <span className={`item-badge status-${status || 'active'}`}>{statusLabel}</span>
            ) : null}
            {!entity.complete ? <span className="item-badge is-gap">待补全</span> : null}
          </div>
        </div>
      </header>

      <div className="item-panels">
        {[
          { h: '来源', body: origin },
          { h: '用途', body: usage },
          { h: '当前状态', body: current },
        ].map((p) => (
          <section key={p.h} className="item-panel">
            <h4>{p.h}</h4>
            {p.body ? (
              <MarkdownView source={p.body} variant="reader" />
            ) : (
              <p className="desk-muted">（待补）</p>
            )}
          </section>
        ))}
      </div>

      {entity.gaps?.length ? (
        <p className="item-gaps">缺口：{entity.gaps.join('、')}</p>
      ) : null}
    </div>
  )
}
