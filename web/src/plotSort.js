/**
 * Group arcs + plots by volume for the reader nav.
 * Returns [{ volume, title, arc, plots }] sorted by volume.
 */
export function buildVolumePlotGroups(arcOutlines, plots) {
  const arcs = [...(arcOutlines || [])].filter((a) => a?.volume >= 1)
  const sortedPlots = sortPlotsByProgress(plots || [])
  const plotsByVol = new Map()
  for (const p of sortedPlots) {
    const vol = Number(p.volume_index || p.arc_index || 1) || 1
    if (!plotsByVol.has(vol)) plotsByVol.set(vol, [])
    plotsByVol.get(vol).push(p)
  }
  const volumes = new Set([
    ...arcs.map((a) => Number(a.volume)),
    ...plotsByVol.keys(),
  ])
  return [...volumes]
    .filter((v) => v >= 1)
    .sort((a, b) => a - b)
    .map((vol) => {
      const arc = arcs.find((a) => Number(a.volume) === vol) || null
      const title = arc?.title || `第${vol}卷`
      return {
        volume: vol,
        title,
        shortLabel: `第${vol}卷`,
        arc: arc || { volume: vol, title, markdown: '（尚无本卷卷纲正文）' },
        plots: plotsByVol.get(vol) || [],
      }
    })
}

/** @deprecated use buildVolumePlotGroups — flat tree for legacy callers */
export function buildVolumePlotTree(arcOutlines, plots) {
  const out = []
  for (const g of buildVolumePlotGroups(arcOutlines, plots)) {
    out.push({
      kind: 'arc',
      key: `v${g.volume}`,
      label: g.title,
      complete: true,
      level: 0,
      volume: g.volume,
      raw: g.arc,
    })
    for (const p of g.plots) {
      const key = p?.slug || p?.id || p?.title || ''
      if (!key) continue
      out.push({
        kind: 'plot',
        key,
        label: p.title || '未命名剧情',
        complete: !!p.complete,
        level: 1,
        volume: g.volume,
        raw: p,
      })
    }
  }
  return out
}

/** Plot status → tie-break rank (lower = earlier when chain ties). */
const PLOT_PROGRESS_RANK = {
  completed: 0,
  bridging: 1,
  in_progress: 2,
  planned: 3,
  abandoned: 4,
}

export function normalizePlotStatus(status) {
  const s = String(status || '').trim().toLowerCase()
  if (['in_progress', 'inprogress', 'active', 'running', '进行中', '进行'].includes(s)) {
    return 'in_progress'
  }
  if (['bridging', 'bridge', '衔接', '衔接中'].includes(s)) return 'bridging'
  if (['completed', 'done', 'finished', '已完成', '完成'].includes(s)) return 'completed'
  if (['abandoned', 'dropped', 'cancelled', '放弃'].includes(s)) return 'abandoned'
  if (['planned', 'plan', 'todo', '规划', '规划中'].includes(s)) return 'planned'
  return s || 'planned'
}

function plotRefMatches(title, slug, ref) {
  const n = String(ref || '').trim()
  if (!n) return false
  const t = String(title || '')
  const s = String(slug || '')
  return n === t || n === s || t.startsWith(n) || n.startsWith(t)
}

function progressRank(status) {
  return PLOT_PROGRESS_RANK[normalizePlotStatus(status)] ?? 5
}

/**
 * Sort plot cards by story progress (same as Rust `sort_plot_entries_by_progress`):
 * volume → next_plot chain (topo) → status → slug/title.
 * Do NOT sort completed cards by Chinese title — that scrambles narrative order.
 */
export function sortPlotsByProgress(plots) {
  const list = [...(plots || [])]
  if (list.length <= 1) return list

  const byVol = new Map()
  list.forEach((p, i) => {
    const vol = Number(p.volume_index || p.arc_index || 1)
    if (!byVol.has(vol)) byVol.set(vol, [])
    byVol.get(vol).push({ p, i })
  })

  const out = []
  ;[...byVol.keys()]
    .sort((a, b) => a - b)
    .forEach((vol) => {
      const group = byVol.get(vol).map(({ p }) => p)
      out.push(...sortVolumePlotsByChain(group))
    })
  return out
}

function sortVolumePlotsByChain(plots) {
  const n = plots.length
  if (n <= 1) return plots

  const incoming = Array(n).fill(0)
  const edges = Array.from({ length: n }, () => [])
  for (let i = 0; i < n; i++) {
    const next = String(plots[i].next_plot || '').trim()
    if (!next) continue
    const j = plots.findIndex(
      (b, idx) => idx !== i && plotRefMatches(b.title, b.slug, next),
    )
    if (j >= 0) {
      edges[i].push(j)
      incoming[j] += 1
    }
  }

  const tie = (a, b) => {
    const ra = progressRank(plots[a].status)
    const rb = progressRank(plots[b].status)
    if (ra !== rb) return ra - rb
    return String(plots[a].slug || plots[a].title || '').localeCompare(
      String(plots[b].slug || plots[b].title || ''),
      'zh',
    )
  }

  const order = []
  let ready = []
  for (let i = 0; i < n; i++) {
    if (incoming[i] === 0) ready.push(i)
  }
  ready.sort(tie)

  while (ready.length) {
    const i = ready.shift()
    order.push(i)
    for (const j of edges[i]) {
      incoming[j] -= 1
      if (incoming[j] === 0) {
        ready.push(j)
        ready.sort(tie)
      }
    }
  }

  if (order.length < n) {
    const rest = []
    for (let i = 0; i < n; i++) {
      if (!order.includes(i)) rest.push(i)
    }
    rest.sort(tie)
    order.push(...rest)
  }

  return order.map((i) => plots[i])
}
