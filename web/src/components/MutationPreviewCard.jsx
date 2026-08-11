import { useState } from 'react'
import MarkdownView from './MarkdownView'
import { mutationKindZh } from './toolLabels'

/** Mutation preview shown before the author confirms. */
export default function MutationPreviewCard({ item }) {
  const fields = item?.fields && typeof item.fields === 'object' ? item.fields : null
  const markdown = String(item?.markdown || '').trim()
  const title = mutationKindZh(item?.kind)
  const summary = summarizeMutation(fields, markdown)
  const [openDetail, setOpenDetail] = useState(false)
  const hasDetail = !!(fields || markdown)

  return (
    <div className={`nx-card nx-mutation-preview nx-status-${item.status || 'completed'}`}>
      <div className="nx-card-head">
        <span className="nx-card-kind">待确认修改</span>
        <span className="nx-card-title">{title}</span>
      </div>
      {summary ? (
        <div className="nx-card-output muted">{summary}</div>
      ) : null}
      {hasDetail ? (
        <details
          className="nx-mutation-detail"
          open={openDetail}
          onToggle={(e) => setOpenDetail(e.target.open)}
        >
          <summary>查看详细内容</summary>
          {fields ? (
            <ul className="nx-mutation-fields">
              {Object.entries(fields).map(([k, v]) => (
                <li key={k}>
                  <span className="nx-mutation-field-key">{fieldKeyZh(k)}</span>
                  <span className="nx-mutation-field-val">{stringifyField(v)}</span>
                </li>
              ))}
            </ul>
          ) : null}
          {markdown ? (
            <MarkdownView className="nx-card-output" source={markdown} variant="chat" />
          ) : null}
        </details>
      ) : (
        <div className="nx-card-output muted">（暂无预览内容）</div>
      )}
    </div>
  )
}

function summarizeMutation(fields, markdown) {
  if (fields && typeof fields === 'object') {
    const keys = Object.keys(fields)
    if (keys.length) {
      const labels = keys.slice(0, 3).map(fieldKeyZh)
      const more = keys.length > 3 ? ` 等 ${keys.length} 项` : ''
      return `将更新：${labels.join('、')}${more}`
    }
  }
  if (markdown) {
    const line = markdown.replace(/\s+/g, ' ').trim().slice(0, 80)
    return line ? `${line}${markdown.length > 80 ? '…' : ''}` : ''
  }
  return ''
}

function fieldKeyZh(key) {
  const map = {
    title: '标题',
    name: '名称',
    kind: '类型',
    status: '状态',
    content: '内容',
    summary: '摘要',
    path: '路径',
    chapter: '章节',
    body: '正文',
  }
  if (map[key]) return map[key]
  if (/[\u4e00-\u9fff]/.test(key)) return key
  return '字段'
}

function stringifyField(v) {
  if (v == null) return '—'
  if (typeof v === 'string') return v.length > 120 ? `${v.slice(0, 120)}…` : v
  try {
    const s = JSON.stringify(v)
    return s.length > 120 ? `${s.slice(0, 120)}…` : s
  } catch {
    return String(v)
  }
}
