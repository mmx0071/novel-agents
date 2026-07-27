import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

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
      <ReactMarkdown remarkPlugins={[remarkGfm]} components={mdComponents}>
        {text}
      </ReactMarkdown>
    </div>
  )
}

const mdComponents = {
  a({ href, children }) {
    const safe = typeof href === 'string' && /^(https?:|mailto:|#)/i.test(href)
    if (!safe) return <span>{children}</span>
    return (
      <a href={href} target="_blank" rel="noreferrer noopener">
        {children}
      </a>
    )
  },
  table({ children }) {
    return (
      <div className="nx-md-table-wrap">
        <table>{children}</table>
      </div>
    )
  },
}
