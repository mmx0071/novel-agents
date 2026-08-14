import { useEffect, useMemo, useRef, useState } from 'react'
import MarkdownView from '../MarkdownView'
import { splitMarkdownByH2 } from '../../mdSections'

const CORE_PREFIX = /^(0|1|2|7)[.．、]/

function sectionNum(heading) {
  const m = String(heading || '').match(/^(\d+)[.．、]/)
  return m ? Number(m[1]) : null
}

function isCore(heading) {
  return CORE_PREFIX.test(String(heading || '').trim())
}

export default function BibleDesk({ text }) {
  const empty = !String(text || '').trim()
  const { title, sections } = useMemo(() => splitMarkdownByH2(text), [text])
  const [active, setActive] = useState('')
  const bodyRef = useRef(null)

  useEffect(() => {
    if (!sections.length) return
    setActive(sections[0].heading)
  }, [sections])

  if (empty) {
    return (
      <div className="bible-desk desk-empty">
        <p>尚无世界观。立项后会在此按编号章节展示。</p>
      </div>
    )
  }

  if (!sections.length) {
    return (
      <div className="bible-desk">
        <MarkdownView source={text} variant="reader" />
      </div>
    )
  }

  const openQ = sections.find((s) => {
    const n = sectionNum(s.heading)
    return n === 7 || String(s.heading).includes('开放问题')
  })
  const main = sections.filter((s) => s !== openQ)
  const shown = active
    ? sections.find((s) => s.heading === active) || sections[0]
    : sections[0]

  function jump(heading) {
    setActive(heading)
    bodyRef.current?.scrollTo?.(0, 0)
  }

  return (
    <div className="bible-desk" aria-label="世界观百科">
      <header className="bible-desk-head">
        <h3>{title || '世界观'}</h3>
      </header>
      <div className="bible-desk-layout">
        <nav className="bible-toc" aria-label="章节目录">
          {sections.map((s) => {
            const core = isCore(s.heading)
            return (
              <button
                key={s.heading}
                type="button"
                className={[
                  'bible-toc-item',
                  active === s.heading ? 'active' : '',
                  core ? 'is-core' : '',
                ].filter(Boolean).join(' ')}
                onClick={() => jump(s.heading)}
              >
                <span className="bible-toc-label">{s.heading}</span>
                {core ? <span className="bible-toc-core">核心</span> : null}
              </button>
            )
          })}
        </nav>
        <div className="bible-body" ref={bodyRef}>
          {shown ? (
            <section className="bible-section">
              <h4>{shown.heading}</h4>
              <MarkdownView source={shown.body} variant="reader" />
            </section>
          ) : (
            main.map((s) => (
              <section key={s.heading} className="bible-section">
                <h4>{s.heading}</h4>
                <MarkdownView source={s.body} variant="reader" />
              </section>
            ))
          )}
        </div>
      </div>
      {openQ && active !== openQ.heading ? (
        <footer className="bible-open-q">
          <button type="button" className="bible-open-q-btn" onClick={() => jump(openQ.heading)}>
            <strong>开放问题</strong>
            <span>{openQ.body.replace(/\s+/g, ' ').trim().slice(0, 80) || '（点击查看）'}</span>
          </button>
        </footer>
      ) : null}
    </div>
  )
}
