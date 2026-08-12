import { useCallback, useEffect, useState } from 'react'
import { wordTargetsForMode } from '../chapterTargets'
import {
  auditTierZh,
  foreshadowClassZh,
  foreshadowPhaseZh,
  impactScanZh,
  qualityTierZh,
  volumeQaPhaseZh,
} from '../healthLabels'
import { toolLabelZh } from './toolLabels'

export const HEALTH_LEVEL_LABEL = { ok: '正常', warn: '警告', bad: '异常' }

/** Format cost_by_agent for status page. */
export function formatCostTop(costByAgent) {
  if (!Array.isArray(costByAgent)) return ''
  return costByAgent
    .filter((c) => c?.agent && c.agent !== 'pipeline')
    .slice(0, 4)
    .map((c) => `${toolLabelZh(c.agent)}约 ${Math.round((c.approx_tokens || 0) / 1000)}k`)
    .join(' · ')
}

function VersionHistory({ project, unitLabel }) {
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
    if (!window.confirm('回退到这份存档？会先自动留一份安全点，再还原文件。')) {
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
    <section className="writing-prefs-card">
      <h3>历史版本</h3>
      <p className="writing-prefs-note">
        重要改动前会自动存档，出问题时可一键回退。
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
        <p className="writing-prefs-empty">尚无存档（首次改设定或通过一章后会出现）</p>
      ) : null}
      <ul className="version-nodes-list">
        {nodes.map((n) => {
          const when = n.ts
            ? new Date(n.ts).toLocaleString('zh-CN', {
              month: 'numeric',
              day: 'numeric',
              hour: '2-digit',
              minute: '2-digit',
            })
            : ''
          return (
            <li key={`${n.sha}-${n.ts}-${n.label}`}>
              <div className="version-node-meta">
                <span>{n.label || '存档'}</span>
                {when ? <span>{when}</span> : null}
                {n.chapter ? <span>第{n.chapter}{unitLabel}</span> : null}
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
  )
}

function StatusActions({
  publishedCount,
  nextChapter,
  unitLabel,
  onAuditChapter,
  onOpenEngine,
}) {
  return (
    <section className="writing-prefs-card">
      <h3>进度</h3>
      <ul>
        <li>已发布 {publishedCount || 0} {unitLabel}</li>
        <li>下一{unitLabel}第 {nextChapter || 1} {unitLabel}</li>
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
            {`审校最近一${unitLabel}`}
          </button>
        ) : null}
        {typeof onOpenEngine === 'function' ? (
          <button
            type="button"
            className="btn-primary btn-inline"
            onClick={onOpenEngine}
          >
            打开设置
          </button>
        ) : null}
      </div>
      <p className="writing-prefs-note">改模型与写作规则在「设置」里。</p>
    </section>
  )
}

/**
 * Left-rail status page: health + progress + version history (merged from former 偏好).
 */
function loopLevel(loop) {
  if (!loop?.hasJob) return 'ok'
  if (loop.requiresHuman) return 'bad'
  if (loop.resumable || loop.pendingWake) return 'warn'
  if (loop.status === 'running') return 'warn'
  return 'ok'
}

function LoopStatusCard({ loop, onArmWake }) {
  if (!loop?.hasJob) return null
  const level = loopLevel(loop)
  const unit = loop.unit || '章'
  const stop = loop.lastStop?.reason || ''
  const soft = Array.isArray(loop.softGatesSkipped) && loop.softGatesSkipped.length
    ? loop.softGatesSkipped.join(', ')
    : ''
  const verify = loop.loopEndVerify?.summary || ''
  return (
    <div className={`longform-health-card level-${level}`}>
      <div className="longform-health-title">
        <span>连写外环</span>
        <span className={`longform-health-badge level-${level}`}>
          {loop.statusLabel || '已停止'}
        </span>
      </div>
      <div className="longform-health-body">
        本批已发布 {loop.chaptersDone ?? 0} {unit}
        {stop ? ` · 停于 ${stop}` : ''}
      </div>
      <div className="longform-health-sub">
        {soft ? `软跳过：${soft}` : '硬门仍会停；软相位可按无人值守策略跳过'}
        {verify ? ` · ${verify}` : ''}
      </div>
      {loop.requiresHuman && loop.resumable && typeof onArmWake === 'function' ? (
        <div className="longform-health-actions" style={{ marginTop: 8 }}>
          <button type="button" className="btn-ghost btn-inline" onClick={onArmWake}>
            允许自动续写
          </button>
        </div>
      ) : null}
    </div>
  )
}

