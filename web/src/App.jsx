import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import {
  DRAFT_WORKSPACE_KEY,
  emptyWorkspace,
  loadStudioCache,
  migrateCache,
  saveStudioCache,
  snapshotWorkspace,
  workspaceKey,
} from './studioCache'
import NovelXChat from './components/NovelXChat'
import SubAgentPage from './components/SubAgentPage'
import {
  foreshadowHealthLevel,
  lengthHealthLevel,
  longformTierLevel,
  volumeHealthLevel,
  worstHealthLevel,
} from './healthLabels'
import {
  CreationStatusBar,
  CreationStatusPage,
  formatCostTop,
} from './components/CreationStatus'
import ChapterStrip from './components/ChapterStrip'
import DangerConfirmModal from './components/DangerConfirmModal'
import NewNovelModal from './components/NewNovelModal'
import { LinedProseEditor, LinedProseView } from './components/LinedProse'
import InlineDiffView from './components/InlineDiffView'
import ConfigPanel from './components/ConfigPanel'
import VolumeWorkspace from './components/VolumeWorkspace'
import { normalizeSubAgent, upsertSubAgent } from './subAgents'
import { buildVolumePlotGroups, sortPlotsByProgress } from './plotSort'
import {
  buildCreateNovelMessage,
  deriveStudioStage,
  resolveStudioCta,
} from './studioPhase'
import {
  formatUnitTitle,
  wordProgressLabel,
  wordProgressTone,
  wordTargetsForMode,
} from './chapterTargets'
import { pickActivePlot, summarizePlotForDesk } from './plotSummary'
import { textHunkDiffs } from './textHunkDiffs'
import { applyParaPatches, resolveInlineDiffPair } from './lineDiff'

const API = '/api'
const initialCache = typeof window !== 'undefined' ? loadStudioCache() : migrateCache(null)
const initialProject = initialCache.activeProject || ''
const initialWorkspace = initialCache.projectSessions[
  workspaceKey(initialProject)
] || emptyWorkspace(initialProject)

