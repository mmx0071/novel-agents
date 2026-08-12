import { useMemo, useState } from 'react'
import MarkdownView from './MarkdownView'
import { mutationKindZh } from './toolLabels'
import {
  groupDiffBlocks,
  hunkSummary,
  lineDiffOps,
  resolveInlineDiffPair,
} from '../lineDiff'

/** Mutation preview — same line-diff semantics as writing-desk InlineDiffView. */
export default function MutationPreviewCard({ item }) {
  const fields = item?.fields && typeof item.fields === 'object' ? item.fields : null
  const markdown = String(item?.markdown || '').trim()
  const title = mutationKindZh(item?.kind) || item?.topic || item?.title || item?.name || '设定变更'

  const blocks = useMemo(() => {
    const { before, after } = resolveInlineDiffPair({
      beforeFull: item?.before_full || item?.beforeFull || '',
      afterFull: item?.after_full || item?.afterFull || '',
      before: '',
      after: markdown,
      diffs: Array.isArray(item?.diffs) ? item.diffs : [],
    })
    if (before || after) {
      return groupDiffBlocks(lineDiffOps(before, after), { context: 2 })
    }
    // Legacy: only crude diffs[] — still normalize so shared context ≠ false −/+.
    return blocksFromLegacyDiffs(item?.diffs)
  }, [item, markdown])

  const hunks = useMemo(() => blocks.filter((b) => b.type === 'hunk'), [blocks])
  const summary = summarizeMutation(fields, markdown, hunks, item)
  const [openDetail, setOpenDetail] = useState(false)
  const hasHunks = hunks.length > 0
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
      {hasHunks ? (
        <div className="nx-mutation-diff-sync" aria-label="变更对照">
          {hunks.map((hunk) => (
            <div
              key={hunk.id}
              className={`nx-mutation-hunk-sync kind-${hunk.changeKind || 'replace'}`}
            >
              <div className="nx-mutation-hunk-sync-head">
                {hunkSummary(hunk)}
                {hunk.changeKind === 'insert' ? ' · 纯新增' : ''}
              </div>
              <div className="nx-mutation-hunk-sync-body">
                {(hunk.lines || []).map((hl, i) => {
                  if (hl.kind === 'ctx') {
                    return (
                      <div key={`c-${i}`} className="nx-diff-row is-ctx">
                        <span className="nx-diff-gutter" aria-hidden="true"> </span>
                        <pre>{hl.text || ' '}</pre>
                      </div>
                    )
                  }
                  const mark = hl.kind === 'del' ? '−' : '+'
                  return (
                    <div
                      key={`${hl.kind}-${i}`}
                      className={`nx-diff-row ${hl.kind === 'del' ? 'is-del' : 'is-ins'}`}
                    >
                      <span className="nx-diff-gutter" aria-hidden="true">{mark}</span>
                      <pre>{hl.text || ' '}</pre>
                    </div>
                  )
                })}
              </div>
            </div>
          ))}
        </div>
      ) : null}
      {!hasHunks && hasDetail ? (
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
      ) : null}
      {!hasHunks && !hasDetail ? (
        <div className="nx-card-output muted">（暂无预览内容）</div>
      ) : null}
      <div className="nx-mutation-hint muted">
        与写作台同一套对照：灰=上下文，绿+=新增，红−=删减。纯新增可无红行。
      </div>
    </div>
  )
}

/** Fallback when only {before,after} hunk blobs exist (no before_full). */
function blocksFromLegacyDiffs(diffs) {
  if (!Array.isArray(diffs) || !diffs.length) return []
  return diffs.map((d, idx) => {
    const before = String(d?.before || '')
    const after = String(d?.after || '')
    // Re-diff the blob pair so shared context lines become ctx, not −/+.
    const sub = groupDiffBlocks(lineDiffOps(before, after), { context: 1 })
      .filter((b) => b.type === 'hunk')
    if (sub.length) {
      return { ...sub[0], id: `h${idx + 1}` }
    }
    const dels = before ? before.split('\n') : []
    const ins = after ? after.split('\n') : []
    return {
      type: 'hunk',
      id: `h${idx + 1}`,
      dels,
      ins,
      changeKind: !dels.length && ins.length
        ? 'insert'
        : (dels.length && !ins.length ? 'delete' : 'replace'),
      lines: [
        ...dels.map((text) => ({ kind: 'del', text })),
        ...ins.map((text) => ({ kind: 'ins', text })),
      ],
    }
  })
}

function summarizeMutation(fields, markdown, hunks, item) {
  if (Array.isArray(hunks) && hunks.length) {
    const topic = String(item?.topic || item?.title || item?.name || '').trim()
    const kinds = hunks.map(hunkSummary).slice(0, 2).join('；')
    if (topic) return `对照变更：${topic}（${kinds}）`
    return `对照变更：${kinds}`
  }
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