export function CreationStatusPage({
  project,
  isShortDrama,
  foreshadowDebt,
  foreshadowLevel,
  foreshadowPhase = '',
  foreshadowDebtExpanded,
  onToggleForeshadowExpanded,
  volumeHealth,
  volumeLevel,
  volumeQaPhase = '',
  lengthHealth,
  lengthLevel,
  lengthChip,
  longformTier,
  longformLevel,
  costTop,
  publishedCount,
  nextChapter,
  onOpenEngine,
  onAuditChapter,
  showHeading = true,
  loopStatus = null,
  onArmLoopWake,
}) {
  const unitLabel = isShortDrama ? '集' : '章'
  const wordBand = wordTargetsForMode(isShortDrama ? 'short_drama' : 'longform')

  if (!project) {
    return (
      <div className="side-status-page">
        {showHeading ? (
          <>
            <h2 className="side-title">创作状态</h2>
            <p className="side-hint">先打开一本小说，这里会汇总进度、篇幅与健康概况。</p>
          </>
        ) : null}
        <p className="empty-list side-status-empty">暂无作品</p>
      </div>
    )
  }

  if (isShortDrama) {
    return (
      <div className="side-status-page writing-prefs">
        {showHeading ? (
          <>
            <h2 className="side-title">创作状态</h2>
            <p className="side-hint">
              漫剧按「集」推进；进度、篇幅目标与历史版本集中在这里。
            </p>
          </>
        ) : null}
        <div className="longform-health longform-health--side">
          <div className="longform-health-card level-ok">
            <div className="longform-health-title">
              <span>模式</span>
              <span className="longform-health-badge level-ok">漫剧</span>
            </div>
            <div className="longform-health-body">AI 漫剧剧本 · 以集为单位</div>
            <div className="longform-health-sub">
              目标 {wordBand.min}–{wordBand.max} 字 · 发布至少 {wordBand.hardMin} 字
            </div>
          </div>
          <LoopStatusCard loop={loopStatus} onArmWake={onArmLoopWake} />
        </div>
        <StatusActions
          publishedCount={publishedCount}
          nextChapter={nextChapter}
          unitLabel={unitLabel}
          onAuditChapter={onAuditChapter}
          onOpenEngine={onOpenEngine}
        />
        <VersionHistory project={project} unitLabel={unitLabel} />
      </div>
    )
  }

  return (
    <div className="side-status-page writing-prefs">
      {showHeading ? (
        <>
          <h2 className="side-title">创作状态</h2>
          <p className="side-hint">进度、伏笔、本卷与篇幅概况；历史版本也可在此回退。</p>
        </>
      ) : null}
      <div className="longform-health longform-health--side">
        <LoopStatusCard loop={loopStatus} onArmWake={onArmLoopWake} />
        <div
          className={`longform-health-card longform-health-card--foreshadow level-${foreshadowLevel}${
            foreshadowDebtExpanded ? ' is-expanded' : ''
          }`}
        >
          <div className="longform-health-title">
            <span>未收伏笔</span>
            <span className={`longform-health-badge level-${foreshadowLevel}`}>
              {HEALTH_LEVEL_LABEL[foreshadowLevel]}
            </span>
          </div>
          <div className="longform-health-body">
            共 {foreshadowDebt?.dangling_total ?? 0} 条
            {foreshadowDebt?.dangling_pressure != null
              ? ` · 近期该收 ${foreshadowDebt.dangling_pressure}`
              : ''}
            {foreshadowDebt?.open_cold ? ` · 久未回收 ${foreshadowDebt.open_cold}` : ''}
            {foreshadowPhaseZh(foreshadowPhase)
              ? ` · 相位 ${foreshadowPhaseZh(foreshadowPhase)}`
              : ''}
          </div>
          <div className="longform-health-sub">
            刚埋下 {foreshadowDebt?.dangling_fresh ?? 0}
            {' · '}宜回收 {foreshadowDebt?.dangling_mid ?? 0}
            {' · '}可稍后 {foreshadowDebt?.dangling_far ?? 0}
            <span className="longform-health-debt-hint">
              （批量续写会优先拦近期该收的）
            </span>
          </div>
          {Array.isArray(foreshadowDebt?.oldest) && foreshadowDebt.oldest.length > 0 && (
            <>
              <ul
                className={`longform-health-list${
                  foreshadowDebtExpanded ? ' is-expanded' : ''
                }`}
              >
                {(foreshadowDebtExpanded
                  ? foreshadowDebt.oldest
                  : foreshadowDebt.oldest.slice(0, 5)
                ).map((t) => {
                  const cls = foreshadowClassZh(t.debt_class)
                  return (
                    <li key={t.id || `${t.planted_chapter}-${t.text}`}>
                      {cls ? (
                        <span
                          className={`longform-debt-tag debt-${t.debt_class || 'fresh'}`}
                        >
                          {cls}
                        </span>
                      ) : null}
                      第{t.planted_chapter || '?'}章 · {t.text}
                    </li>
                  )
                })}
              </ul>
              {foreshadowDebt.oldest.length > 5 && (
                <button
                  type="button"
                  className="longform-health-more"
                  onClick={onToggleForeshadowExpanded}
                >
                  {foreshadowDebtExpanded
                    ? '收起'
                    : `展开全部 ${foreshadowDebt.dangling_total ?? foreshadowDebt.oldest.length}`}
                </button>
              )}
            </>
          )}
        </div>
        <div className={`longform-health-card level-${volumeLevel}`}>
          <div className="longform-health-title">
            <span>本卷进度</span>
            <span className={`longform-health-badge level-${volumeLevel}`}>
              {HEALTH_LEVEL_LABEL[volumeLevel]}
            </span>
          </div>
          <div className="longform-health-body">
            {volumeHealth?.active_index
              ? `第${volumeHealth.active_index}卷 · 已写 ${volumeHealth.chapters_in_volume || 0} 章`
              : '还没有进行中的卷'}
            {volumeHealth?.has_audit_report ? ' · 已做过复盘' : ''}
            {volumeHealth?.thick_volume_warning ? ' · 本卷偏长' : ''}
            {volumeQaPhaseZh(volumeQaPhase)
              ? ` · ${volumeQaPhaseZh(volumeQaPhase)}`
              : ''}
          </div>
          <div className="longform-health-sub">
            卷内摘要 {volumeHealth?.rollup_total ?? 0} 条
            {volumeHealth?.mid_audit_threshold
              ? ` · 写满 ${volumeHealth.mid_audit_threshold} 章建议复盘一次`
              : ''}
          </div>
        </div>
        <div className={`longform-health-card level-${lengthLevel || 'ok'}`}>
          <div className="longform-health-title">
            <span>章节篇幅</span>
            <span className={`longform-health-badge level-${lengthLevel || 'ok'}`}>
              {HEALTH_LEVEL_LABEL[lengthLevel] || '正常'}
            </span>
          </div>
          <div className="longform-health-body">
            {lengthChip || '—'}
            {` · 发布至少 ${lengthHealth?.word_hard_min ?? wordBand.hardMin} 字`}
          </div>
          <div className="longform-health-sub">
            目标 {lengthHealth?.word_min ?? wordBand.min}–{lengthHealth?.word_max ?? wordBand.max} 字
            {lengthHealth?.target_chapters
              ? ` · 规划约 ${lengthHealth.target_chapters} 章`
              : ''}
          </div>
        </div>
        {longformTier ? (
          <div className={`longform-health-card level-${longformLevel}`}>
            <div className="longform-health-title">
              <span>创作档位</span>
              <span className={`longform-health-badge level-${longformLevel}`}>
                {HEALTH_LEVEL_LABEL[longformLevel]}
              </span>
            </div>
            <div className="longform-health-body">
              质量 {qualityTierZh(longformTier.quality_tier)}
              {longformTier.audit_tier
                ? ` · ${auditTierZh(longformTier.audit_tier)}`
                : ''}
            </div>
            <div className="longform-health-sub">
              设定联动检查 {impactScanZh(longformTier.impact_scan_mode)}
              {longformTier.drift_samples != null
                ? ` · 抽查 ${longformTier.drift_samples} 处`
                : ''}
            </div>
          </div>
        ) : null}
        {costTop ? (
          <div className="longform-health-card level-ok">
            <div className="longform-health-title">
              <span>近期模型用量</span>
              <span className="longform-health-badge level-ok">参考</span>
            </div>
            <div className="longform-health-body longform-health-cost">
              {costTop}
              <div className="longform-health-sub">单位为模型用量约数，不是正文字数</div>
            </div>
          </div>
        ) : null}
      </div>
      <StatusActions
        publishedCount={publishedCount}
        nextChapter={nextChapter}
        unitLabel={unitLabel}
        onAuditChapter={onAuditChapter}
        onOpenEngine={onOpenEngine}
      />
      <VersionHistory project={project} unitLabel={unitLabel} />
    </div>
  )
}

