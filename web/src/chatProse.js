/**
 * Chat-facing prose cleanup for Studio agent bubbles:
 * - drop planning preambles that leaked before the real report
 * - turn bare「标签：值」blocks into 2-col tables (rendered as .nx-kv)
 */

const PLANNING_CUE = /按规则第|所以我要调用|可以并行|禁止\s*continue_writing|询问进度\/|这是询问进度|独立调用/

/** Agent bubbles that only announce / restate an audit-queue To-do list. */
export function looksLikeAuditQueueLaunchProse(text) {
  const t = String(text || '').trim()
  if (!t) return false
  if (/审阅队列\s*(?:To-dos|待办)|(?:To-dos|待办)[：:]/.test(t) && /第\s*\d+\s*章|逐章推进/.test(t)) {
    return true
  }
  if (/启动全部章节逐章审阅|逐章审阅队列|审阅队列（第\s*\d+/.test(t)) return true
  if (/项目共\s*\d+\s*章/.test(t) && /审阅/.test(t)) return true
  const lines = t.split(/\n/).map((l) => l.trim()).filter(Boolean)
  if (lines.length < 3) return false
  const chapterBullets = lines.filter((l) => /^[-*·•]?\s*审[阅校]第\s*\d+\s*章/.test(l))
  return chapterBullets.length >= 3 && chapterBullets.length >= lines.length - 3
}

/** True if a line looks like `标签：值` / `标签: 值` (short label). */
export function isKvLine(line) {
  const t = String(line || '').trim()
  if (!t || t.startsWith('#') || t.startsWith('-') || t.startsWith('*') || /^\d+\./.test(t)) {
    return false
  }
  const m = t.match(/^([^：:]{1,16})[：:]\s*(.+)$/)
  if (!m) return false
  const key = m[1].trim()
  // Avoid treating full sentences as keys.
  if (/[。！？；]/.test(key)) return false
  if (key.length > 14) return false
  return true
}

export function looksLikePlanning(text) {
  const t = String(text || '').trim()
  if (!t) return false
  if (PLANNING_CUE.test(t)) return true
  if (/^用户要求[「"'"]/.test(t)) return true
  return false
}

/**
 * If the reply starts with meta-planning then a markdown heading, keep from the heading.
 * Also strips a leading block that duplicates a sibling reasoning item.
 */
export function stripPlanningPreamble(text, reasoningTexts = []) {
  let t = String(text || '').replace(/^\uFEFF/, '').trim()
  if (!t) return t

  for (const raw of reasoningTexts || []) {
    const r = String(raw || '').trim()
    if (r.length < 24) continue
    if (t.startsWith(r)) {
      t = t.slice(r.length).replace(/^[\s\n]+/, '')
      break
    }
    // Reasoning often matches the start of the agent bubble with minor drift.
    const head = r.slice(0, Math.min(80, r.length))
    const idx = t.indexOf(head)
    if (idx >= 0 && idx < 40) {
      // Find first markdown heading after the overlap.
      const after = t.slice(idx + head.length)
      const h = after.search(/\n#{1,3}\s+\S/)
      if (h >= 0) {
        t = after.slice(h + 1).trim()
        break
      }
    }
  }

  const headingAt = t.search(/^#{1,3}\s+\S/m)
  if (headingAt > 0) {
    const before = t.slice(0, headingAt).trim()
    if (looksLikePlanning(before)) {
      t = t.slice(headingAt).trim()
    }
  }

  // Soft-wrap case: planning + "## title" without newline (seen in streams).
  const glued = t.match(/^(.*?)(#{1,3}\s+\S[\s\S]*)$/)
  if (glued && looksLikePlanning(glued[1]) && glued[2].startsWith('#')) {
    t = glued[2].trim()
  }

  return t
}

function escapeCell(s) {
  return String(s || '').replace(/\|/g, '\\|').replace(/\n+/g, ' ').trim()
}

function nextNonEmpty(lines, from) {
  for (let j = from; j < lines.length; j += 1) {
    const t = lines[j].trim()
    if (t) return { index: j, text: t }
  }
  return null
}

function isSectionTitle(trimmed) {
  if (!trimmed) return false
  if (trimmed.startsWith('#') || isKvLine(trimmed)) return false
  if (trimmed.startsWith('-') || trimmed.startsWith('*') || /^\d+\.\s/.test(trimmed)) return false
  if (trimmed.length > 18) return false
  if (/[。！？；：:]$/.test(trimmed)) return false
  return true
}

/** Convert consecutive kv lines into a GFM 2-col table; promote short section titles. */
export function structureStatusProse(text) {
  const src = String(text || '')
  if (!src.trim()) return src
  const lines = src.split('\n')
  const out = []
  let i = 0

  while (i < lines.length) {
    const line = lines[i]
    const trimmed = line.trim()

    // Short bare title followed (soon) by kv lines → ### heading
    if (isSectionTitle(trimmed)) {
      const nxt = nextNonEmpty(lines, i + 1)
      let promote = false
      if (nxt && isKvLine(nxt.text)) promote = true
      else if (nxt) {
        // e.g. 当前主线卡 → 第1卷「…」 → 收束条件：…
        const nxt2 = nextNonEmpty(lines, nxt.index + 1)
        if (nxt2 && isKvLine(nxt2.text) && nxt.text.length <= 40) promote = true
      }
      if (promote) {
        out.push('')
        out.push(`### ${trimmed}`)
        i += 1
        continue
      }
    }

    if (isKvLine(trimmed)) {
      const rows = []
      while (i < lines.length && isKvLine(lines[i])) {
        const m = lines[i].trim().match(/^([^：:]{1,16})[：:]\s*(.+)$/)
        rows.push([m[1].trim(), m[2].trim()])
        i += 1
      }
      out.push('')
      out.push('| 项 | 内容 |')
      out.push('| --- | --- |')
      for (const [k, v] of rows) {
        out.push(`| ${escapeCell(k)} | ${escapeCell(v)} |`)
      }
      out.push('')
      continue
    }

    out.push(line)
    i += 1
  }

  return out.join('\n').replace(/\n{3,}/g, '\n\n').trim()
}

/** Full pipeline for agent chat bubbles. */
export function prepareChatAgentProse(text, reasoningTexts = []) {
  const stripped = stripPlanningPreamble(text, reasoningTexts)
  return structureStatusProse(stripped)
}

/**
 * Display order: trailing「思考」must not sit after the final NovelX reply.
 * Move reasoning items that appear after the last agent_message to just before it.
 */
export function orderTurnItemsForDisplay(items) {
  const list = Array.isArray(items) ? [...items] : []
  if (list.length < 2) return list

  let lastAgent = -1
  for (let i = list.length - 1; i >= 0; i -= 1) {
    if (list[i]?.type === 'agent_message' && String(list[i].text || '').trim()) {
      lastAgent = i
      break
    }
  }
  if (lastAgent < 0) return list

  const trailing = []
  for (let i = list.length - 1; i > lastAgent; i -= 1) {
    if (list[i]?.type === 'reasoning' && String(list[i].text || '').trim()) {
      trailing.push(list[i])
      list.splice(i, 1)
    }
  }
  if (!trailing.length) return list
  trailing.reverse()
  // Indices ≤ lastAgent unchanged (we only removed items after it).
  list.splice(lastAgent, 0, ...trailing)
  return list
}
