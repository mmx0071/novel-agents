import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import SkillPopup from './SkillPopup'
import { collectAuditState } from '../auditParse'
import TurnTimeline from './TurnTimeline'
import SubAgentRail from './SubAgentRail'
import StudioNavIcons from './StudioNavIcons'
import {
  chapterForBulkTool,
  formatBulkReadSummary,
  isBulkContextTool,
  parseXmlToolCalls,
  readerTabForBulkTool,
  stripToolMarkup,
} from './toolMarkup'
import { skillLabelZh, skillPromptForAuthor, stepLabelZh, toolLabelZh } from './toolLabels'
import { mergeSubAgentList, upsertSubAgent } from '../subAgents'
import { looksLikeToolGateDone } from '../toolGateDone.js'
import { looksLikeLiveToolProgress, todosStillOpen } from '../auditQueueStatus.js'

const API = '/api'

function wsUrl(path) {
  const proto = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${window.location.host}${path}`
}

function normalizeItem(raw) {
  if (!raw) return null
  const type = raw.type
  const status = normalizeWireStatus(raw.status)
  // Stable key — Math.random() remounted rows every delta and made Turn N flash.
  const key = raw.id || raw._key || `${type}:${raw.name || raw.agent || 'x'}`
  return { ...raw, type, status, _key: key }
}

/** Wire may send camelCase (inProgress) or PascalCase (Completed). */
function normalizeWireStatus(s) {
  if (s == null || s === '') return s
  const v = String(s)
  if (v === 'completed' || v === 'Completed') return 'completed'
  if (v === 'failed' || v === 'Failed') return 'failed'
  if (v === 'cancelled' || v === 'Cancelled') return 'cancelled'
  if (v === 'in_progress' || v === 'inProgress' || v === 'InProgress') return 'in_progress'
  return v
}

function isTerminalStatus(s) {
  const st = normalizeWireStatus(s)
  return st === 'completed' || st === 'failed' || st === 'cancelled'
}

/** When the turn is waiting on the user, stop all spinning tool / agent cards. */
function finishOpenItems(items) {
  return (items || []).map((it) => {
    if (!it) return it
    if (it.type === 'tool_call' || it.type === 'agent_message' || it.type === 'skill_load'
      || it.type === 'pipeline_step' || it.type === 'reasoning') {
      if (isTerminalStatus(it.status)) return it
      return { ...it, status: 'completed' }
    }
    return it
  })
}

/** Close prior turns so only `keepId` can show streaming carets /「进行中」. */
function sealOtherTurns(turns, keepId) {
  return (turns || []).map((t) => {
    if (!t || t.id === keepId) return t
    if (t.status !== 'running' && t.status !== 'awaiting') {
      return t.approval ? { ...t, approval: null } : t
    }
    return {
      ...t,
      status: 'complete',
      approval: null,
      items: finishOpenItems(t.items),
    }
  })
}

function isAuditToolName(name) {
  return name === 'audit_chapter' || name === 'audit_chapters' || name === 'steer_run'
}

/** Tools that write project files — refresh reader when they finish (don't wait for turn_complete). */
const PREVIEW_MUTATING_TOOLS = new Set([
  'continue_writing',
  'continue_writing_batch',
  'replan_volume',
  'revise_chapter',
  'revise_outline',
  'design_plot',
  'update_plot',
  'design_entity',
  'delete_entity',
  'design_arc_outline',
  'design_master_outline',
  'upsert_setting',
  'supplement_setting',
  'sync_volume',
  'confirm_volume_memory',
  'create_novel',
  'init_novel',
  'steer_run',
  'apply_draft_patch',
  'audit_volume',
  'audit_chapter',
  'audit_chapters',
  'audit_setting',
  'lock_brief',
  'confirm_setup',
])

/** Pipeline progress lines that mean readable files likely changed mid-tool. */
function looksLikePreviewStepDone(delta) {
  return !!readerHintFromPipelineDelta(delta)
}

/** Map pipeline progress → reader surface. Only writer/planner drive live reader sync. */
function readerHintFromPipelineDelta(delta) {
  if (!delta) return null
  const t = String(delta)
  // 章纲：仅章纲规划落盘后刷新（审校/润色流式不碰阅读区）
  // 不用 \b：中文标签后紧跟「:」时 JS 词边界不可靠。
  if (/✓\s+(?:chapter_planner|章纲规划|规划章纲)(?:\b|[:：\s]|$)/i.test(t)) {
    return { focusLatestChapter: true, readerTab: 'outline' }
  }
  // 正文：仅写作间歇落盘 / 正文写作步骤完成。文学润色 / 审校 / 专改只在工具卡流式。
  if (
    /正文已写入/i.test(t)
    || /↻\s*draft/i.test(t)
    || /✓\s+(?:writer|正文写作|撰写正文)(?:\b|[:：\s]|$)/i.test(t)
  ) {
    return { focusLatestChapter: true, readerTab: 'draft' }
  }
  // 批写章横幅：切到最新章，便于阅读区跟上连写进度。
  if (/▶\s*批写第\d+章/i.test(t) || /✓\s*第\d+章已发布/i.test(t)) {
    return { focusLatestChapter: true, readerTab: 'draft' }
  }
  return null
}

function finishAuditTools(items) {
  return (items || []).map((it) => {
    if (!it || it.type !== 'tool_call') return it
    if (!isAuditToolName(it.name)) return it
    if (isTerminalStatus(it.status)) return it
    return { ...it, status: 'completed' }
  })
}

/** Normalize server open_gate / turn.approval into ApprovalOptions props. */
function normalizeServerGate(gate) {
  if (!gate || typeof gate !== 'object') return null
  const options = Array.isArray(gate.options) ? gate.options : []
  if (!options.length && !gate.prompt) return null
  return {
    prompt: gate.prompt || '',
    options: options.map((o) => ({
      id: o.id || o.label,
      label: o.label || o.id || '',
    })),
    // Keep preview so restore can re-attach DraftPatch / MutationPreview cards.
    preview: gate.preview || null,
    kind: gate.kind || null,
    mutation_kind: gate.mutation_kind || null,
  }
}

function previewItemHasDiffs(it) {
  if (!it) return false
  if (it.type === 'draft_patch') return !!(it.before || it.after)
  if (it.type === 'mutation_preview') {
    return Array.isArray(it.diffs) && it.diffs.some((d) => d?.before || d?.after)
  }
  return false
}

/** True when open_gate / turn.approval is a live mutation confirm (cm_apply / cm_discard). */
function isMutationConfirmGate(gate) {
  if (!gate || typeof gate !== 'object') return false
  if (gate.kind === 'confirm_mutation') return true
  if (gate.mutation_kind && gate.kind !== 'chapter_next') return true
  return isLiveMutationApproval(gate)
}

function stripPreviewItems(items) {
  return (items || []).filter((it) => (
    it?.type !== 'mutation_preview' && it?.type !== 'draft_patch'
  ))
}

/** Attach open_gate.preview diffs/markdown onto the last turn's items (HTTP restore / live gate). */
function attachGatePreviewItems(turns, gate) {
  if (!Array.isArray(turns) || !turns.length || !gate?.preview) return turns
  // Never mount「待确认修改」under chapter_next / audit / setup cards.
  if (!isMutationConfirmGate(gate)) return turns
  const preview = gate.preview
  const items = []
  const diffs = Array.isArray(preview.diffs) ? preview.diffs : []
  const chapter = Number(preview.chapter) || 0
  const kind = String(gate.mutation_kind || preview.kind || '')
  const bodyParaPatch = chapter > 0 && (
    !kind
    || kind === 'revise_chapter'
    || kind === 'split_chapter'
    || kind === 'apply_draft_patch'
    || kind === 'draft_patch'
  ) && !preview.before_full && !preview.after_full
  if (diffs.length && bodyParaPatch) {
    for (const d of diffs) {
      items.push({
        type: 'draft_patch',
        status: 'completed',
        project: preview.project || '',
        chapter,
        start_para: d.start_para || 1,
        end_para: d.end_para || d.start_para || 1,
        before: d.before || '',
        after: d.after || '',
      })
    }
  } else if (
    diffs.length
    || preview.before_full
    || preview.after_full
    || preview.markdown
    || preview.fields
  ) {
    items.push({
      type: 'mutation_preview',
      status: 'completed',
      kind: gate.mutation_kind || preview.kind || 'mutation',
      topic: preview.topic || '',
      path: typeof preview.path === 'string' ? preview.path : '',
      title: preview.title || '',
      name: preview.name || '',
      markdown: preview.markdown || '',
      before_full: preview.before_full || '',
      after_full: preview.after_full || '',
      chapter: Number(preview.chapter) || 0,
      fields: preview.fields || null,
      diffs: diffs.map((d) => ({
        before: d.before || '',
        after: d.after || '',
      })),
    })
  }
  if (!items.length) return turns
  return turns.map((t, idx) => {
    if (idx !== turns.length - 1) return t
    const existing = Array.isArray(t.items) ? t.items : []
    const rich = items.some(previewItemHasDiffs)
    // Upgrade a weak card (markdown-only) when gate now carries hunks.
    if (rich) {
      const weakIdx = existing.findIndex((it) => (
        (it?.type === 'mutation_preview' || it?.type === 'draft_patch')
        && !previewItemHasDiffs(it)
      ))
      if (weakIdx >= 0) {
        const cleaned = existing.filter((_, i) => i !== weakIdx)
        return { ...t, items: [...cleaned, ...items] }
      }
    }
    const hasPreview = existing.some(
      (it) => it?.type === 'draft_patch' || it?.type === 'mutation_preview',
    )
    if (hasPreview) return t
    return { ...t, items: [...existing, ...items] }
  })
}

/** Live WS gate has prompt/options only — hydrate mutation preview from open_gate (not history). */
function mergeSnapshotPreviewIntoTurns(turns, snap) {
  if (!Array.isArray(turns) || !turns.length || !snap || typeof snap !== 'object') return turns
  const gate = normalizeServerGate(snap.open_gate)
  const mutationLive = !!(snap.pending_mutation) || isMutationConfirmGate(gate)
  // Wrong gate (chapter_next after audit, etc.): drop ghost「拟修改」cards.
  if (!mutationLive) {
    return turns.map((t, idx) => {
      if (idx !== turns.length - 1) return t
      const items = stripPreviewItems(t.items)
      if (items.length === (t.items || []).length) return t
      return { ...t, items }
    })
  }
  let next = turns
  if (gate?.preview) {
    next = next.map((t, idx) => {
      if (idx !== next.length - 1) return t
      if (!t.approval) return { ...t, approval: gate }
      return {
        ...t,
        approval: {
          ...t.approval,
          preview: gate.preview,
          kind: gate.kind || t.approval.kind,
          mutation_kind: gate.mutation_kind || t.approval.mutation_kind,
        },
      }
    })
    next = attachGatePreviewItems(next, gate)
  }
  // Only pull preview cards from the *last* server turn (current gate), never scrape history.
  const lastServer = Array.isArray(snap.turns) && snap.turns.length
    ? snap.turns[snap.turns.length - 1]
    : null
  const serverItems = (lastServer?.items || []).filter((it) => (
    it?.type === 'mutation_preview' || it?.type === 'draft_patch'
  ))
  if (!serverItems.length) return next
  return next.map((t, idx) => {
    if (idx !== next.length - 1) return t
    const existing = Array.isArray(t.items) ? t.items : []
    if (existing.some(previewItemHasDiffs)) return t
    const cleaned = stripPreviewItems(existing)
    return { ...t, items: [...cleaned, ...serverItems] }
  })
}

/** Prefer approval already on the last turn (from gates.yaml via server). */
function approvalFromTurns(turns) {
  if (!Array.isArray(turns) || !turns.length) return null
  const last = turns[turns.length - 1]
  return normalizeServerGate(last?.approval)
}

function queueToTodos(q) {
  if (!q?.chapters?.length) return []
  const results = q.results || []
  const finished = (q.index ?? 0) >= q.chapters.length
  return results.map((r, i) => {
    let status = 'pending'
    let note = '待审'
    if (finished || i < (q.index ?? 0)) {
      status = 'completed'
      note = r.status === 'skipped' ? '已跳过'
        : r.status === 'revised' ? '已修订'
          : r.status === 'failed' ? '未通过' : '通过'
    } else if (i === (q.index ?? 0)) {
      status = 'in_progress'
      note = r.status === 'failed' ? '未通过 · 待处理' : '进行中'
    }
    return { content: `审校第${r.chapter}章（${note}）`, status }
  })
}

function newLocalId(prefix) {
  return `${prefix}_${Math.random().toString(16).slice(2, 10)}`
}

/** Optimistic client turns use this prefix so they never collide with server `turn_<uuid>`. */
function newOptimisticTurnId() {
  return `local_${Math.random().toString(16).slice(2, 10)}`
}

function isOptimisticTurnId(id) {
  return String(id || '').startsWith('local_')
}

/**
 * Whether ending `endedTurnId` should release the composer.
 * Keeps the lock when a newer *server* turn already owns activeTurnRef, or when a
 * still-visible optimistic turn is waiting on a different known turn to finish.
 * Absorbs missed turn_started: optimistic active + unknown ended id → release.
 */
function shouldReleaseComposer(activeId, endedTurnId, turns) {
  if (!activeId || activeId === endedTurnId) return true
  if (!isOptimisticTurnId(activeId)) return false
  const list = turns || []
  const optimisticStill = list.some(
    (t) => t.id === activeId && (t.status === 'running' || t.status === 'awaiting'),
  )
  if (!optimisticStill) return true
  // Known other turn ending while optimistic still runs (interrupt+sendNow).
  if (list.some((t) => t.id === endedTurnId)) return false
  // ended id not in state → likely missed turn_started for our optimistic turn.
  return true
}

function chatCacheKey(project) {
  return `novelx-chat-v1:${project || '__draft__'}`
}

function loadChatCache(project) {
  try {
    const raw = localStorage.getItem(chatCacheKey(project))
    if (!raw) return null
    const data = JSON.parse(raw)
    if (!data || typeof data !== 'object') return null
    return {
      threadId: data.threadId || '',
      turns: Array.isArray(data.turns) ? data.turns : [],
    }
  } catch {
    return null
  }
}

function saveChatCache(project, threadId, turns) {
  try {
    localStorage.setItem(
      chatCacheKey(project),
      JSON.stringify({
        threadId: threadId || '',
        turns: (turns || []).slice(-80),
        savedAt: Date.now(),
      }),
    )
  } catch {
    /* quota */
  }
}

function clearChatCache(project) {
  try {
    localStorage.removeItem(chatCacheKey(project))
  } catch {
    /* ignore */
  }
}

function messagesToTurns(messages) {
  if (!Array.isArray(messages) || !messages.length) return []
  const turns = []
  let buf = null
  const flush = () => {
    if (buf) turns.push(buf)
    buf = null
  }
  const toolOutById = new Map()
  messages.forEach((m) => {
    if (m?.role === 'tool' && m.tool_call_id) {
      toolOutById.set(m.tool_call_id, m.content || '')
    }
  })
  messages.forEach((m, i) => {
    if (!m || m.role === 'tool') return
    if (m.role === 'user') {
      flush()
      buf = {
        id: `restored-${i}`,
        status: 'complete',
        items: [{ type: 'user_message', id: `u-${i}`, text: m.content || '' }],
      }
    } else if (m.role === 'assistant') {
      const text = (m.content || '').trim()
      const calls = Array.isArray(m.tool_calls) ? m.tool_calls : []
      if (!text && !calls.length) return
      if (!buf) {
        buf = { id: `restored-${i}`, status: 'complete', items: [] }
      }
      if (text) {
        buf.items.push({
          type: 'agent_message',
          id: `a-${i}`,
          text,
          status: 'completed',
        })
      }
      calls.forEach((tc, j) => {
        const fn = tc?.function || tc
        const name = fn?.name || tc?.name || 'tool'
        let args = {}
        try {
          args = typeof fn?.arguments === 'string'
            ? JSON.parse(fn.arguments || '{}')
            : (fn?.arguments || tc?.arguments || {})
        } catch {
          args = { raw: fn?.arguments }
        }
        const id = tc?.id || `tc-${i}-${j}`
        const rawOut = toolOutById.get(id) || ''
        const compact = formatBulkReadSummary(name, args, rawOut)
        buf.items.push({
          type: 'tool_call',
          id,
          name,
          arguments: args,
          output: isBulkContextTool(name)
            ? (compact || formatBulkReadSummary(name, args, '') || '正在查阅…')
            : rawOut,
          status: 'completed',
        })
      })
    }
  })
  flush()
  return turns
}

/** Prefer short UI line for bulk-read tools (full text stays server-side for the model). */
function compactToolItem(item, project) {
  if (!item || item.type !== 'tool_call' || !isBulkContextTool(item.name)) return item
  const summary = formatBulkReadSummary(item.name, item.arguments, item.output, project)
  if (!summary) return item
  return { ...item, output: summary }
}

/** Prefer the richer timeline so HTTP restore / remount cannot flash-back mid-turn. */
function turnsContentScore(turns) {
  if (!Array.isArray(turns) || !turns.length) return 0
  let score = turns.length * 10
  for (const t of turns) {
    for (const it of t.items || []) {
      score += 3
      if (it.type === 'tool_call') score += String(it.output || '').length
      if (it.type === 'agent_message') score += String(it.text || '').length
      if (it.type === 'draft_patch' || it.type === 'mutation_preview' || it.type === 'audit_report') {
        score += 80
      }
    }
    if (t.approval) score += 40
  }
  return score
}

function pickRicherTurns(candidate, current) {
  if (!candidate?.length) return null
  if (!current?.length) return candidate
  return turnsContentScore(candidate) >= turnsContentScore(current) ? candidate : null
}

/** When ui_turns only kept Read tool cards, recover NovelX prose from messages. */
function mergeAgentProseFromMessages(serverTurns, fromMessages) {
  if (!serverTurns?.length) return fromMessages || serverTurns
  if (!fromMessages?.length) return serverTurns
  const proseFrom = [...fromMessages].reverse().find((t) => (
    (t.items || []).some((it) => it.type === 'agent_message' && String(it.text || '').trim())
  ))
  const proseItems = (proseFrom?.items || []).filter((it) => (
    it.type === 'agent_message' && String(it.text || '').trim()
  ))
  if (!proseItems.length) return serverTurns
  return serverTurns.map((turn, idx) => {
    const isLast = idx === serverTurns.length - 1
    if (!isLast) return turn
    const items = [...(turn.items || [])]
    const hasProse = items.some((it) => it.type === 'agent_message' && String(it.text || '').trim())
    if (hasProse) return turn
    // Drop empty leading NovelX placeholders; append recovered answer after tools.
    const withoutEmptyAgent = items.filter((it) => !(
      it.type === 'agent_message' && !String(it.text || '').trim()
    ))
    return {
      ...turn,
      items: [
        ...withoutEmptyAgent,
        ...proseItems.map((it, i) => ({
          ...it,
          id: it.id || `recovered-agent-${i}`,
          status: 'completed',
        })),
      ],
    }
  })
}

/**
 * Codex-style NovelX chat:
 * - Turn timeline with separators
 * - Tool / Skill cells (Ran / Loaded)
 * - StartTurn via WebSocket Op (same connection as events)
 */
const AGENT_PANEL_KEY = 'novelx-agent-panel-v1'

function loadAgentCollapsed() {
  try {
    const raw = localStorage.getItem(AGENT_PANEL_KEY)
    if (!raw) return false
    return !!JSON.parse(raw)?.collapsed
  } catch {
    return false
  }
}

function saveAgentCollapsed(collapsed) {
  try {
    localStorage.setItem(AGENT_PANEL_KEY, JSON.stringify({ collapsed: !!collapsed }))
  } catch {
    /* ignore */
  }
}

/** Map mutation preview → writing-desk reader tab (本卷/正文/章纲/总纲/卷纲/世界观…). */
export function readerTabForDocPatch(item) {
  const kind = String(item?.kind || '').toLowerCase()
  const topic = String(item?.topic || '')
  const path = String(item?.path || '').replace(/\\/g, '/')
  const title = String(item?.title || item?.name || '')
  if (kind.includes('revise_chapter') || kind.includes('split_chapter') || kind.includes('draft_patch')) {
    return 'draft'
  }
  if (kind.includes('revise_outline') || /chapters\/\d+\/outline/.test(path)) {
    return 'outline'
  }
  if (kind.includes('replan_volume')) {
    return 'volume'
  }
  if (
    kind.includes('upsert_setting')
    || kind.includes('supplement_setting')
    || /bible|世界观/.test(topic)
    || /bible\.md|world_architect/.test(path)
  ) {
    return 'art:bible'
  }
  if (kind.includes('design_master') || /master_outline|series_outline/.test(path) || topic.includes('总纲')) {
    return 'master'
  }
  if (kind.includes('design_arc') || /arc_outline/.test(path) || /卷纲/.test(topic)) {
    return 'arcs'
  }
  if (kind.includes('design_plot') || /plots\//.test(path)) {
    return 'arcs'
  }
  if (kind.includes('design_entity') || /entities\//.test(path)) {
    const m = path.match(/entities\/([^/]+)\//)
    if (m) return `ent:${m[1]}`
    return 'entity_gaps'
  }
  if (title) return null
  return null
}

function isLiveMutationApproval(approval) {
  const opts = Array.isArray(approval?.options) ? approval.options : []
  return opts.some((o) => {
    const id = String(o?.id || '')
    const label = String(o?.label || '')
    return id === 'cm_apply'
      || id === 'cm_discard'
      || label === '全部应用'
      || label === '应用修改'
      || label === '取消变更'
  })
}

function collectDraftPatches(turns) {
  // Only surface desk diffs while a mutation confirm gate is still open.
  // Historical mutation_preview cards must not keep Apply alive after discard.
  if (!isLiveMutationApproval(approvalFromTurns(turns))) return []

  const list = []
  // Prefer the latest turn (where open_gate / preview are attached).
  const last = Array.isArray(turns) && turns.length ? turns[turns.length - 1] : null
  const scan = last ? [last] : []
  for (const turn of scan) {
    for (const it of turn.items || []) {
      if (it?.type === 'draft_patch' && (it.before || it.after)) {
        list.push({
          ...it,
          type: 'draft_patch',
          readerTab: it.readerTab || 'draft',
          label: it.label || `第${it.chapter || '?'}章正文`,
        })
        continue
      }
      // Document mutations → one inline_doc_diff (full before/after at real positions).
      if (it?.type === 'mutation_preview') {
        const hunks = Array.isArray(it.diffs) ? it.diffs : []
        const tab = readerTabForDocPatch(it)
        const label = it.topic || it.title || it.name || mutationDocLabel(it.kind) || '设定变更'
        const beforeFull = String(it.before_full || '')
        const afterFull = String(it.after_full || '')
        if (beforeFull || afterFull || hunks.length) {
          list.push({
            type: 'inline_doc_diff',
            status: 'completed',
            chapter: Number(it.chapter) || 0,
            start_para: 1,
            end_para: 1,
            before: beforeFull,
            after: afterFull || String(it.markdown || ''),
            beforeFull,
            afterFull: afterFull || String(it.markdown || ''),
            diffs: hunks,
            readerTab: tab,
            label,
            kind: it.kind || '',
            topic: it.topic || '',
            path: it.path || '',
          })
        }
      }
    }
  }
  return list
}

function mutationDocLabel(kind) {
  const k = String(kind || '')
  if (k.includes('upsert_setting') || k.includes('supplement')) return '世界观'
  if (k.includes('design_master')) return '总纲'
  if (k.includes('design_arc')) return '卷纲'
  if (k.includes('design_entity')) return '设定卡'
  if (k.includes('design_plot')) return '剧情卡'
  if (k.includes('revise_outline')) return '章纲'
  if (k.includes('replan_volume')) return '本卷'
  if (k.includes('revise_chapter') || k.includes('split_chapter')) return '正文'
  return ''
}

export default function NovelXChat({
  project,
  /** Author-facing book title; fall back to project id. */
  displayTitle = '',
  onPreviewRefresh,
  onProjectBound,
  onDraftPatchesChange,
  onAuditStateChange,
  sendRef,
  onSetupGateChange,
  onBusyChange,
  onSubAgentsChange,
  onOpenSubAgent,
  onThreadIdChange,
  openSubAgentId,
  /** When writing-desk patch dock is showing diffs, collapse chat cards to a short hint. */
  compactDraftPatches = false,
  statusOpen = false,
  onOpenStatus,
  engineOpen = false,
  onOpenEngine,
}) {
  const bookTitle = String(displayTitle || '').trim() || project
  const [threadId, setThreadId] = useState('')
  const [turns, setTurns] = useState([])
  const [input, setInput] = useState('')
  const [loading, setLoading] = useState(false)
  /** Server-authoritative: idle | working | awaiting_human */
  const [composerPhase, setComposerPhase] = useState('idle')
  const [wsState, setWsState] = useState('idle')
  const [skills, setSkills] = useState([])
  const [skillOpen, setSkillOpen] = useState(false)
  const [skillQuery, setSkillQuery] = useState('')
  const [otherText, setOtherText] = useState('')
  const [showOther, setShowOther] = useState(false)
  const [activity, setActivity] = useState('')
  const [todos, setTodos] = useState([])
  const todosRef = useRef([])
  todosRef.current = todos
  const [subAgents, setSubAgents] = useState([])
  const [setupGateOpen, setSetupGateOpen] = useState(false)
  const [agentCollapsed, setAgentCollapsed] = useState(() => loadAgentCollapsed())
  /** Sticky id for「回合 N · 进行中」— ignores mid-stream status flaps. */
  const [liveTurnId, setLiveTurnId] = useState('')
  const wsRef = useRef(null)
  const endRef = useRef(null)
  const messagesRef = useRef(null)
  const followChatBottomRef = useRef(true)
  /** Ignore scroll events caused by our own stick-to-bottom writes. */
  const pinningScrollRef = useRef(false)
  const chatScrollRafRef = useRef(0)
  const activeTurnRef = useRef('')
  const readyRef = useRef(false)
  const loadingRef = useRef(false)
  loadingRef.current = loading
  const composerPhaseRef = useRef('idle')
  /**
   * True after RequestUserInput with options until the user starts a new turn.
   * Late pipeline ticks must not re-lock the composer (that grayed out「修正本章」).
   */
  const humanGateOpenRef = useRef(false)
  /** Frontend task queue (Cursor-style). Not the server InputQueue / mid-turn steer. */
  const [taskQueue, setTaskQueue] = useState([])
  const taskQueueRef = useRef([])
  taskQueueRef.current = taskQueue
  const flushQueueIfIdleRef = useRef(() => {})
  const turnsRef = useRef(turns)
  turnsRef.current = turns
  const previewRefreshAtRef = useRef(0)
  const previewRefreshTimerRef = useRef(null)
  /** Coalesce tool output into React state (~8fps) so Turn separators don't repaint every token. */
  const toolDeltaBufRef = useRef(new Map())
  const toolDeltaTimerRef = useRef(0)
  /** Latest handlers for WS — keep connectWs / mount effect identity stable. */
  const handleEventRef = useRef(() => {})
  const wsPendingRef = useRef([])
  /** Bumped on「新开任务」so in-flight cache/POST cannot revive cleared history. */
  const chatGenRef = useRef(0)

  useEffect(() => () => {
    if (previewRefreshTimerRef.current) clearTimeout(previewRefreshTimerRef.current)
    if (toolDeltaTimerRef.current) clearTimeout(toolDeltaTimerRef.current)
    if (chatScrollRafRef.current) cancelAnimationFrame(chatScrollRafRef.current)
  }, [])

  /** Stick chat pane to bottom (tool cards grow); skip if user scrolled up. */
  const stickChatBottom = useCallback(() => {
    if (!followChatBottomRef.current) return
    const el = messagesRef.current
    if (!el) return
    if (chatScrollRafRef.current) cancelAnimationFrame(chatScrollRafRef.current)
    chatScrollRafRef.current = requestAnimationFrame(() => {
      chatScrollRafRef.current = 0
      const pane = messagesRef.current
      if (!pane || !followChatBottomRef.current) return
      pinningScrollRef.current = true
      pane.scrollTop = pane.scrollHeight
      // Late layout (tool card body grow) — pin again next frame.
      requestAnimationFrame(() => {
        const again = messagesRef.current
        if (again && followChatBottomRef.current) {
          again.scrollTop = again.scrollHeight
        }
        pinningScrollRef.current = false
      })
    })
  }, [])

  const onChatScroll = useCallback(() => {
    if (pinningScrollRef.current) return
    const el = messagesRef.current
    if (!el) return
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight
    followChatBottomRef.current = dist < 160
  }, [])

  // Wheel/trackpad: explicit scroll-up unpins; near-bottom scroll-down re-pins.
  useEffect(() => {
    const el = messagesRef.current
    if (!el) return undefined
    const onWheel = (e) => {
      if (e.deltaY < 0) {
        followChatBottomRef.current = false
        return
      }
      const dist = el.scrollHeight - el.scrollTop - el.clientHeight
      if (dist < 160) followChatBottomRef.current = true
    }
    el.addEventListener('wheel', onWheel, { passive: true })
    return () => el.removeEventListener('wheel', onWheel)
  }, [threadId])

  /** Throttled mid-turn preview sync; pipeline ✓ can jump reader to 章纲/正文. */
  const syncPreview = useCallback((opts = {}) => {
    if (!project || !onPreviewRefresh) return
    const keepSelection = opts.keepSelection !== false
    const minGap = opts.immediate ? 0 : 2800
    const now = Date.now()
    const wait = Math.max(0, minGap - (now - previewRefreshAtRef.current))
    const run = () => {
      previewRefreshAtRef.current = Date.now()
      previewRefreshTimerRef.current = null
      onPreviewRefresh(project, {
        keepSelection,
        focusLatestChapter: !!opts.focusLatestChapter,
        readerTab: opts.readerTab || undefined,
        chapter: opts.chapter || undefined,
      })
    }
    if (previewRefreshTimerRef.current) {
      clearTimeout(previewRefreshTimerRef.current)
      previewRefreshTimerRef.current = null
    }
    if (wait === 0) run()
    else previewRefreshTimerRef.current = setTimeout(run, wait)
  }, [onPreviewRefresh, project])

  const upsertTurn = useCallback((turnId, mutator) => {
    setTurns((prev) => {
      const idx = prev.findIndex((t) => t.id === turnId)
      if (idx >= 0) {
        const next = [...prev]
        next[idx] = mutator({ ...next[idx], items: [...(next[idx].items || [])] })
        return next
      }
      // Events can arrive before turn_started. Adopt the optimistic local turn
      // instead of spawning a sibling empty "回合 N" that only flashes.
      const localIdx = prev.findIndex(
        (t) => t.status === 'running' && isOptimisticTurnId(t.id),
      )
      if (localIdx >= 0 && turnId && !isOptimisticTurnId(turnId)) {
        const next = [...prev]
        const base = {
          ...next[localIdx],
          id: turnId,
          status: 'running',
          items: [...(next[localIdx].items || [])],
        }
        next[localIdx] = mutator(base)
        activeTurnRef.current = turnId
        return next
      }
      const created = mutator({
        id: turnId,
        status: 'running',
        items: [],
        approval: null,
      })
      return [...prev, created]
    })
  }, [])

  const applyToolOutputDelta = useCallback((ev) => {
    upsertTurn(ev.turn_id, (t) => {
      // Late WS deltas after turn_complete must not revive「生成中 / 进行中」.
      if (t.status === 'complete' || t.status === 'aborted') {
        return t
      }
      // Late/spurious complete must not leave the live turn unlabeled mid-tool.
      const nextStatus = t.status === 'awaiting' ? 'awaiting' : 'running'
      const items = [...(t.items || [])]
      let i = items.findIndex((x) => x.id === ev.item_id)
      // Missed item_started / id mismatch — still show live progress.
      if (i < 0 && ev.item_id) {
        items.push({
          type: 'tool_call',
          id: ev.item_id,
          name: 'tool',
          arguments: {},
          output: '',
          status: 'in_progress',
          _key: `tool_call:${ev.item_id}`,
        })
        i = items.length - 1
      }
      if (i >= 0) {
        const prevStatus = items[i].status
        const bulk = isBulkContextTool(items[i].name)
        // Bulk-read: replace with short line (never append multi-KB context into React state).
        const delta = String(ev.delta || '')
        const prevOut = String(items[i].output || '')
        const dTrim = delta.trim()
        const pTrim = prevOut.trim()
        // Skip full duplicate appends (streamed report + same coda / double WS).
        const skipDup = !!(
          dTrim
          && pTrim
          && (
            prevOut.includes(dTrim)
            || (dTrim.includes(pTrim) && dTrim.length <= pTrim.length + 8)
            || (dTrim.includes('审校报告') && pTrim.includes('审校报告') && dTrim.length > 80)
            || (dTrim.includes('已提交') && pTrim.includes('已提交') && /决策/.test(dTrim + pTrim))
          )
        )
        const output = bulk
          ? (formatBulkReadSummary(items[i].name, items[i].arguments, ev.delta, project)
            || formatBulkReadSummary(items[i].name, items[i].arguments, '', project)
            || delta.slice(0, 200))
          : (skipDup ? prevOut : `${prevOut}${delta}`)
        // Pipeline may finish (report / ⏸ gate) while ItemCompleted is delayed by WS backpressure.
        // Audit parent: never complete from coda heuristics — fail reports like「审校未通过」
        // arrive before Decision Council nests revise/reaudit into the same card. Trust
        // ItemCompleted (deferred on server) and reopen if live progress keeps streaming.
        const auditParent = items[i].name === 'audit_chapters' || items[i].name === 'audit_chapter'
        const liveProgress = auditParent && looksLikeLiveToolProgress(output)
        const doneHint = !bulk
          && !auditParent
          && looksLikeToolGateDone(output, {
            todosOpen: todosStillOpen(todosRef.current),
          })
        let nextToolStatus
        if (auditParent && liveProgress) {
          nextToolStatus = 'in_progress'
        } else if (isTerminalStatus(prevStatus)) {
          nextToolStatus = prevStatus
        } else if (doneHint) {
          nextToolStatus = 'completed'
        } else {
          nextToolStatus = prevStatus || 'in_progress'
        }
        items[i] = {
          ...items[i],
          output,
          status: nextToolStatus,
        }
        // Keep mirrored agent status lines quiet (no caret) while the tool streams.
        for (let j = 0; j < items.length; j += 1) {
          if (items[j]?.type === 'agent_message' && items[j].status === 'in_progress') {
            items[j] = { ...items[j], status: 'completed' }
          }
        }
      }
      return { ...t, status: nextStatus, items }
    })
    const turn = turnsRef.current?.find((t) => t.id === ev.turn_id)
    if (
      ev.turn_id
      && turn?.status !== 'complete'
      && turn?.status !== 'aborted'
    ) {
      activeTurnRef.current = ev.turn_id
      setLiveTurnId(ev.turn_id)
    }
  }, [project, upsertTurn])

  const dropToolDeltaBufForTurn = useCallback((turnId) => {
    if (!turnId) return
    const prefix = `${turnId}:`
    for (const key of [...toolDeltaBufRef.current.keys()]) {
      if (key.startsWith(prefix)) toolDeltaBufRef.current.delete(key)
    }
  }, [])

  const flushToolDeltaBuf = useCallback(() => {
    if (toolDeltaTimerRef.current) {
      clearTimeout(toolDeltaTimerRef.current)
      toolDeltaTimerRef.current = 0
    }
    const batch = [...toolDeltaBufRef.current.values()]
    toolDeltaBufRef.current.clear()
    for (const e of batch) applyToolOutputDelta(e)
  }, [applyToolOutputDelta])

  const queueToolOutputDelta = useCallback((ev) => {
    const key = `${ev.turn_id}:${ev.item_id}`
    const prev = toolDeltaBufRef.current.get(key)
    if (prev) {
      prev.delta = `${prev.delta || ''}${ev.delta || ''}`
    } else {
      toolDeltaBufRef.current.set(key, {
        turn_id: ev.turn_id,
        item_id: ev.item_id,
        delta: ev.delta || '',
      })
    }
    if (!toolDeltaTimerRef.current) {
      toolDeltaTimerRef.current = window.setTimeout(() => {
        toolDeltaTimerRef.current = 0
        flushToolDeltaBuf()
      }, 120)
    }
  }, [flushToolDeltaBuf])

  /** Drive composer from server ComposerPhase (or optimistic local mirror). */
  const applyComposerPhase = useCallback((phase, opts = {}) => {
    const p = phase === 'working' || phase === 'awaiting_human' ? phase : 'idle'
    composerPhaseRef.current = p
    setComposerPhase(p)
    if (p === 'working') {
      humanGateOpenRef.current = false
      loadingRef.current = true
      setLoading(true)
      if (opts.activeTurnId) {
        activeTurnRef.current = opts.activeTurnId
        setLiveTurnId(opts.activeTurnId)
      }
      if (opts.activityText) setActivity(opts.activityText)
    } else if (p === 'awaiting_human') {
      humanGateOpenRef.current = true
      loadingRef.current = false
      setLoading(false)
      activeTurnRef.current = ''
      setLiveTurnId('')
      setActivity(opts.activityText != null ? opts.activityText : 'waiting for input')
    } else {
      humanGateOpenRef.current = false
      loadingRef.current = false
      setLoading(false)
      activeTurnRef.current = ''
      setLiveTurnId('')
      if (opts.activityText !== undefined) setActivity(opts.activityText)
      else setActivity('')
      window.setTimeout(() => flushQueueIfIdleRef.current?.(), 50)
    }
  }, [])

  const markTurnLive = useCallback((turnId, activityText) => {
    // Server / gate says human must choose — ignore late ▶/✓ / 生成中 ticks.
    if (composerPhaseRef.current === 'awaiting_human' || humanGateOpenRef.current) return
    const turns = turnsRef.current || []
    if (turns.some((t) => t?.approval?.options?.length)) {
      applyComposerPhase('awaiting_human')
      return
    }
    if (turnId) {
      const turn = turns.find((t) => t.id === turnId)
      if (
        turn?.status === 'complete'
        || turn?.status === 'aborted'
        || turn?.status === 'awaiting'
        || turn?.approval?.options?.length
      ) {
        return
      }
    }
    applyComposerPhase('working', { activeTurnId: turnId, activityText })
  }, [applyComposerPhase])

  const handleEvent = useCallback(
    (ev) => {
      if (!ev || !ev.type) return
      switch (ev.type) {
        case 'session_configured':
          setThreadId(ev.thread_id)
          break
        case 'agent_status_changed': {
          setSubAgents((prev) => upsertSubAgent(prev, ev))
          break
        }
        case 'session_phase_changed': {
          const phase = ev.phase || 'idle'
          applyComposerPhase(phase, {
            activeTurnId: ev.active_turn_id || undefined,
            activityText: phase === 'working'
              ? undefined
              : (phase === 'awaiting_human' ? 'waiting for input' : ''),
          })
          if (phase === 'working') {
            setActivity((prev) => (prev && prev !== 'waiting for input' ? prev : 'working…'))
          }
          break
        }
        case 'turn_started': {
          const serverTurn = ev.turn_id
          applyComposerPhase('working', {
            activeTurnId: serverTurn,
            activityText: 'working…',
          })
          // Merge optimistic `local_*` into the server turn. Never drop an existing
          // approval on the server turn (RequestUserInput may have already arrived).
          setTurns((prev) => {
            const localIdx = prev.findIndex(
              (t) => t.status === 'running' && isOptimisticTurnId(t.id),
            )
            const serverIdx = prev.findIndex((t) => t.id === serverTurn)

            let next = prev
            if (localIdx >= 0 && serverIdx >= 0 && localIdx !== serverIdx) {
              const local = prev[localIdx]
              const server = prev[serverIdx]
              const userItems = (local.items || []).filter((i) => i.type === 'user_message')
              const serverItems = server.items || []
              const seen = new Set()
              const mergedItems = []
              for (const it of [...userItems, ...serverItems.filter((i) => i.type !== 'user_message')]) {
                const k = it.id || it._key
                if (k && seen.has(k)) continue
                if (k) seen.add(k)
                mergedItems.push(it)
              }
              const merged = {
                ...server,
                status: 'running',
                approval: server.approval || local.approval || null,
                items: mergedItems,
              }
              next = prev
                .filter((_, i) => i !== localIdx)
                .map((t) => (t.id === serverTurn ? merged : t))
            } else if (localIdx >= 0 && prev[localIdx].id !== serverTurn) {
              next = [...prev]
              next[localIdx] = {
                ...next[localIdx],
                id: serverTurn,
                status: 'running',
                approval: next[localIdx].approval ?? null,
              }
            } else if (serverIdx < 0) {
              next = [...prev, { id: serverTurn, status: 'running', items: [], approval: null }]
            }
            // Prior awaiting/running turns must not keep carets — that was「两次回复一起闪」.
            return sealOtherTurns(next, serverTurn)
          })
          break
        }
        case 'turn_complete': {
          // Drop this turn's buffered deltas first — flushing them would revive
          // status=running / activity「生成中…」after the turn already ended.
          dropToolDeltaBufForTurn(ev.turn_id)
          flushToolDeltaBuf()
          const activeBefore = activeTurnRef.current
          const release = shouldReleaseComposer(
            activeBefore,
            ev.turn_id,
            turnsRef.current,
          )
          // Missed turn_started: absorb optimistic local_* into the server turn id.
          if (
            release
            && activeBefore
            && isOptimisticTurnId(activeBefore)
            && activeBefore !== ev.turn_id
            && !(turnsRef.current || []).some((t) => t.id === ev.turn_id)
          ) {
            activeTurnRef.current = ev.turn_id
          }
          // Only finish the matched turn — completing every running turn made「进行中」闪灭.
          setTurns((prev) => {
            let next = prev
            if (
              activeBefore
              && isOptimisticTurnId(activeBefore)
              && activeBefore !== ev.turn_id
              && !prev.some((t) => t.id === ev.turn_id)
            ) {
              next = prev.map((t) => (t.id === activeBefore ? { ...t, id: ev.turn_id } : t))
            }
            return next.map((t) => {
              if (t.id !== ev.turn_id) return t
              return {
                ...t,
                status: 'complete',
                approval: t.approval,
                items: finishOpenItems(t.items),
              }
            })
          })
          setLiveTurnId((id) => {
            if (id === ev.turn_id) return ''
            if (release && id === activeBefore) return ''
            return id
          })
          // Composer unlock is owned by session_phase_changed (after TurnGate release).
          syncPreview({ keepSelection: false, immediate: true })
          break
        }
        case 'turn_aborted': {
          dropToolDeltaBufForTurn(ev.turn_id)
          flushToolDeltaBuf()
          const activeBefore = activeTurnRef.current
          const release = shouldReleaseComposer(
            activeBefore,
            ev.turn_id,
            turnsRef.current,
          )
          // upsertTurn adopts a lone optimistic local_* when server id is unseen.
          upsertTurn(ev.turn_id, (t) => ({
            ...t,
            status: 'aborted',
            items: finishOpenItems(t.items),
          }))
          setLiveTurnId((id) => {
            if (id === ev.turn_id) return ''
            if (release && id === activeBefore) return ''
            return id
          })
          // Do not auto-flush the task queue on abort (Stop should leave items queued).
          // Composer phase event unlocks; surface interrupt hint immediately.
          if (release) setActivity('已中断')
          break
        }
        case 'item_started':
        case 'item_completed': {
          if (ev.type === 'item_completed') {
            // CRITICAL path bypasses rAF — pull matching tool deltas out of the
            // pending queue first so we don't apply ItemCompleted on an empty card
            // then append late deltas (闪回 / 结束后才见全文).
            const itemId = ev.item?.id
            const turnId = ev.turn_id
            if (itemId && turnId) {
              const pending = wsPendingRef.current
              let mergedDelta = ''
              const kept = []
              for (const p of pending) {
                if (
                  p?.type === 'tool_call_output_delta'
                  && p.item_id === itemId
                  && p.turn_id === turnId
                ) {
                  mergedDelta += p.delta || ''
                } else {
                  kept.push(p)
                }
              }
              wsPendingRef.current = kept
              if (mergedDelta) {
                queueToolOutputDelta({
                  turn_id: turnId,
                  item_id: itemId,
                  delta: mergedDelta,
                })
              }
            }
            flushToolDeltaBuf()
          }
          const item = normalizeItem(ev.item)
          if (!item) break
          if (item.type === 'tool_call') {
            const compact = compactToolItem(item, project)
            const zhName = toolLabelZh(item.name)
            const label = isBulkContextTool(item.name) && compact.output
              ? compact.output
              : zhName
            if (item.status === 'completed') {
              const turnDone = (() => {
                const turn = turnsRef.current?.find((t) => t.id === ev.turn_id)
                return turn?.status === 'complete' || turn?.status === 'aborted'
              })()
              if (!turnDone) {
                setActivity(isBulkContextTool(item.name) ? label : `✓ ${zhName}`)
              }
            } else if (isBulkContextTool(item.name)) {
              setActivity('查阅中…')
            } else if (item.name === 'revise_chapter' || item.name === 'steer_run') {
              markTurnLive(ev.turn_id, '修订中…')
            } else if (item.name === 'continue_writing') {
              markTurnLive(ev.turn_id, '写作中…')
            } else {
              markTurnLive(ev.turn_id, `运行中 · ${zhName}…`)
            }
          } else if (item.type === 'skill_load') {
            const sk = skillLabelZh(item.name)
            setActivity(
              item.status === 'completed' ? `已加载写作指南 · ${sk}` : `加载写作指南 · ${sk}…`,
            )
          }
          // Skip duplicate user_message if we already optimistic-inserted
          if (item.type === 'user_message') {
            upsertTurn(ev.turn_id, (t) => {
              if (t.items.some((x) => x.type === 'user_message' && x.text === item.text)) return t
              return { ...t, items: [...t.items, item] }
            })
            break
          }
          if (item.type === 'agent_spawn') {
            const childId = item.child_thread_id || item.childThreadId || ''
            if (childId) {
              setSubAgents((prev) => upsertSubAgent(prev, {
                threadId: childId,
                role: item.role || '',
                agentPath: item.agent_path || item.agentPath || '',
                parentThreadId: threadId || '',
                lifecycle: 'running',
                summary: '',
              }))
            }
          }
          upsertTurn(ev.turn_id, (t) => {
            let items = [...t.items]
            // Tool / skill start ⇒ stop blinking carets on prior agent bubbles.
            if (
              ev.type === 'item_started'
              && (item.type === 'tool_call' || item.type === 'skill_load')
            ) {
              items = items.map((x) => (
                x.type === 'agent_message' && x.status === 'in_progress'
                  ? { ...x, status: 'completed' }
                  : x
              ))
            }
            const stored = item.type === 'tool_call' ? compactToolItem(item, project) : item
            const i = items.findIndex((x) => x.id === stored.id)
            if (i >= 0) {
              const prev = items[i]
              // ItemCompleted often carries a short coda; never shrink away streamed ▶/✓ process.
              if (
                stored.type === 'agent_message'
                && prev.type === 'agent_message'
                && String(prev.text || '').length > String(stored.text || '').length
              ) {
                items[i] = {
                  ...prev,
                  ...stored,
                  text: prev.text,
                  status: stored.status || prev.status,
                }
              } else if (
                stored.type === 'tool_call'
                && prev.type === 'tool_call'
                && String(prev.output || '').length > String(stored.output || '').length
              ) {
                items[i] = {
                  ...prev,
                  ...stored,
                  output: prev.output,
                  name: stored.name || prev.name,
                  arguments: stored.arguments || prev.arguments,
                  status: stored.status || prev.status,
                  duration_ms: stored.duration_ms ?? prev.duration_ms,
                }
              } else {
                items[i] = { ...prev, ...stored }
              }
            } else items.push(stored)
            // Promote client-recovered placeholder with same tool name
            if (item.type === 'tool_call') {
              for (let j = items.length - 1; j >= 0; j -= 1) {
                if (items[j]._clientRecovered && items[j].name === item.name && items[j].id !== item.id) {
                  items.splice(j, 1)
                }
              }
              // ItemCompleted for audit may miss id match — finish all audit spinners by name.
              if (
                ev.type === 'item_completed'
                && isTerminalStatus(item.status)
                && isAuditToolName(item.name)
              ) {
                items = finishAuditTools(items)
              }
            }
            return { ...t, items }
          })
          if (
            ev.type === 'item_completed'
            && item.type === 'tool_call'
            && isTerminalStatus(item.status)
          ) {
            if (item.name === 'create_novel' || item.name === 'init_novel') {
              const bound = String(
                item.data?.project
                || item.arguments?.name
                || item.arguments?.project
                || item.arguments?.id
                || '',
              ).trim()
              if (bound && typeof onProjectBound === 'function') {
                onProjectBound(bound)
              } else {
                syncPreview({ keepSelection: false, immediate: true })
              }
            } else if (PREVIEW_MUTATING_TOOLS.has(item.name)) {
              const writing = item.name === 'continue_writing' || item.name === 'revise_chapter'
              const batching = item.name === 'continue_writing_batch'
              const outlining = item.name === 'revise_outline'
              const ch = Number(item.arguments?.chapter) || undefined
              syncPreview({
                keepSelection: !(writing || outlining || batching),
                // Invalidate chapterCache so outline-first cache doesn't keep empty draft.
                chapter: (writing || outlining) ? ch : undefined,
                focusLatestChapter: writing || batching,
                readerTab: (writing || batching) ? 'draft' : outlining ? 'outline' : undefined,
                immediate: writing || outlining || batching,
              })
            } else if (isBulkContextTool(item.name)) {
              const tab = readerTabForBulkTool(item.name, item.arguments)
              const ch = chapterForBulkTool(item.name, item.arguments)
              syncPreview({
                keepSelection: ch == null,
                chapter: ch || undefined,
                focusLatestChapter: false,
                readerTab: tab || undefined,
                immediate: true,
              })
            }
          }
          break
        }
        case 'reasoning_content_delta': {
          const delta = stripToolMarkup(ev.delta || '')
          if (!delta) break
          upsertTurn(ev.turn_id, (t) => {
            const items = [...t.items]
            const i = items.findIndex((x) => x.id === ev.item_id)
            if (i >= 0) {
              const prev = items[i]
              items[i] = {
                ...prev,
                type: 'reasoning',
                text: String(prev.text || '') + delta,
                status: prev.status === 'completed' ? 'completed' : 'in_progress',
              }
            } else {
              items.push({
                id: ev.item_id,
                type: 'reasoning',
                text: delta,
                status: 'in_progress',
              })
            }
            return { ...t, items }
          })
          break
        }
        case 'agent_message_content_delta': {
          const delta = stripToolMarkup(ev.delta || '')
          const recovered = parseXmlToolCalls(ev.delta || '')
          if (recovered.length) {
            upsertTurn(ev.turn_id, (t) => {
              // Don't spawn ghost Running cards after the turn already paused/finished.
              if (t.status === 'awaiting' || t.status === 'complete' || t.status === 'aborted') {
                return t
              }
              let items = t.items.map((x) => (
                x.type === 'agent_message' && x.status === 'in_progress'
                  ? { ...x, status: 'completed' }
                  : x
              ))
              for (const tc of recovered) {
                if (!items.some((x) => x.name === tc.name && (
                  x._clientRecovered
                  || JSON.stringify(x.arguments) === JSON.stringify(tc.arguments)
                ))) {
                  items = [...items, tc]
                }
              }
              return { ...t, items }
            })
          }
          if (!delta) break
          upsertTurn(ev.turn_id, (t) => {
            const turnDone = t.status === 'awaiting' || t.status === 'complete' || t.status === 'aborted'
            // Gate/steer mirrors ▶/✓ into the agent bubble while a tool runs. Never reopen
            // that bubble as in_progress — the streaming caret made the whole Turn flash.
            const hasTool = (t.items || []).some((x) => x.type === 'tool_call')
            const toolRunning = (t.items || []).some(
              (x) => x.type === 'tool_call' && !isTerminalStatus(x.status),
            )
            const quietAgent = turnDone || toolRunning || hasTool
            const items = [...t.items]
            const i = items.findIndex((x) => x.id === ev.item_id)
            if (i >= 0) {
              const prevStatus = items[i].status
              const prevText = String(items[i].text || '')
              const dTrim = delta.trim()
              // Don't paste the same checklist / brief twice into one bubble.
              const skipDup = !dTrim
                || prevText.includes(dTrim)
                || (prevText.includes('问题清单') && dTrim.includes('问题清单'))
              const nextText = skipDup
                ? prevText
                : stripToolMarkup(`${prevText}${delta}`)
              items[i] = {
                ...items[i],
                type: 'agent_message',
                text: nextText,
                status: quietAgent
                  ? (isTerminalStatus(prevStatus) ? prevStatus : 'completed')
                  : 'in_progress',
              }
            } else if (!turnDone) {
              items.push({
                type: 'agent_message',
                id: ev.item_id,
                text: delta,
                status: quietAgent ? 'completed' : 'in_progress',
              })
            }
            return { ...t, items }
          })
          break
        }
        case 'tool_call_output_delta':
          // Buffer into React ~8fps — per-token setTurns was flashing「回合 N · 进行中」.
          {
            const turnMeta = turnsRef.current?.find((t) => t.id === ev.turn_id)
            if (turnMeta?.status === 'complete' || turnMeta?.status === 'aborted') {
              break
            }
          }
          queueToolOutputDelta(ev)
          {
            const d = String(ev.delta || '')
            // English id or Chinese label after ▶ / ✓
            const step = d.match(/▶\s*([^\n:：]+)/)
            const done = d.match(/✓\s*([^\n:：]+)/)
            const draftChars = d.match(/(?:正文已写入|正文生成中|↻\s*draft)[^\d]*(\d+)/i)
            const genChars = d.match(/生成中\s*·\s*(\d+)\s*字/)
            const wait = /调用模型|等待首包/.test(d)
            // revise_chapter / steer_run also run writer — don't label that as new-chapter Writing.
            const activeTool = (() => {
              const items = turnsRef.current
                ?.find((t) => t.id === ev.turn_id)
                ?.items || []
              const running = [...items].reverse().find((it) => (
                it.type === 'tool_call'
                && (it.status === 'in_progress' || it.status === 'inProgress' || !it.status)
              ))
              return running?.name || ''
            })()
            const revising = activeTool === 'revise_chapter' || activeTool === 'steer_run'
              || /整章修订|局部修订|局部改稿/.test(d)
            // continue_writing / batch are「写作中」; steer/revise (incl. full rewrite) is「修订中」.
            const writingNew = activeTool === 'continue_writing'
              || activeTool === 'continue_writing_batch'
            const stepName = (step?.[1] || done?.[1] || '').trim()
            const stepZh = stepLabelZh(stepName)
            const localRev = /local_reviser|局部修订|局部改稿/i.test(stepName)
              || /局部修订|局部改稿/.test(d)
            // Pipeline ticks mean the turn is still live — keep composer busy.
            if (writingNew || revising || step || draftChars || genChars || wait
              || /生成中|流式生成中|正文已写入|↻\s*draft|调用模型|等待首包/i.test(d)) {
              markTurnLive(
                ev.turn_id,
                writingNew ? '写作中…' : (revising ? '修订中…' : undefined),
              )
            }
            if (draftChars) {
              setActivity(
                writingNew
                  ? `写作中 · ${draftChars[1]}字`
                  : `修订中 · ${draftChars[1]}字`,
              )
            } else if (genChars) {
              setActivity(
                writingNew
                  ? `生成中 · ${genChars[1]}字`
                  : (revising ? `修订中 · ${genChars[1]}字` : `生成中 · ${genChars[1]}字`),
              )
            } else if (step) {
              setActivity(
                localRev
                  ? '局部改稿中…'
                  : (revising ? `修订 · ${stepZh}` : `运行中 · ${stepZh}…`),
              )
            } else if (done) {
              setActivity(localRev ? '✓ 局部改稿' : `✓ ${stepZh}`)
            } else if (wait) {
              setActivity((prev) => prev || '生成中…')
            } else if (/生成中|流式生成中|正文已写入|↻\s*draft/i.test(d)) {
              setActivity((prev) => prev || (revising ? '修订中…' : (writingNew ? '写作中…' : '生成中…')))
            }
            // Do NOT default every tool delta to「生成中…」— upsert_setting / list_*
            // completion text used to leave the bar stuck after the turn ended.
          }
          // Mid-pipeline: chapter/card files land before the whole tool finishes.
          // Jump reader to latest chapter + 章纲/正文 so creation is visible live.
          // Cache refresh relies on App.jsx body_chars staleness (outline-first
          // cache must not block draft fetch after 「正文已写入」).
          const hint = readerHintFromPipelineDelta(ev.delta)
          if (hint) {
            const liveFlush = /正文已写入|↻\s*draft/i.test(String(ev.delta || ''))
            // Live draft flush: keepSelection to avoid reader/chapter thrash (was
            // re-rendering the whole studio and amplifying Turn flicker).
            syncPreview({
              keepSelection: true,
              focusLatestChapter: !liveFlush && !!hint.focusLatestChapter,
              readerTab: hint.readerTab,
              // Step ✓: refresh ASAP; intermittent draft flush: throttle harder.
              immediate: !liveFlush,
            })
          }
          break
        case 'todo_updated': {
          const list = Array.isArray(ev.todos) ? ev.todos : []
          setTodos(list.map((t) => ({
            content: t.content || '',
            status: t.status === 'in_progress' || t.status === 'inProgress'
              ? 'in_progress'
              : t.status === 'completed' || t.status === 'Completed'
                ? 'completed'
                : 'pending',
          })))
          // Only dismiss gates when the queue is fully done — NOT on empty todos
          // from a single-chapter audit (that was wiping RequestUserInput options).
          const queueDone = list.length > 0
            && list.every((t) => t.status === 'completed' || t.status === 'Completed')
          if (queueDone) {
            setTurns((prev) => prev.map((t) => ({
              ...t,
              approval: null,
              items: finishAuditTools(finishOpenItems(t.items)),
              status: (t.status === 'running' || t.status === 'awaiting') ? 'complete' : t.status,
            })))
            // Leave composer to session_phase_changed (queue done ≠ agents idle).
          }
          break
        }
        case 'chat_history_reset': {
          const summary = ev.summary || '对话历史已清理。'
          const keepId = ev.keep_turn_id || ''
          if (keepId) {
            // New chapter write is still running — do not look like the turn ended.
            markTurnLive(keepId, '写作中…')
          }
          setTurns((prev) => {
            let next
            if (keepId) {
              // New-chapter start: drop prior turns, keep the in-flight write turn.
              const kept = prev.filter((t) => (
                t.id === keepId || isOptimisticTurnId(t.id)
              ))
              const liveNote = `${summary}\n\n正在撰写中，进度见下方「继续创作」工具卡…`
              next = kept.length
                ? kept.map((t) => {
                  if (!(t.id === keepId || isOptimisticTurnId(t.id))) return t
                  const items = [...(t.items || [])]
                  const hasAgent = items.some((it) => it.type === 'agent_message')
                  if (!hasAgent) {
                    items.push({
                      id: newLocalId('item_reset'),
                      type: 'agent_message',
                      text: liveNote,
                      status: 'in_progress',
                      _key: newLocalId('key'),
                    })
                  }
                  return {
                    ...t,
                    approval: null,
                    status: 'running',
                    items,
                  }
                })
                : [{
                  id: keepId,
                  status: 'running',
                  approval: null,
                  items: [{
                    id: newLocalId('item_reset'),
                    type: 'agent_message',
                    text: liveNote,
                    status: 'in_progress',
                    _key: newLocalId('key'),
                  }],
                }]
            } else {
              next = [{
                id: newLocalId('turn_reset'),
                status: 'complete',
                approval: null,
                items: [{
                  id: newLocalId('item_reset'),
                  type: 'agent_message',
                  text: summary,
                  status: 'completed',
                  _key: newLocalId('key'),
                }],
              }]
            }
            const tid = ev.thread_id || ''
            if (tid) saveChatCache(project, tid, next)
            return next
          })
          if (!keepId) setActivity('历史已清理')
          break
        }
        case 'request_user_input': {
          const opts = ev.options || []
          const activeId = activeTurnRef.current
          // Empty options = queue finished / dismiss all gate cards.
          if (!opts.length) {
            setSetupGateOpen(false)
            let activeStillRunning = false
            setTurns((prev) => prev.map((t) => {
              const isActiveRunning = !!(activeId && t.id === activeId && t.status === 'running')
              if (isActiveRunning) activeStillRunning = true
              return {
                ...t,
                approval: null,
                // Do not force-complete the in-flight steer/reaudit turn — that was
                // unlocking the composer while consistency_auditor was still calling.
                status: isActiveRunning
                  ? 'running'
                  : (t.status === 'awaiting' || t.status === 'running' ? 'complete' : t.status),
                items: isActiveRunning
                  ? (t.items || [])
                  : finishAuditTools(finishOpenItems(t.items)),
              }
            }))
            // Phase event is authoritative; optimistic mirror if still working / idle.
            if (activeStillRunning) {
              applyComposerPhase('working', { activeTurnId: activeId })
            } else if (composerPhaseRef.current === 'awaiting_human') {
              applyComposerPhase('idle', { activityText: ev.prompt || '' })
            }
            break
          }
          // Human must choose — unlock immediately; ignore late pipeline re-locks.
          applyComposerPhase('awaiting_human', { activityText: 'waiting for input' })
          const isSetupGate = opts.some((o) => o.id === 'sc_approve' || o.id === 'sc_revise'
            || o.label === '确认定稿' || o.label === '修改再生成')
          if (isSetupGate) setSetupGateOpen(true)
          // Attach gate to matching turn; if turn_id missed, prefer last turn that has content
          // (never spawn an empty approval-only turn that makes the chat look wiped).
          setTurns((prev) => {
            const gate = { prompt: ev.prompt, options: opts }
            const mutationGate = isMutationConfirmGate(gate)
            const hasMatch = prev.some((t) => t.id === ev.turn_id)
            const lastWithItems = [...prev].reverse().find((t) => (t.items || []).length > 0)
            const targetId = hasMatch
              ? ev.turn_id
              : (lastWithItems?.id || prev[prev.length - 1]?.id || ev.turn_id)
            if (!prev.length) {
              return [{
                id: targetId,
                status: 'awaiting',
                approval: gate,
                items: [{
                  id: `gate-${Date.now()}`,
                  type: 'agent_message',
                  text: ev.prompt || '请选择下一步',
                  status: 'completed',
                }],
              }]
            }
            return prev.map((t) => {
              const matched = t.id === targetId
              let items = finishAuditTools(finishOpenItems(t.items || []))
              // chapter_next / audit / setup: never keep stale「待确认修改」cards.
              if (matched && !mutationGate) items = stripPreviewItems(items)
              // If matched turn is empty, keep a prompt line so dialogue does not vanish.
              const nextItems = matched && !items.length
                ? [{
                  id: `gate-${Date.now()}`,
                  type: 'agent_message',
                  text: ev.prompt || '请选择下一步',
                  status: 'completed',
                }]
                : items
              return {
                ...t,
                // Any open gate ends the "running" busy state for UI purposes.
                status: matched
                  ? (t.status === 'complete' ? 'complete' : 'awaiting')
                  : (t.status === 'running' ? 'awaiting' : t.status),
                approval: matched ? gate : null,
                items: nextItems,
              }
            })
          })
          // Gates often follow a write (plot/chapter); sync reader before user decides.
          syncPreview({ keepSelection: true })
          // Live WS payload has no preview — hydrate open_gate.diffs so writing desk shows −/+.
          const snapTid = ev.thread_id || threadId || ''
          if (snapTid) {
            fetch(`${API}/thread/${encodeURIComponent(snapTid)}/snapshot`)
              .then((r) => r.json())
              .then((snap) => {
                if (!snap || snap.error) return
                setTurns((prev) => mergeSnapshotPreviewIntoTurns(prev, snap))
              })
              .catch(() => {})
          }
          break
        }
        case 'error':
          setActivity(ev.message || 'error')
          break
        default:
          break
      }
    },
    [applyComposerPhase, dropToolDeltaBufForTurn, flushToolDeltaBuf, markTurnLive, onProjectBound, project, queueToolOutputDelta, syncPreview, threadId, upsertTurn],
  )

  handleEventRef.current = handleEvent

  const connectWs = useCallback(
    (tid) => new Promise((resolve, reject) => {
      if (!tid) {
        reject(new Error('no thread'))
        return
      }
      if (wsRef.current) {
        try { wsRef.current.close() } catch { /* ignore */ }
      }
      setWsState('connecting')
      const ws = new WebSocket(wsUrl(`/ws/thread/${tid}`))
      wsRef.current = ws
      const pending = wsPendingRef.current
      pending.length = 0
      let raf = 0
      const BATCH = 128
      const CRITICAL = new Set([
        'item_completed',
        'item_started',
        'request_user_input',
        'todo_updated',
        'chat_history_reset',
        'turn_complete',
        'turn_aborted',
        'turn_started',
        'session_phase_changed',
        'agent_status_changed',
        'error',
      ])
      const dispatch = (ev) => handleEventRef.current?.(ev)
      const flush = () => {
        raf = 0
        if (!pending.length) return
        const n = Math.min(BATCH, pending.length)
        let i = 0
        while (i < n) {
          const ev = pending[i]
          if (ev?.type === 'tool_call_output_delta') {
            let merged = { ...ev, delta: ev.delta || '' }
            i += 1
            while (
              i < n
              && pending[i]?.type === 'tool_call_output_delta'
              && pending[i].item_id === merged.item_id
              && pending[i].turn_id === merged.turn_id
            ) {
              merged = {
                ...merged,
                delta: `${merged.delta || ''}${pending[i].delta || ''}`,
              }
              i += 1
            }
            dispatch(merged)
          } else {
            dispatch(ev)
            i += 1
          }
        }
        pending.splice(0, n)
        if (pending.length) raf = requestAnimationFrame(flush)
      }
      ws.onopen = () => {
        setWsState('open')
        readyRef.current = true
        resolve(ws)
      }
      ws.onerror = () => {
        setWsState('error')
        reject(new Error('ws error'))
      }
      ws.onclose = () => {
        setWsState('closed')
        readyRef.current = false
        if (raf) cancelAnimationFrame(raf)
        raf = 0
      }
      // Drain many events per frame. Critical control events bypass the queue
      // so ItemCompleted / RequestUserInput are never stuck behind token spam.
      ws.onmessage = (msg) => {
        try {
          const ev = JSON.parse(msg.data)
          if (CRITICAL.has(ev?.type)) {
            dispatch(ev)
            return
          }
          pending.push(ev)
          if (!raf) raf = requestAnimationFrame(flush)
        } catch {
          /* ignore */
        }
      }
    }),
    [],
  )

  const boundProjectRef = useRef(null)
  useEffect(() => {
    let cancelled = false
    // Drop previous novel's timeline immediately so async /thread/start cannot
    // merge richer-but-wrong `prev` turns into the new book's session.
    if (boundProjectRef.current !== project) {
      boundProjectRef.current = project
      setTurns([])
      setTodos([])
      setSubAgents([])
      setLiveTurnId('')
      activeTurnRef.current = ''
      composerPhaseRef.current = 'idle'
      setComposerPhase('idle')
      setLoading(false)
      loadingRef.current = false
      humanGateOpenRef.current = false
      setActivity('')
      setSetupGateOpen(false)
      setShowOther(false)
      setOtherText('')
      setInput('')
      setSkillOpen(false)
      taskQueueRef.current = []
      setTaskQueue([])
    }
    ;(async () => {
      const skillsRes = await fetch(`${API}/skills/list`)
        .then((r) => r.json())
        .catch(() => ({ skills: [] }))
      if (!cancelled) setSkills(skillsRes.skills || [])

      // Local cache may help mid-turn crash recovery, but never override an empty
      // server thread (e.g. after「新开任务」cleared disk — stale cache used to revive).
      const cached = loadChatCache(project)

      const started = await fetch(`${API}/thread/start`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ project: project || null }),
      }).then((r) => r.json()).catch(() => ({}))
      if (cancelled) return
      const tid = started.threadId || started.thread_id
      if (tid) {
        setThreadId(tid)
        // Hydrate SubAgent rail after refresh (in-memory children on server).
        fetch(`${API}/thread/${encodeURIComponent(tid)}/agents`)
          .then((r) => r.json())
          .then((data) => {
            if (cancelled || !Array.isArray(data?.agents)) return
            setSubAgents((prev) => mergeSubAgentList(prev, data.agents))
          })
          .catch(() => {})
        const serverTurns = Array.isArray(started.turns) ? started.turns : []
        const fromMessages = Array.isArray(started.messages) && started.messages.length
          ? messagesToTurns(started.messages)
          : []
        const serverEmpty = !serverTurns.length && !fromMessages.length
        // Stale cache from a previous thread must not paint or re-POST.
        const cacheUsable = !!(
          cached?.turns?.length
          && cached.threadId
          && cached.threadId === tid
          && !serverEmpty
        )
        if (serverEmpty && cached?.turns?.length) {
          clearChatCache(project)
          saveChatCache(project, tid, [])
        }
        let nextTurns = null
        if (serverTurns.length && fromMessages.length > serverTurns.length) {
          // ui_turns lagged behind (e.g. revise finished but client overwrote with old audit card).
          nextTurns = fromMessages
        } else if (serverTurns.length) {
          // Tool cards alone with empty NovelX bubbles → fill prose from messages.
          nextTurns = mergeAgentProseFromMessages(serverTurns, fromMessages)
        } else if (fromMessages.length && !cacheUsable) {
          nextTurns = fromMessages
        } else if (cacheUsable) {
          nextTurns = mergeAgentProseFromMessages(cached.turns, fromMessages)
        }
        const hasQueue = !!(started.pending_audit_queue?.chapters?.length)
        setTodos(hasQueue ? queueToTodos(started.pending_audit_queue) : [])
        const turnActive = started.turn_active === true
        // gates.yaml via server open_gate / turn.approval — never invent buttons from Chinese scrape.
        // While a turn is active, ignore restored gates (revise/reaudit in flight).
        const restoredGate = turnActive
          ? null
          : (normalizeServerGate(started.open_gate)
            || approvalFromTurns(serverTurns)
            || approvalFromTurns(nextTurns))
        const hasHumanGate = !!(
          restoredGate
          || (!turnActive && started.pending_audit)
          || started.pending_volume_sync
          || started.pending_setup
          || started.pending_volume_handoff
          || started.pending_chapter_next
          || started.pending_mutation
          || started.pending_chapter_order
          || started.pending_studio_next
          || started.pending_impact
        )
        setSetupGateOpen(!!started.pending_setup && !turnActive)
        const serverPhase = started.composer_phase
          || (turnActive ? 'working' : (hasHumanGate ? 'awaiting_human' : 'idle'))
        if (nextTurns?.length && restoredGate) {
          const mutationGate = isMutationConfirmGate(restoredGate)
            || !!started.pending_mutation
          nextTurns = nextTurns.map((t, idx) => {
            const last = idx === nextTurns.length - 1
            let items = finishAuditTools(finishOpenItems(t.items))
            if (last && !mutationGate) items = stripPreviewItems(items)
            return {
              ...t,
              status: last ? 'awaiting' : (t.status === 'running' ? 'awaiting' : t.status),
              items,
              approval: last ? restoredGate : null,
            }
          })
          nextTurns = attachGatePreviewItems(nextTurns, restoredGate)
        } else if (nextTurns?.length && !hasHumanGate) {
          // Drop stale approval cards; also clear「调用模型中」if the turn already ended.
          nextTurns = nextTurns.map((t) => {
            const stuckRunning = !turnActive && t.status === 'running'
            return {
              ...t,
              approval: null,
              status: stuckRunning ? 'complete' : t.status,
              items: stuckRunning
                ? finishOpenItems(finishAuditTools(t.items || []))
                : finishAuditTools(t.items || []),
            }
          })
        }
        const liveId = turnActive
          ? (started.active_turn_id
            || [...(nextTurns || [])].reverse().find((t) => t.status === 'running')?.id
            || '')
          : ''
        applyComposerPhase(serverPhase, {
          activeTurnId: liveId || undefined,
          activityText: serverPhase === 'awaiting_human'
            ? 'waiting for input'
            : (serverPhase === 'working' ? 'working…' : ''),
        })
        if (cancelled || boundProjectRef.current !== project) return
        if (nextTurns?.length) {
          setTurns((prev) => {
            // StrictMode remount: keep richer same-thread snapshot.
            // Never fall back to `prev` alone — that retained another novel's chat.
            const chosen = pickRicherTurns(nextTurns, prev) || nextTurns
            saveChatCache(project, tid, chosen)
            return chosen
          })
        } else {
          setTurns([])
          saveChatCache(project, tid, [])
        }
        try {
          await connectWs(tid)
        } catch {
          /* HTTP fallback still available */
        }
      }
    })()
    return () => {
      cancelled = true
      // Only close when this effect is torn down for a real project switch /
      // unmount — connectWs is stable so parent re-renders no longer reconnect.
      wsRef.current?.close()
    }
  }, [project, connectWs])

  // Persist chat timeline locally + to disk-backed API (debounced).
  // Never POST while a turn is loading — that revived Running audit cards after the server finished.
  useEffect(() => {
    if (!threadId || !turns.length) return
    const gen = chatGenRef.current
    saveChatCache(project, threadId, turns)
    if (loadingRef.current) return undefined
    const t = setTimeout(() => {
      if (loadingRef.current) return
      if (gen !== chatGenRef.current) return
      fetch(`${API}/thread/turns`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ thread_id: threadId, turns }),
      }).catch(() => {})
    }, 600)
    return () => clearTimeout(t)
  }, [project, threadId, turns, loading])

  // Structure change (new tool/bubble) → resume follow + stick.
  const scrollStructSig = turns.map((t) => (
    `${t.id}:${(t.items || []).map((it) => `${it.id || it._key}:${it.type}`).join('|')}`
  )).join(';')
  const lastScrollSigRef = useRef('')
  useLayoutEffect(() => {
    if (scrollStructSig === lastScrollSigRef.current) return
    lastScrollSigRef.current = scrollStructSig
    followChatBottomRef.current = true
    stickChatBottom()
  }, [scrollStructSig, stickChatBottom])

  // Tool / agent stream growth → keep chat window pinned while following.
  // (Inner tool pre also scrolls; outer pane must move as the card grows.)
  const streamLenSig = turns.reduce((acc, t) => (
    acc + (t.items || []).reduce((n, it) => {
      if (it.type === 'tool_call') return n + String(it.output || '').length
      if (it.type === 'agent_message') return n + String(it.text || '').length
      return n
    }, 0)
  ), 0)
  useLayoutEffect(() => {
    if (!loading && !liveTurnId) return
    stickChatBottom()
  }, [streamLenSig, loading, liveTurnId, activity, stickChatBottom])

  const detectSkillTrigger = (value, caret) => {
    const before = value.slice(0, caret)
    const m = before.match(/\$([a-zA-Z0-9_-]*)$/)
    if (m) {
      setSkillOpen(true)
      setSkillQuery(m[1] || '')
    } else {
      setSkillOpen(false)
      setSkillQuery('')
    }
  }

  const forceUnlockComposer = () => {
    applyComposerPhase('idle')
  }

  const looksStatusQuestion = (msg) => (
    // Narrow status match — do NOT use bare「第N卷」(kills real writes on mis-send).
    /到哪了|写到哪|进行到哪|剧情进度|对照.*剧情|本卷到哪|这一卷到哪|卷进行到/.test(msg)
  )

  const handleStop = async () => {
    const turnId = activeTurnRef.current
    if (threadId && turnId) {
      const op = {
        op: 'interrupt_turn',
        thread_id: threadId,
        turn_id: turnId,
      }
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify(op))
      } else {
        await fetch(`${API}/turn/interrupt`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(op),
        }).catch(() => {})
      }
    }
    forceUnlockComposer()
    setActivity(turnId ? '已中断' : '')
  }

  /** Start a real turn. Callers must ensure the composer is idle (or use sendNow). */
  const sendText = async (text) => {
    const msg = (text || '').trim()
    if (!msg || !threadId) return
    // Guard double-start; busy path should enqueue / sendNow instead.
    if (loadingRef.current) return

    followChatBottomRef.current = true
    setShowOther(false)
    setOtherText('')
    // Choosing a next step dismisses any pending approval cards.
    setSetupGateOpen(false)

    // Optimistic local turn (Codex shows user input immediately)
    const localTurnId = newOptimisticTurnId()
    applyComposerPhase('working', {
      activeTurnId: localTurnId,
      activityText: 'sending…',
    })
    setTurns((prev) => {
      const sealed = sealOtherTurns(prev, localTurnId)
      return [
        ...sealed,
        {
          id: localTurnId,
          status: 'running',
          approval: null,
          items: [{
            type: 'user_message',
            id: newLocalId('item'),
            text: msg,
          }],
        },
      ]
    })

    const op = {
      op: 'start_turn',
      thread_id: threadId,
      text: msg,
      skills: [],
    }

    const ws = wsRef.current
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify(op))
      return
    }

    // Fallback: reconnect then HTTP
    try {
      await connectWs(threadId)
      if (wsRef.current?.readyState === WebSocket.OPEN) {
        wsRef.current.send(JSON.stringify(op))
        return
      }
    } catch {
      /* fall through */
    }
    await fetch(`${API}/turn/start`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        thread_id: threadId,
        text: msg,
        skills: [],
      }),
    }).catch(() => {})
  }

  const enqueueTask = (text) => {
    const msg = (text || '').trim()
    if (!msg || !threadId) return
    const item = { id: newLocalId('q'), text: msg }
    const next = [...taskQueueRef.current, item]
    taskQueueRef.current = next
    setTaskQueue(next)
    // Do not overwrite live activity (写作中…); queue strip already shows the count.
  }

  /** Interrupt the active turn (if any), then start `text` immediately. Keeps other queued items. */
  const sendNow = async (text) => {
    const msg = (text || '').trim()
    if (!msg || !threadId) return
    if (loadingRef.current) {
      try {
        await handleStop()
      } catch {
        /* continue */
      }
      forceUnlockComposer()
    }
    await sendText(msg)
  }

  const flushQueueIfIdle = () => {
    // Only drain when server says idle (not working, not awaiting_human).
    if (composerPhaseRef.current !== 'idle' || loadingRef.current) return
    const gateOpen = (turnsRef.current || []).some(
      (t) => t?.approval || t?.status === 'awaiting',
    )
    if (gateOpen) return
    const q = taskQueueRef.current
    if (!q.length) return
    const [next, ...rest] = q
    taskQueueRef.current = rest
    setTaskQueue(rest)
    void sendText(next.text)
  }
  flushQueueIfIdleRef.current = flushQueueIfIdle

  const removeQueuedTask = (id) => {
    const next = taskQueueRef.current.filter((t) => t.id !== id)
    taskQueueRef.current = next
    setTaskQueue(next)
  }

  const runQueuedTaskNow = (id) => {
    const item = taskQueueRef.current.find((t) => t.id === id)
    if (!item) return
    const next = taskQueueRef.current.filter((t) => t.id !== id)
    taskQueueRef.current = next
    setTaskQueue(next)
    void sendNow(item.text)
  }

  const handleSend = () => {
    const msg = input
    setInput('')
    setSkillOpen(false)
    if (loadingRef.current) {
      if (looksStatusQuestion(msg)) {
        void sendNow(msg)
      } else {
        enqueueTask(msg)
      }
      return
    }
    void sendText(msg)
  }

  const handleSendNow = () => {
    const msg = input
    setInput('')
    setSkillOpen(false)
    void sendNow(msg)
  }

  const handlePickOption = (opt) => {
    // Prefer option id (gates.yaml); labels are aliases for typed replies only.
    // Never enqueue gate tokens — if somehow busy, interrupt and send.
    const text = String(opt.id || opt.label || '')
    if (loadingRef.current) {
      void sendNow(text)
    } else {
      void sendText(text)
    }
  }

  const handleOtherSubmit = () => {
    if (!otherText.trim()) return
    const text = otherText.trim()
    if (loadingRef.current) {
      void sendNow(text)
    } else {
      void sendText(text)
    }
  }

  // Stable wrappers so parent sendRef does not get nulled on every render.
  const sendTextRef = useRef(sendText)
  const sendNowRef = useRef(sendNow)
  const enqueueTaskRef = useRef(enqueueTask)
  sendTextRef.current = sendText
  sendNowRef.current = sendNow
  enqueueTaskRef.current = enqueueTask

  // Parent (reader 下一步 / 定稿栏) can send. busy policy:
  //   interrupt (default) — gate tokens may cut in
  //   ignore — desk CTAs must not double-fire / restart a running turn
  //   queue — soft follow-ups while a turn is running
  useEffect(() => {
    if (!sendRef) return undefined
    sendRef.current = (text, opts = {}) => {
      if (!text) return false
      const msg = String(text)
      const busyPolicy = opts.busy || 'interrupt'
      if (loadingRef.current) {
        if (busyPolicy === 'ignore') return false
        if (busyPolicy === 'queue') {
          enqueueTaskRef.current(msg)
          return true
        }
        void sendNowRef.current(msg)
        return true
      }
      void sendTextRef.current(msg)
      return true
    }
    return () => {
      if (sendRef.current) sendRef.current = null
    }
  }, [sendRef])

  useEffect(() => {
    if (typeof onBusyChange !== 'function') return undefined
    // Desk CTA / 审校附栏 must stay clickable while awaiting human or idle.
    onBusyChange(composerPhase === 'working')
    return undefined
  }, [composerPhase, onBusyChange])

  useEffect(() => {
    if (typeof onSubAgentsChange === 'function') onSubAgentsChange(subAgents)
  }, [subAgents, onSubAgentsChange])

  useEffect(() => {
    if (typeof onThreadIdChange === 'function') onThreadIdChange(threadId || '')
  }, [threadId, onThreadIdChange])

  // Sync draft patches to writing desk for side-by-side review.
  useEffect(() => {
    if (typeof onDraftPatchesChange !== 'function') return undefined
    onDraftPatchesChange(collectDraftPatches(turns))
    return undefined
  }, [turns, onDraftPatchesChange])

  // Sync audit checklist / queue to writing-desk audit dock.
  useEffect(() => {
    if (typeof onAuditStateChange !== 'function') return undefined
    onAuditStateChange(collectAuditState(turns, todos))
    return undefined
  }, [turns, todos, onAuditStateChange])

  // Auto-expand agent when busy or waiting for human choice.
  useEffect(() => {
    const waiting = turns.some((t) => t.approval && (t.status === 'awaiting' || t.status === 'running'))
    if (loading || waiting || setupGateOpen) {
      setAgentCollapsed((prev) => {
        if (!prev) return prev
        saveAgentCollapsed(false)
        return false
      })
    }
  }, [loading, turns, setupGateOpen])

  const toggleAgentCollapsed = () => {
    setAgentCollapsed((prev) => {
      const next = !prev
      saveAgentCollapsed(next)
      return next
    })
  }

  // Mirror setup gate to parent so reader「确认定稿」stays in sync with chat ApprovalOptions.
  useEffect(() => {
    const fromTurns = turns.some((t) => {
      const opts = t.approval?.options || []
      return opts.some((o) => o.id === 'sc_approve' || o.id === 'sc_revise'
        || o.label === '确认定稿' || o.label === '修改再生成')
    })
    const open = fromTurns || setupGateOpen
    onSetupGateChange?.(open)
  }, [turns, setupGateOpen, onSetupGateChange])

  const handleNewTask = async () => {
    const label = project || '当前会话'
    if (!window.confirm(
      `清理「${label}」的对话并新开任务？\n\n会清空聊天与待处理的审校选择；不影响已写大纲、正文与项目文件。`,
    )) {
      return
    }
    // Invalidate any debounced /thread/turns POST still holding old turns.
    chatGenRef.current += 1
    const gen = chatGenRef.current
    if (loading && threadId && activeTurnRef.current) {
      await handleStop()
    }
    clearChatCache(project)
    setTurns([])
    setTodos([])
    setSubAgents([])
    taskQueueRef.current = []
    setTaskQueue([])
    setInput('')
    setOtherText('')
    setShowOther(false)
    setSkillOpen(false)
    setSetupGateOpen(false)
    setActivity('新开任务…')
    applyComposerPhase('idle', { activityText: '新开任务…' })
    try {
      wsRef.current?.close()
    } catch {
      /* ignore */
    }
    const started = await fetch(`${API}/thread/new`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ project: project || null }),
    }).then((r) => r.json()).catch((e) => ({ error: e.message }))
    if (gen !== chatGenRef.current) return
    if (started?.error || started?.ok === false) {
      setActivity(`新开失败：${started.error || '未知错误'}`)
      return
    }
    const tid = started.threadId || started.thread_id
    if (!tid) {
      setActivity('新开失败：无 thread')
      return
    }
    setThreadId(tid)
    clearChatCache(project)
    saveChatCache(project, tid, [])
    setTurns([])
    try {
      await connectWs(tid)
      if (gen === chatGenRef.current) setActivity('已新开任务')
    } catch {
      if (gen === chatGenRef.current) setActivity('已新开任务（HTTP）')
    }
  }

  const onPickSkill = (s) => {
    const phrase = skillPromptForAuthor(s.name, s.description)
    setInput((prev) => {
      const base = String(prev || '').replace(/\$[a-zA-Z0-9_-]*$/, '').trimEnd()
      if (!base) return phrase
      if (base.includes(phrase)) return base
      return `${base}\n${phrase}`
    })
    setSkillOpen(false)
  }

  const placeholder = useMemo(
    () => {
      if (composerPhase === 'working' || loading) {
        return bookTitle
          ? `对《${bookTitle}》排队下一条… Enter 入队 · ⌘/Ctrl+Enter 中断并发送`
          : '排队下一条… Enter 入队 · ⌘/Ctrl+Enter 中断并发送'
      }
      if (composerPhase === 'awaiting_human') {
        return bookTitle
          ? `对《${bookTitle}》选择上方选项，或输入补充…`
          : '选择上方选项，或输入补充…'
      }
      return bookTitle
        ? `对《${bookTitle}》说说下一步… Enter 发送 · 点「常用动作」快速开始`
        : '描述你想写的小说… Enter 发送 · 点「常用动作」快速开始'
    },
    [bookTitle, loading, composerPhase],
  )

  const truncateQueueText = (text, max = 72) => {
    const t = String(text || '').replace(/\s+/g, ' ').trim()
    if (t.length <= max) return t
    return `${t.slice(0, max)}…`
  }

  const connLabel = wsState === 'open' ? '已连接'
    : wsState === 'connecting' ? '连接中'
      : threadId ? '备用通道' : '未连接'
  const connTone = wsState === 'open' ? 'ok'
    : wsState === 'connecting' ? 'pending'
      : threadId ? 'warn' : 'idle'

  const openPatchInDesk = (item) => {
    if (typeof onDraftPatchesChange === 'function') {
      // Ensure parent has latest patches, then signal focus via same payload.
      onDraftPatchesChange(collectDraftPatches(turns), { focus: item })
    }
    setAgentCollapsed((prev) => {
      if (!prev) return prev
      // Keep expanded so approvals stay reachable; desk shows the diff.
      return prev
    })
  }

  if (agentCollapsed) {
    const pendingTodos = todos.filter((t) => t.status === 'pending' || t.status === 'in_progress').length
    return (
      <section className="panel agent-chat nx-agent is-collapsed" aria-label="创作助手（已收拢）">
        <StudioNavIcons
          compact
          statusOpen={statusOpen}
          engineOpen={engineOpen}
          onOpenStatus={onOpenStatus}
          onOpenSettings={onOpenEngine}
        />
        <button
          type="button"
          className="agent-rail-toggle"
          onClick={toggleAgentCollapsed}
          title="展开创作助手"
          aria-keyshortcuts="Enter"
        >
          <span className="agent-rail-label">助手</span>
          {loading ? <span className="agent-rail-pulse" aria-label="进行中" /> : null}
          {pendingTodos > 0 ? (
            <span className="agent-rail-badge">{pendingTodos}</span>
          ) : null}
          <SubAgentRail
            agents={subAgents}
            openThreadId={openSubAgentId}
            onOpen={onOpenSubAgent}
            collapsed
          />
        </button>
      </section>
    )
  }

  return (
    <section className="panel agent-chat nx-agent">
      <div className="chat-head">
        <div className="chat-head-brand">
          <h2>创作助手</h2>
          <span
            className={`conn-light conn-${connTone}`}
            role="status"
            aria-label={connLabel}
            title={connLabel}
          />
          {loading ? <span className="status-chip status-running">进行中</span> : null}
        </div>
        <div className="chat-head-actions" role="toolbar" aria-label="助手操作">
          <StudioNavIcons
            statusOpen={statusOpen}
            engineOpen={engineOpen}
            onOpenStatus={onOpenStatus}
            onOpenSettings={onOpenEngine}
          />
          <span className="chat-head-sep" aria-hidden="true" />
          <button
            type="button"
            className="chat-icon-btn"
            onClick={toggleAgentCollapsed}
            title="收拢助手，扩大写作台"
            aria-label="收拢"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <path
                fill="currentColor"
                d="M2 2.5h7.5a.5.5 0 0 1 0 1H3v9h6.5a.5.5 0 0 1 0 1H2A.5.5 0 0 1 1.5 13V3A.5.5 0 0 1 2 2.5zm8.85 2.65a.5.5 0 0 1 .7 0l2.5 2.5a.5.5 0 0 1 0 .7l-2.5 2.5a.5.5 0 1 1-.7-.7L12.79 8.5H7.5a.5.5 0 0 1 0-1h5.29l-1.44-1.65a.5.5 0 0 1 0-.7z"
              />
            </svg>
          </button>
          <button
            type="button"
            className="chat-icon-btn"
            onClick={handleNewTask}
            title="清空对话历史并新开任务（不删项目文件）"
            aria-label="新开任务"
          >
            <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
              <path
                fill="currentColor"
                d="M8 1.5a.5.5 0 0 1 .5.5v5.5H14a.5.5 0 0 1 0 1H8.5V14a.5.5 0 0 1-1 0V8.5H2a.5.5 0 0 1 0-1h5.5V2a.5.5 0 0 1 .5-.5z"
              />
            </svg>
          </button>
        </div>
      </div>
      <SubAgentRail
        agents={subAgents}
        openThreadId={openSubAgentId}
        onOpen={onOpenSubAgent}
      />
      <div className="chat-messages" ref={messagesRef} onScroll={onChatScroll}>
        <TurnTimeline
          turns={turns}
          loading={loading}
          project={project}
          liveTurnId={liveTurnId}
          todos={todos}
          onReaderJump={(opts) => syncPreview({
            ...opts,
            immediate: true,
            focusLatestChapter: false,
          })}
          onOpenPatchInDesk={openPatchInDesk}
          onOpenSubAgent={onOpenSubAgent}
          compactDraftPatches={compactDraftPatches}
          onPickOption={handlePickOption}
          otherText={otherText}
          setOtherText={setOtherText}
          showOther={showOther}
          setShowOther={setShowOther}
          onOtherSubmit={handleOtherSubmit}
        />
        <div ref={endRef} />
      </div>
      {activity ? (
        <div className="nx-activity" aria-live="polite">
          <span className={`nx-activity-dot${loading ? ' pulse' : ''}`} />
          {activity}
        </div>
      ) : null}
      <div className="chat-compose">
        {taskQueue.length > 0 ? (
          <div className="chat-task-queue" aria-label="任务队列">
            <div className="chat-task-queue-head">
              <span className="chat-task-queue-title">队列 {taskQueue.length}</span>
              <span className="chat-task-queue-hint">当前结束后自动执行</span>
            </div>
            <ul className="chat-task-queue-list">
              {taskQueue.map((item, idx) => (
                <li key={item.id} className="chat-task-queue-item">
                  <span className="chat-task-queue-idx">{idx + 1}</span>
                  <span className="chat-task-queue-text" title={item.text}>
                    {truncateQueueText(item.text)}
                  </span>
                  <div className="chat-task-queue-actions">
                    <button
                      type="button"
                      className="btn-ghost btn-inline"
                      onClick={() => runQueuedTaskNow(item.id)}
                      title="中断当前任务并立即执行此项"
                    >
                      立即执行
                    </button>
                    <button
                      type="button"
                      className="btn-ghost btn-inline"
                      onClick={() => removeQueuedTask(item.id)}
                      title="从队列移除"
                    >
                      删除
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          </div>
        ) : null}
        <div className="chat-input-wrap">
          <SkillPopup
            open={skillOpen}
            query={skillQuery}
            skills={skills}
            onPick={onPickSkill}
            onClose={() => setSkillOpen(false)}
          />
          <textarea
            className="chat-input"
            placeholder={placeholder}
            value={input}
            onChange={(e) => {
              const v = e.target.value
              setInput(v)
              detectSkillTrigger(v, e.target.selectionStart ?? v.length)
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault()
                if ((e.metaKey || e.ctrlKey) && loading) {
                  handleSendNow()
                } else {
                  handleSend()
                }
              }
            }}
            rows={3}
            disabled={!threadId}
          />
          <div className="chat-input-actions">
            {(loading || /working|sending|运行中/.test(activity || '')) && (
              <button type="button" className="btn-ghost btn-inline" onClick={handleStop}>
                中断
              </button>
            )}
            {loading && (
              <button
                type="button"
                className="btn-ghost btn-inline"
                onClick={handleSendNow}
                disabled={!input.trim() || !threadId}
                title="中断当前任务并立刻发送"
              >
                中断并发送
              </button>
            )}
            <button
              type="button"
              className="btn-primary btn-inline"
              onClick={handleSend}
              disabled={!input.trim() || !threadId}
            >
              {loading ? '加入队列' : '发送'}
            </button>
          </div>
        </div>
        <div className="nx-composer-hint">
          {composerPhase === 'working' || loading
            ? '忙时 Enter 入队 · ⌘/Ctrl+Enter 中断并发送'
            : composerPhase === 'awaiting_human'
              ? '请先点上方选项；也可输入补充后发送'
              : 'Enter 发送 · 「常用动作」可快速开始 · 进度与审阅在上方'}
          {bookTitle ? ` · 《${bookTitle}》` : ''}
        </div>
      </div>
    </section>
  )
}
