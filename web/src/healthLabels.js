/** Display labels for longform health / prefs (author-facing). */

export function qualityTierZh(tier) {
  const t = String(tier || '').toLowerCase()
  if (t === 'economy') return '精简'
  if (t === 'standard' || t === 'balanced') return '标准'
  if (t === 'full' || t === 'premium') return '精细'
  if (!t) return '—'
  return tier
}

export function auditTierZh(tier) {
  const t = String(tier || '').toLowerCase()
  if (t === 'layered') return '分层审阅'
  if (t === 'full') return '细审'
  if (t === 'light') return '轻量审阅'
  if (!t) return ''
  return tier
}

export function impactScanZh(mode) {
  const t = String(mode || '').toLowerCase()
  if (t === 'indexed') return '按索引'
  if (t === 'volume') return '本卷范围'
  if (t === 'all') return '全书范围'
  if (!t) return '—'
  return mode
}

export function foreshadowClassZh(c) {
  switch (String(c || '').toLowerCase()) {
    case 'near':
      return '宜尽快收'
    case 'mid':
      return '宜回收'
    case 'far':
      return '可稍后'
    case 'fresh':
      return '刚埋下'
    default:
      return ''
  }
}

const LEVEL_RANK = { ok: 0, warn: 1, bad: 2 }

/** Worst of ok / warn / bad. */
export function worstHealthLevel(...levels) {
  let worst = 'ok'
  for (const lv of levels) {
    const key = String(lv || 'ok')
    if ((LEVEL_RANK[key] ?? 0) > (LEVEL_RANK[worst] ?? 0)) worst = key
  }
  return worst
}

export function foreshadowHealthLevel(foreshadowDebt, foreshadowPhase) {
  const phase = String(foreshadowPhase || '').toLowerCase()
  if (phase === 'pressure_high') return 'bad'
  if (phase === 'paydown') return 'warn'
  const pressure = foreshadowDebt?.dangling_pressure ?? 0
  const cold = foreshadowDebt?.open_cold ?? 0
  if (pressure > 24 || cold > 40) return 'bad'
  if (pressure > 8 || cold > 0) return 'warn'
  return 'ok'
}

export function volumeHealthLevel(volumeHealth, volumeQaPhase) {
  const phase = String(volumeQaPhase || '').toLowerCase()
  if (phase === 'handoff_required') return 'bad'
  if (phase === 'mid_due') return 'warn'
  if (!volumeHealth?.active_index) return 'warn'
  if (volumeHealth.thick_volume_warning) return 'bad'
  const ch = volumeHealth.chapters_in_volume || 0
  const mid = volumeHealth.mid_audit_threshold || 0
  if (mid > 0 && ch >= mid && !volumeHealth.has_audit_report) return 'warn'
  return 'ok'
}

/** Author-facing label for preview.volume_qa_phase. */
export function volumeQaPhaseZh(phase) {
  switch (String(phase || '').toLowerCase()) {
    case 'mid_due':
      return '建议卷中复盘'
    case 'handoff_required':
      return '卷末复盘待做'
    case 'ok':
      return '正常'
    default:
      return ''
  }
}

/** Author-facing label for preview.foreshadow_phase. */
export function foreshadowPhaseZh(phase) {
  switch (String(phase || '').toLowerCase()) {
    case 'pressure_high':
      return '压力偏高'
    case 'paydown':
      return '回收中'
    case 'clear':
      return '平稳'
    default:
      return ''
  }
}

export function lengthHealthLevel(lengthHealth) {
  if (!lengthHealth) return 'ok'
  const rate = lengthHealth.soft_short_rate || 0
  const streak = lengthHealth.consecutive_soft_short || 0
  const hard = lengthHealth.recent_hard_short || 0
  if (streak >= 3 || rate > 0.4 || hard >= 2) return 'bad'
  if (streak >= 1 || rate > 0.15 || hard >= 1) return 'warn'
  return 'ok'
}

export function longformTierLevel(longformTier) {
  if (!longformTier) return 'ok'
  const q = String(longformTier.quality_tier || '').toLowerCase()
  const impact = String(longformTier.impact_scan_mode || '').toLowerCase()
  const drift = longformTier.drift_samples ?? 0
  if (impact === 'all' || drift > 80) return 'bad'
  if (q === 'economy' || drift > 20) return 'warn'
  return 'ok'
}
