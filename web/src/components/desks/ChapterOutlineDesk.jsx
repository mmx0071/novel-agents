import MarkdownView from '../MarkdownView'
import { parseChapterOutlineDisplay } from '../../mdSections'

function ChipList({ items, empty = '—' }) {
  if (!items?.length) return <span className="desk-muted">{empty}</span>
  return (
    <ul className="desk-chip-list">
      {items.map((x) => (
        <li key={x}>{x}</li>
      ))}
    </ul>
  )
}

function BeatList({ items }) {
  if (!items?.length) return <p className="desk-muted">（尚无关键事件）</p>
  return (
    <ol className="outline-beats">
      {items.map((ev, i) => (
        <li key={`${i}-${ev.slice(0, 24)}`}>
          <span className="outline-beat-n">{i + 1}</span>
          <span className="outline-beat-text">{ev}</span>
        </li>
      ))}
    </ol>
  )
}

export default function ChapterOutlineDesk({ text, unitLabel = '章纲' }) {
  const empty = !String(text || '').trim()
    || String(text).includes('尚无章纲')
    || String(text).includes('尚无集纲')
    || String(text).includes('加载章纲')

  if (empty) {
    return (
      <div className="outline-desk desk-empty">
        <p>尚无{unitLabel}。可在右侧请助手生成，或先从本卷进入本章。</p>
      </div>
    )
  }

  const parsed = parseChapterOutlineDisplay(text)
  if (!parsed?.title && !parsed?.goal && !parsed?.events?.length) {
    return (
      <div className="outline-desk">
        <MarkdownView source={text} variant="reader" />
      </div>
    )
  }

  return (
    <div className="outline-desk" aria-label={`${unitLabel}节拍板`}>
      <header className="outline-desk-head">
        <h3>{parsed.title || unitLabel}</h3>
        <p className="outline-desk-meta">
          {parsed.pov ? <span>视角 · {parsed.pov}</span> : null}
          {parsed.pov && parsed.timeLocation ? <span className="desk-dot">·</span> : null}
          {parsed.timeLocation ? <span>时空 · {parsed.timeLocation}</span> : null}
        </p>
      </header>

      <section className="outline-triad" aria-label="目标冲突情绪">
        <div className="outline-triad-cell">
          <h4>目标</h4>
          <p>{parsed.goal || '—'}</p>
        </div>
        <div className="outline-triad-cell">
          <h4>冲突</h4>
          <p>{parsed.conflict || '—'}</p>
        </div>
        <div className="outline-triad-cell">
          <h4>情绪</h4>
          <p>{parsed.emotion || '—'}</p>
        </div>
      </section>

      <section className="outline-desk-section">
        <h4>关键事件</h4>
        <BeatList items={parsed.events} />
      </section>

      <div className="outline-split">
        <section className="outline-desk-section">
          <h4>本章纳入</h4>
          <ChipList items={parsed.includes} empty="（未标注）" />
        </section>
        <section className="outline-desk-section">
          <h4>顺延后章</h4>
          <ChipList items={parsed.defers} empty="（未标注）" />
        </section>
      </div>

      <section className="outline-desk-section">
        <h4>出场</h4>
        <div className="outline-roster">
          <div>
            <span className="outline-roster-label">人物</span>
            <ChipList items={parsed.characters} />
          </div>
          <div>
            <span className="outline-roster-label">物品</span>
            <ChipList items={parsed.items} />
          </div>
          <div>
            <span className="outline-roster-label">地点</span>
            <ChipList items={parsed.locations} />
          </div>
        </div>
        {parsed.tags?.length ? (
          <div className="outline-tags">
            <span className="outline-roster-label">场景</span>
            <ChipList items={parsed.tags} />
          </div>
        ) : null}
      </section>

      {parsed.cliffhanger ? (
        <section className="outline-hook" aria-label="章末钩子">
          <h4>章末钩子</h4>
          <p>{parsed.cliffhanger}</p>
        </section>
      ) : null}

      {parsed.extra?.length ? (
        <section className="outline-desk-section outline-extra">
          {parsed.extra.map((s) => (
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
