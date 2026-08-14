import MarkdownView from '../MarkdownView'
import { sectionBody, splitMarkdownByH2 } from '../../mdSections'

const STATUS_ZH = {
  active: '在场',
  background: '背景',
  exited: '退场',
  consumed: '已消耗',
}

function portraitSrc(portrait) {
  const p = String(portrait || '').trim()
  if (!p) return ''
  if (/^(https?:|data:|blob:)/i.test(p)) return p
  // Relative project paths need a file API later — keep slot ready.
  return ''
}

function PortraitSlot({ name, status, portrait, complete }) {
  const src = portraitSrc(portrait)
  const inactive = status === 'exited' || status === 'consumed'
  const initial = String(name || '?').trim().slice(0, 1) || '?'

  return (
    <div
      className={[
        'char-portrait',
        inactive ? 'is-inactive' : '',
        complete ? '' : 'is-incomplete',
      ].filter(Boolean).join(' ')}
      aria-label="立绘"
    >
      {src ? (
        <img src={src} alt="" className="char-portrait-img" />
      ) : (
        <div className="char-portrait-placeholder">
          <span className="char-portrait-initial">{initial}</span>
          <span className="char-portrait-cap">立绘待接入</span>
        </div>
      )}
    </div>
  )
}

export default function CharacterDesk({ entity }) {
  if (!entity) {
    return (
      <div className="char-desk desk-empty">
        <p>暂无人物设定卡。</p>
      </div>
    )
  }

  const name = entity.name || '未命名'
  const status = String(entity.status || '').trim()
  const statusLabel = STATUS_ZH[status] || status
  const holdings = String(entity.holdings || '')
    .split(/[,，;；]/)
    .map((s) => s.trim())
    .filter(Boolean)
  const { sections } = splitMarkdownByH2(entity.markdown || '')
  const history = sectionBody(sections, ['经历', 'History'])
  const personality = sectionBody(sections, ['性格', 'Personality'])
  const events = sectionBody(sections, ['核心事件', 'Core events'])
  const current = sectionBody(sections, ['当前状态', 'Current status'])
  const known = new Set(['经历', 'History', '性格', 'Personality', '核心事件', 'Core events', '当前状态', 'Current status'])
  const extra = sections.filter((s) => !known.has(s.heading))

  return (
    <div className="char-desk" aria-label="角色档案">
      <div className="char-desk-top">
        <PortraitSlot
          name={name}
          status={status}
          portrait={entity.portrait}
          complete={entity.complete}
        />
        <div className="char-desk-id">
          <header className="char-desk-head">
            <h3>{name}</h3>
            <div className="char-badges">
              {statusLabel ? (
                <span className={`char-badge status-${status || 'active'}`}>{statusLabel}</span>
              ) : null}
              {!entity.complete ? <span className="char-badge is-gap">待补全</span> : null}
            </div>
          </header>
          {holdings.length ? (
            <div className="char-holdings">
              <span className="desk-label">持有</span>
              <ul className="desk-chip-list">
                {holdings.map((h) => (
                  <li key={h}>{h}</li>
                ))}
              </ul>
            </div>
          ) : null}
          {entity.body_state ? (
            <section className="char-body-state">
              <h4>身体与能力状态</h4>
              <MarkdownView source={entity.body_state} variant="reader" />
            </section>
          ) : null}
          {entity.gaps?.length ? (
            <p className="char-gaps">缺口：{entity.gaps.join('、')}</p>
          ) : null}
        </div>
      </div>

      <div className="char-desk-grid">
        <section className="char-col">
          <h4>性格</h4>
          {personality ? (
            <MarkdownView source={personality} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
          <h4>经历</h4>
          {history ? (
            <MarkdownView source={history} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
        <section className="char-col">
          <h4>核心事件</h4>
          {events ? (
            <MarkdownView source={events} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
          <h4>当前状态</h4>
          {current ? (
            <MarkdownView source={current} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
      </div>

      {extra.length ? (
        <section className="char-extra">
          {extra.map((s) => (
            <div key={s.heading}>
              <h4>{s.heading}</h4>
              <MarkdownView source={s.body} variant="reader" />
            </div>
          ))}
        </section>
      ) : null}
    </div>
  )
}
