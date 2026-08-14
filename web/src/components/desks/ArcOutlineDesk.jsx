import MarkdownView from '../MarkdownView'
import {
  listItemsFromBody,
  pickSection,
  sectionBody,
  splitMarkdownByH2,
} from '../../mdSections'

function Timeline({ items, empty }) {
  if (!items?.length) return <p className="desk-muted">{empty}</p>
  return (
    <ol className="arc-timeline">
      {items.map((t, i) => (
        <li key={`${i}-${t.slice(0, 20)}`}>
          <span className="arc-timeline-dot" aria-hidden />
          <span className="arc-timeline-text">{t}</span>
        </li>
      ))}
    </ol>
  )
}

function Checklist({ items }) {
  if (!items?.length) return <p className="desk-muted">（尚无终止条件）</p>
  return (
    <ul className="arc-checklist">
      {items.map((t, i) => (
        <li key={`${i}-${t.slice(0, 20)}`}>{t}</li>
      ))}
    </ul>
  )
}

export default function ArcOutlineDesk({ text, volumeTitle = '' }) {
  const empty = !String(text || '').trim() || String(text).includes('尚无卷纲')

  if (empty) {
    return (
      <div className="arc-desk desk-empty">
        <p>尚无卷纲。完成分卷设计后，这里会显示冲突阶梯与卷末条件。</p>
      </div>
    )
  }

  const { title, sections } = splitMarkdownByH2(text)
  const positioning = sectionBody(sections, ['卷定位'])
  const openState = sectionBody(sections, ['开卷状态'])
  const ladder = listItemsFromBody(sectionBody(sections, ['冲突升级阶梯', '冲突阶梯']))
  const nodes = listItemsFromBody(sectionBody(sections, ['关键节点']))
  const charArc = sectionBody(sections, ['人物弧'])
  const foreshadow = sectionBody(sections, ['伏笔'])
  const softCount = sectionBody(sections, ['目标章数（软）', '目标章数'])
  const endings = listItemsFromBody(sectionBody(sections, ['卷末终止条件', '终止条件']))
  const delivery = sectionBody(sections, ['卷末交付'])

  const known = new Set(
    ['卷定位', '开卷状态', '冲突升级阶梯', '冲突阶梯', '关键节点', '人物弧', '伏笔',
      '目标章数（软）', '目标章数', '卷末终止条件', '终止条件', '卷末交付'],
  )
  const extra = sections.filter((s) => !known.has(s.heading)
    && !Array.from(known).some((k) => String(s.heading).includes(k)))

  const hasStructure = positioning || ladder.length || nodes.length || endings.length
  if (!hasStructure && !pickSection(sections, ['卷定位'])) {
    return (
      <div className="arc-desk">
        <MarkdownView source={text} variant="reader" />
      </div>
    )
  }

  const ladderOrNodes = ladder.length || nodes.length
    ? [
        ...ladder.map((t) => ({ kind: 'ladder', t })),
        ...nodes.map((t) => ({ kind: 'node', t })),
      ]
    : []

  return (
    <div className="arc-desk" aria-label="卷纲升级阶梯">
      <header className="arc-desk-head">
        <h3>{title || volumeTitle || '卷纲'}</h3>
        {softCount ? (
          <p className="arc-desk-meta">目标章数（软）· {softCount.replace(/\n/g, ' ').slice(0, 40)}</p>
        ) : null}
      </header>

      {(positioning || openState) ? (
        <section className="arc-desk-section arc-position">
          {positioning ? (
            <>
              <h4>卷定位</h4>
              <MarkdownView source={positioning} variant="reader" />
            </>
          ) : null}
          {openState ? (
            <>
              <h4>开卷状态</h4>
              <MarkdownView source={openState} variant="reader" />
            </>
          ) : null}
        </section>
      ) : null}

      <section className="arc-desk-section">
        <h4>冲突阶梯 · 关键节点</h4>
        {ladderOrNodes.length ? (
          <ol className="arc-timeline">
            {ladderOrNodes.map((row, i) => (
              <li key={`${row.kind}-${i}`} className={`arc-tl-${row.kind}`}>
                <span className="arc-timeline-dot" aria-hidden />
                <span className="arc-timeline-kind">
                  {row.kind === 'ladder' ? '升级' : '节点'}
                </span>
                <span className="arc-timeline-text">{row.t}</span>
              </li>
            ))}
          </ol>
        ) : (
          <Timeline items={[]} empty="（尚无阶梯或节点）" />
        )}
      </section>

      <div className="arc-split">
        <section className="arc-desk-section">
          <h4>人物弧</h4>
          {charArc ? (
            <MarkdownView source={charArc} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
        <section className="arc-desk-section">
          <h4>伏笔</h4>
          {foreshadow ? (
            <MarkdownView source={foreshadow} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
      </div>

      <section className="arc-desk-section arc-end">
        <h4>卷末终止条件</h4>
        <Checklist items={endings} />
        {delivery ? (
          <>
            <h4>卷末交付</h4>
            <MarkdownView source={delivery} variant="reader" />
          </>
        ) : null}
      </section>

      {extra.length ? (
        <section className="arc-desk-section">
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
