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
        </>
      )}
    </div>
  )
}
