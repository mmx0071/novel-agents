/**
 * Parse Studio audit checklist markdown into structured issues for the desk dock.
 * Matches format_audit_issue_checklist lines:
 *   1. `p0-xxx` [P0/META] message（location）
 */

const ISSUE_LINE = /^\s*(\d+)\.\s*`([^`]+)`\s*\[(P[012])(?:\/([^\]]+))?\]\s*(.+?)\s*$/

export function parseAuditChecklist(text) {
  const raw = String(text || '')
  if (!raw.includes('问题清单') && !/第\d+章审校/.test(raw)) {
    return null
  }
  const chapterMatch = raw.match(/第\s*(\d+)\s*章审校/)
  const chapter = chapterMatch ? Number(chapterMatch[1]) : 0
  const failed = /未通过/.test(raw) || /阻断\s*\*\*\d+/.test(raw)
  const passed = /审校通过/.test(raw) && !failed
  const issues = []
  for (const line of raw.split('\n')) {
    const m = line.match(ISSUE_LINE)
    if (!m) continue
    let rest = m[5] || ''
    let location = ''
    const locMatch = rest.match(/（([^）]+)）\s*$/)
    if (locMatch) {
      location = locMatch[1]
      rest = rest.slice(0, locMatch.index).trim()
    }
    issues.push({
      index: Number(m[1]),
      id: m[2],
      severity: m[3],
      type: m[4] || '',
      message: rest,
      location,
    })
  }
  return {
    chapter,
    passed,
    failed: failed || (!passed && issues.length > 0),
    summary: raw.split('\n').slice(0, 4).join('\n').slice(0, 200),
    report: raw,
    issues,
  }
}

function chapterFromToolItem(it) {
  const args = String(it?.args || it?.arguments || '')
  const out = String(it?.output || '')
  const m = args.match(/\bchapter["\s:=]+(\d+)/i)
    || args.match(/第\s*(\d+)\s*章/)
    || out.match(/第\s*(\d+)\s*章/)
  const n = m ? Number(m[1]) : 0
  return n > 0 ? n : 0
}

/** True when revise_chapter / steer_run has already written patches to disk. */
export function isReviseAppliedItem(it) {
  if (!it || it.type !== 'tool_call') return false
  if (it.name !== 'revise_chapter' && it.name !== 'steer_run') return false
  if (it.status && it.status !== 'completed') return false
  const out = String(it.output || '')
  const args = String(it.args || it.arguments || '')
  if (/已应用/.test(out)) return true
  if (/\bapply\s*=\s*true\b/i.test(args) && /局部修订|修订/.test(`${args}\n${out}`)) {
    return !/待确认|修订预览|needs_confirm/.test(out)
  }
  return false
}

/** Collect latest audit snapshot from chat turns (+ optional queue todos). */
export function collectAuditState(turns, todos) {
  const reports = []
  let openApproval = null
  let seq = 0
  let lastFail = null
  let lastRevise = null
  for (const turn of turns || []) {
    if (turn?.approval?.options?.length) {
      openApproval = {
        prompt: turn.approval.prompt || '',
        options: turn.approval.options,
        turnId: turn.id,
      }
    }
    for (const it of turn.items || []) {
      seq += 1
      if (it?.type === 'audit_report' && it.report) {
        const parsed = parseAuditChecklist(it.report) || {
          chapter: Number(it.chapter) || 0,
          passed: !!it.passed,
          failed: !it.passed,
          summary: '',
          report: it.report,
          issues: [],
        }
        reports.push(parsed)
        if (parsed.failed) {
          lastFail = { chapter: Number(parsed.chapter) || 0, seq }
        }
      } else if (
        (it?.type === 'agent_message' || it?.type === 'reasoning')
        && String(it.text || '').includes('问题清单')
      ) {
        const parsed = parseAuditChecklist(it.text)
        if (parsed) {
          reports.push(parsed)
          if (parsed.failed) {
            lastFail = { chapter: Number(parsed.chapter) || 0, seq }
          }
        }
      } else if (isReviseAppliedItem(it)) {
        lastRevise = { chapter: chapterFromToolItem(it), seq }
      }
    }
  }
  const latestByChapter = new Map()
  for (const r of reports) {
    const key = r.chapter || 0
    latestByChapter.set(key, r)
  }
  const reviseAppliedAfterFail = Boolean(
    lastFail
    && lastRevise
    && lastRevise.seq > lastFail.seq
    && lastFail.chapter > 0
    && lastRevise.chapter === lastFail.chapter,
  )
  return {
    todos: Array.isArray(todos) ? todos : [],
    reports: [...latestByChapter.values()],
    latest: reports.length ? reports[reports.length - 1] : null,
    openApproval,
    reviseAppliedAfterFail,
    reviseChapter: lastRevise?.chapter || 0,
  }
}
