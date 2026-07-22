import { useEffect, useRef, useState } from 'react'
import {
  formatArgsSummary,
  getBulkReadNav,
  isBulkContextTool,
} from './toolMarkup'

/**
 * Codex-like tool cell.
 * Bulk-read tools show「正在阅读《书》」+ clickable jumps (世界观 / 第N章·正文 …).
 * Pipeline tools auto-scroll their output pane as steps append.
 */
export default function ToolCallCard({ item, project, onReaderJump }) {
  const status = normalizeStatus(item.status)
  const bulk = isBulkContextTool(item.name)
  const nav = bulk
    ? getBulkReadNav(item.name, item.arguments, item.output, project)
    : null
  const showExpandableBody = !bulk && !!(item.output || formatArgsSummary(item.arguments))
  const [open, setOpen] = useState(
    !bulk
      && (status === 'in_progress'
        || status === 'failed'
        || !!(item.output && String(item.output).length > 80)),
  )
  const outRef = useRef(null)
  const argsSummary = bulk ? '' : formatArgsSummary(item.arguments)

  // Keep open while running so progress stays visible.
  useEffect(() => {
    if (!bulk && status === 'in_progress') setOpen(true)
  }, [bulk, status])

  // Follow latest ▶ / ✓ lines inside the card's own scroll box.
  useEffect(() => {
    if (bulk || !open) return
    const el = outRef.current
    if (!el) return
    el.scrollTop = el.scrollHeight
  }, [bulk, open, item.output, status])

  const jump = (link) => {
    if (!onReaderJump || !link?.readerTab) return
    onReaderJump({
      readerTab: link.readerTab,
      chapter: link.chapter,
      keepSelection: link.chapter == null,
    })
  }

  return (
    <div className={`nx-toolcell nx-toolcell-${status}${bulk ? ' nx-toolcell-bulk' : ''}`}>
      {bulk ? (
        <div className="nx-toolcell-head static">
          <span className="nx-toolcell-bullet" aria-hidden="true">
            {status === 'completed' ? '✓' : status === 'failed' ? '✕' : '•'}
          </span>
          <span className="nx-toolcell-verb">{verbForBulk(status)}</span>
          <span className="nx-toolcell-name nx-toolcell-readlabel">
            {status === 'in_progress' && !nav ? (
              <>正在阅读《{project || item.arguments?.project || '小说'}》…</>
            ) : nav ? (
              <>
                {nav.prefix}
                {nav.links?.length ? (
                  <span className="nx-read-jumps">
                    {nav.links.map((link) => (
                      <button
                        key={`${link.readerTab}:${link.label}`}
                        type="button"
                        className="nx-read-jump"
                        onClick={(e) => {
                          e.stopPropagation()
                          jump(link)
                        }}
                        title={`打开${link.label}`}
                      >
                        {link.label}
                      </button>
                    ))}
                  </span>
                ) : null}
              </>
            ) : (
              item.name
            )}
          </span>
          {item.duration_ms != null && status !== 'in_progress' ? (
            <span className="nx-toolcell-dur">{formatDur(item.duration_ms)}</span>
          ) : null}
          {status === 'in_progress' ? <span className="nx-toolcell-spin" aria-label="running" /> : null}
        </div>
      ) : (
        <button
          type="button"
          className="nx-toolcell-head"
          onClick={() => showExpandableBody && setOpen((v) => !v)}
          aria-expanded={showExpandableBody ? open : undefined}
        >
          <span className="nx-toolcell-bullet" aria-hidden="true">
            {status === 'completed' ? '✓' : status === 'failed' ? '✕' : '•'}
          </span>
          <span className="nx-toolcell-verb">{verbFor(status)}</span>
          <span className="nx-toolcell-name">{item.name}</span>
          {argsSummary ? (
            <span className="nx-toolcell-args">· {argsSummary}</span>
          ) : null}
          {item.duration_ms != null && status !== 'in_progress' ? (
            <span className="nx-toolcell-dur">{formatDur(item.duration_ms)}</span>
          ) : null}
          {status === 'in_progress' ? <span className="nx-toolcell-spin" aria-label="running" /> : null}
          {showExpandableBody ? (
            <span className="nx-toolcell-chevron">{open ? '▾' : '▸'}</span>
          ) : null}
        </button>
      )}
      {open && showExpandableBody && (
        <div className="nx-toolcell-body">
          {item.output ? (
            <pre className="nx-toolcell-output" ref={outRef}>
              <span className="nx-toolcell-branch">└ </span>
              {item.output}
            </pre>
          ) : status === 'in_progress' ? (
            <pre className="nx-toolcell-output nx-muted" ref={outRef}>
              <span className="nx-toolcell-branch">└ </span>
              执行中…
            </pre>
          ) : null}
        </div>
      )}
    </div>
  )
}

function normalizeStatus(s) {
  if (s === 'completed' || s === 'Completed') return 'completed'
  if (s === 'failed' || s === 'Failed') return 'failed'
  if (s === 'cancelled' || s === 'Cancelled') return 'cancelled'
  if (s === 'in_progress' || s === 'inProgress' || s === 'InProgress' || s == null || s === '') {
    return 'in_progress'
  }
  return 'in_progress'
}

function verbFor(status) {
  if (status === 'completed') return 'Ran'
  if (status === 'failed') return 'Failed'
  if (status === 'cancelled') return 'Cancelled'
  return 'Running'
}

function verbForBulk(status) {
  if (status === 'failed') return 'Failed'
  if (status === 'cancelled') return 'Cancelled'
  if (status === 'completed') return 'Read'
  return 'Reading'
}

function formatDur(ms) {
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}
