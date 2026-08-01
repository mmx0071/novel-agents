import { useCallback, useEffect, useState } from 'react'
import {
  CHAPTER_WORD_HARD_MIN,
  CHAPTER_WORD_MAX,
  CHAPTER_WORD_MIN,
} from '../chapterTargets'

/**
 * Author-facing writing preferences (read-mostly).
 * Engine YAML editing stays in the engine drawer.
 */
export default function WritingPrefs({
  project,
  longformHealth,
  publishedCount,
  nextChapter,
  onOpenEngine,
  onAuditChapter,
}) {
  const longform = longformHealth?.longform || null
  const length = longformHealth?.length || null
  const volume = longformHealth?.volume || null
  const [nodes, setNodes] = useState([])
  const [nodesBusy, setNodesBusy] = useState(false)
  const [nodesErr, setNodesErr] = useState('')

  const loadNodes = useCallback(async () => {
    if (!project) {
      setNodes([])
      return
    }
    setNodesBusy(true)
    setNodesErr('')
    try {
      const res = await fetch(`/api/projects/${encodeURIComponent(project)}/version_nodes?limit=20`)
      const data = await res.json().catch(() => ({}))
      if (!res.ok || data.ok === false) {
        setNodesErr(data.error || `加载失败 (${res.status})`)
        setNodes([])
      } else {
        setNodes(Array.isArray(data.nodes) ? data.nodes : [])
      }
    } catch (e) {
      setNodesErr(String(e?.message || e))
      setNodes([])
    } finally {
      setNodesBusy(false)
    }
  }, [project])

  useEffect(() => {
    loadNodes()
  }, [loadNodes])

  async function restoreNode(sha) {
    if (!project || !sha) return
    const short = sha.slice(0, 10)
    if (!window.confirm(`回退到版本 ${short}？将先打安全点，再还原工作树文件。`)) {
      return
    }
    setNodesBusy(true)
    setNodesErr('')
    try {
      const res = await fetch(
        `/api/projects/${encodeURIComponent(project)}/version_nodes/${encodeURIComponent(sha)}/restore`,
        { method: 'POST' },
      )
      const data = await res.json().catch(() => ({}))
      if (!res.ok || data.ok === false) {
        setNodesErr(data.error || `回退失败 (${res.status})`)
      } else {
        await loadNodes()
      }
    } catch (e) {
      setNodesErr(String(e?.message || e))
    } finally {
      setNodesBusy(false)
    }
  }

  return (
    <div className="writing-prefs" aria-label="写作偏好">
      <h2 className="side-title">写作偏好</h2>
      <p className="side-hint">
        篇幅与吞吐目标；改模型 / 硬规则 / 能力包请打开引擎室。
      </p>

      {!project ? (
        <p className="writing-prefs-empty">选中小说后显示本书写作参数。</p>
      ) : (
        <>
          <section className="writing-prefs-card">
            <h3>章篇幅</h3>
            <ul>
              <li>目标 {CHAPTER_WORD_MIN}–{CHAPTER_WORD_MAX} 字</li>
              <li>发布硬门 ≥{CHAPTER_WORD_HARD_MIN} 字</li>
              {length ? (
                <li>
                  近期偏短率 {Math.round((length.soft_short_rate || 0) * 100)}%
                  {length.consecutive_soft_short
                    ? ` · 连短 ${length.consecutive_soft_short}`
                    : ''}
                </li>
              ) : null}
            </ul>
            <p className="writing-prefs-note">与 config/chapter.yaml 默认一致（只读展示）。</p>
          </section>

          <section className="writing-prefs-card">
            <h3>长篇吞吐</h3>
            <ul>
              <li>
                质量档 {longform?.quality_tier || '—'}
                {longform?.audit_tier ? ` · 审校档 ${longform.audit_tier}` : ''}
              </li>
              <li>
                影响扫描 {longform?.impact_scan_mode || '—'}
                {longform?.drift_samples != null ? ` · 漂移样本 ${longform.drift_samples}` : ''}
              </li>
              {volume?.active_index ? (
                <li>
                  当前卷 #{volume.active_index}
                  {volume.chapters_in_volume != null
                    ? ` · 本卷已写 ${volume.chapters_in_volume} 章`
                    : ''}
                </li>
              ) : (
                <li>卷况见书库「长篇健康」</li>
              )}
            </ul>
            <p className="writing-prefs-note">来自项目 longform 快照；细调在引擎室 / YAML。</p>
          </section>

          <section className="writing-prefs-card">
            <h3>进度</h3>
            <ul>
              <li>已发布 {publishedCount || 0} 章</li>
              <li>下一章第 {nextChapter || 1} 章</li>
            </ul>
            <div className="writing-prefs-actions">
              {typeof onAuditChapter === 'function' && (publishedCount > 0 || nextChapter > 1) ? (
                <button
                  type="button"
                  className="btn-ghost btn-inline"
                  onClick={() => onAuditChapter(
                    publishedCount > 0
                      ? publishedCount
                      : Math.max(1, (nextChapter || 1) - 1),
                  )}
                >
                  审校最近章
                </button>
              ) : null}
              <button
                type="button"
                className="btn-primary btn-inline"
                onClick={onOpenEngine}
              >
                打开引擎室
              </button>
            </div>
          </section>

          <section className="writing-prefs-card">
            <h3>版本节点</h3>
            <p className="writing-prefs-note">
              本地 shadow git；改盘前与章通过时自动打点，可回退工作树。
            </p>
            <div className="writing-prefs-actions">
              <button
                type="button"
                className="btn-ghost btn-inline"
                disabled={nodesBusy}
                onClick={loadNodes}
              >
                {nodesBusy ? '刷新中…' : '刷新'}
              </button>
            </div>
            {nodesErr ? <p className="writing-prefs-empty">{nodesErr}</p> : null}
            {!nodesErr && nodes.length === 0 ? (
              <p className="writing-prefs-empty">尚无节点（首次改盘后出现）</p>
            ) : null}
            <ul className="version-nodes-list">
              {nodes.map((n) => {
                const short = (n.sha || '').slice(0, 10)
                return (
                  <li key={`${n.sha}-${n.ts}-${n.label}`}>
                    <div className="version-node-meta">
                      <code>{short}</code>
                      <span>{n.label}</span>
                      {n.chapter ? <span>ch{n.chapter}</span> : null}
                    </div>
                    <div className="version-node-summary">
                      {(n.summary || '').slice(0, 80)}
                    </div>
                    <button
                      type="button"
                      className="btn-ghost btn-inline"
                      disabled={nodesBusy}
                      onClick={() => restoreNode(n.sha)}
                    >
                      回退到此
                    </button>
                  </li>
                )
              })}
            </ul>
          </section>
        </>
      )}
    </div>
  )
}
