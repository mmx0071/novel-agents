import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { tableAsKvRows } from '../mdastText'

/**
 * Render markdown for reader / chat. Raw HTML is not enabled (safe by default).
 * While agent text is still streaming, prefer a plain <pre> at the call site.
 */
export default function MarkdownView({
  source,
  className = '',
  variant = 'chat', // 'chat' | 'reader'
}) {
  const text = source == null ? '' : String(source)
  if (!text.trim()) {
    return <div className={`nx-md nx-md-${variant} ${className}`.trim()} />
  }
  return (
    <div className={`nx-md nx-md-${variant} ${className}`.trim()}>
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={componentsFor(variant)}>
        {text}
      </ReactMarkdown>
    </div>
  )
}

function componentsFor(variant) {
  return {
    a({ href, children }) {
      const safe = typeof href === 'string' && /^(https?:|mailto:|#)/i.test(href)
      if (!safe) return <span>{children}</span>
      return (
        <a href={href} target="_blank" rel="noreferrer noopener">
          {children}
        </a>
      )
    },
    table({ node, children }) {
      // Narrow chat: 2-col status tables → key/value rows (avoids cramped grids).
      if (variant === 'chat') {
        const rows = tableAsKvRows(node)
        if (rows) {
          return (
            <dl className="nx-kv">
              {rows.map((r, i) => (
                <div key={`${r.key}-${i}`} className="nx-kv-row">
                  <dt>{r.key}</dt>
                  <dd>{r.value}</dd>
                </div>
              ))}
            </dl>
          )
        }
      }
      return (
        <div className="nx-md-table-wrap">
          <table>{children}</table>
        </div>
      )
    },
  }
}
