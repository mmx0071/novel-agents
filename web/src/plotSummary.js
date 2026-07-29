/** Pick the active / in-progress plot card for the writing desk rail. */
export function pickActivePlot(plots) {
  const list = Array.isArray(plots) ? plots : []
  if (!list.length) return null
  const rank = (p) => {
    const s = String(p?.status || '').toLowerCase()
    if (s === 'in_progress' || s === '进行中') return 0
    if (s === 'bridging' || s === '衔接中') return 1
    if (!p?.complete && (s === 'planned' || s === 'planned' || s === '计划中' || !s)) return 2
    return 9
  }
  const sorted = [...list].sort((a, b) => rank(a) - rank(b))
  const top = sorted[0]
  return rank(top) < 9 ? top : null
}

function sectionFromMarkdown(md, headings) {
  const text = String(md || '')
  if (!text.trim()) return ''
  for (const h of headings) {
    const re = new RegExp(
      `##\\s*${h}\\s*\\n([\\s\\S]*?)(?=\\n##\\s|$)`,
      'i',
    )
    const m = text.match(re)
    if (m?.[1]?.trim()) return m[1].trim()
  }
  return ''
}

/** Compact one-line / short paragraph summary for the desk rail. */
export function summarizePlotForDesk(plot, maxChars = 160) {
  if (!plot) return null
  const overview = String(
    plot.overview
    || plot.summary
    || sectionFromMarkdown(plot.markdown, ['概览', '剧情概览', '本段概览'])
    || '',
  ).replace(/\s+/g, ' ').trim()
  const direction = String(
    plot.plot_direction
    || sectionFromMarkdown(plot.markdown, ['剧情走向', '走向'])
    || '',
  ).replace(/\s+/g, ' ').trim()
  let body = overview || direction
  if (overview && direction && direction !== overview) {
    body = `${overview} · ${direction}`
  }
  if (body.length > maxChars) body = `${body.slice(0, maxChars)}…`
  const status = String(plot.status || '').trim()
  const statusLabel = (
    status === 'in_progress' || status === '进行中' ? '进行中'
      : status === 'bridging' || status === '衔接中' ? '衔接中'
        : status === 'completed' || status === '已完成' ? '已完成'
          : status === 'planned' || status === '计划中' || status === '规划中' ? '计划中'
            : status || '剧情卡'
  )
  return {
    title: plot.title || plot.slug || '未命名剧情',
    status,
    statusLabel,
    body: body || '（暂无概览，可在「卷纲」查看完整剧情卡）',
    volume: plot.volume || plot.arc || '',
    chapterRange: plot.chapter_range || '',
  }
}
