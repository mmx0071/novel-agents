import { useState } from 'react'
import MarkdownView from '../MarkdownView'
import { pickSection, splitMarkdownByH2 } from '../../mdSections'

const CORE = [
  { key: 'logline', aliases: ['一句话卖点', 'logline', '卖点'], label: '一句话卖点' },
  { key: 'acts', aliases: ['三幕结构', '分卷', '分集骨架', '分集', '分季'], label: '结构' },
  { key: 'arc', aliases: ['主角弧'], label: '主角弧' },
  { key: 'conflict', aliases: ['主线冲突'], label: '主线冲突' },
]

export default function MasterOutlineDesk({ text }) {
  const [moreOpen, setMoreOpen] = useState(false)
  const empty = !String(text || '').trim() || String(text).includes('尚无总纲')

  if (empty) {
    return (
      <div className="master-desk desk-empty">
        <p>尚无总纲。立项完成后会在此展示卖点与全书骨架。</p>
      </div>
    )
  }

  const { title, sections } = splitMarkdownByH2(text)
  const logline = pickSection(sections, CORE[0].aliases)
  const acts = pickSection(sections, CORE[1].aliases)
  const arc = pickSection(sections, CORE[2].aliases)
  const conflict = pickSection(sections, CORE[3].aliases)

  const coreHeadings = new Set(
    [logline, acts, arc, conflict].filter(Boolean).map((s) => s.heading),
  )
  const more = sections.filter((s) => !coreHeadings.has(s.heading))

  if (!logline && !acts && !arc && !conflict) {
    return (
      <div className="master-desk">
        <MarkdownView source={text} variant="reader" />
      </div>
    )
  }

  return (
    <div className="master-desk" aria-label="总纲卖点海报">
      <header className="master-desk-head">
        <p className="master-desk-kicker">{title || '总纲'}</p>
        <h3 className="master-logline">
          {logline?.body?.trim() || '（尚无一句话卖点）'}
        </h3>
      </header>

      <div className="master-pillars">
        {[
          { sec: acts, label: acts?.heading || '结构' },
          { sec: arc, label: '主角弧' },
          { sec: conflict, label: '主线冲突' },
        ].map(({ sec, label }) => (
          <section key={label} className="master-pillar">
            <h4>{label}</h4>
            {sec?.body ? (
              <MarkdownView source={sec.body} variant="reader" />
            ) : (
              <p className="desk-muted">（待补）</p>
            )}
          </section>
        ))}
      </div>

      {more.length ? (
        <section className="master-more">
          <button
            type="button"
            className="btn-ghost btn-inline"
            onClick={() => setMoreOpen((v) => !v)}
          >
            {moreOpen ? '收起更多' : `更多章节（${more.length}）`}
          </button>
          {moreOpen ? (
            <div className="master-more-body">
              {more.map((s) => (
                <div key={s.heading} className="master-more-sec">
                  <h4>{s.heading}</h4>
                  <MarkdownView source={s.body} variant="reader" />
                </div>
              ))}
            </div>
          ) : null}
        </section>
      ) : null}
    </div>
  )
}
