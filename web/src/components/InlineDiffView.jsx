import { useEffect, useMemo, useRef, useState } from 'react'
import MarkdownView from './MarkdownView'
import {
  buildPartialAfter,
  groupDiffBlocks,
  hunkSummary,
  lineDiffOps,
} from '../lineDiff'

/**
 * Cursor-style inline diff: no chrome box; Cancel / Apply under each hunk.
 * Apply / Cancel on a single hunk only updates local state until none remain;
 * then parent finalizes (cm_apply / PUT / cm_discard).
 */
export default function InlineDiffView({
  before = '',
  after = '',
  label = '',
  onSelectionChange,
  onHunkApply,
  onHunkCancel,
  busy = false,
}) {
  const rootRef = useRef(null)
  const blocks = useMemo(
    () => groupDiffBlocks(lineDiffOps(before, after)),
    [before, after],
  )
  const hunkIds = useMemo(
    () => blocks.filter((b) => b.type === 'hunk').map((b) => b.id),
    [blocks],
  )
  /** Hunks user dismissed with Cancel (keep original). */
  const [dismissed, setDismissed] = useState(() => new Set())
  /** Hunks user accepted with Apply (take revision). */
  const [accepted, setAccepted] = useState(() => new Set())

  useEffect(() => {
    setDismissed(new Set())
    setAccepted(new Set())
  }, [hunkIds.join('|')])

  const pendingIds = useMemo(
    () => hunkIds.filter((id) => !dismissed.has(id) && !accepted.has(id)),
    [hunkIds, dismissed, accepted],
  )

  useEffect(() => {
    if (typeof onSelectionChange !== 'function') return
    // 全部应用 = 已接受 + 仍待决（未 Cancel）的块。
    const forFull = new Set(hunkIds.filter((id) => !dismissed.has(id)))
    const fullAfter = buildPartialAfter(blocks, forFull)
    onSelectionChange({
      hunkIds,
      selectedIds: [...forFull],
      partialAfter: buildPartialAfter(blocks, accepted),
      fullAfter,
      selectedCount: forFull.size,
      totalCount: hunkIds.length,
      pendingCount: pendingIds.length,
      acceptedCount: accepted.size,
    })
  }, [blocks, hunkIds, dismissed, accepted, pendingIds, onSelectionChange])

  useEffect(() => {
    const el = rootRef.current?.querySelector('[data-diff-anchor="1"]')
    if (el && typeof el.scrollIntoView === 'function') {
      el.scrollIntoView({ block: 'center', behavior: 'smooth' })
    }
  }, [before, after])

  const finalize = (nextAccepted, nextDismissed) => {
    const remaining = hunkIds.filter(
      (id) => !nextDismissed.has(id) && !nextAccepted.has(id),
    )
    if (remaining.length) return
    if (nextAccepted.size === 0) {
      onHunkCancel?.(null, { remaining: [], allCancelled: true })
      return
    }
    const afterText = buildPartialAfter(blocks, nextAccepted)
    onHunkApply?.(null, {
      after: afterText,
      remaining: [],
      allAccepted: nextAccepted.size === hunkIds.length,
      acceptedCount: nextAccepted.size,
    })
  }

  const applyHunk = (id) => {
    if (busy || accepted.has(id) || dismissed.has(id)) return
    const nextAccepted = new Set([...accepted, id])
    setAccepted(nextAccepted)
    finalize(nextAccepted, dismissed)
  }

  const cancelHunk = (id) => {
    if (busy || accepted.has(id) || dismissed.has(id)) return
    const nextDismissed = new Set([...dismissed, id])
    setDismissed(nextDismissed)
    finalize(accepted, nextDismissed)
  }

  if (!hunkIds.length) {
    return (
      <MarkdownView
        variant="reader"
        source={after || before || '（无变更）'}
      />
    )
  }

  let anchored = false
  return (
    <div className="inline-diff-view" ref={rootRef} aria-label={label || '内联变更对照'}>
      {blocks.map((block, idx) => {
        if (block.type === 'equal') {
          if (!String(block.text || '').trim()) return null
          return (
            <div key={`eq-${idx}`} className="inline-diff-equal">
              <MarkdownView variant="reader" source={block.text} />
            </div>
          )
        }
        if (dismissed.has(block.id)) {
          const kept = (block.dels || []).join('\n')
          if (!kept.trim()) return null
          return (
            <div key={block.id} className="inline-diff-equal">
              <MarkdownView variant="reader" source={kept} />
            </div>
          )
        }
        if (accepted.has(block.id)) {
          const kept = (block.ins || []).join('\n')
          if (!kept.trim()) return null
          return (
            <div key={block.id} className="inline-diff-equal is-accepted">
              <MarkdownView variant="reader" source={kept} />
            </div>
          )
        }
        const isAnchor = !anchored
        if (isAnchor) anchored = true
        const summary = hunkSummary(block)
        const lines = Array.isArray(block.lines) && block.lines.length
          ? block.lines
          : [
            ...block.dels.map((text) => ({ kind: 'del', text })),
            ...block.ins.map((text) => ({ kind: 'ins', text })),
          ]
        return (
          <div
            key={block.id}
            className="inline-diff-hunk"
            data-diff-anchor={isAnchor ? '1' : undefined}
          >
            <div className="inline-diff-hunk-body" role="table" aria-label={summary}>
              {lines.map((hl, i) => {
                const mark = hl.kind === 'del' ? '−' : hl.kind === 'ins' ? '+' : ' '
                const rowCls = hl.kind === 'del'
                  ? 'is-del'
                  : hl.kind === 'ins'
                    ? 'is-ins'
                    : 'is-ctx'
                return (
                  <div
                    key={`${hl.kind}-${i}`}
                    className={`inline-diff-row ${rowCls}`}
                    role="row"
                  >
                    <span className="inline-diff-gutter" aria-hidden="true">{mark}</span>
                    <pre className="inline-diff-code">{hl.text || ' '}</pre>
                  </div>
                )
              })}
            </div>
            <div className="inline-diff-hunk-actions">
              <button
                type="button"
                className="inline-diff-btn inline-diff-btn-cancel"
                disabled={busy}
                onClick={() => cancelHunk(block.id)}
              >
                Cancel
              </button>
              <button
                type="button"
                className="inline-diff-btn inline-diff-btn-apply"
                disabled={busy}
                onClick={() => applyHunk(block.id)}
              >
                Apply
              </button>
            </div>
          </div>
        )
      })}
    </div>
  )
}
