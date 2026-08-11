import { useEffect, useMemo, useState } from 'react'
import {
  buildChapterStripPage,
  chapterNumbers,
  neighborChapter,
  pageIndexForChapter,
  snapChapterNumber,
} from '../chapterStrip'
import { formatUnitTitle } from '../chapterTargets'

/**
 * Paginated chapter / episode navigator:
 * 5 per page · prev/next chapter · page flip · jump · locate current.
 */
export default function ChapterStrip({
  chapterList,
  selectedChapter,
  nextChapter,
  projectMode,
  unitLabel = '章',
  onSelect,
}) {
  const [page, setPage] = useState(0)
  const [jumpText, setJumpText] = useState('')
  const [pagePinned, setPagePinned] = useState(false)

  const numbers = useMemo(() => chapterNumbers(chapterList), [chapterList])

  // Follow selection unless user is browsing another page.
  useEffect(() => {
    if (pagePinned) return
    setPage(pageIndexForChapter(numbers, selectedChapter))
  }, [selectedChapter, numbers, pagePinned])

  useEffect(() => {
    setJumpText(selectedChapter > 0 ? String(selectedChapter) : '')
  }, [selectedChapter])

  const strip = useMemo(
    () => buildChapterStripPage(chapterList, selectedChapter, page),
    [chapterList, selectedChapter, page],
  )

  const byNumber = useMemo(() => {
    const map = new Map()
    for (const ch of chapterList || []) map.set(ch.number, ch)
    return map
  }, [chapterList])

  if (!chapterList?.length) return null

  const selectChapter = (nOrCh, { follow = true } = {}) => {
    const n = typeof nOrCh === 'number' ? nOrCh : Number(nOrCh?.number)
    const ch = byNumber.get(n)
    if (!ch || typeof onSelect !== 'function') return
    if (follow) setPagePinned(false)
    onSelect(ch)
  }

  const prevChapter = neighborChapter(strip.numbers, selectedChapter, -1)
  const nextChapterNum = neighborChapter(strip.numbers, selectedChapter, 1)
  const canPrevPage = strip.page > 0
  const canNextPage = strip.page < strip.pageCount - 1

  const goPage = (delta) => {
    setPagePinned(true)
    setPage((p) => Math.max(0, Math.min(strip.pageCount - 1, p + delta)))
  }

  const locateCurrent = () => {
    setPagePinned(false)
    setPage(pageIndexForChapter(strip.numbers, selectedChapter))
  }

  const submitJump = (e) => {
    e?.preventDefault?.()
    const n = snapChapterNumber(strip.numbers, jumpText)
    if (!n) return
    setJumpText(String(n))
    selectChapter(n, { follow: true })
  }

  return (
    <div className="chapter-strip" role="tablist" aria-label={unitLabel === '集' ? '分集' : '章节'}>
      <button
        type="button"
        className="ch-nav-btn"
        disabled={!canPrevPage}
        title="上一页"
        aria-label="上一页"
        onClick={() => goPage(-1)}
      >
        «
      </button>
      <button
        type="button"
        className="ch-nav-btn"
        disabled={!prevChapter}
        title={`上一${unitLabel}`}
        aria-label={`上一${unitLabel}`}
        onClick={() => prevChapter && selectChapter(prevChapter)}
      >
        ‹
      </button>

      <div className="chapter-strip-pills">
        {strip.visible.map((num) => {
          const ch = byNumber.get(num)
          if (!ch) return null
          return (
            <button
              key={ch.number}
              type="button"
              role="tab"
              aria-selected={selectedChapter === ch.number}
              className={[
                'ch-pill',
                selectedChapter === ch.number ? 'active' : '',
                ch.status === 'published' ? 'published' : '',
                ch.status === 'draft' ? 'draft' : '',
                ch.status === 'outline' ? 'outline' : '',
                ch.status === 'next' || ch.number === nextChapter ? 'next' : '',
              ].join(' ')}
              onClick={() => selectChapter(ch.number)}
              title={`${formatUnitTitle(ch.number, ch.title, projectMode)} · ${ch.statusLabel}${
                ch.bodyChars ? ` · ${ch.bodyChars}字` : ''
              }`}
            >
              {ch.number}
            </button>
          )
        })}
      </div>

      <button
        type="button"
        className="ch-nav-btn"
        disabled={!nextChapterNum}
        title={`下一${unitLabel}`}
        aria-label={`下一${unitLabel}`}
        onClick={() => nextChapterNum && selectChapter(nextChapterNum)}
      >
        ›
      </button>
      <button
        type="button"
        className="ch-nav-btn"
        disabled={!canNextPage}
        title="下一页"
        aria-label="下一页"
        onClick={() => goPage(1)}
      >
        »
      </button>

      <form className="chapter-strip-jump" onSubmit={submitJump}>
        <label className="chapter-strip-jump-label">
          <span className="sr-only">跳转到{unitLabel}</span>
          <input
            type="number"
            min={1}
            inputMode="numeric"
            value={jumpText}
            onChange={(e) => setJumpText(e.target.value)}
            onBlur={() => {
              if (String(jumpText) !== String(selectedChapter)) submitJump()
            }}
            aria-label={`跳转到${unitLabel}`}
            title={`输入${unitLabel}号后回车跳转`}
          />
        </label>
        <button type="submit" className="ch-nav-btn ch-jump-go" title={`跳转到${unitLabel}`}>
          跳转
        </button>
      </form>

      <button
        type="button"
        className="ch-nav-btn ch-locate-btn"
        disabled={strip.selectedOnPage && !pagePinned}
        title={`定位到当前第${selectedChapter || ''}${unitLabel}`}
        aria-label="定位当前章"
        onClick={locateCurrent}
      >
        当前
      </button>

      <span
        className="chapter-strip-meta"
        title={`第 ${strip.page + 1}/${strip.pageCount} 页 · 共 ${strip.total} ${unitLabel}`}
      >
        {strip.page + 1}/{strip.pageCount}页 · {selectedChapter || '—'}/{strip.total}
      </span>
    </div>
  )
}
