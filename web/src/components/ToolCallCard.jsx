import { useEffect, useMemo, useRef, useState } from 'react'
import {
  formatArgsSummary,
  getBulkReadNav,
  isBulkContextTool,
} from './toolMarkup'
import { toolLabelZh, toolVerbZh } from './toolLabels'
import ToolProgressList from './ToolProgressList'
import { parseToolProgress, summarizeToolProgress } from '../toolProgress.js'

/**
 * Codex/Cursor-like tool cell.
 * Pipeline tools lift ▶/✓ streams into a nested step list (not a raw wall of text).
 */
export default function ToolCallCard({
  item,
  project,
  onReaderJump,
  omitQueueProgress = false,
  compact = false,
  /** When To-dos own micro steps, hide this tool's structured progress entirely. */
  hideStructuredProgress = false,
}) {
  const status = normalizeStatus(item.status)
  const bulk = isBulkContextTool(item.name)
  const nav = bulk
    ? getBulkReadNav(item.name, item.arguments, item.output, project)
    : null
  const parsed = useMemo(
    () => (bulk
      ? { entries: [], structured: false }
      : parseToolProgress(item.output, { omitQueue: omitQueueProgress })),
    [bulk, item.output, omitQueueProgress],
  )
  const stepSummary = useMemo(
    () => summarizeToolProgress(parsed.entries),
    [parsed.entries],
  )
  const showExpandableBody = !bulk && (
    status === 'in_progress'
    || status === 'failed'
    || parsed.structured
    || !!(item.output || formatArgsSummary(item.arguments))
  )
  const [open, setOpen] = useState(() => !bulk && status === 'in_progress')
  const outRef = useRef(null)
  const argsSummary = bulk ? '' : formatArgsSummary(item.arguments)

  useEffect(() => {
    if (bulk) return
    setOpen(status === 'in_progress')
  }, [bulk, status])

  const scrollRafRef = useRef(0)
  useEffect(() => {
    if (bulk || !open || parsed.structured) return
    if (scrollRafRef.current) cancelAnimationFrame(scrollRafRef.current)
    scrollRafRef.current = requestAnimationFrame(() => {
      scrollRafRef.current = 0
      const el = outRef.current
      if (el) el.scrollTop = el.scrollHeight
    })
    return () => {
      if (scrollRafRef.current) cancelAnimationFrame(scrollRafRef.current)
    }
  }, [bulk, open, item.output, status, parsed.structured])

  const jump = (link) => {
    if (!onReaderJump || !link?.readerTab) return
    onReaderJump({
      readerTab: link.readerTab,
      chapter: link.chapter,
      keepSelection: link.chapter == null,
    })
  }

  // Inside「处理中」work card: structured pipeline = steps only (no 运行中/args/spinner head).
  const headless = compact && parsed.structured
  // Todos already render N major tasks + current micro steps — hide host pipeline card.
  if (hideStructuredProgress) {
    return null
  }

  return (
    <div className={`nx-toolcell nx-toolcell-${status}${bulk ? ' nx-toolcell-bulk' : ''}${parsed.structured ? ' nx-toolcell-structured' : ''}${compact ? ' nx-toolcell-compact' : ''}${headless ? ' nx-toolcell-headless' : ''}`}>
      {bulk ? (
        <div className="nx-toolcell-head static">
          <span className="nx-toolcell-bullet" aria-hidden="true">
            {status === 'completed' ? '✓' : status === 'failed' ? '✕' : '•'}
          </span>
          <span className="nx-toolcell-verb">{toolVerbZh(status, { bulk: true })}</span>
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
              toolLabelZh(item.name)
            )}
          </span>
          {item.duration_ms != null && status !== 'in_progress' ? (
            <span className="nx-toolcell-dur">{formatDur(item.duration_ms)}</span>
          ) : null}
          {status === 'in_progress' ? <span className="nx-toolcell-spin" aria-label="进行中" /> : null}
        </div>
      ) : headless ? null : (
        <button
          type="button"
          className="nx-toolcell-head"
          onClick={() => showExpandableBody && setOpen((v) => !v)}
          aria-expanded={showExpandableBody ? open : undefined}
        >
          <span className="nx-toolcell-bullet" aria-hidden="true">
            {status === 'completed' ? '✓' : status === 'failed' ? '✕' : '•'}
          </span>
          {!compact ? (
            <span className="nx-toolcell-verb">{toolVerbZh(status)}</span>
          ) : null}
          <span className="nx-toolcell-name">{toolLabelZh(item.name)}</span>
          {!open && stepSummary ? (
            <span className="nx-toolcell-args">· {stepSummary}</span>
          ) : (!compact && argsSummary) ? (
            <span className="nx-toolcell-args">· {argsSummary}</span>
          ) : null}
          {item.duration_ms != null && status !== 'in_progress' ? (
            <span className="nx-toolcell-dur">{formatDur(item.duration_ms)}</span>
          ) : null}
          {/* Structured pipeline: only the tip step spins (plus outer「处理中」). */}
          {status === 'in_progress' && !parsed.structured ? (
            <span className="nx-toolcell-spin" aria-label="进行中" />
          ) : null}
          {showExpandableBody ? (
            <span className="nx-toolcell-chevron">{open ? '▾' : '▸'}</span>
          ) : null}
        </button>
      )}
      {(headless || (open && showExpandableBody)) && (
        <div className="nx-toolcell-body">
          {parsed.structured ? (
            <ToolProgressList
              entries={parsed.entries}
              live={status === 'in_progress'}
            />
          ) : item.output ? (
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

function formatDur(ms) {
  if (ms < 1000) return `${ms}ms`
  return `${(ms / 1000).toFixed(1)}s`
}
