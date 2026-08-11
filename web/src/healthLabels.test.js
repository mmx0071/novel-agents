import { describe, expect, it } from 'vitest'
import {
  auditTierZh,
  foreshadowClassZh,
  foreshadowHealthLevel,
  foreshadowPhaseZh,
  impactScanZh,
  lengthHealthLevel,
  qualityTierZh,
  volumeHealthLevel,
  volumeQaPhaseZh,
  worstHealthLevel,
} from './healthLabels.js'

describe('healthLabels', () => {
  it('maps tiers for authors', () => {
    expect(qualityTierZh('economy')).toBe('精简')
    expect(auditTierZh('layered')).toBe('分层审阅')
    expect(impactScanZh('volume')).toBe('本卷范围')
    expect(foreshadowClassZh('near')).toBe('宜尽快收')
    expect(volumeQaPhaseZh('mid_due')).toBe('建议卷中复盘')
    expect(foreshadowPhaseZh('pressure_high')).toBe('压力偏高')
  })

  it('computes traffic-light levels', () => {
    expect(foreshadowHealthLevel({ dangling_pressure: 0, open_cold: 0 })).toBe('ok')
    expect(foreshadowHealthLevel({ dangling_pressure: 10, open_cold: 0 })).toBe('warn')
    expect(foreshadowHealthLevel({ dangling_pressure: 0, open_cold: 0 }, 'pressure_high')).toBe('bad')
    expect(volumeHealthLevel({ active_index: 1, chapters_in_volume: 2 })).toBe('ok')
    expect(volumeHealthLevel({ active_index: 1, chapters_in_volume: 2 }, 'mid_due')).toBe('warn')
    expect(volumeHealthLevel(null, 'handoff_required')).toBe('bad')
    expect(volumeHealthLevel(null)).toBe('warn')
    expect(lengthHealthLevel({ soft_short_rate: 0, consecutive_soft_short: 0 })).toBe('ok')
    expect(worstHealthLevel('ok', 'warn', 'ok')).toBe('warn')
    expect(worstHealthLevel('warn', 'bad')).toBe('bad')
  })
})
