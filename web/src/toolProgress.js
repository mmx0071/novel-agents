/**
 * Parse streamed tool output (▶/✓/⚙/draft spam) into Cursor-like step rows.
 * Genre-neutral — no work-title special cases.
 */

const QUEUE_CHAPTER_LINE = /^[▶·]\s*第\d+章/
const STEP_START = /^▶\s+(.+)$/
const STEP_DONE = /^✓\s+([^:：]+)[:：]\s*(.*)$/
const STEP_DONE_BARE = /^✓\s+(.+)$/
const STEP_FAIL = /^✕\s+(.+)$/
const STEP_GEAR = /^⚙\s+(.+)$/
const STEP_PAUSE = /^⏸\s+(.+)$/
const DRAFT_TICK = /^↻\s*draft\b/i
const INDENT_LIVE = /^(?:调用模型中|仍在等待首包|生成中|正文生成中|正文已写入)/

/**
 * @returns {{
 *   entries: Array<{
 *     kind: 'queue'|'step'|'note',
 *     mark?: string,
 *     name?: string,
 *     summary?: string,
 *     live?: string,
 *     status?: 'running'|'done'|'failed'|'paused'|'note',
 *   }>,
 *   structured: boolean,
 * }}
 */
export function parseToolProgress(output, opts = {}) {
  const text = String(output || '')
  if (!text.trim()) return { entries: [], structured: false }
  // Audit streams default to dropping chapter checklist / milestones — To-dos own that.
  // Pass omitQueue:false to keep raw queue rows (tests / debug).
  const looksAudit = /审阅队列/.test(text) || /▶\s*第\d+章/.test(text)
  const omitQueue = opts.omitQueue === false
    ? false
    : !!(opts.omitQueue || opts.compactChapters || looksAudit)
  const compactHistory = opts.compactHistory !== false && omitQueue

  const entries = []
  let curStep = null
  let inQueueBlock = false
  let queueEntry = null
  let pendingPause = false
  let currentChapter = 0

  const pushStep = (step) => {
    entries.push(step)
    curStep = step
  }

  const onChapterAdvance = (ch) => {
    const n = Number(ch) || 0
    if (!n) return
    if (compactHistory && currentChapter > 0 && n > currentChapter) {
      collapsePriorChapterSteps(entries, currentChapter)
      curStep = entries.length ? entries[entries.length - 1] : null
    }
    currentChapter = n
  }

  const pushPause = (rawPause) => {
    let pause = String(rawPause || '').trim()
    if (/请按问题选择|请在下方|选择处理项/.test(pause)) {
      pause = pause
        .replace(/，?请按问题选择处理项.*$/, '')
        .replace(/，?请在下方.*$/, '')
        .trim()
      pause = `${truncate(pause || '待选择处理项', 48)} · 见下方选项`
    } else {
      pause = truncate(pause, 100)
    }
    pushStep({
      kind: 'step',
      mark: '⏸',
      name: pause,
      status: 'paused',
    })
  }

  const lines = text.split(/\n/)
  for (const raw of lines) {
    const line = raw.replace(/\s+$/, '')
    const t = line.trim()
    if (!t || t === '——' || t === '---') {
      inQueueBlock = false
      continue
    }
    if (pendingPause) {
      pendingPause = false
      pushPause(t)
      continue
    }
    if (t === '⏸' || t === '⏸️') {
      pendingPause = true
      continue
    }

    // Queue header variants (omit project title — keep progress genre-neutral in UI).
    // When To-dos already host the chapter checklist, skip these rows entirely.
    const qMd = t.match(/^##\s*审阅队列[《「].+?[》」][（(](\d+)\s*\/\s*(\d+)[）)]/)
    const qLine = t.match(/^审阅队列[：:]\s*第(\d+)章[（(](\d+)\s*\/\s*(\d+)[）)]/)
    if (qMd || qLine) {
      inQueueBlock = true
      const ch = Number((qMd && qMd[1]) || (qLine && qLine[1]) || 0)
      onChapterAdvance(ch)
      if (omitQueue) {
        curStep = null
        continue
      }
      // Keep a single rolling queue header (latest chapter only).
      const title = qMd
        ? `审阅队列（${qMd[1]}/${qMd[2]}）`
        : `审阅队列：第${qLine[1]}章（${qLine[2]}/${qLine[3]}）`
      if (queueEntry && entries.includes(queueEntry)) {
        queueEntry.name = title
        queueEntry.summary = ''
      } else {
        queueEntry = {
          kind: 'queue',
          mark: '•',
          name: title,
          summary: '',
          status: 'note',
        }
        entries.push(queueEntry)
      }
      curStep = null
      continue
    }
    if (inQueueBlock) {
      // Queue body: checklist /「当前」. Exit as soon as a real step line appears.
      if (/^当前[：:]/.test(t) || QUEUE_CHAPTER_LINE.test(t) || t.startsWith('·')) {
        if (!omitQueue && /^当前[：:]/.test(t) && queueEntry) {
          queueEntry.summary = t.replace(/^当前[：:]\s*/, '')
        } else if (!omitQueue && QUEUE_CHAPTER_LINE.test(t) && t.startsWith('▶') && queueEntry) {
          const note = t.replace(/^▶\s*/, '')
          queueEntry.summary = queueEntry.summary
            ? `${queueEntry.summary} · ${note}`
            : note
        }
        continue
      }
      inQueueBlock = false
      // fall through — this line is the next pipeline step / note
    }

    if (/^【评审团】/.test(t) || /^评审团自动修订中/.test(t) || /^正在接续审阅队列/.test(t) || /^正在复审/.test(t)) {
      pushStep({
        kind: 'note',
        mark: '•',
        name: t,
        status: 'note',
      })
      continue
    }

    // Chapter milestones — To-dos own the queue; skip duplicates when omitQueue.
    // Note: do not use \b after 章 — JS word-boundary is unreliable for CJK.
    const chStart = t.match(/^▶\s*(第\d+章.*)$/)
    if (chStart) {
      onChapterAdvance(chapterNum(chStart[1]))
      if (omitQueue) continue
      settleOpenSteps(entries)
      const name = chStart[1].trim()
      const prev = findChapterStep(entries, name)
      if (prev) {
        prev.mark = '▶'
        prev.name = name
        prev.summary = ''
        prev.live = ''
        prev.status = 'note'
        curStep = prev
      } else {
        pushStep({
          kind: 'step',
          mark: '▶',
          name,
          summary: '',
          live: '',
          status: 'note',
        })
      }
      continue
    }
    const chDone = t.match(/^✓\s*(第\d+章.*)$/)
    if (chDone) {
      onChapterAdvance(chapterNum(chDone[1]))
      if (omitQueue) continue
      const name = chDone[1].trim()
      const prev = findChapterStep(entries, name)
      if (prev) {
        prev.mark = '✓'
        prev.name = name
        prev.live = ''
        prev.status = /未通过|失败/.test(name) ? 'failed' : 'done'
        curStep = prev
      } else {
        pushStep({
          kind: 'step',
          mark: '✓',
          name,
          status: /未通过|失败/.test(name) ? 'failed' : 'done',
        })
      }
      continue
    }

    let m = t.match(STEP_START)
    if (m) {
      // Only one live pipeline step at a time — close prior ▶ rows.
      settleOpenSteps(entries)
      pushStep({
        kind: 'step',
        mark: '▶',
        name: m[1].trim(),
        summary: '',
        live: '',
        status: 'running',
      })
      continue
    }

    m = t.match(STEP_DONE)
    if (m) {
      const name = m[1].trim()
      const summary = (m[2] || '').trim()
      const prev = findOpenStep(entries, name) || (
        curStep && curStep.kind === 'step' && sameStepName(curStep.name, name) ? curStep : null
      )
      if (prev) {
        prev.mark = '✓'
        prev.summary = summary
        prev.live = ''
        prev.status = /未通过|失败|阻断/.test(summary) ? 'failed' : 'done'
        curStep = prev
      } else {
        pushStep({
          kind: 'step',
          mark: '✓',
          name,
          summary,
          live: '',
          status: /未通过|失败|阻断/.test(summary) ? 'failed' : 'done',
        })
      }
      continue
    }

    m = t.match(STEP_DONE_BARE)
    if (m && !t.includes('一致性未通过（详见')) {
      const name = m[1].trim()
      const prev = findOpenStep(entries, name) || (
        curStep && curStep.kind === 'step' && sameStepName(curStep.name, name) ? curStep : null
      )
      if (prev) {
        prev.mark = '✓'
        prev.live = ''
        prev.status = 'done'
        curStep = prev
      } else {
        pushStep({
          kind: 'step',
          mark: '✓',
          name,
          summary: '',
          status: 'done',
        })
      }
      continue
    }

    m = t.match(STEP_FAIL)
    if (m) {
      settleOpenSteps(entries)
      pushStep({
        kind: 'step',
        mark: '✕',
        name: m[1].trim(),
        status: 'failed',
      })
      continue
    }

    m = t.match(STEP_GEAR)
    if (m) {
      settleOpenSteps(entries)
      pushStep({
        kind: 'step',
        mark: '⚙',
        name: m[1].trim(),
        status: 'running',
      })
      continue
    }

    m = t.match(STEP_PAUSE)
    if (m) {
      pushPause(m[1])
      continue
    }

    if (DRAFT_TICK.test(t)) {
      if (curStep && curStep.kind === 'step') {
        const n = t.match(/(\d+)/)
        curStep.live = n ? `草稿 ${n[1]} 字` : '草稿写入中'
        curStep.status = 'running'
      }
      continue
    }

    // Indented / ellipsis live heartbeats under current step
    const bare = t.replace(/^[…·.\s]+/, '')
    if (
      /^\s/.test(raw)
      || INDENT_LIVE.test(bare)
      || /^正文已写入/.test(bare)
      || /^生成中/.test(bare)
    ) {
      if (curStep && (curStep.kind === 'step' || curStep.kind === 'note')) {
        // Chapter milestones never become the spinning tip.
        if (isChapterMilestoneName(curStep.name)) {
          continue
        }
        curStep.live = truncate(bare || t, 80)
        if (curStep.kind === 'step' && curStep.status === 'done') {
          // ignore stale live after done
        } else if (curStep.kind === 'step') {
          curStep.status = 'running'
        }
      }
      continue
    }

    // Fail coda lines — attach to last step / as note
    if (/一致性未通过|审校未通过|详见上方/.test(t)) {
      if (curStep && curStep.kind === 'step') {
        curStep.summary = curStep.summary || truncate(t, 80)
        if (/未通过/.test(t)) curStep.status = 'failed'
      }
      continue
    }
  }

  // Belt-and-suspenders: at most one spinning step (the tip).
  enforceSingleRunning(entries)

  const structured = entries.some((e) => e.kind === 'step' || e.kind === 'queue')
  return { entries, structured }
}

function sameStepName(a, b) {
  const x = String(a || '').trim()
  const y = String(b || '').trim()
  if (!x || !y) return false
  if (x === y || x.includes(y) || y.includes(x)) return true
  const cx = chapterNum(x)
  const cy = chapterNum(y)
  return !!(cx && cy && cx === cy)
}

function chapterNum(name) {
  const m = String(name || '').match(/第(\d+)章/)
  return m ? m[1] : null
}

function isChapterMilestoneName(name) {
  return /^第\d+章/.test(String(name || '').trim())
}

/** Drop prior-chapter micro steps; keep one rolled-up done row. */
function collapsePriorChapterSteps(entries, finishedChapter) {
  const n = Number(finishedChapter) || 0
  if (!n || !entries.length) return
  const keptNotes = entries.filter(
    (e) => e.kind === 'note' && /评审团|接续审阅|复审/.test(String(e.name || '')),
  )
  entries.length = 0
  entries.push({
    kind: 'step',
    mark: '✓',
    name: n <= 1 ? '第1章' : `第1–${n}章`,
    summary: '已审过',
    status: 'done',
  })
  for (const note of keptNotes.slice(-2)) entries.push(note)
}

/** Close superseded ▶ rows so only the newest work can spin. */
function settleOpenSteps(entries) {
  for (const e of entries) {
    if (e.kind !== 'step' || e.status !== 'running') continue
    e.status = 'done'
    e.live = ''
    if (e.mark === '▶') e.mark = '·'
  }
}

function enforceSingleRunning(entries) {
  let last = -1
  for (let i = 0; i < entries.length; i += 1) {
    if (entries[i].kind === 'step' && entries[i].status === 'running') last = i
  }
  if (last < 0) return
  for (let i = 0; i < entries.length; i += 1) {
    if (i === last) continue
    const e = entries[i]
    if (e.kind === 'step' && e.status === 'running') {
      e.status = 'done'
      e.live = ''
      if (e.mark === '▶') e.mark = '·'
    }
  }
}

/** Prefer the latest still-open step with the same name (⏸ notes may sit in between). */
function findOpenStep(entries, name) {
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const e = entries[i]
    if (e.kind !== 'step') continue
    if (!sameStepName(e.name, name)) continue
    if (e.status === 'running' || e.mark === '▶' || e.status === 'note') return e
  }
  return null
}

function findChapterStep(entries, name) {
  const n = chapterNum(name)
  if (!n) return null
  for (let i = entries.length - 1; i >= 0; i -= 1) {
    const e = entries[i]
    if (e.kind !== 'step') continue
    if (chapterNum(e.name) === n) return e
  }
  return null
}

function truncate(s, n) {
  const t = String(s || '')
  if (t.length <= n) return t
  return `${t.slice(0, n - 1)}…`
}

/** One-line summary for collapsed Worked-for / tool headers. */
export function summarizeToolProgress(entries) {
  const list = Array.isArray(entries) ? entries : []
  const steps = list.filter((e) => e.kind === 'step')
  if (!steps.length) {
    const q = list.find((e) => e.kind === 'queue')
    return q ? q.name : ''
  }
  const running = steps.filter((e) => e.status === 'running')
  const done = steps.filter((e) => e.status === 'done' || e.status === 'failed' || e.status === 'paused')
  if (running.length) {
    const tip = running[running.length - 1]
    return tip.live ? `${tip.name} · ${tip.live}` : tip.name
  }
  if (done.length === 1) {
    const s = done[0]
    return s.summary ? `${s.name}: ${s.summary}` : s.name
  }
  return `${done.length} 个步骤`
}
