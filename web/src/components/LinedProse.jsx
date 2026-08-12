import { useEffect, useMemo, useRef } from 'react'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

function splitLines(text) {
  const s = String(text ?? '')
  if (!s) return ['']
  return s.split('\n')
}

function lineKind(line) {
  const t = String(line ?? '')
  if (/^#{1,6}\s/.test(t)) return 'heading'
  if (/^\s*[-*+]\s/.test(t) || /^\s*\d+\.\s/.test(t)) return 'list'
  if (!t.trim()) return 'blank'
  return 'body'
}

const lineMdComponents = {
  p({ children }) {
    return <span className="lined-md-inline">{children}</span>
  },
  h1({ children }) { return <span className="lined-md-h lined-md-h1">{children}</span> },
  h2({ children }) { return <span className="lined-md-h lined-md-h2">{children}</span> },
  h3({ children }) { return <span className="lined-md-h lined-md-h3">{children}</span> },
  h4({ children }) { return <span className="lined-md-h lined-md-h4">{children}</span> },
  ul({ children }) { return <span className="lined-md-inline">{children}</span> },
  ol({ children }) { return <span className="lined-md-inline">{children}</span> },
  li({ children }) { return <span className="lined-md-inline">{children}</span> },
  a({ href, children }) {
    const safe = typeof href === 'string' && /^(https?:|mailto:|#)/i.test(href)
    if (!safe) return <span>{children}</span>
    return (
      <a href={href} target="_blank" rel="noreferrer noopener">
        {children}
      </a>
    )
  },
}

/**
 * Writing-desk reader with source line numbers (1-based).
 * Renders each source line so numbers stay aligned with the file.
 */
export function LinedProseView({ text = '', className = '' }) {
  const lines = useMemo(() => splitLines(text), [text])
  const width = String(Math.max(lines.length, 1)).length

  return (
    <div
      className={`lined-prose ${className}`.trim()}
      role="region"
      aria-label="带行号正文"
    >
      {lines.map((line, i) => {
        const kind = lineKind(line)
        return (
          <div key={`L${i + 1}`} className={`lined-prose-row kind-${kind}`}>
            <span
              className="lined-prose-num"
              style={{ minWidth: `${width + 1}ch` }}
              aria-hidden="true"
            >
              {i + 1}
            </span>
            <div className="lined-prose-text">
              {kind === 'blank' ? (
                <span className="lined-prose-blank">{'\u00a0'}</span>
              ) : (
                <ReactMarkdown remarkPlugins={[remarkGfm]} components={lineMdComponents}>
                  {line}
                </ReactMarkdown>
              )}
            </div>
          </div>
        )
      })}
    </div>
  )
}

/**
 * Textarea editor with a scroll-synced line-number gutter.
 */
export function LinedProseEditor({
  value = '',
  onChange,
  className = '',
  'aria-label': ariaLabel = '编辑写作台内容',
}) {
  const gutterRef = useRef(null)
  const areaRef = useRef(null)
  const lines = useMemo(() => splitLines(value), [value])
  const width = String(Math.max(lines.length, 1)).length

  const syncGutter = () => {
    const g = gutterRef.current
    const a = areaRef.current
    if (g && a) g.scrollTop = a.scrollTop
  }

  useEffect(() => {
    syncGutter()
  }, [value, lines.length])

  return (
    <div className={`lined-editor ${className}`.trim()}>
      <div className="lined-editor-gutter" ref={gutterRef} aria-hidden="true">
        {lines.map((_, i) => (
          <div
            key={`N${i + 1}`}
            className="lined-editor-num"
            style={{ minWidth: `${width + 1}ch` }}
          >
            {i + 1}
          </div>
        ))}
      </div>
      <textarea
        ref={areaRef}
        className="reader-editor lined-editor-area"
        value={value}
        onChange={onChange}
        onScroll={syncGutter}
        spellCheck={false}
        aria-label={ariaLabel}
      />
    </div>
  )
}

export default LinedProseView
