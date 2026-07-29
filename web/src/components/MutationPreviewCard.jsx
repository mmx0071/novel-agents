import MarkdownView from './MarkdownView'

/** Generic mutation preview (markdown / fields) shown before confirm. */
export default function MutationPreviewCard({ item }) {
  const fields = item?.fields && typeof item.fields === 'object' ? item.fields : null
  const markdown = String(item?.markdown || '').trim()
  return (
    <div className={`nx-card nx-mutation-preview nx-status-${item.status || 'completed'}`}>
      <div className="nx-card-head">
        <span className="nx-card-kind">变更预览</span>
        <span className="nx-card-title">{item.kind || '变更'}</span>
      </div>
      {fields ? (
        <pre className="nx-card-output">{JSON.stringify(fields, null, 2)}</pre>
      ) : null}
      {markdown ? (
        <MarkdownView className="nx-card-output" source={markdown} variant="chat" />
      ) : null}
      {!fields && !markdown ? (
        <div className="nx-card-output muted">（无预览正文）</div>
      ) : null}
    </div>
  )
}