function StatusKv({ label, value, level }) {
  return (
    <span className={`studio-status-chip${level ? ` level-${level}` : ''}`}>
      <span className="studio-status-key">{label}</span>
      <span className="studio-status-sep">：</span>
      <span className="studio-status-value">{value}</span>
    </span>
  )
}

/**
 * Bottom status bar: key:value core chips; detail opens the status page.
 * Longform: 状态 / 进度 / 伏笔 / 本卷 / 篇幅.
 * Short drama (漫剧): 模式 / 进度 / 下一集.
 */
export function CreationStatusBar({
  visible,
  isShortDrama = false,
  overallLevel,
  foreshadowDebt,
  foreshadowLevel,
  volumeHealth,
  volumeLevel,
  lengthLevel,
  lengthChip,
  progressLabel,
  nextEpisodeLabel,
  onOpenDetail,
  loopStatus = null,
}) {
  if (!visible) return null
  const loopChip = loopStatus?.hasJob
    ? (
      <StatusKv
        label="连写"
        value={loopStatus.statusLabel || '已停止'}
        level={loopLevel(loopStatus)}
      />
    )
    : null

  const chips = isShortDrama
    ? (
      <>
        <StatusKv label="模式" value="漫剧" />
        {progressLabel ? <StatusKv label="进度" value={progressLabel} /> : null}
        {nextEpisodeLabel ? (
          <StatusKv label="下一集" value={nextEpisodeLabel} />
        ) : null}
        {loopChip}
      </>
    )
    : (() => {
      const total = foreshadowDebt?.dangling_total ?? 0
      const pressure = foreshadowDebt?.dangling_pressure
      const foreshadowValue = pressure != null && pressure > 0
        ? `${total} 条（近期该收 ${pressure}）`
        : `${total} 条`
      const volumeValue = volumeHealth?.active_index
        ? `第${volumeHealth.active_index}卷 · 已写 ${volumeHealth.chapters_in_volume || 0} 章`
        : '尚未定卷'
      const lengthValue = lengthChip
        || (lengthLevel === 'ok' ? '正常' : (HEALTH_LEVEL_LABEL[lengthLevel] || '正常'))
      return (
        <>
          <StatusKv
            label="状态"
            value={HEALTH_LEVEL_LABEL[overallLevel] || '正常'}
            level={overallLevel}
          />
          {progressLabel ? (
            <StatusKv label="进度" value={progressLabel} />
          ) : null}
          {loopChip}
          <StatusKv label="未收伏笔" value={foreshadowValue} level={foreshadowLevel} />
          <StatusKv label="本卷" value={volumeValue} level={volumeLevel} />
          <StatusKv label="篇幅" value={lengthValue} level={lengthLevel} />
        </>
      )
    })()

  return (
    <footer className="studio-status-bar" aria-label="创作状态摘要">
      <button
        type="button"
        className="studio-status-bar-core"
        onClick={onOpenDetail}
        title="查看创作状态详情"
      >
        {chips}
      </button>
      <button
        type="button"
        className="studio-status-detail-btn"
        onClick={onOpenDetail}
      >
        详情
      </button>
    </footer>
  )
}