async function api(path, options = {}) {
  const res = await fetch(`${API}${path}`, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
  let data = {}
  try {
    data = await res.json()
  } catch {
    data = {}
  }
  if (!res.ok) {
    const detail = data.detail
    const errText = typeof data.error === 'string' && data.error
      ? data.error
      : typeof detail === 'string'
        ? detail
        : Array.isArray(detail)
          ? detail.map((d) => d.msg).join('; ')
          : `请求失败 (${res.status})`
    return { error: errText }
  }
  return data
}

/**
 * Approved reader artifact modules only.
 * Unknown files under artifacts/ must NOT auto-create tabs (no `|| key` fallback).
 * Add a row here only after product review — same layer as 总纲/卷纲, not a dump of disk stems.
 */
const READER_ART_MODULES = [
  {
    key: 'bible',
    altKeys: ['world_architect'], // legacy dual-write → one「世界观」tab
    label: '世界观',
  },
]

const ENTITY_TAB_LABELS = {
  characters: '人物',
  items: '物品',
  locations: '地点',
}

const ENTITY_STATUS_LABELS = {
  active: '在场',
  background: '背景',
  exited: '退场',
  consumed: '已消耗',
}

function entityCardKey(e) {
  return e?.slug || e?.id || e?.name || ''
}

function plotCardKey(p) {
  return p?.slug || p?.id || p?.title || ''
}

function entityStatusLabel(status) {
  const s = String(status || '').trim()
  if (!s) return ''
  return ENTITY_STATUS_LABELS[s] || s
}

/** Display entity card with lifecycle status/holdings (frontmatter); edit uses markdown only. */
function formatEntityCard(e, emptyLabel = '设定卡') {
  if (!e) return `（暂无${emptyLabel}）`
  const mark = e.complete ? '' : '（待补全）'
  const gaps = e.gaps?.length ? `\n> 缺口：${e.gaps.join('、')}\n` : ''
  const status = entityStatusLabel(e.status)
  const holdings = String(e.holdings || '').trim()
  const bodyState = String(e.body_state || '').trim()
  const meta = []
  if (status) meta.push(`- **状态**：${status}`)
  if (holdings) meta.push(`- **持有**：${holdings}`)
  const parts = []
  if (bodyState) {
    parts.push(`## 身体与能力状态\n\n${bodyState}`)
  }
  if (meta.length) {
    parts.push(`## 当前状态\n\n${meta.join('\n')}`)
  }
  const metaBlock = parts.length ? `${parts.join('\n\n')}\n\n---\n\n` : ''
  return `${metaBlock}${e.markdown || `# ${e.name}`}${gaps}${mark ? `\n${mark}` : ''}`
}

const EXPECTED_KIND_LABELS = {
  add_character: '加角色',
  exit_character: '退场',
  revive_character: '复活',
  plot: '剧情',
  setting: '设定',
  other: '其他',
}

const EXPECTED_STATUS_LABELS = {
  waiting: '等待中',
  eligible: '可检阅',
  approved: '已批准',
  incorporated: '已纳入',
  dismissed: '已搁置',
}

const PLOT_STATUS_LABELS = {
  active: '进行中',
  in_progress: '进行中',
  pending: '待开始',
  completed: '已完成',
  done: '已完成',
  paused: '暂停',
  archived: '已归档',
}

function formatExpectedConditions(c) {
  if (!c || typeof c !== 'object') return '随时可检阅'
  const parts = []
  if (c.min_chapter) parts.push(`大约从第${c.min_chapter}章起`)
  if (c.max_chapter) parts.push(`大约到第${c.max_chapter}章前`)
  if (c.volume) parts.push(`适合第${c.volume}卷`)
  if (c.require_plot_id) {
    const st = c.require_plot_status
      ? `（${EXPECTED_STATUS_LABELS[c.require_plot_status] || '指定状态'}）`
      : ''
    parts.push(`需相关剧情推进${st}`)
  } else if (c.require_plot_status) {
    parts.push(`需剧情处于「${EXPECTED_STATUS_LABELS[c.require_plot_status] || '指定状态'}」`)
  }
  if (c.require_entity) {
    parts.push(`涉及相关人物或设定`)
  }
  if (c.after_event_id) parts.push('需先完成前序预期')
  if (c.freeform) parts.push(String(c.freeform).slice(0, 80))
  return parts.length ? parts.join(' · ') : '随时可检阅'
}

function formatExpectedEvent(e) {
  if (!e) return '（还没有登记「以后想写的情节」）'
  const kind = EXPECTED_KIND_LABELS[e.kind] || (/[\u4e00-\u9fff]/.test(String(e.kind || '')) ? e.kind : '其他')
  const status = EXPECTED_STATUS_LABELS[e.status] || (/[\u4e00-\u9fff]/.test(String(e.status || '')) ? e.status : '待定')
  const elig = e.eligibility?.label || ''
  const reason = e.last_review?.reason || ''
  const suggestion = e.last_review?.suggestion || ''
  return (
    `# ${String(e.text || '未命名预期').slice(0, 80)}\n\n`
    + `- **类型**：${kind}\n`
    + `- **状态**：${status}${elig ? `（${elig}）` : ''}\n`
    + `- **来源**：${e.source === 'reader' ? '读者' : '作者'}\n`
    + `- **何时适合写**：${formatExpectedConditions(e.conditions)}\n`
    + (reason ? `- **最近看法**：${reason}\n` : '')
    + (suggestion ? `- **建议做法**：${suggestion}\n` : '')
    + (e.notes ? `\n## 备注\n\n${e.notes}\n` : '')
    + '\n> 只读展示。是否纳入请在右侧创作助手中选择。\n'
  )
}

function formatPlotCard(p) {
  if (!p) {
    return (
      '（暂无剧情卡）\n\n'
      + '剧情卡 = 卷内一段情节（不是整卷卷纲）。须含：概览、剧情走向、出场人物/物品/设定、可核验收束。\n'
      + '节奏：本段收束 → 衔接章 → 下一张剧情卡；一卷通常多张依次推进。\n'
      + '可说「第1卷下一段剧情：…」'
    )
  }
  const joinList = (arr) => (arr?.length ? arr.join('、') : '—')
  const mark = p.complete ? '' : '（待补全）'
  const gaps = p.gaps?.length ? `\n> 缺口：${p.gaps.join('、')}` : ''
  const scope = '卷内局部'
  const statusZh = PLOT_STATUS_LABELS[p.status]
    || (/[\u4e00-\u9fff]/.test(String(p.status || '')) ? p.status : '')
  // 优先完整 markdown（与磁盘卡一致）；无则拼结构化预览
  if (p.markdown?.trim()) {
    return `${p.markdown.trim()}${gaps}${mark ? `\n${mark}` : ''}`
  }
  return (
    `# ${p.title || '未命名'}\n`
    + `${p.anchor || `${scope} · ${p.arc || ''} · ${p.chapter_range || ''}`}\n`
    + `类型：${p.plot_type || '剧情'} · 状态：${statusZh || '进行中'}\n\n`
    + `## 概览\n${p.overview || p.summary || '（无）'}\n\n`
    + `## 剧情走向\n${p.plot_direction || '（无）'}\n\n`
    + `## 出场人物\n${joinList(p.characters)}\n\n`
    + `## 出场物品\n${joinList(p.items)}\n\n`
    + `## 相关设定\n${joinList(p.settings)}\n\n`
    + `## 衔接章\n${p.bridge_chapter ? `第 ${p.bridge_chapter} 章` : '（默认收束章+1）'}\n\n`
    + `## 下一剧情卡\n${p.next_plot || '（待指定）'}\n`
    + `${gaps}${mark ? `\n${mark}` : ''}`
  )
}

export default function App() {
  /** Status / settings both open as right drawers (consistent chrome). */
  const [statusOpen, setStatusOpen] = useState(false)
  const [engineOpen, setEngineOpen] = useState(false)
  /** High-risk delete: { kind, name?, title, chapter?, unit? } */
  const [dangerConfirm, setDangerConfirm] = useState(null)
  const [dangerBusy, setDangerBusy] = useState(false)
  const [library, setLibrary] = useState([])
  const [novelRecord, setNovelRecord] = useState(null)
  const [project, setProject] = useState(initialProject)
  const [preview, setPreview] = useState(null)
  /** On-demand chapter body (preview lists omit full drafts for longform). */
  const [chapterCache, setChapterCache] = useState({})
  const [chapterLoading, setChapterLoading] = useState(false)
  const [selectedChapter, setSelectedChapter] = useState(() => initialWorkspace.selectedChapter || 1)
  const [readerTab, setReaderTab] = useState('draft')
  const [readerCardKey, setReaderCardKey] = useState('')
  /** Selected volume when browsing 卷纲 (two-level nav). */
  const [readerVolume, setReaderVolume] = useState(0)
  const [readerEditing, setReaderEditing] = useState(false)
  const [readerEditText, setReaderEditText] = useState('')
  /** Baseline on-disk text when edit began (for git-style confirm). */
  const [readerEditBaseline, setReaderEditBaseline] = useState('')
  /** Pending save confirm: { before, after, diffs, saveTab, cardKey, chapter } */
  const [readerSaveConfirm, setReaderSaveConfirm] = useState(null)
  const [readerSaving, setReaderSaving] = useState(false)
  const [readerEditError, setReaderEditError] = useState('')
  const [chatSetupGateOpen, setChatSetupGateOpen] = useState(false)
  const [foreshadowDebtExpanded, setForeshadowDebtExpanded] = useState(false)
  const [showNewNovelForm, setShowNewNovelForm] = useState(false)
  const [newNovelTitle, setNewNovelTitle] = useState('')
  const [newNovelGenre, setNewNovelGenre] = useState('')
  const [newNovelBrief, setNewNovelBrief] = useState('')
  const [newNovelMode, setNewNovelMode] = useState('longform')
  const [newNovelBusy, setNewNovelBusy] = useState(false)
  const [deskPatches, setDeskPatches] = useState([])
  const [deskPatchFocus, setDeskPatchFocus] = useState(null)
  const [deskPatchHidden, setDeskPatchHidden] = useState(false)
  /** Selection from InlineDiffView: { selectedIds, partialAfter, fullAfter, selectedCount, totalCount } */
  const [deskDiffSel, setDeskDiffSel] = useState(null)
  const handleInlineDiffSelection = useCallback((sel) => {
    setDeskDiffSel(sel || null)
  }, [])
  const [auditState, setAuditState] = useState({
    todos: [],
    reports: [],
    latest: null,
    openApproval: null,
  })
  const [chatBusy, setChatBusy] = useState(false)
  /** SubAgent dedicated page — null means main Studio (reader + chat). */
  const [openSubAgent, setOpenSubAgent] = useState(null)
  const [subAgents, setSubAgents] = useState([])
  const chatSendRef = useRef(null)
  const workspacesRef = useRef(initialCache.projectSessions || {})
  const currentProjectRef = useRef(initialProject)
  const restoredRef = useRef(false)
  const readerRef = useRef(null)
  /** Writer 落盘刷新时默认贴底；用户上滑阅读则暂停跟滚。 */
  const followDraftBottomRef = useRef(true)
  const lastDraftLenRef = useRef(0)
  /** Leave inline diff / Apply：禁止误贴底，并尽量恢复对照时的阅读位置。 */
  const suppressDraftPinRef = useRef(false)
  const readerScrollRestoreRef = useRef(null)

  const captureReaderScrollForDiffExit = useCallback(() => {
    const el = readerRef.current
    if (!el) return
    const max = Math.max(1, el.scrollHeight - el.clientHeight)
    readerScrollRestoreRef.current = {
      top: el.scrollTop,
      ratio: max > 0 ? el.scrollTop / el.scrollHeight : 0,
    }
    suppressDraftPinRef.current = true
    // Apply 后不应继续「写作跟滚」到文末。
    followDraftBottomRef.current = false
  }, [])

  const refreshLibrary = useCallback(async () => {
    const data = await api('/library')
    setLibrary(data.novels || [])
  }, [])

  const persistCurrentWorkspace = useCallback(() => {
    const key = workspaceKey(currentProjectRef.current)
    const prev = workspacesRef.current[key] || emptyWorkspace(currentProjectRef.current)
    workspacesRef.current[key] = snapshotWorkspace({
      project: currentProjectRef.current,
      studioSessionId: prev.studioSessionId,
      chatMessages: prev.chatMessages || [],
      run: prev.run || null,
      batchId: prev.batchId || null,
      selectedChapter,
    })
  }, [selectedChapter])

  const clearToDraft = useCallback(() => {
    persistCurrentWorkspace()
    workspacesRef.current[DRAFT_WORKSPACE_KEY] = emptyWorkspace('')
    setProject('')
    currentProjectRef.current = ''
    setPreview(null)
    setNovelRecord(null)
    setReaderTab('draft')
    setSelectedChapter(1)
    setOpenSubAgent(null)
    setSubAgents([])
  }, [persistCurrentWorkspace])

  // Switching novels closes SubAgent page (child threads belong to prior root).
  useEffect(() => {
    setOpenSubAgent(null)
  }, [project])

  const requestDeleteNovel = useCallback((name, displayTitle, event) => {
    event?.stopPropagation()
    event?.preventDefault()
    if (!name) return
    const title = displayTitle || name
    setDangerConfirm({
      kind: 'novel',
      name,
      title,
      confirmText: title,
      dialogTitle: `删除《${title}》？`,
      message: '项目文件将永久删除，不可恢复。请输入书名以二次确认。',
    })
  }, [])

  const performDeleteNovel = useCallback(async (name) => {
    if (!name) return false
    const data = await api(`/library/${encodeURIComponent(name)}`, { method: 'DELETE' })
    if (data.error || data.ok === false) {
      window.alert(data.error || '删除失败')
      return false
    }
    delete workspacesRef.current[workspaceKey(name)]
    const wasActive = currentProjectRef.current === name
    if (wasActive) {
      clearToDraft()
    }
    saveStudioCache({
      activeProject: wasActive ? '' : currentProjectRef.current,
      projectSessions: workspacesRef.current,
    })
    await refreshLibrary()
    return true
  }, [clearToDraft, refreshLibrary])

  const fetchNovelPreview = useCallback(async (name, tabHint) => {
    const data = await api(`/library/${encodeURIComponent(name)}`)
    if (data.error) return false
    setNovelRecord(data.novel)
    setPreview(data.preview)
    // Preview no longer embeds bodies — drop stale chapter cache on refresh.
    setChapterCache({})
    setProject(name)
    currentProjectRef.current = name
    if (data.preview?.chapters?.length) {
      setSelectedChapter(data.preview.chapters[data.preview.chapters.length - 1].number)
    }
    if (tabHint === 'master') {
      setReaderTab('master')
    } else if (data.preview?.setup_phase === 'awaiting_confirm') {
      setReaderTab('master')
    } else if (data.preview?.story_outline && !data.preview?.chapters?.length) {
      setReaderTab('master')
    } else if (
      (data.preview?.arc_outlines?.length || data.preview?.plots?.length)
      && !data.preview?.chapters?.length
    ) {
      // No chapters yet — land on volume workspace, not raw arc strip.
      setReaderTab('volume')
    } else {
      setReaderTab('draft')
    }
    return true
  }, [])

  const selectProject = useCallback(async (name, tabHint) => {
    if (name === currentProjectRef.current && preview) {
      return
    }
    persistCurrentWorkspace()
    const ok = await fetchNovelPreview(name, tabHint)
    if (!ok) return
    setForeshadowDebtExpanded(false)
    const projectKey = workspaceKey(name)
    const ws = workspacesRef.current[projectKey] || emptyWorkspace(name)
    workspacesRef.current[projectKey] = ws
    if (ws.selectedChapter != null) {
      setSelectedChapter(ws.selectedChapter)
    }
  }, [fetchNovelPreview, persistCurrentWorkspace, preview])

  const loadNovel = selectProject

  const refreshPreview = useCallback(async (name, opts = {}) => {
    if (!name) return
    const data = await api(`/projects/${name}/preview`)
    setPreview(data)
    const chapters = data.chapters || []
    // Invalidate only the written chapter (or prune deleted ones). Full wipe flashes empty body.
    if (opts.chapter != null && opts.chapter > 0) {
      setChapterCache((prev) => {
        const next = { ...prev }
        delete next[opts.chapter]
        return next
      })
      setSelectedChapter(opts.chapter)
    } else {
      const nums = new Set(chapters.map((c) => c.number))
      setChapterCache((prev) => {
        const next = {}
        for (const [k, v] of Object.entries(prev)) {
          if (nums.has(Number(k))) next[k] = v
        }
        return next
      })
      if ((opts.focusLatestChapter || !opts.keepSelection) && chapters.length) {
        setSelectedChapter(chapters[chapters.length - 1].number)
      }
    }
    if (opts.readerTab) {
      setReaderTab(opts.readerTab === 'plots' ? 'arcs' : opts.readerTab)
    }
    // Mid-stream draft flushes pass keepSelection — skip library refetch (was
    // re-rendering the whole studio and amplifying chat Turn flicker).
    if (opts.keepSelection) return
    const lib = await api(`/library/${name}`)
    if (!lib.error) setNovelRecord(lib.novel)
    refreshLibrary()
  }, [refreshLibrary])

  // Stable identity — inline arrow here remounted NovelXChat's WS effect and
  // overwrote live turns with lagging ui_turns (「闪回」).
  const handleChatPreviewRefresh = useCallback((name, opts) => {
    if (name) refreshPreview(name, opts)
  }, [refreshPreview])

  const handleProjectBound = useCallback(async (name) => {
    const id = String(name || '').trim()
    if (!id) return
    setShowNewNovelForm(false)
    setNewNovelBusy(false)
    await refreshLibrary()
    await selectProject(id, 'master')
  }, [refreshLibrary, selectProject])

  const deskPatchesRef = useRef([])
  // Keep audit snapshot for top「下一步」alignment only (no bottom audit dock).
  const handleAuditStateChange = useCallback((state) => {
    setAuditState(state || { todos: [], reports: [], latest: null, openApproval: null })
  }, [])

  const focusDeskPatch = useCallback((patch) => {
    if (!patch) return
    setDeskPatchFocus(patch)
    setDeskPatchHidden(false)
    const tab = String(patch.readerTab || '').trim()
    const ch = Number(patch.chapter) || 0
    if (tab === 'draft' || tab === 'outline') {
      if (ch > 0) setSelectedChapter(ch)
      setReaderTab(tab)
      return
    }
    if (tab) {
      setReaderTab(tab)
      return
    }
    if (ch > 0) {
      setSelectedChapter(ch)
      setReaderTab('draft')
    }
  }, [])

  const handleDraftPatchesChange = useCallback((patches, opts = {}) => {
    const list = Array.isArray(patches) ? patches : []
    const patchSig = (p) => (
      `${p.type}:${p.chapter}:${p.readerTab || ''}:${p.path || ''}:`
      + `${String(p.beforeFull || p.before || '').length}:`
      + `${String(p.afterFull || p.after || '').length}:${p.start_para || ''}`
    )
    const prevSig = deskPatchesRef.current.map(patchSig).join('|')
    const nextSig = list.map(patchSig).join('|')
    const arrived = list.length && prevSig !== nextSig
    const leaving = deskPatchesRef.current.length > 0 && !list.length
    if (leaving) captureReaderScrollForDiffExit()
    deskPatchesRef.current = list
    setDeskPatches(list)
    if (opts.focus || arrived) {
      setDeskPatchHidden(false)
    }
    if (opts.focus) {
      focusDeskPatch(opts.focus)
      return
    }
    if (!list.length) {
      setDeskPatchFocus(null)
      return
    }
    // New diffs → jump writing desk so inline −/+ is visible on the right tab.
    if (arrived) {
      focusDeskPatch(list[list.length - 1])
      return
    }
    setDeskPatchFocus((prev) => {
      if (!prev) return list[list.length - 1]
      const still = list.find((p) => patchSig(p) === patchSig(prev))
      return still || list[list.length - 1]
    })
  }, [focusDeskPatch, captureReaderScrollForDiffExit])

  const sendChatMessage = useCallback((text, opts) => {
    if (!text || typeof chatSendRef.current !== 'function') return false
    return chatSendRef.current(text, opts) !== false
  }, [])

  const handleSubAgentsChange = useCallback((list) => {
    const next = Array.isArray(list) ? list : []
    setSubAgents(next)
    setOpenSubAgent((cur) => {
      if (!cur?.threadId) return cur
      const fresh = next.find((a) => a.threadId === cur.threadId)
      return fresh ? { ...cur, ...fresh } : cur
    })
  }, [])

  const handleOpenSubAgent = useCallback((agent) => {
    const next = normalizeSubAgent(agent)
    if (!next?.threadId) return
    setSubAgents((prev) => upsertSubAgent(prev, next))
    setOpenSubAgent(next)
  }, [])

  const handleSubAgentUpdate = useCallback((agent) => {
    const next = normalizeSubAgent(agent)
    if (!next?.threadId) return
    setSubAgents((prev) => upsertSubAgent(prev, next))
    setOpenSubAgent((cur) => (
      cur && cur.threadId === next.threadId ? { ...cur, ...next } : cur
    ))
  }, [])

  const handleCloseSubAgent = useCallback(() => {
    setOpenSubAgent(null)
  }, [])

  const handleChatBusyChange = useCallback((busy) => {
    setChatBusy(Boolean(busy))
  }, [])

  const submitNewNovel = useCallback(() => {
    const title = newNovelTitle.trim()
    if (!title) {
      window.alert('请填写书名')
      return
    }
    const msg = buildCreateNovelMessage({
      title,
      genre: newNovelGenre,
      brief: newNovelBrief,
      mode: newNovelMode,
    })
    if (project) clearToDraft()
    setNewNovelBusy(true)
    setNewNovelTitle('')
    setNewNovelGenre('')
    setNewNovelBrief('')
    setNewNovelMode('longform')
    setShowNewNovelForm(false)

    const startPoll = () => {
      let tries = 0
      const poll = window.setInterval(async () => {
        tries += 1
        const data = await api('/library')
        const novels = data.novels || []
        setLibrary(novels)
        const hit = novels.find((n) => (
          n.id === title
          || n.display_title === title
          || String(n.display_title || '').includes(title)
        ))
        if (hit?.id && hit.id !== project) {
          window.clearInterval(poll)
          setNewNovelBusy(false)
          await selectProject(hit.id, 'master')
          return
        }
        if (tries >= 24) {
          window.clearInterval(poll)
          setNewNovelBusy(false)
        }
      }, 1500)
    }

    // Chat may rebind sendRef after clearing project — retry briefly.
    let attempt = 0
    const trySend = () => {
      if (sendChatMessage(msg)) {
        startPoll()
        return
      }
      attempt += 1
      if (attempt < 12) {
        window.setTimeout(trySend, 200)
        return
      }
      setNewNovelBusy(false)
      window.alert('创作助手尚未就绪，请稍后再试或直接在右侧输入')
    }
    window.setTimeout(trySend, 120)
  }, [
    clearToDraft,
    newNovelBrief,
    newNovelGenre,
    newNovelMode,
    newNovelTitle,
    project,
    selectProject,
    sendChatMessage,
  ])

  useEffect(() => { refreshLibrary() }, [refreshLibrary])

  useEffect(() => {
    persistCurrentWorkspace()
    saveStudioCache({
      activeProject: project,
      projectSessions: workspacesRef.current,
    })
  }, [project, selectedChapter, persistCurrentWorkspace])

  useEffect(() => {
    if (restoredRef.current) return
    restoredRef.current = true
    if (initialProject) {
      selectProject(initialProject).then(() => {
        const ws = workspacesRef.current[workspaceKey(initialProject)]
        if (ws?.selectedChapter) setSelectedChapter(ws.selectedChapter)
      })
    }
  }, [selectProject])

  const nextChapter = novelRecord?.next_chapter ?? (preview?.chapters?.length || 0) + 1
  const chapterFocus = selectedChapter > 0 ? selectedChapter : nextChapter
  const chapterMeta = preview?.chapters?.find((c) => c.number === chapterFocus)
  const chapterData = (() => {
    const unit = preview?.project_mode === 'short_drama' ? '集' : '章'
    const n = chapterFocus
    if (!project || !(n > 0)) return null
    const fallbackTitle = `第${n}${unit}`
    const cached = chapterCache[n]
    if (cached) {
      return {
        number: n,
        title: cached.title || chapterMeta?.title || fallbackTitle,
        draft: cached.draft || '',
        outline: cached.outline || '',
        body_chars: cached.body_chars ?? chapterMeta?.body_chars,
      }
    }
    // Legacy: preview still embeds bodies (older servers).
    if (chapterMeta?.draft || chapterMeta?.outline) return chapterMeta
    // Keep 正文/章纲 tabs available even before the chapter file exists on disk.
    return {
      number: n,
      title: chapterMeta?.title || fallbackTitle,
      draft: '',
      outline: '',
      body_chars: chapterMeta?.body_chars || 0,
    }
  })()

  // Fetch chapter body when selection changes (longform-safe preview).
  const chapterCacheRef = useRef(chapterCache)
  chapterCacheRef.current = chapterCache
  useEffect(() => {
    const proj = project
    const ch = selectedChapter
    if (!proj || !ch) return
    const meta = preview?.chapters?.find((c) => c.number === ch)
    if (!meta) return
    if (meta.draft || meta.outline) return // embedded (legacy servers)
    if (!meta.has_draft && !meta.has_outline) return
    const hit = chapterCacheRef.current[ch]
    const metaChars = Number(meta.body_chars) || 0
    const cachedChars = Number(hit?.body_chars) || 0
    // Outline often lands first and caches { draft: '', outline }. Do not skip
    // refetch solely because outline exists — draft can appear/grow later.
    const draftStale = meta.has_draft && (!hit?.draft || metaChars > cachedChars)
    const outlineStale = meta.has_outline && !hit?.outline
    if (hit && !draftStale && !outlineStale) return
    let cancelled = false
    setChapterLoading(true)
    api(`/projects/${encodeURIComponent(proj)}/chapters/${ch}`)
      .then((data) => {
        if (cancelled || data.error) return
        setChapterCache((prev) => ({
          ...prev,
          [ch]: {
            title: data.title,
            draft: data.draft || '',
            outline: data.outline || '',
            body_chars: data.body_chars,
          },
        }))
      })
      .finally(() => {
        if (!cancelled) setChapterLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [project, selectedChapter, preview])

  // Drop body cache when switching novels.
  useEffect(() => {
    setChapterCache({})
  }, [project])

  const storyOutline = preview?.story_outline
  const publishedCount = Number(
    novelRecord?.published_count ?? preview?.published_count ?? 0,
  ) || 0
  const isChapterPublished = (n) => Number(n) > 0 && Number(n) <= publishedCount
  // 总纲：master_outline.md；短剧 series_outline.md 为同层别名，不单独占 Tab。
  // story_outline.json 仅存 acts，不对读者展示第二份总纲。
  const isShortDrama = preview?.project_mode === 'short_drama'
  const arcOutlines = (Array.isArray(preview?.arc_outlines) ? preview.arc_outlines : [])
    .filter((a) => a?.volume >= 1 && String(a?.markdown || '').trim())
    .slice()
    .sort((a, b) => a.volume - b.volume)
  const masterOutlineText = preview?.artifacts?.master_outline
    || preview?.artifacts?.series_outline
    || preview?.artifacts?.master_planner
    || storyOutline?.markdown
    || ''
  const approvedArtTabs = READER_ART_MODULES.map((mod) => {
    const text = preview?.artifacts?.[mod.key]
      || (mod.altKeys || []).map((k) => preview?.artifacts?.[k]).find((v) => String(v || '').trim())
      || ''
    return {
      id: `art:${mod.key}`,
      label: mod.label,
      show: Boolean(String(text).trim()),
      text: String(text || ''),
    }
  }).filter((t) => t.show)
  const entities = preview?.entities || {}
  const plots = sortPlotsByProgress(preview?.plots || [])
  const entityGaps = preview?.entity_gaps || []
  const expectedEvents = preview?.expected_events || []
  // 短剧无卷相位：不挂「本卷 / 卷纲」工作台（剧情 beat 仍可由助手落地，不单独开未核定 Tab）
  const volumeGroups = isShortDrama
    ? []
    : buildVolumePlotGroups(arcOutlines, plots)
  const isArcNav = readerTab === 'arcs' || readerTab === 'plots'
  const isExpectedNav = readerTab === 'expected'
  const isVolumeWorkspace = readerTab === 'volume'

  const activeVolumeGroup = (() => {
    if (!volumeGroups.length) return null
    const byState = volumeGroups.find((g) => g.volume === readerVolume)
    if (byState) return byState
    // Prefer volume of current card key.
    if (readerCardKey?.startsWith('v')) {
      const n = Number(readerCardKey.slice(1))
      const g = volumeGroups.find((x) => x.volume === n)
      if (g) return g
    }
    for (const g of volumeGroups) {
      if (g.plots.some((p) => plotCardKey(p) === readerCardKey)) return g
    }
    return volumeGroups[volumeGroups.length - 1] || volumeGroups[0]
  })()

  const readerCardList = (() => {
    if (isArcNav) {
      if (!activeVolumeGroup) return []
      const vol = activeVolumeGroup.volume
      const items = [{
        kind: 'arc',
        key: `v${vol}`,
        label: '卷纲',
        complete: true,
        volume: vol,
        raw: activeVolumeGroup.arc,
      }]
      for (const p of activeVolumeGroup.plots) {
        const key = plotCardKey(p)
        if (!key) continue
        items.push({
          kind: 'plot',
          key,
          label: p.title || '未命名剧情',
          complete: !!p.complete,
          volume: vol,
          raw: p,
        })
      }
      return items
    }
    if (readerTab.startsWith('ent:')) {
      const group = readerTab.slice(4)
      return (entities[group] || []).map((e) => ({
        kind: 'entity',
        key: entityCardKey(e),
        label: e.name || '未命名',
        complete: !!e.complete,
        status: e.status || '',
        statusLabel: entityStatusLabel(e.status),
        raw: e,
      }))
    }
    if (isExpectedNav) {
      return expectedEvents.map((e) => ({
        kind: 'expected',
        key: e.id || e.text || '',
        label: String(e.text || e.id || '未命名').slice(0, 28),
        complete: e.status === 'incorporated' || e.status === 'approved',
        status: e.status || '',
        statusLabel: e.eligibility?.label
          || EXPECTED_STATUS_LABELS[e.status]
          || e.status
          || '',
        raw: e,
      }))
    }
    return []
  })()

  const selectedReaderEntry = readerCardList.find((c) => c.key === readerCardKey)
    || readerCardList[0]
    || null
  const selectedReaderCard = selectedReaderEntry?.raw || null

  const readerCardKeysSig = readerCardList.map((c) => c.key).join('\0')
  const volumeSig = volumeGroups.map((g) => g.volume).join(',')

  useEffect(() => {
    if (readerTab === 'plots') setReaderTab('arcs')
  }, [readerTab])

  // Short drama: leave longform-only modules if a stale tab was selected.
  useEffect(() => {
    if (!isShortDrama) return
    if (readerTab === 'volume' || readerTab === 'arcs' || readerTab === 'plots') {
      setReaderTab(chapterData ? 'draft' : 'master')
    }
  }, [isShortDrama, readerTab, chapterData])

  // Keep readerVolume in sync with available groups / selection.
  useEffect(() => {
    if (!isArcNav) return
    if (!volumeSig) {
      if (readerVolume !== 0) setReaderVolume(0)
      return
    }
    const vols = volumeSig.split(',').map(Number).filter((n) => n >= 1)
    if (!vols.includes(readerVolume)) {
      setReaderVolume(vols[vols.length - 1] || vols[0] || 0)
    }
  }, [isArcNav, volumeSig, readerVolume])

  useEffect(() => {
    if (!readerCardKeysSig) {
      if (readerCardKey) setReaderCardKey('')
      return
    }
    const keys = readerCardKeysSig.split('\0')
    if (!keys.includes(readerCardKey)) {
      setReaderCardKey(keys[0])
    }
  }, [readerTab, readerCardKeysSig, readerCardKey, readerVolume])

  const chapterList = (preview?.chapters || []).map((c) => {
    const number = c.number
    const published = isChapterPublished(number)
    const hasDraft = !!c.has_draft || (Number(c.body_chars) || 0) > 0
    let status = 'empty'
    let statusLabel = '待写'
    if (published) {
      status = 'published'
      statusLabel = '已发布'
    } else if (hasDraft) {
      status = 'draft'
      statusLabel = '草稿'
    } else if (c.has_outline) {
      status = 'outline'
      statusLabel = '有章纲'
    } else if (number === nextChapter) {
      status = 'next'
      statusLabel = '下一章'
    }
    return {
      number,
      title: c.title,
      summary: '',
      hasDraft,
      hasOutline: !!c.has_outline,
      bodyChars: Number(c.body_chars) || 0,
      status,
      statusLabel,
    }
  })

  const selectedChapterStatus = chapterList.find((c) => c.number === selectedChapter)

  const draftBodyChars = (() => {
    if (readerTab !== 'draft') return 0
    if (readerEditing) return Array.from(readerEditText || '').length
    const fromCache = Number(chapterData?.body_chars) || 0
    if (fromCache > 0) return fromCache
    const draft = String(chapterData?.draft || '')
    return draft ? Array.from(draft.replace(/\s+/g, '')).length || draft.length : 0
  })()
  const projectMode = preview?.project_mode || novelRecord?.project_mode || 'longform'
  const wordBand = wordTargetsForMode(projectMode)
  const wordTone = wordProgressTone(draftBodyChars, projectMode)
  const wordLabel = wordProgressLabel(draftBodyChars, projectMode)
  const unitWordChars = (() => {
    if (readerTab === 'draft' && draftBodyChars > 0) return draftBodyChars
    return Number(selectedChapterStatus?.bodyChars) || 0
  })()
  const readerUnitTitle = formatUnitTitle(
    selectedChapter,
    chapterData?.title,
    projectMode,
  )
  const unitLabel = projectMode === 'short_drama' ? '集' : '章'
  const activePlotSummary = summarizePlotForDesk(pickActivePlot(plots))
  const visibleDeskPatches = deskPatches.filter((p) => {
    if (p.type === 'inline_doc_diff' || p.type === 'doc_patch') return true
    const ch = Number(p.chapter) || 0
    if (ch <= 0) return true
    return !selectedChapter || ch === Number(selectedChapter)
  })
  const activeDeskPatch = (() => {
    if (!visibleDeskPatches.length) return null
    if (deskPatchFocus && visibleDeskPatches.some((p) => (
      p.type === deskPatchFocus.type
      && Number(p.chapter) === Number(deskPatchFocus.chapter)
      && Number(p.start_para) === Number(deskPatchFocus.start_para)
      && String(p.readerTab || '') === String(deskPatchFocus.readerTab || '')
      && String(p.path || '') === String(deskPatchFocus.path || '')
    ))) {
      return deskPatchFocus
    }
    return visibleDeskPatches[visibleDeskPatches.length - 1]
  })()

  const longformHealth = preview?.longform_health || null
  const foreshadowDebt = longformHealth?.foreshadow || null
  const volumeHealth = longformHealth?.volume || null
  const lengthHealth = longformHealth?.length || null
  const longformTier = longformHealth?.longform || null
  const costTop = formatCostTop(preview?.cost_by_agent)
  const lengthChip = lengthHealth
    ? `近期偏短 ${Math.round((lengthHealth.soft_short_rate || 0) * 100)}%`
      + (lengthHealth.consecutive_soft_short
        ? ` · 连续偏短 ${lengthHealth.consecutive_soft_short} 章`
        : '')
    : ''

  // 创作状态交通灯：ok 绿 / warn 黄 / bad 红（含相位字段 + 引擎档位）
  const volumeQaPhase = preview?.volume_qa_phase || ''
  const foreshadowPhase = preview?.foreshadow_phase || ''
  const foreshadowLevel = foreshadowHealthLevel(foreshadowDebt, foreshadowPhase)
  const volumeLevel = volumeHealthLevel(volumeHealth, volumeQaPhase)
  const lengthLevel = lengthHealthLevel(lengthHealth)
  const longformLevel = longformTierLevel(longformTier)
  const overallHealthLevel = worstHealthLevel(
    foreshadowLevel,
    volumeLevel,
    lengthLevel,
    longformLevel,
  )

  const hasReaderMaterial = Boolean(
    chapterList.length
    || approvedArtTabs.length
    || arcOutlines.length
    || volumeGroups.length
    || entities.characters?.length
    || entities.items?.length
    || entities.locations?.length
    || plots.length
    || masterOutlineText
    || storyOutline?.acts?.length
    || project
  )

  const readerContent = (() => {
    if (readerTab === 'volume') return ''
    if (readerTab === 'draft') {
      if (chapterLoading && !chapterData?.draft) return '（加载正文中…）'
      return chapterData?.draft || '（尚无正文）'
    }
    if (readerTab === 'outline') {
      if (chapterLoading && !chapterData?.outline) return '（加载章纲中…）'
      return chapterData?.outline || (isShortDrama ? '（尚无集纲）' : '（尚无章纲）')
    }
    if (readerTab === 'master') {
      return masterOutlineText || '（尚无总纲）'
    }
    if (readerTab === 'entity_gaps') {
      return preview?.entity_gaps_display
        || (entityGaps.length
          ? '待补全（不妨碍继续写）：\n\n' + entityGaps.map((g) => `- ${g}`).join('\n')
          : '设定卡与世界观暂无明显缺口。')
    }
    if (isExpectedNav) {
      if (!expectedEvents.length) return '（尚无预处理预期事件）'
      return formatExpectedEvent(selectedReaderCard || expectedEvents[0])
    }
    if (isArcNav) {
      if (!volumeGroups.length) return '（尚无卷纲 / 剧情卡）'
      if (selectedReaderEntry?.kind === 'plot') {
        return formatPlotCard(selectedReaderCard)
      }
      return selectedReaderCard?.markdown || '（尚无卷纲）'
    }
    if (readerTab.startsWith('ent:')) {
      const group = readerTab.slice(4)
      const label = ENTITY_TAB_LABELS[group] || '设定卡'
      if (!readerCardList.length) return `（暂无${label}设定卡）`
      return formatEntityCard(selectedReaderCard, label)
    }
    if (readerTab.startsWith('art:')) {
      const key = readerTab.slice(4)
      const approved = approvedArtTabs.find((t) => t.id === `art:${key}`)
      if (approved) return approved.text
      // Not on allowlist — never render arbitrary disk stems as modules.
      return ''
    }
    return ''
  })()

  const onReaderScroll = () => {
    const el = readerRef.current
    if (!el || readerEditing) return
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight
    followDraftBottomRef.current = dist < 180
  }

  // Writer 间歇刷新正文时强制贴底；Apply/离开对照时恢复原位置，禁止甩到文末。
  useLayoutEffect(() => {
    if (readerEditing) return
    const el = readerRef.current
    if (!el) return
    const len = (readerContent || '').length

    if (suppressDraftPinRef.current) {
      suppressDraftPinRef.current = false
      lastDraftLenRef.current = len
      const saved = readerScrollRestoreRef.current
      readerScrollRestoreRef.current = null
      if (saved) {
        const applyRestore = () => {
          const pane = readerRef.current
          if (!pane) return
          const byRatio = Math.round((saved.ratio || 0) * pane.scrollHeight)
          const top = Number.isFinite(saved.top) ? saved.top : byRatio
          pane.scrollTop = Math.max(0, Math.min(top, pane.scrollHeight))
        }
        applyRestore()
        requestAnimationFrame(applyRestore)
      }
      return
    }

    if (readerTab === 'draft') {
      const grew = len > lastDraftLenRef.current
      if (grew && followDraftBottomRef.current) {
        el.scrollTop = el.scrollHeight
      }
      lastDraftLenRef.current = len
      return
    }
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 160
    if (nearBottom) el.scrollTop = el.scrollHeight
  }, [readerContent, readerEditing, readerTab, deskPatches.length])

  // 切章 / 切到正文 tab：重新开启贴底跟随（写作跟滚；勿在 Apply 卸对照时触发）。
  useEffect(() => {
    if (readerTab !== 'draft') return
    followDraftBottomRef.current = true
    lastDraftLenRef.current = 0
    const el = readerRef.current
    if (el) {
      requestAnimationFrame(() => {
        if (followDraftBottomRef.current && readerRef.current && !suppressDraftPinRef.current) {
          readerRef.current.scrollTop = readerRef.current.scrollHeight
        }
      })
    }
  }, [readerTab, selectedChapter])

  // First-class reader modules only. New surfaces require an entry here / in READER_ART_MODULES.
  const readerTabs = [
    {
      id: 'volume',
      label: '本卷',
      title: '当前卷的写作台：进度、剧情与写下一章',
      show: !isShortDrama && Boolean(volumeGroups.length || project),
    },
    {
      id: 'draft',
      label: isShortDrama ? '剧本' : '正文',
      // Always offer the body surface once a project is open (empty state is fine).
      show: Boolean(project),
    },
    {
      id: 'outline',
      label: isShortDrama ? '集纲' : '章纲',
      show: Boolean(project),
    },
    {
      id: 'master',
      label: '总纲',
      show: Boolean(masterOutlineText || project),
    },
    {
      id: 'arcs',
      label: '卷纲',
      title: '卷结构与剧情卡树：看整卷怎么排',
      show: !isShortDrama && Boolean(arcOutlines.length || plots.length),
    },
    ...Object.entries(ENTITY_TAB_LABELS).map(([key, label]) => ({
      id: `ent:${key}`,
      label,
      show: Boolean(entities[key]?.length),
    })),
    { id: 'entity_gaps', label: '设定缺口', show: Boolean(entityGaps.length) },
    { id: 'expected', label: '预期', show: Boolean(expectedEvents.length) },
    ...approvedArtTabs.map(({ id, label, show }) => ({ id, label, show })),
  ].filter((t) => t.show)

  const readerCanEdit = Boolean(
    project
    && readerTab
    && readerTab !== 'entity_gaps'
    && readerTab !== 'expected'
    && readerTab !== 'volume',
  )

  const currentVolumeGroup = (() => {
    if (!volumeGroups.length) return null
    const fromState = volumeHealth?.active_index
    if (fromState) {
      const hit = volumeGroups.find((g) => g.volume === fromState)
      if (hit) return hit
    }
    const activePlot = pickActivePlot(plots)
    if (activePlot) {
      const vol = Number(activePlot.volume_index || activePlot.arc_index || 0)
      const hit = volumeGroups.find((g) => g.volume === vol)
      if (hit) return hit
    }
    return volumeGroups[volumeGroups.length - 1]
  })()

  const setupAwaitingConfirm = preview?.setup_phase === 'awaiting_confirm' || chatSetupGateOpen
  const setupOutlineTab = readerTab === 'master'
    || readerTab === 'arcs'
    || readerTab === 'volume'
    || readerTab === 'art:arc_outline'
    || readerTab === 'art:arc_planner'
  const showSetupConfirmBar = Boolean(project && setupAwaitingConfirm && setupOutlineTab)

  const deskPatchChapter = (() => {
    const fromFocus = Number(deskPatchFocus?.chapter) || 0
    if (fromFocus > 0) return fromFocus
    const last = deskPatches[deskPatches.length - 1]
    return Number(last?.chapter) || 0
  })()
  // Inline −/+ in reader body (not a top split dock). Active when patch targets current tab.
  const activeInlineDiff = (() => {
    if (readerSaveConfirm) {
      return {
        before: readerSaveConfirm.before || '',
        after: readerSaveConfirm.after || '',
        label: '保存前变更',
        source: 'save',
      }
    }
    if (deskPatchHidden || !activeDeskPatch) return null
    const tab = String(activeDeskPatch.readerTab || '').trim()
    const onTargetTab = !tab || readerTab === tab
      || (tab === 'draft' && readerTab === 'draft')
      || (tab === 'outline' && readerTab === 'outline')
    if (!onTargetTab) return null

    if (activeDeskPatch.type === 'inline_doc_diff' || activeDeskPatch.type === 'doc_patch') {
      const disk = readerContent && !/^（尚无|暂无内容|加载/.test(readerContent)
        ? readerContent
        : ''
      const { before, after } = resolveInlineDiffPair(activeDeskPatch, disk)
      if (!before && !after) return null
      return {
        before,
        after,
        label: activeDeskPatch.label || '设定变更',
        source: 'agent',
        chapter: Number(activeDeskPatch.chapter) || selectedChapter || 0,
        readerTab: activeDeskPatch.readerTab || readerTab,
      }
    }
    if (activeDeskPatch.type === 'draft_patch' && readerTab === 'draft') {
      const patches = visibleDeskPatches.filter((p) => p.type === 'draft_patch')
      const base = readerContent && !/加载正文|尚无正文/.test(readerContent) ? readerContent : ''
      if (!base && patches.length === 1) {
        return {
          before: patches[0].before || '',
          after: patches[0].after || '',
          label: patches[0].label || '正文修订',
          source: 'agent',
        }
      }
      const { before, after } = applyParaPatches(base, patches)
      return { before, after, label: '正文修订', source: 'agent' }
    }
    return null
  })()
  const deskInlineDiffActive = Boolean(activeInlineDiff)
  const studioStage = resolveStudioCta(
    deriveStudioStage(preview, {
      nextChapter,
      publishedCount,
    }),
    {
      openApproval: auditState.openApproval,
      latest: auditState.latest,
      patchChapter: deskPatchChapter || undefined,
      reviseAppliedAfterFail: auditState.reviseAppliedAfterFail,
      reviseChapter: auditState.reviseChapter,
    },
  )
  const showNextCta = Boolean(
    studioStage.cta
    && !showSetupConfirmBar
    // Volume workspace already has a primary write button for the same action.
    && !(readerTab === 'volume' && studioStage.cta.id === 'write_next'),
  )

  const prevSetupGateRef = useRef(false)
  useEffect(() => {
    const wasOpen = prevSetupGateRef.current
    prevSetupGateRef.current = chatSetupGateOpen
    if (!project || !chatSetupGateOpen || wasOpen) return
    setReaderTab('master')
  }, [project, chatSetupGateOpen])

  const sendSetupGate = (actionId) => {
    if (chatBusy) return
    sendChatMessage(actionId, { busy: 'ignore' })
  }

  const runStudioCta = () => {
    const cta = studioStage.cta
    if (!cta || chatBusy) return
    // Apply：留在当前对照页（世界观/总纲/…）；仅正文修订才切 draft。
    if (cta.isApply) {
      const docTab = String(activeDeskPatch?.readerTab || '').trim()
      const tab = docTab || cta.readerTab || readerTab
      if (tab) setReaderTab(tab === 'plots' ? 'arcs' : tab)
      let chapter = Number(cta.chapter) || 0
      if (deskPatchChapter > 0) chapter = deskPatchChapter
      if (chapter > 0 && (tab === 'draft' || tab === 'outline')) {
        setSelectedChapter(chapter)
      }
    } else if (cta.readerTab) {
      setReaderTab(cta.readerTab === 'plots' ? 'arcs' : cta.readerTab)
      const chapter = Number(cta.chapter) || 0
      if (chapter > 0) setSelectedChapter(chapter)
    } else {
      const chapter = Number(cta.chapter) || 0
      if (chapter > 0) setSelectedChapter(chapter)
    }
    // Desk CTAs must not interrupt/restart a running turn on double-click.
    if (cta.message) sendChatMessage(cta.message, { busy: 'ignore' })
  }

  // Esc closes status/settings drawer; lock body scroll while open.
  useEffect(() => {
    if (!engineOpen && !statusOpen) return undefined
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const onKey = (e) => {
      if (e.key === 'Escape') {
        setEngineOpen(false)
        setStatusOpen(false)
      }
    }
    window.addEventListener('keydown', onKey)
    return () => {
      document.body.style.overflow = prev
      window.removeEventListener('keydown', onKey)
    }
  }, [engineOpen, statusOpen])

  const beginReaderEdit = () => {
    setReaderEditError('')
    setReaderSaveConfirm(null)
    // Entity/plot display may prepend status meta — edit the on-disk body only.
    let text = readerContent || ''
    if (readerTab.startsWith('ent:') && selectedReaderCard?.markdown) {
      text = selectedReaderCard.markdown
    } else if (isArcNav && selectedReaderEntry?.kind === 'plot' && selectedReaderCard?.markdown) {
      text = selectedReaderCard.markdown
    }
    setReaderEditBaseline(text)
    setReaderEditText(text)
    setReaderEditing(true)
  }

  const cancelReaderEdit = () => {
    setReaderEditing(false)
    setReaderEditText('')
    setReaderEditBaseline('')
    setReaderSaveConfirm(null)
    setReaderEditError('')
  }

  const requestReaderSaveConfirm = () => {
    if (!project || !readerCanEdit) return
    setReaderEditError('')
    const after = String(readerEditText ?? '')
    const before = String(readerEditBaseline ?? '')
    if (after === before) {
      cancelReaderEdit()
      return
    }
    const activeEntry = readerCardList.length
      ? (readerCardList.find((c) => c.key === readerCardKey) || readerCardList[0])
      : null
    const activeCardKey = activeEntry?.key || ''
    const saveTab = activeEntry?.kind === 'plot' ? 'plots' : readerTab
    setReaderSaveConfirm({
      before,
      after,
      diffs: textHunkDiffs(before, after),
      saveTab,
      cardKey: activeCardKey,
      chapter: selectedChapter || 0,
    })
  }

  const discardReaderSaveConfirm = () => {
    // 取消变更：退回基线，不落盘
    setReaderEditText(readerEditBaseline)
    setReaderSaveConfirm(null)
    setReaderEditError('')
  }

  const putReaderTabContent = useCallback(async (content, opts = {}) => {
    if (!project) return { error: '无项目' }
    const tab = opts.tab || readerTab
    const activeEntry = readerCardList.length
      ? (readerCardList.find((c) => c.key === readerCardKey) || readerCardList[0])
      : null
    const saveTab = activeEntry?.kind === 'plot' ? 'plots' : tab
    const cardKey = opts.cardKey || activeEntry?.key || ''
    const chapter = opts.chapter || selectedChapter || 0
    const res = await api(`/projects/${encodeURIComponent(project)}/content`, {
      method: 'PUT',
      body: JSON.stringify({
        tab: saveTab,
        content,
        chapter,
        card_key: cardKey || '',
      }),
    })
    if (res?.error) return res
    if (res?.preview) setPreview(res.preview)
    else await fetchNovelPreview(project)
    if (saveTab === 'draft' || saveTab === 'outline') {
      setChapterCache((prev) => {
        const next = { ...prev }
        delete next[selectedChapter]
        return next
      })
    }
    return res
  }, [
    project,
    readerTab,
    readerCardList,
    readerCardKey,
    selectedChapter,
    fetchNovelPreview,
  ])

  const clearDeskDiff = useCallback(() => {
    if (deskPatchesRef.current.length) captureReaderScrollForDiffExit()
    setDeskPatches([])
    setDeskPatchFocus(null)
    setDeskPatchHidden(false)
    setDeskDiffSel(null)
    deskPatchesRef.current = []
  }, [captureReaderScrollForDiffExit])

  const applyReaderSaveConfirm = async () => {
    if (!project || !readerSaveConfirm) return
    setReaderSaving(true)
    setReaderEditError('')
    const { saveTab, after, cardKey, chapter } = readerSaveConfirm
    const res = await putReaderTabContent(after, { tab: saveTab, cardKey, chapter })
    setReaderSaving(false)
    if (res?.error) {
      setReaderEditError(typeof res.error === 'string' ? res.error : '保存失败')
      return
    }
    setReaderEditing(false)
    setReaderEditText('')
    setReaderEditBaseline('')
    setReaderSaveConfirm(null)
  }

  /**
   * Desk already wrote (or accepted all hunks). Close mutation gate without
   * claiming「放弃 / 磁盘未变更」— that was the multi-hunk Apply bug.
   */
  const closeMutationGateAfterDeskWrite = () => {
    if (studioStage.cta?.isApply || studioStage.cta?.id === 'cm_apply') {
      sendChatMessage('cm_desk_applied', { busy: 'ignore' })
    }
  }

  /** True discard: user cancelled every hunk / 取消变更. */
  const discardMutationGateIfOpen = () => {
    if (studioStage.cta?.isApply || studioStage.cta?.id === 'cm_apply') {
      sendChatMessage('cm_discard', { busy: 'ignore' })
    }
  }

  /** 全部应用：有确认门控时走确定性 cm_apply；否则直接写盘，绝不裸发 cm_apply 进 LLM。 */
  const applyDeskDiffAll = () => {
    if (!activeInlineDiff) return
    if (activeInlineDiff.source === 'save') {
      // 保存预览若已 Cancel 部分块，写 remaining；否则写全文 after。
      if (
        deskDiffSel
        && deskDiffSel.selectedCount > 0
        && deskDiffSel.selectedCount < deskDiffSel.totalCount
      ) {
        applyDeskDiffSelected()
        return
      }
      applyReaderSaveConfirm()
      return
    }
    if (
      studioStage.cta?.isApply
      && (!deskDiffSel
        || deskDiffSel.selectedCount === deskDiffSel.totalCount)
    ) {
      runStudioCta()
      return
    }
    const body = deskDiffSel?.fullAfter || activeInlineDiff.after
    if (!body) return
    setReaderSaving(true)
    putReaderTabContent(body, {
      tab: activeInlineDiff.readerTab || readerTab,
      chapter: activeInlineDiff.chapter || selectedChapter || 0,
    }).then((res) => {
      setReaderSaving(false)
      if (res?.error) {
        setReaderEditError(typeof res.error === 'string' ? res.error : '写入失败')
        return
      }
      clearDeskDiff()
      closeMutationGateAfterDeskWrite()
    })
  }

  /** 应用勾选：只合并选中变更块；Agent 预览随后关闭门控（已手写落盘）。 */
  const applyDeskDiffSelected = async () => {
    if (!activeInlineDiff || !deskDiffSel) return
    if (!deskDiffSel.selectedCount) {
      setReaderEditError('请先选择要应用的变更块')
      return
    }
    // 全选时与「全部应用」同路径（保留 mutation 审计/版本节点）。
    if (
      deskDiffSel.selectedCount === deskDiffSel.totalCount
      && activeInlineDiff.source === 'agent'
      && studioStage.cta?.isApply
    ) {
      runStudioCta()
      return
    }
    const body = deskDiffSel.fullAfter ?? deskDiffSel.partialAfter
    if (body == null) return
    setReaderSaving(true)
    setReaderEditError('')
    if (activeInlineDiff.source === 'save') {
      const { saveTab, cardKey, chapter } = readerSaveConfirm || {}
      const res = await putReaderTabContent(body, { tab: saveTab, cardKey, chapter })
      setReaderSaving(false)
      if (res?.error) {
        setReaderEditError(typeof res.error === 'string' ? res.error : '写入失败')
        return
      }
      setReaderEditing(false)
      setReaderEditText('')
      setReaderEditBaseline('')
      setReaderSaveConfirm(null)
      return
    }
    const res = await putReaderTabContent(body, {
      tab: activeInlineDiff.readerTab || readerTab,
      chapter: activeInlineDiff.chapter || selectedChapter || 0,
    })
    setReaderSaving(false)
    if (res?.error) {
      setReaderEditError(typeof res.error === 'string' ? res.error : '写入失败')
      return
    }
    clearDeskDiff()
    closeMutationGateAfterDeskWrite()
  }

  /**
   * 单块流程结束（InlineDiffView 仅在无剩余块时回调）：
   * - 全部 Apply → cm_apply
   * - 部分 Apply / Cancel 混合 → PUT 合并结果 + cm_desk_applied
   */
  const applyDeskDiffHunk = async (_hunkId, { after, remaining, allAccepted } = {}) => {
    if (!activeInlineDiff || after == null) return
    if (Array.isArray(remaining) && remaining.length > 0) return
    if (
      activeInlineDiff.source === 'agent'
      && allAccepted
      && studioStage.cta?.isApply
    ) {
      runStudioCta()
      return
    }
    setReaderSaving(true)
    setReaderEditError('')
    if (activeInlineDiff.source === 'save') {
      const { saveTab, cardKey, chapter } = readerSaveConfirm || {}
      const res = await putReaderTabContent(after, { tab: saveTab, cardKey, chapter })
      setReaderSaving(false)
      if (res?.error) {
        setReaderEditError(typeof res.error === 'string' ? res.error : '写入失败')
        return
      }
      setReaderEditing(false)
      setReaderEditText('')
      setReaderEditBaseline('')
      setReaderSaveConfirm(null)
      return
    }
    const res = await putReaderTabContent(after, {
      tab: activeInlineDiff.readerTab || readerTab,
      chapter: activeInlineDiff.chapter || selectedChapter || 0,
    })
    setReaderSaving(false)
    if (res?.error) {
      setReaderEditError(typeof res.error === 'string' ? res.error : '写入失败')
      return
    }
    clearDeskDiff()
    closeMutationGateAfterDeskWrite()
  }

  /** 单块 Cancel：全部取消才丢弃预览；否则仅本地剔除该块。 */
  const cancelDeskDiffHunk = (_hunkId, { allCancelled } = {}) => {
    if (!allCancelled) return
    if (activeInlineDiff?.source === 'save') {
      discardReaderSaveConfirm()
      return
    }
    clearDeskDiff()
    discardMutationGateIfOpen()
  }

  const switchReaderTab = (id) => {
    if ((readerEditing || readerSaveConfirm) && id !== readerTab) {
      if (!window.confirm('正在编辑，切换将丢弃未保存修改，继续？')) return
      cancelReaderEdit()
    }
    // 剧情卡已挂在卷纲下，旧链接统一落到卷纲树。
    setReaderTab(id === 'plots' ? 'arcs' : id)
    setReaderCardKey('')
  }

  const switchReaderVolume = (vol) => {
    if (vol === readerVolume) return
    if (readerEditing || readerSaveConfirm) {
      if (!window.confirm('正在编辑，切换卷将丢弃未保存修改，继续？')) return
      cancelReaderEdit()
    }
    setReaderVolume(vol)
    setReaderCardKey(`v${vol}`)
  }

  const switchReaderCard = (key) => {
    if (key === readerCardKey) return
    if (readerEditing) {
      if (!window.confirm('正在编辑，切换卡片将丢弃未保存修改，继续？')) return
      cancelReaderEdit()
    }
    setReaderCardKey(key)
  }

  const requestDeleteChapter = useCallback((chapter) => {
    const proj = currentProjectRef.current || project
    if (!proj || !chapter) return
    const mode = preview?.project_mode || 'longform'
    const unit = mode === 'short_drama' ? '集' : '章'
    const title = preview?.chapters?.find((c) => c.number === chapter)?.title
    const label = formatUnitTitle(chapter, title, mode)
    setDangerConfirm({
      kind: 'chapter',
      project: proj,
      chapter,
      unit,
      title: label,
      dialogTitle: `删除${label}？`,
      confirmText: String(chapter),
      message: `${unit}目录将永久删除，不可恢复。请输入章节号 ${chapter} 以二次确认。`,
    })
  }, [project, preview])

  const performDeleteChapter = useCallback(async (proj, chapter) => {
    if (!proj || !chapter) return false
    if (readerEditing) {
      setReaderEditing(false)
      setReaderEditText('')
      setReaderEditError('')
    }
    const data = await api(
      `/projects/${encodeURIComponent(proj)}/chapters/${chapter}`,
      { method: 'DELETE' },
    )
    if (data.error) {
      window.alert(data.error)
      return false
    }
    if (data.preview) {
      setPreview(data.preview)
    }
    setChapterCache((prev) => {
      const next = { ...prev }
      delete next[chapter]
      return next
    })
    const remaining = Array.isArray(data.remaining)
      ? data.remaining
      : (data.preview?.chapters || []).map((c) => c.number)
    if (selectedChapter === chapter) {
      const lower = remaining.filter((n) => n < chapter)
      const higher = remaining.filter((n) => n > chapter)
      const next = lower.length
        ? lower[lower.length - 1]
        : higher.length
          ? higher[0]
          : 0
      setSelectedChapter(next || 1)
    }
    const lib = await api(`/library/${encodeURIComponent(proj)}`)
    if (!lib.error) setNovelRecord(lib.novel)
    refreshLibrary()
    return true
  }, [readerEditing, refreshLibrary, selectedChapter])

  const runDangerConfirm = useCallback(async () => {
    if (!dangerConfirm || dangerBusy) return
    setDangerBusy(true)
    try {
      let ok = false
      if (dangerConfirm.kind === 'novel') {
        ok = await performDeleteNovel(dangerConfirm.name)
      } else if (dangerConfirm.kind === 'chapter') {
        ok = await performDeleteChapter(dangerConfirm.project, dangerConfirm.chapter)
      }
      if (ok) setDangerConfirm(null)
    } finally {
      setDangerBusy(false)
    }
  }, [dangerConfirm, dangerBusy, performDeleteNovel, performDeleteChapter])

  return (
    <div className="studio">
      <header className="studio-header">
        <div className="studio-header-left">
          <h1>NovelX</h1>
          <div className="header-novel" role="group" aria-label="作品">
            <select
              className="header-novel-select"
              value={project || ''}
              onChange={(e) => {
                const id = e.target.value
                if (id) loadNovel(id)
              }}
              aria-label="切换小说"
              disabled={newNovelBusy}
            >
              {!project ? (
                <option value="" disabled>
                  {library.length ? '选择小说…' : '暂无小说'}
                </option>
              ) : null}
              {project && !library.some((n) => n.id === project) ? (
                <option value={project}>
                  {novelRecord?.display_title || project}
                </option>
              ) : null}
              {library.map((n) => (
                <option key={n.id} value={n.id}>
                  {n.display_title || n.id}
                </option>
              ))}
            </select>
            <button
              type="button"
              className={`header-icon-btn header-icon-btn--primary${showNewNovelForm ? ' is-active' : ''}`}
              onClick={() => setShowNewNovelForm((v) => !v)}
              disabled={newNovelBusy}
              title={showNewNovelForm ? '收起新建' : '新建小说'}
              aria-label={showNewNovelForm ? '收起新建' : '新建小说'}
              aria-expanded={showNewNovelForm}
            >
              <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
                <path
                  fill="currentColor"
                  d="M8 1.5a.5.5 0 0 1 .5.5v5.5H14a.5.5 0 0 1 0 1H8.5V14a.5.5 0 0 1-1 0V8.5H2a.5.5 0 0 1 0-1h5.5V2a.5.5 0 0 1 .5-.5z"
                />
              </svg>
            </button>
            {project ? (
              <button
                type="button"
                className="header-icon-btn"
                title="删除当前小说"
                aria-label="删除当前小说"
                onClick={(e) => requestDeleteNovel(
                  project,
                  novelRecord?.display_title || project,
                  e,
                )}
              >
                <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
                  <path
                    fill="currentColor"
                    d="M5.5 2a.5.5 0 0 1 .5-.5h4a.5.5 0 0 1 .5.5V3h3a.5.5 0 0 1 0 1h-.55l-.7 9.1A1.5 1.5 0 0 1 10.76 14H5.24a1.5 1.5 0 0 1-1.49-1.4L3.05 4H2.5a.5.5 0 0 1 0-1h3V2zm1 .5V3h3v-.5h-3zM4.06 4l.68 8.9a.5.5 0 0 0 .5.45h5.52a.5.5 0 0 0 .5-.45L11.94 4H4.06z"
                  />
                </svg>
              </button>
            ) : null}
          </div>
        </div>
        {project && preview ? (
          <nav className="stage-strip" aria-label="下一步推荐">
            {studioStage.detail ? (
              <span className="stage-detail" title={studioStage.detail}>
                {studioStage.detail}
              </span>
            ) : null}
            {showNextCta ? (
              <button
                type="button"
                className="btn-primary btn-inline stage-next-btn"
                onClick={runStudioCta}
                disabled={chatBusy}
                aria-busy={chatBusy}
                title={
                  chatBusy
                    ? '助手进行中，完成后可再点'
                    : (studioStage.cta.hint || studioStage.cta.label)
                }
              >
                {chatBusy ? '进行中…' : studioStage.cta.label}
              </button>
            ) : null}
          </nav>
        ) : (
          <nav className="stage-strip" aria-label="下一步推荐">
            <span className="stage-detail">选择或新建一本小说开始</span>
          </nav>
        )}
      </header>

      <NewNovelModal
        open={showNewNovelForm}
        title={newNovelTitle}
        genre={newNovelGenre}
        brief={newNovelBrief}
        mode={newNovelMode}
        busy={newNovelBusy}
        onTitleChange={setNewNovelTitle}
        onGenreChange={setNewNovelGenre}
        onBriefChange={setNewNovelBrief}
        onModeChange={setNewNovelMode}
        onCancel={() => {
          if (!newNovelBusy) setShowNewNovelForm(false)
        }}
        onSubmit={submitNewNovel}
      />

      <div className="studio-grid">
        <div className="work-area">
          {openSubAgent ? (
            <SubAgentPage
              agent={openSubAgent}
              agents={subAgents}
              onClose={handleCloseSubAgent}
              onSelectAgent={handleOpenSubAgent}
              onAgentUpdate={handleSubAgentUpdate}
            />
          ) : null}
          <section className={`panel reader-panel${openSubAgent ? ' is-parked' : ''}`}>
          <div className="reader-head">
            <h2>写作台</h2>
            {chapterList.length > 0 ? (
              <ChapterStrip
                chapterList={chapterList}
                selectedChapter={selectedChapter}
                nextChapter={nextChapter}
                projectMode={projectMode}
                unitLabel={unitLabel}
                onSelect={(ch) => {
                  if (readerEditing) {
                    if (!window.confirm('正在编辑，切换章节将丢弃未保存修改，继续？')) return
                    cancelReaderEdit()
                  }
                  setSelectedChapter(ch.number)
                  setReaderTab(ch.hasDraft || ch.status === 'published' ? 'draft' : 'master')
                }}
              />
            ) : null}
          </div>

          {!hasReaderMaterial ? (
            <div className="empty small">建好作品后，大纲、设定和章节会出现在这里</div>
          ) : (
            <>
              <div className="reader-meta">
                <div className="reader-meta-row">
                  {chapterList.length > 0 && selectedChapter > 0 ? (
                    <span className="reader-ch-title">
                      <span className="reader-unit-name">{readerUnitTitle}</span>
                      {activePlotSummary ? (
                        <button
                          type="button"
                          className={`reader-plot-inline${isShortDrama ? ' is-static' : ''}`}
                          title={activePlotSummary.body}
                          onClick={() => {
                            if (isShortDrama) return
                            setReaderTab('arcs')
                            const key = plotCardKey(pickActivePlot(plots))
                            if (key) setReaderCardKey(key)
                          }}
                        >
                          <span className="reader-plot-key">当前剧情</span>
                          <span className="reader-plot-sep">：</span>
                          <span className="reader-plot-value">{activePlotSummary.title}</span>
                          <span className="desk-plot-status">{activePlotSummary.statusLabel}</span>
                        </button>
                      ) : null}
                      {selectedChapterStatus ? (
                        <span
                          className={[
                            'ch-status-badge',
                            `ch-status-${selectedChapterStatus.status}`,
                            readerTab === 'draft' ? `word-tone-${wordTone}` : '',
                          ].filter(Boolean).join(' ')}
                          title={
                            readerTab === 'draft'
                              ? `目标 ${wordBand.min}–${wordBand.max} 字 · 发布至少 ${wordBand.hardMin} 字`
                              : undefined
                          }
                        >
                          {selectedChapterStatus.statusLabel}
                          {unitWordChars > 0 ? ` · ${unitWordChars}字` : ''}
                          {readerTab === 'draft' ? (
                            <>
                              <span className="ch-word-sep"> · </span>
                              <span className="ch-word-target">
                                {wordBand.min}–{wordBand.max}
                              </span>
                              {wordTone !== 'ok' && wordTone !== 'empty' ? (
                                <span className="ch-word-hint"> · {wordLabel}</span>
                              ) : null}
                            </>
                          ) : null}
                        </span>
                      ) : null}
                    </span>
                  ) : (
                    <span className="reader-ch-title reader-ch-title-muted">写作台</span>
                  )}
                  <div className="reader-head-actions">
                    {readerCanEdit && !readerEditing && (
                      <button
                        type="button"
                        className="btn-ghost btn-inline reader-edit-btn"
                        onClick={beginReaderEdit}
                        title="编辑当前内容"
                      >
                        编辑
                      </button>
                    )}
                    {chapterList.length > 0 && selectedChapter > 0 && !readerEditing && (
                      <button
                        type="button"
                        className="btn-ghost btn-inline chapter-delete-btn"
                        onClick={() => requestDeleteChapter(selectedChapter)}
                        title={`删除当前${unitLabel}`}
                      >
                        {`删除本${unitLabel}`}
                      </button>
                    )}
                    {readerEditing && !readerSaveConfirm && (
                      <div className="reader-edit-actions">
                        <button
                          type="button"
                          className="btn-primary btn-inline"
                          disabled={readerSaving}
                          onClick={requestReaderSaveConfirm}
                        >
                          保存
                        </button>
                        <button
                          type="button"
                          className="btn-ghost btn-inline"
                          disabled={readerSaving}
                          onClick={cancelReaderEdit}
                        >
                          取消编辑
                        </button>
                      </div>
                    )}
                    {readerSaveConfirm && (
                      <div className="reader-edit-actions">
                        <button
                          type="button"
                          className="btn-primary btn-inline"
                          disabled={readerSaving}
                          onClick={applyReaderSaveConfirm}
                          title="将对照中的变更写入磁盘"
                        >
                          {readerSaving ? '写入中…' : '应用修改'}
                        </button>
                        <button
                          type="button"
                          className="btn-ghost btn-inline"
                          disabled={readerSaving}
                          onClick={discardReaderSaveConfirm}
                          title="丢弃本次预览，退回编辑前内容"
                        >
                          取消变更
                        </button>
                      </div>
                    )}
                  </div>
                </div>
                <div className="reader-tabs" role="tablist" aria-label="写作台分类">
                  {readerTabs.map((t) => (
                    <button
                      key={t.id}
                      type="button"
                      className={readerTab === t.id ? 'active' : ''}
                      title={t.title || t.label}
                      onClick={() => switchReaderTab(t.id)}
                    >
                      {t.label}
                    </button>
                  ))}
                </div>
              </div>
              {readerEditError ? (
                <div className="reader-edit-error">{readerEditError}</div>
              ) : null}
              {deskInlineDiffActive ? (
                <div className="desk-inline-diff-bar" role="region" aria-label="内联变更对照">
                  <strong>
                    内联对照 · {activeInlineDiff.label || '变更'}
                    {deskDiffSel?.totalCount
                      ? ` · ${deskDiffSel.selectedCount}/${deskDiffSel.totalCount} 块`
                      : ''}
                  </strong>
                  <div className="desk-patch-dock-actions">
                    <button
                      type="button"
                      className="btn-primary btn-inline"
                      disabled={readerSaving || chatBusy || !deskDiffSel?.selectedCount}
                      onClick={applyDeskDiffAll}
                      title="应用全部剩余变更"
                    >
                      {readerSaving || chatBusy ? '进行中…' : '全部应用'}
                    </button>
                    {activeInlineDiff.source === 'save' ? (
                      <button
                        type="button"
                        className="btn-ghost btn-inline"
                        disabled={readerSaving}
                        onClick={discardReaderSaveConfirm}
                      >
                        取消变更
                      </button>
                    ) : (
                      <button
                        type="button"
                        className="btn-ghost btn-inline"
                        onClick={() => setDeskPatchHidden(true)}
                      >
                        收起对照
                      </button>
                    )}
                  </div>
                </div>
              ) : null}
              {showSetupConfirmBar && (
                <div className="setup-confirm-bar" role="region" aria-label="定稿确认">
                  <div className="setup-confirm-copy">
                    <strong>总纲与卷纲已就绪</strong>
                    <span>
                      {chatBusy
                        ? '助手进行中，请稍候再确认'
                        : '确认后进入写章；或打回后重新生成大纲。'}
                    </span>
                  </div>
                  <div className="setup-confirm-actions">
                    <button
                      type="button"
                      className="btn-primary btn-inline"
                      onClick={() => sendSetupGate('sc_approve')}
                      disabled={chatBusy}
                      aria-busy={chatBusy}
                    >
                      {chatBusy ? '进行中…' : '确认定稿'}
                    </button>
                    <button
                      type="button"
                      className="btn-ghost btn-inline"
                      onClick={() => sendSetupGate('sc_revise')}
                      disabled={chatBusy}
                    >
                      修改再生成
                    </button>
                  </div>
                </div>
              )}
              {isArcNav && volumeGroups.length > 0 && (
                <nav className="reader-arc-nav" aria-label="卷纲与剧情">
                  <div className="reader-volume-strip" role="tablist" aria-label="分卷">
                    {volumeGroups.map((g) => {
                      const active = (activeVolumeGroup?.volume || readerVolume) === g.volume
                      return (
                        <button
                          key={`vol-${g.volume}`}
                          type="button"
                          role="tab"
                          aria-selected={active}
                          className={active ? 'active' : ''}
                          onClick={() => switchReaderVolume(g.volume)}
                          title={g.title}
                        >
                          {g.shortLabel}
                          {g.plots.length > 0 ? (
                            <span className="reader-vol-count">{g.plots.length}</span>
                          ) : null}
                        </button>
                      )
                    })}
                  </div>
                  <div className="reader-card-strip reader-card-strip-arc" role="tablist" aria-label="本卷内容">
                    {readerCardList.map((c) => {
                      const active = (readerCardKey || readerCardList[0]?.key) === c.key
                      return (
                        <button
                          key={`${c.kind}:${c.key}`}
                          type="button"
                          role="tab"
                          aria-selected={active}
                          className={[
                            active ? 'active' : '',
                            c.kind === 'arc' ? 'arc-head' : 'plot-item',
                          ].filter(Boolean).join(' ')}
                          onClick={() => switchReaderCard(c.key)}
                          title={c.complete ? c.label : `${c.label}（待补全）`}
                        >
                          {c.label}
                          {!c.complete && c.kind === 'plot'
                            ? <span className="reader-card-gap">·</span>
                            : null}
                        </button>
                      )
                    })}
                  </div>
                  {activeVolumeGroup ? (
                    <div className="reader-arc-context">
                      {activeVolumeGroup.title}
                      {selectedReaderEntry?.kind === 'plot'
                        ? ` · ${selectedReaderEntry.label}`
                        : ' · 卷纲'}
                    </div>
                  ) : null}
                </nav>
              )}
              {!isArcNav && readerCardList.length > 0 && (
                <div className="reader-card-strip" role="tablist" aria-label="卡片列表">
                  {readerCardList.map((c) => {
                    const active = (readerCardKey || readerCardList[0]?.key) === c.key
                    const inactive = c.status === 'exited' || c.status === 'consumed'
                    const titleParts = [c.label]
                    if (c.statusLabel) titleParts.push(c.statusLabel)
                    if (!c.complete) titleParts.push('待补全')
                    return (
                      <button
                        key={`${c.kind || 'card'}:${c.key}`}
                        type="button"
                        role="tab"
                        aria-selected={active}
                        className={[
                          active ? 'active' : '',
                          inactive ? 'entity-inactive' : '',
                        ].filter(Boolean).join(' ')}
                        onClick={() => switchReaderCard(c.key)}
                        title={titleParts.join(' · ')}
                      >
                        {c.label}
                        {c.statusLabel && c.status !== 'active' ? (
                          <span className="reader-card-status">{c.statusLabel}</span>
                        ) : null}
                        {!c.complete ? <span className="reader-card-gap">·</span> : null}
                      </button>
                    )
                  })}
                </div>
              )}
              <div
                className={`reader-body${readerEditing ? ' reader-body-editing' : ''}${
                  deskInlineDiffActive ? ' with-inline-diff' : ''
                }${isVolumeWorkspace && !deskInlineDiffActive ? ' reader-body-workspace' : ''}${
                  readerTab === 'draft' ? ' is-draft-prose' : ''
                }`}
                ref={readerRef}
                onScroll={onReaderScroll}
              >
                {deskInlineDiffActive ? (
                  <InlineDiffView
                    before={activeInlineDiff.before}
                    after={activeInlineDiff.after}
                    label={activeInlineDiff.label}
                    busy={readerSaving || chatBusy}
                    onSelectionChange={handleInlineDiffSelection}
                    onHunkApply={applyDeskDiffHunk}
                    onHunkCancel={cancelDeskDiffHunk}
                  />
                ) : isVolumeWorkspace ? (
                  <VolumeWorkspace
                    group={currentVolumeGroup}
                    nextChapter={nextChapter}
                    publishedCount={publishedCount}
                    onOpenArc={() => {
                      setReaderTab('arcs')
                      if (currentVolumeGroup?.volume) {
                        setReaderVolume(currentVolumeGroup.volume)
                        setReaderCardKey(`v${currentVolumeGroup.volume}`)
                      }
                    }}
                    onOpenPlot={(p) => {
                      setReaderTab('arcs')
                      const vol = Number(p?.volume_index || p?.arc_index || currentVolumeGroup?.volume)
                      if (vol) setReaderVolume(vol)
                      const key = plotCardKey(p)
                      if (key) setReaderCardKey(key)
                    }}
                    writeBusy={chatBusy}
                    onWriteNext={() => {
                      if (chatBusy) return
                      if (!sendChatMessage(`写第${nextChapter}章`, { busy: 'ignore' })) return
                      setReaderTab('draft')
                      if (nextChapter > 0) setSelectedChapter(nextChapter)
                    }}
                    onOpenChapter={(ch, tab) => {
                      setSelectedChapter(ch)
                      setReaderTab(tab || 'draft')
                    }}
                  />
                ) : readerEditing ? (
                  <LinedProseEditor
                    value={readerEditText}
                    onChange={(e) => setReaderEditText(e.target.value)}
                    aria-label="编辑写作台内容"
                  />
                ) : (
                  <LinedProseView
                    text={readerContent || '暂无内容'}
                    className={readerTab === 'draft' ? 'is-draft' : ''}
                  />
                )}
              </div>
            </>
          )}
        </section>

        <div className={`agent-chat-host${openSubAgent ? ' is-parked' : ''}`}>
          <NovelXChat
            project={project}
            displayTitle={novelRecord?.display_title || project}
            sendRef={chatSendRef}
            onSetupGateChange={setChatSetupGateOpen}
            onBusyChange={handleChatBusyChange}
            onPreviewRefresh={handleChatPreviewRefresh}
            onProjectBound={handleProjectBound}
            onDraftPatchesChange={handleDraftPatchesChange}
            onAuditStateChange={handleAuditStateChange}
            onSubAgentsChange={handleSubAgentsChange}
            onOpenSubAgent={handleOpenSubAgent}
            openSubAgentId={openSubAgent?.threadId || ''}
            compactDraftPatches={deskInlineDiffActive}
            statusOpen={statusOpen}
            onOpenStatus={() => {
              setEngineOpen(false)
              setStatusOpen(true)
            }}
            engineOpen={engineOpen}
            onOpenEngine={() => {
              setStatusOpen(false)
              setEngineOpen(true)
            }}
          />
        </div>
        </div>
      </div>

      <CreationStatusBar
        visible={Boolean(project && (isShortDrama || longformHealth))}
        isShortDrama={isShortDrama}
        overallLevel={overallHealthLevel}
        foreshadowDebt={foreshadowDebt}
        foreshadowLevel={foreshadowLevel}
        volumeHealth={volumeHealth}
        volumeLevel={volumeLevel}
        lengthLevel={lengthLevel}
        lengthChip={lengthChip}
        progressLabel={
          novelRecord
            ? `已写 ${novelRecord.published_count || 0} ${isShortDrama ? '集' : '章'}`
            : ''
        }
        nextEpisodeLabel={isShortDrama ? `第 ${nextChapter || 1} 集` : ''}
        onOpenDetail={() => {
          setEngineOpen(false)
          setStatusOpen(true)
        }}
      />

      {statusOpen ? (
        <div className="engine-drawer-root" role="dialog" aria-label="创作状态">
          <button
            type="button"
            className="engine-drawer-backdrop"
            aria-label="关闭创作状态"
            onClick={() => setStatusOpen(false)}
          />
          <div className="engine-drawer">
            <div className="drawer-chrome">
              <h2 className="drawer-chrome-title">创作状态</h2>
              <button
                type="button"
                className="btn-ghost btn-inline"
                onClick={() => setStatusOpen(false)}
              >
                关闭
              </button>
            </div>
            <CreationStatusPage
              showHeading={false}
              project={project}
              isShortDrama={isShortDrama}
              foreshadowDebt={foreshadowDebt}
              foreshadowLevel={foreshadowLevel}
              foreshadowPhase={foreshadowPhase}
              foreshadowDebtExpanded={foreshadowDebtExpanded}
              onToggleForeshadowExpanded={() => setForeshadowDebtExpanded((v) => !v)}
              volumeHealth={volumeHealth}
              volumeLevel={volumeLevel}
              volumeQaPhase={volumeQaPhase}
              lengthHealth={lengthHealth}
              lengthLevel={lengthLevel}
              lengthChip={lengthChip}
              longformTier={longformTier}
              longformLevel={longformLevel}
              costTop={costTop}
              publishedCount={publishedCount}
              nextChapter={nextChapter}
              onOpenEngine={() => {
                setStatusOpen(false)
                setEngineOpen(true)
              }}
              onAuditChapter={(ch) => {
                setStatusOpen(false)
                setSelectedChapter(ch)
                setReaderTab('draft')
                const unit = isShortDrama ? '集' : '章'
                sendChatMessage(`审校第${ch}${unit}`, { busy: 'ignore' })
              }}
            />
          </div>
        </div>
      ) : null}

      {engineOpen ? (
        <div className="engine-drawer-root" role="dialog" aria-label="设置">
          <button
            type="button"
            className="engine-drawer-backdrop"
            aria-label="关闭设置"
            onClick={() => setEngineOpen(false)}
          />
          <div className="engine-drawer">
            <ConfigPanel onClose={() => setEngineOpen(false)} title="设置" />
          </div>
        </div>
      ) : null}

      <DangerConfirmModal
        open={Boolean(dangerConfirm)}
        title={dangerConfirm?.dialogTitle || '确认删除'}
        message={dangerConfirm?.message || ''}
        confirmText={dangerConfirm?.confirmText || ''}
        confirmLabel="确认删除"
        busy={dangerBusy}
        onCancel={() => {
          if (!dangerBusy) setDangerConfirm(null)
        }}
        onConfirm={runDangerConfirm}
      />
    </div>
  )
}
