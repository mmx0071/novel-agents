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
import MarkdownView from './components/MarkdownView'
import ConfigPanel from './components/ConfigPanel'
import VolumeWorkspace from './components/VolumeWorkspace'
import WritingPrefs from './components/WritingPrefs'
import { buildVolumePlotGroups, sortPlotsByProgress } from './plotSort'
import {
  STAGE_ORDER,
  buildCreateNovelMessage,
  deriveStudioStage,
  resolveStudioCta,
} from './studioPhase'
import {
  CHAPTER_WORD_HARD_MIN,
  CHAPTER_WORD_MAX,
  CHAPTER_WORD_MIN,
  wordProgressLabel,
  wordProgressTone,
} from './chapterTargets'
import { pickActivePlot, summarizePlotForDesk } from './plotSummary'

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

const ARTIFACT_LABELS = {
  // Prefer `bible` when both exist (see artifactTabs filter).
  world_architect: '世界观',
  bible: '世界观',
  // master_outline / master_planner → single reader tab「总纲」
  // 卷纲：preview.arc_outlines → 单 Tab「卷纲」；其下挂靠同卷剧情卡
}

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

function formatExpectedConditions(c) {
  if (!c || typeof c !== 'object') return '无硬条件（随时可检阅）'
  const parts = []
  if (c.min_chapter) parts.push(`≥第${c.min_chapter}章`)
  if (c.max_chapter) parts.push(`≤第${c.max_chapter}章`)
  if (c.volume) parts.push(`第${c.volume}卷`)
  if (c.require_plot_id) {
    parts.push(`剧情卡${c.require_plot_id}${c.require_plot_status ? `:${c.require_plot_status}` : ''}`)
  } else if (c.require_plot_status) {
    parts.push(`剧情status=${c.require_plot_status}`)
  }
  if (c.require_entity) {
    parts.push(`实体${c.require_entity}${c.require_entity_status ? `:${c.require_entity_status}` : ''}`)
  }
  if (c.after_event_id) parts.push(`依赖${c.after_event_id}`)
  if (c.freeform) parts.push(String(c.freeform).slice(0, 80))
  return parts.length ? parts.join(' · ') : '无硬条件（随时可检阅）'
}

function formatExpectedEvent(e) {
  if (!e) return '（暂无预处理预期）'
  const kind = EXPECTED_KIND_LABELS[e.kind] || e.kind || '其他'
  const status = EXPECTED_STATUS_LABELS[e.status] || e.status || ''
  const elig = e.eligibility?.label || ''
  const fit = e.last_review?.agent_fit || '—'
  const reason = e.last_review?.reason || '—'
  const suggestion = e.last_review?.suggestion || ''
  return (
    `# ${String(e.text || '未命名预期').slice(0, 80)}\n\n`
    + `- **id**：\`${e.id || ''}\`\n`
    + `- **类型**：${kind}\n`
    + `- **状态**：${status}${elig ? `（${elig}）` : ''}\n`
    + `- **来源**：${e.source === 'reader' ? '读者' : '作者'}\n`
    + (e.entity_ref ? `- **实体**：${e.entity_ref}\n` : '')
    + `- **触发条件**：${formatExpectedConditions(e.conditions)}\n`
    + `- **最近检阅**：拟合 ${fit} · ${reason}\n`
    + (suggestion ? `- **建议做法**：${suggestion}\n` : '')
    + (e.notes ? `\n## 备注\n\n${e.notes}\n` : '')
    + '\n> 只读展示。纳入或跳过请在右侧创作助手中选择；确认后再由设定/剧情工具落地。\n'
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
  // 优先完整 markdown（与磁盘卡一致）；无则拼结构化预览
  if (p.markdown?.trim()) {
    return `${p.markdown.trim()}${gaps}${mark ? `\n${mark}` : ''}`
  }
  return (
    `# ${p.title || '未命名'}\n`
    + `${p.anchor || `${scope} · ${p.arc || ''} · ${p.chapter_range || ''}`}\n`
    + `类型：${p.plot_type || ''} · 状态：${p.status || ''}\n\n`
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
  /** `library` = 书库；`prefs` = 写作偏好；引擎室用 engineOpen 抽屉 */
  const [appMode, setAppMode] = useState('library')
  const [engineOpen, setEngineOpen] = useState(false)
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
  const [plotRailOpen, setPlotRailOpen] = useState(false)
  const [auditState, setAuditState] = useState({
    todos: [],
    reports: [],
    latest: null,
    openApproval: null,
  })
  const [chatBusy, setChatBusy] = useState(false)
  const chatSendRef = useRef(null)
  const workspacesRef = useRef(initialCache.projectSessions || {})
  const currentProjectRef = useRef(initialProject)
  const restoredRef = useRef(false)
  const readerRef = useRef(null)
  /** Writer 落盘刷新时默认贴底；用户上滑阅读则暂停跟滚。 */
  const followDraftBottomRef = useRef(true)
  const lastDraftLenRef = useRef(0)

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
  }, [persistCurrentWorkspace])

  const deleteNovel = useCallback(async (name, displayTitle, event) => {
    event?.stopPropagation()
    event?.preventDefault()
    const title = displayTitle || name
    if (!window.confirm(`确定删除《${title}》？\n项目文件将永久删除，不可恢复。`)) {
      return
    }
    const data = await api(`/library/${encodeURIComponent(name)}`, { method: 'DELETE' })
    if (data.error) {
      window.alert(data.error)
      return
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

  const handleDraftPatchesChange = useCallback((patches, opts = {}) => {
    const list = Array.isArray(patches) ? patches : []
    const prevSig = deskPatchesRef.current
      .map((p) => `${p.chapter}:${p.start_para}:${p.before}`)
      .join('|')
    const nextSig = list.map((p) => `${p.chapter}:${p.start_para}:${p.before}`).join('|')
    const arrived = list.length && prevSig !== nextSig
    deskPatchesRef.current = list
    setDeskPatches(list)
    if (opts.focus || arrived) {
      setDeskPatchHidden(false)
    }
    if (opts.focus) {
      setDeskPatchFocus(opts.focus)
      const ch = Number(opts.focus.chapter) || 0
      if (ch > 0) setSelectedChapter(ch)
      setReaderTab('draft')
      return
    }
    if (!list.length) {
      setDeskPatchFocus(null)
      return
    }
    // New revise diffs → jump writing desk to draft so before/after is visible.
    if (arrived) {
      const latest = list[list.length - 1]
      const ch = Number(latest?.chapter) || 0
      if (ch > 0) setSelectedChapter(ch)
      setReaderTab('draft')
      setDeskPatchFocus(latest)
      return
    }
    setDeskPatchFocus((prev) => {
      if (!prev) return list[list.length - 1]
      const still = list.find((p) => (
        Number(p.chapter) === Number(prev.chapter)
        && Number(p.start_para) === Number(prev.start_para)
        && String(p.before || '') === String(prev.before || '')
      ))
      return still || list[list.length - 1]
    })
  }, [])

  const sendChatMessage = useCallback((text, opts) => {
    if (!text || typeof chatSendRef.current !== 'function') return false
    return chatSendRef.current(text, opts) !== false
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
    setAppMode('library')
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

  const chapterMeta = preview?.chapters?.find((c) => c.number === selectedChapter)
  const chapterData = (() => {
    const cached = chapterCache[selectedChapter]
    if (cached) {
      return {
        number: selectedChapter,
        title: cached.title || chapterMeta?.title || `第${selectedChapter}章`,
        draft: cached.draft || '',
        outline: cached.outline || '',
        body_chars: cached.body_chars ?? chapterMeta?.body_chars,
      }
    }
    if (!chapterMeta) return null
    // Legacy: preview still embeds bodies (older servers).
    if (chapterMeta.draft || chapterMeta.outline) return chapterMeta
    return {
      number: selectedChapter,
      title: chapterMeta.title || `第${selectedChapter}章`,
      draft: '',
      outline: '',
      body_chars: chapterMeta.body_chars,
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
  const nextChapter = novelRecord?.next_chapter ?? (preview?.chapters?.length || 0) + 1
  const publishedCount = Number(
    novelRecord?.published_count ?? preview?.published_count ?? 0,
  ) || 0
  const isChapterPublished = (n) => Number(n) > 0 && Number(n) <= publishedCount
  // bible.md is canonical; world_architect.md is legacy dual-write — show one「世界观」tab.
  // master_outline.md is the only「总纲」surface (story_outline.json is internal acts store).
  // nomenclature overlaps 人物/物品/地点 entity cards — hide from reader.
  const arcOutlines = (Array.isArray(preview?.arc_outlines) ? preview.arc_outlines : [])
    .filter((a) => a?.volume >= 1 && String(a?.markdown || '').trim())
    .slice()
    .sort((a, b) => a.volume - b.volume)
  const artifactTabs = Object.keys(preview?.artifacts || {}).filter((k) => {
    if (k === 'world_architect' && preview?.artifacts?.bible) return false
    if (
      k === 'master_outline'
      || k === 'master_planner'
      || k === 'story_outline'
      || k === 'relation_graph'
      || k === 'nomenclature'
      || k === 'arc_outline'
      || k === 'arc_planner'
    ) {
      return false
    }
    return true
  })
  const entities = preview?.entities || {}
  const plots = sortPlotsByProgress(preview?.plots || [])
  const entityGaps = preview?.entity_gaps || []
  const expectedEvents = preview?.expected_events || []
  const volumeGroups = buildVolumePlotGroups(arcOutlines, plots)
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
  const wordTone = wordProgressTone(draftBodyChars)
  const wordPct = Math.min(100, Math.round((draftBodyChars / CHAPTER_WORD_MIN) * 100))
  const activePlotSummary = summarizePlotForDesk(pickActivePlot(plots))
  const visibleDeskPatches = deskPatches.filter((p) => (
    !selectedChapter || Number(p.chapter) === Number(selectedChapter)
  ))
  const activeDeskPatch = (() => {
    if (!visibleDeskPatches.length) return null
    if (deskPatchFocus && visibleDeskPatches.some((p) => (
      Number(p.chapter) === Number(deskPatchFocus.chapter)
      && Number(p.start_para) === Number(deskPatchFocus.start_para)
      && String(p.before || '') === String(deskPatchFocus.before || '')
    ))) {
      return deskPatchFocus
    }
    return visibleDeskPatches[visibleDeskPatches.length - 1]
  })()

  const progressLabel = (() => {
    if (!novelRecord) return ''
    const n = novelRecord.published_count || 0
    const vol = novelRecord.current_volume
    if (vol) return `已写 ${n} 章 · 当前卷：${vol}`
    return `已写 ${n} 章`
  })()

  const longformHealth = preview?.longform_health || null
  const foreshadowDebt = longformHealth?.foreshadow || null
  const volumeHealth = longformHealth?.volume || null
  const lengthHealth = longformHealth?.length || null
  const longformTier = longformHealth?.longform || null
  const costByAgent = Array.isArray(preview?.cost_by_agent) ? preview.cost_by_agent : []
  const costTop = costByAgent
    .filter((c) => c?.agent && c.agent !== 'pipeline')
    .slice(0, 4)
    .map((c) => `${c.agent}≈${Math.round((c.approx_tokens || 0) / 1000)}k`)
    .join(' · ')
  const lengthChip = lengthHealth
    ? `偏短率 ${Math.round((lengthHealth.soft_short_rate || 0) * 100)}%`
      + (lengthHealth.consecutive_soft_short
        ? ` · 连短 ${lengthHealth.consecutive_soft_short}`
        : '')
    : ''

  // 长篇健康交通灯：ok 绿 / warn 黄 / bad 红（伏笔看压力近债，不看总量）
  const foreshadowLevel = (() => {
    const pressure = foreshadowDebt?.dangling_pressure ?? 0
    const cold = foreshadowDebt?.open_cold ?? 0
    if (pressure > 24 || cold > 40) return 'bad'
    if (pressure > 8 || cold > 0) return 'warn'
    return 'ok'
  })()
  const foreshadowDebtClassLabel = (c) => {
    switch (String(c || '').toLowerCase()) {
      case 'near':
        return '近债'
      case 'mid':
        return '中期'
      case 'far':
        return '远期'
      case 'fresh':
        return '宽限'
      default:
        return ''
    }
  }
  const volumeLevel = (() => {
    if (!volumeHealth?.active_index) return 'warn'
    if (volumeHealth.thick_volume_warning) return 'bad'
    const ch = volumeHealth.chapters_in_volume || 0
    const mid = volumeHealth.mid_audit_threshold || 0
    if (mid > 0 && ch >= mid && !volumeHealth.has_audit_report) return 'warn'
    return 'ok'
  })()
  const lengthLevel = (() => {
    if (!lengthHealth) return 'ok'
    const rate = lengthHealth.soft_short_rate || 0
    const streak = lengthHealth.consecutive_soft_short || 0
    const hard = lengthHealth.recent_hard_short || 0
    if (streak >= 3 || rate > 0.4 || hard >= 2) return 'bad'
    if (streak >= 1 || rate > 0.15 || hard >= 1) return 'warn'
    return 'ok'
  })()
  const longformLevel = (() => {
    if (!longformTier) return 'ok'
    const q = String(longformTier.quality_tier || '').toLowerCase()
    const impact = String(longformTier.impact_scan_mode || '').toLowerCase()
    const drift = longformTier.drift_samples ?? 0
    if (impact === 'all' || drift > 80) return 'bad'
    if (q === 'economy' || drift > 20) return 'warn'
    return 'ok'
  })()
  const healthLevelLabel = { ok: '正常', warn: '警告', bad: '异常' }

  const hasReaderMaterial = Boolean(
    chapterList.length
    || artifactTabs.length
    || arcOutlines.length
    || volumeGroups.length
    || entities.characters?.length
    || entities.items?.length
    || entities.locations?.length
    || plots.length
    || preview?.artifacts?.master_outline
    || preview?.artifacts?.master_planner
    || storyOutline?.markdown
    || storyOutline?.acts?.length
    || project
  )

  const readerContent = (() => {
    if (readerTab === 'volume') return ''
    if (readerTab === 'draft') {
      if (chapterLoading && !chapterData?.draft) return '（加载正文中…）'
      return chapterData?.draft
    }
    if (readerTab === 'outline') {
      if (chapterLoading && !chapterData?.outline) return '（加载章纲中…）'
      return chapterData?.outline
    }
    if (readerTab === 'master') {
      return preview?.artifacts?.master_outline
        || preview?.artifacts?.master_planner
        || storyOutline?.markdown
        || '（尚无总纲）'
    }
    if (readerTab === 'entity_gaps') {
      return preview?.entity_gaps_display
        || (entityGaps.length
          ? '（软提示·不阻断写章）待补全：\n\n' + entityGaps.map((g) => `- ${g}`).join('\n')
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
    if (readerTab.startsWith('art:')) return preview?.artifacts?.[readerTab.slice(4)] || ''
    return ''
  })()

  const onReaderScroll = () => {
    const el = readerRef.current
    if (!el || readerEditing) return
    const dist = el.scrollHeight - el.scrollTop - el.clientHeight
    followDraftBottomRef.current = dist < 180
  }

  // Writer 间歇刷新正文时强制贴底（大段跳变时 nearBottom 会失效）。
  useLayoutEffect(() => {
    if (readerEditing) return
    const el = readerRef.current
    if (!el) return
    const len = (readerContent || '').length
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
  }, [readerContent, readerEditing, readerTab])

  // 切章 / 切到正文 tab：重新开启贴底跟随
  useEffect(() => {
    if (readerTab !== 'draft') return
    followDraftBottomRef.current = true
    lastDraftLenRef.current = 0
    const el = readerRef.current
    if (el) {
      requestAnimationFrame(() => {
        if (followDraftBottomRef.current && readerRef.current) {
          readerRef.current.scrollTop = readerRef.current.scrollHeight
        }
      })
    }
  }, [readerTab, selectedChapter])

  const readerTabs = [
    {
      id: 'volume',
      label: '本卷',
      show: Boolean(volumeGroups.length || project),
    },
    { id: 'draft', label: preview?.project_mode === 'short_drama' ? '剧本' : '正文', show: Boolean(chapterData) },
    { id: 'outline', label: preview?.project_mode === 'short_drama' ? '集纲' : '章纲', show: Boolean(chapterData) },
    {
      id: 'master',
      label: '总纲',
      show: Boolean(
        preview?.artifacts?.master_outline
        || preview?.artifacts?.master_planner
        || storyOutline?.markdown
        || project,
      ),
    },
    {
      id: 'arcs',
      label: '卷纲',
      show: Boolean(arcOutlines.length || plots.length),
    },
    ...Object.entries(ENTITY_TAB_LABELS).map(([key, label]) => ({
      id: `ent:${key}`,
      label,
      show: Boolean(entities[key]?.length),
    })),
    { id: 'entity_gaps', label: '设定缺口', show: Boolean(entityGaps.length) },
    { id: 'expected', label: '预期', show: Boolean(expectedEvents.length) },
    ...artifactTabs.map((k) => ({
      id: `art:${k}`, label: ARTIFACT_LABELS[k] || k, show: true,
    })),
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
  const deskPatchDockVisible = Boolean(
    !deskPatchHidden && activeDeskPatch && readerTab === 'draft',
  )
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
    if (cta.readerTab) setReaderTab(cta.readerTab === 'plots' ? 'arcs' : cta.readerTab)
    // Apply: stay on the chapter that has diffs (never jump to phase nextChapter).
    let chapter = Number(cta.chapter) || 0
    if (cta.isApply && deskPatchChapter > 0) chapter = deskPatchChapter
    if (chapter > 0) setSelectedChapter(chapter)
    // Desk CTAs must not interrupt/restart a running turn on double-click.
    if (cta.message) sendChatMessage(cta.message, { busy: 'ignore' })
  }

  // Esc closes engine drawer; lock body scroll while open.
  useEffect(() => {
    if (!engineOpen) return undefined
    const prev = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    const onKey = (e) => {
      if (e.key === 'Escape') setEngineOpen(false)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      document.body.style.overflow = prev
      window.removeEventListener('keydown', onKey)
    }
  }, [engineOpen])

  const beginReaderEdit = () => {
    setReaderEditError('')
    // Entity/plot display may prepend status meta — edit the on-disk body only.
    let text = readerContent || ''
    if (readerTab.startsWith('ent:') && selectedReaderCard?.markdown) {
      text = selectedReaderCard.markdown
    } else if (isArcNav && selectedReaderEntry?.kind === 'plot' && selectedReaderCard?.markdown) {
      text = selectedReaderCard.markdown
    }
    setReaderEditText(text)
    setReaderEditing(true)
  }

  const cancelReaderEdit = () => {
    setReaderEditing(false)
    setReaderEditText('')
    setReaderEditError('')
  }

  const saveReaderEdit = async () => {
    if (!project || !readerCanEdit) return
    setReaderSaving(true)
    setReaderEditError('')
    const activeEntry = readerCardList.length
      ? (readerCardList.find((c) => c.key === readerCardKey) || readerCardList[0])
      : null
    const activeCardKey = activeEntry?.key || ''
    // Nested plot under 卷纲 still saves via plots API.
    const saveTab = activeEntry?.kind === 'plot' ? 'plots' : readerTab
    const res = await api(`/projects/${encodeURIComponent(project)}/content`, {
      method: 'PUT',
      body: JSON.stringify({
        tab: saveTab,
        content: readerEditText,
        chapter: selectedChapter || 0,
        card_key: activeCardKey,
      }),
    })
    setReaderSaving(false)
    if (res?.error) {
      setReaderEditError(typeof res.error === 'string' ? res.error : '保存失败')
      return
    }
    if (res?.preview) {
      setPreview(res.preview)
    } else {
      await fetchNovelPreview(project)
    }
    if (readerTab === 'draft' || readerTab === 'outline') {
      setChapterCache((prev) => {
        const next = { ...prev }
        delete next[selectedChapter]
        return next
      })
    }
    setReaderEditing(false)
    setReaderEditText('')
  }

  const switchReaderTab = (id) => {
    if (readerEditing && id !== readerTab) {
      if (!window.confirm('正在编辑，切换将丢弃未保存修改，继续？')) return
      cancelReaderEdit()
    }
    // 剧情卡已挂在卷纲下，旧链接统一落到卷纲树。
    setReaderTab(id === 'plots' ? 'arcs' : id)
    setReaderCardKey('')
  }

  const switchReaderVolume = (vol) => {
    if (vol === readerVolume) return
    if (readerEditing) {
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

  const deleteChapter = useCallback(async (chapter) => {
    const proj = currentProjectRef.current || project
    if (!proj || !chapter) return
    const title = preview?.chapters?.find((c) => c.number === chapter)?.title
    const label = title ? `第${chapter}章「${title}」` : `第${chapter}章`
    if (!window.confirm(`确定删除${label}？\n章节目录将永久删除，不可恢复。`)) {
      return
    }
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
      return
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
  }, [
    project,
    preview,
    readerEditing,
    refreshLibrary,
    selectedChapter,
  ])

  return (
    <div className="studio">
      <header className="studio-header">
        <div className="studio-header-left">
          <h1>NovelX</h1>
          {project && (
            <div className="header-meta">
              <span>{novelRecord?.display_title || project}</span>
              {novelRecord && progressLabel && (
                <span className="progress-chip">{progressLabel}</span>
              )}
            </div>
          )}
        </div>
        {project && preview ? (
          <nav className="stage-strip" aria-label="创作阶段">
            {STAGE_ORDER.map((s, idx) => {
              const active = studioStage.stageIndex === idx
              const done = studioStage.stageIndex > idx
              return (
                <span
                  key={s.id}
                  className={[
                    'stage-chip',
                    active ? 'is-active' : '',
                    done ? 'is-done' : '',
                  ].filter(Boolean).join(' ')}
                  title={active ? studioStage.detail : s.label}
                >
                  {s.label}
                </span>
              )
            })}
            {studioStage.detail ? (
              <span className="stage-detail">{studioStage.detail}</span>
            ) : null}
          </nav>
        ) : null}
      </header>

      <div className="studio-grid">
        <section className="panel side-panel">
          <div className="panel-tabs side-mode-tabs" role="tablist" aria-label="侧栏模式">
            <button
              type="button"
              role="tab"
              aria-selected={appMode === 'library'}
              className={appMode === 'library' ? 'active' : ''}
              onClick={() => setAppMode('library')}
            >
              书库
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={appMode === 'prefs'}
              className={appMode === 'prefs' ? 'active' : ''}
              onClick={() => setAppMode('prefs')}
            >
              偏好
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={engineOpen}
              className={engineOpen ? 'active' : ''}
              onClick={() => setEngineOpen(true)}
            >
              引擎
            </button>
          </div>
          {appMode === 'prefs' ? (
            <WritingPrefs
              project={project}
              longformHealth={longformHealth}
              publishedCount={publishedCount}
              nextChapter={nextChapter}
              onOpenEngine={() => setEngineOpen(true)}
              onAuditChapter={(ch) => {
                setSelectedChapter(ch)
                setReaderTab('draft')
                sendChatMessage(`审校第${ch}章`, { busy: 'ignore' })
              }}
            />
          ) : null}
          {appMode === 'library' ? (
            <>
              <div className="side-library-head">
                <h2 className="side-title">我的</h2>
                <button
                  type="button"
                  className="btn-primary btn-inline btn-new-novel"
                  onClick={() => setShowNewNovelForm((v) => !v)}
                  disabled={newNovelBusy}
                >
                  {showNewNovelForm ? '收起' : '新建小说'}
                </button>
              </div>
              <p className="side-hint">每本小说独立创作助手会话，可切换并行创作</p>
              {showNewNovelForm ? (
                <form
                  className="new-novel-form"
                  onSubmit={(e) => {
                    e.preventDefault()
                    submitNewNovel()
                  }}
                >
                  <label className="new-novel-field">
                    <span>书名</span>
                    <input
                      type="text"
                      value={newNovelTitle}
                      onChange={(e) => setNewNovelTitle(e.target.value)}
                      placeholder="必填"
                      autoFocus
                      disabled={newNovelBusy}
                    />
                  </label>
                  <label className="new-novel-field">
                    <span>题材</span>
                    <input
                      type="text"
                      value={newNovelGenre}
                      onChange={(e) => setNewNovelGenre(e.target.value)}
                      placeholder="可选，如未定"
                      disabled={newNovelBusy}
                    />
                  </label>
                  <label className="new-novel-field">
                    <span>创作模式</span>
                    <select
                      value={newNovelMode}
                      onChange={(e) => setNewNovelMode(e.target.value)}
                      disabled={newNovelBusy}
                    >
                      <option value="longform">超长篇小说</option>
                      <option value="short_drama">AI 漫剧短篇（剧本）</option>
                    </select>
                  </label>
                  <label className="new-novel-field">
                    <span>灵感</span>
                    <textarea
                      value={newNovelBrief}
                      onChange={(e) => setNewNovelBrief(e.target.value)}
                      placeholder="一两句卖点或设定方向（可选）"
                      rows={3}
                      disabled={newNovelBusy}
                    />
                  </label>
                  <button
                    type="submit"
                    className="btn-primary btn-inline"
                    disabled={newNovelBusy || !newNovelTitle.trim()}
                  >
                    {newNovelBusy ? '创建中…' : '开始立项'}
                  </button>
                </form>
              ) : null}
              <ul className="novel-list">
                {library.length === 0 ? (
                  <li className="empty-list">暂无小说，点「新建小说」或对创作助手说「我想写一本小说」</li>
                ) : (
                  library.map((n) => (
                    <li key={n.id} className={project === n.id ? 'selected' : ''} onClick={() => loadNovel(n.id)}>
                      <div className="novel-row">
                        <div className="novel-info">
                          <div className="novel-title">{n.display_title || n.id}</div>
                          <div className="novel-meta">
                            {n.current_volume
                              ? `已写 ${n.published_count || 0} 章 · ${n.current_volume}`
                              : `已写 ${n.published_count || 0} 章`}
                            {n.genre ? ` · ${n.genre}` : ''}
                          </div>
                        </div>
                        <button
                          type="button"
                          className="novel-delete"
                          title="删除小说"
                          aria-label={`删除 ${n.display_title || n.id}`}
                          onClick={(e) => deleteNovel(n.id, n.display_title, e)}
                        >
                          删除
                        </button>
                      </div>
                    </li>
                  ))
                )}
              </ul>
              {project && longformHealth && (
                <div className="side-health" aria-label="超长篇健康">
                  <h2 className="side-title">长篇健康</h2>
                  <p className="side-hint">伏笔与卷况，不占写作台</p>
                  <div className="longform-health longform-health--side">
                    <div
                      className={`longform-health-card longform-health-card--foreshadow level-${foreshadowLevel}${
                        foreshadowDebtExpanded ? ' is-expanded' : ''
                      }`}
                    >
                      <div className="longform-health-title">
                        <span>伏笔债务</span>
                        <span className={`longform-health-badge level-${foreshadowLevel}`}>
                          {healthLevelLabel[foreshadowLevel]}
                        </span>
                      </div>
                      <div className="longform-health-body">
                        总量 {foreshadowDebt?.dangling_total ?? 0}
                        {foreshadowDebt?.dangling_pressure != null
                          ? ` · 近债 ${foreshadowDebt.dangling_pressure}`
                          : ''}
                        {foreshadowDebt?.open_cold ? ` · 冷档 ${foreshadowDebt.open_cold}` : ''}
                      </div>
                      <div className="longform-health-sub">
                        宽限 {foreshadowDebt?.dangling_fresh ?? 0}
                        {' · '}中期 {foreshadowDebt?.dangling_mid ?? 0}
                        {' · '}远期 {foreshadowDebt?.dangling_far ?? 0}
                        <span className="longform-health-debt-hint">
                          （批写只拦近债/中期压力）
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
                              const cls = foreshadowDebtClassLabel(t.debt_class)
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
                              onClick={() => setForeshadowDebtExpanded((v) => !v)}
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
                        <span>卷健康</span>
                        <span className={`longform-health-badge level-${volumeLevel}`}>
                          {healthLevelLabel[volumeLevel]}
                        </span>
                      </div>
                      <div className="longform-health-body">
                        {volumeHealth?.active_index
                          ? `第${volumeHealth.active_index}卷 · ${volumeHealth.chapters_in_volume || 0}章`
                          : '尚无进行中卷'}
                        {volumeHealth?.has_audit_report ? ' · 已复盘' : ''}
                        {volumeHealth?.thick_volume_warning ? ' · 厚卷' : ''}
                      </div>
                      <div className="longform-health-sub">
                        rollup {volumeHealth?.rollup_total ?? 0}
                        {volumeHealth?.mid_audit_threshold
                          ? ` · 中卷审≥${volumeHealth.mid_audit_threshold}章`
                          : ''}
                      </div>
                    </div>
                    {lengthHealth ? (
                      <div className={`longform-health-card level-${lengthLevel}`}>
                        <div className="longform-health-title">
                          <span>篇幅</span>
                          <span className={`longform-health-badge level-${lengthLevel}`}>
                            {healthLevelLabel[lengthLevel]}
                          </span>
                        </div>
                        <div className="longform-health-body">
                          {lengthChip || '—'}
                          {lengthHealth.word_hard_min
                            ? ` · 硬门 ${lengthHealth.word_hard_min}`
                            : ''}
                        </div>
                        <div className="longform-health-sub">
                          目标 {lengthHealth.word_min}–{lengthHealth.word_max}
                          {lengthHealth.target_chapters
                            ? ` · 规划 ${lengthHealth.target_chapters} 章`
                            : ''}
                        </div>
                      </div>
                    ) : null}
                    {longformTier ? (
                      <div className={`longform-health-card level-${longformLevel}`}>
                        <div className="longform-health-title">
                          <span>长篇档</span>
                          <span className={`longform-health-badge level-${longformLevel}`}>
                            {healthLevelLabel[longformLevel]}
                          </span>
                        </div>
                        <div className="longform-health-body">
                          {longformTier.quality_tier || '—'}
                          {longformTier.audit_tier ? ` · 审 ${longformTier.audit_tier}` : ''}
                        </div>
                        <div className="longform-health-sub">
                          impact {longformTier.impact_scan_mode || '—'}
                          {longformTier.drift_samples != null
                            ? ` · 漂移抽样 ${longformTier.drift_samples}`
                            : ''}
                        </div>
                      </div>
                    ) : null}
                    {costTop ? (
                      <div className="longform-health-card level-ok">
                        <div className="longform-health-title">
                          <span>成本（近录）</span>
                          <span className="longform-health-badge level-ok">参考</span>
                        </div>
                        <div className="longform-health-body longform-health-cost">{costTop}</div>
                      </div>
                    ) : null}
                  </div>
                </div>
              )}
            </>
          ) : null}
        </section>

        <div className="work-area">
          <section className="panel reader-panel">
          <div className="reader-head">
            <h2>写作台</h2>
            {chapterList.length > 0 && (
              <div className="chapter-strip" role="tablist" aria-label="章节">
                {chapterList.map((ch) => (
                  <button
                    key={ch.number}
                    type="button"
                    role="tab"
                    aria-selected={selectedChapter === ch.number}
                    className={[
                      'ch-pill',
                      selectedChapter === ch.number ? 'active' : '',
                      ch.status === 'published' ? 'published' : '',
                      ch.status === 'draft' ? 'draft' : '',
                      ch.status === 'outline' ? 'outline' : '',
                      ch.status === 'next' || ch.number === nextChapter ? 'next' : '',
                    ].join(' ')}
                    onClick={() => {
                      if (readerEditing) {
                        if (!window.confirm('正在编辑，切换章节将丢弃未保存修改，继续？')) return
                        cancelReaderEdit()
                      }
                      setSelectedChapter(ch.number)
                      setReaderTab(ch.hasDraft || ch.status === 'published' ? 'draft' : 'master')
                    }}
                    title={`${ch.title || `第${ch.number}章`} · ${ch.statusLabel}${
                      ch.bodyChars ? ` · ${ch.bodyChars}字` : ''
                    }`}
                  >
                    {ch.number}
                  </button>
                ))}
              </div>
            )}
          </div>

          {!hasReaderMaterial ? (
            <div className="empty small">在 NovelX 创建项目后，大纲、设定卡与正文/剧本将显示在此</div>
          ) : (
            <>
              <div className="reader-meta">
                <div className="reader-meta-row">
                  {chapterList.length > 0 && selectedChapter > 0 ? (
                    <span className="reader-ch-title">
                      第{selectedChapter}章
                      {chapterData?.title ? ` · ${chapterData.title}` : ''}
                      {selectedChapterStatus ? (
                        <span
                          className={`ch-status-badge ch-status-${selectedChapterStatus.status}`}
                        >
                          {selectedChapterStatus.statusLabel}
                          {selectedChapterStatus.bodyChars
                            ? ` · ${selectedChapterStatus.bodyChars}字`
                            : ''}
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
                        onClick={() => deleteChapter(selectedChapter)}
                        title="删除当前章节"
                      >
                        删除本章
                      </button>
                    )}
                    {readerEditing && (
                      <div className="reader-edit-actions">
                        <button
                          type="button"
                          className="btn-primary btn-inline"
                          disabled={readerSaving}
                          onClick={saveReaderEdit}
                        >
                          {readerSaving ? '保存中…' : '保存'}
                        </button>
                        <button
                          type="button"
                          className="btn-ghost btn-inline"
                          disabled={readerSaving}
                          onClick={cancelReaderEdit}
                        >
                          取消
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
              {showNextCta ? (
                <div className="next-cta-bar" role="region" aria-label="下一步">
                  <div className="next-cta-copy">
                    <strong>下一步 · {studioStage.cta.label}</strong>
                    <span>
                      {chatBusy
                        ? '助手进行中，完成后可再点'
                        : (studioStage.cta.hint || studioStage.detail)}
                    </span>
                  </div>
                  <div className="next-cta-actions">
                    <button
                      type="button"
                      className="btn-primary btn-inline"
                      onClick={runStudioCta}
                      disabled={chatBusy}
                      aria-busy={chatBusy}
                    >
                      {chatBusy ? '进行中…' : studioStage.cta.label}
                    </button>
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
              {readerTab === 'draft' && selectedChapter > 0 ? (
                <div className="desk-word-bar" aria-label="字数进度">
                  <div className="desk-word-meta">
                    <span>
                      {draftBodyChars}
                      <span className="desk-word-sep">/</span>
                      {CHAPTER_WORD_MIN}–{CHAPTER_WORD_MAX} 字
                    </span>
                    <span className={`desk-word-tone tone-${wordTone}`}>
                      {wordProgressLabel(draftBodyChars)}
                    </span>
                    <span className="desk-word-hard">硬门 ≥{CHAPTER_WORD_HARD_MIN}</span>
                  </div>
                  <div className="desk-word-track" aria-hidden="true">
                    <div
                      className={`desk-word-fill tone-${wordTone}`}
                      style={{ width: `${wordPct}%` }}
                    />
                    <span
                      className="desk-word-mark hard"
                      style={{ left: `${Math.min(100, (CHAPTER_WORD_HARD_MIN / CHAPTER_WORD_MIN) * 100)}%` }}
                    />
                  </div>
                </div>
              ) : null}
              {readerTab === 'draft' && activePlotSummary ? (
                <div className={`desk-plot-rail${plotRailOpen ? '' : ' is-collapsed'}`}>
                  <button
                    type="button"
                    className="desk-plot-rail-head"
                    onClick={() => setPlotRailOpen((v) => !v)}
                    aria-expanded={plotRailOpen}
                  >
                    <span className="desk-plot-rail-title">
                      当前剧情 · {activePlotSummary.title}
                      <span className="desk-plot-status">{activePlotSummary.statusLabel}</span>
                    </span>
                    <span className="desk-plot-rail-chevron">{plotRailOpen ? '▾' : '▸'}</span>
                  </button>
                  {plotRailOpen ? (
                    <div className="desk-plot-rail-body">
                      <p>{activePlotSummary.body}</p>
                      <button
                        type="button"
                        className="btn-ghost btn-inline"
                        onClick={() => {
                          setReaderTab('arcs')
                          const key = plotCardKey(pickActivePlot(plots))
                          if (key) setReaderCardKey(key)
                        }}
                      >
                        打开完整剧情卡
                      </button>
                    </div>
                  ) : null}
                </div>
              ) : null}
              {!deskPatchHidden && activeDeskPatch && readerTab === 'draft' ? (
                <div className="desk-patch-dock" role="region" aria-label="修订对照">
                  <div className="desk-patch-dock-head">
                    <strong>
                      修订对照 · 第{activeDeskPatch.chapter}章 · 第{activeDeskPatch.start_para}
                      {activeDeskPatch.end_para !== activeDeskPatch.start_para
                        ? `–${activeDeskPatch.end_para}`
                        : ''}
                      段
                      {visibleDeskPatches.length > 1
                        ? ` · ${visibleDeskPatches.length} 处`
                        : ''}
                    </strong>
                    <div className="desk-patch-dock-actions">
                      {visibleDeskPatches.length > 1 ? (
                        <select
                          className="desk-patch-select"
                          value={String(Math.max(0, visibleDeskPatches.findIndex((p) => (
                            Number(p.chapter) === Number(activeDeskPatch.chapter)
                            && Number(p.start_para) === Number(activeDeskPatch.start_para)
                            && String(p.before || '') === String(activeDeskPatch.before || '')
                          ))))}
                          onChange={(e) => {
                            const idx = Number(e.target.value)
                            if (visibleDeskPatches[idx]) setDeskPatchFocus(visibleDeskPatches[idx])
                          }}
                          aria-label="选择修订段"
                        >
                          {visibleDeskPatches.map((p, idx) => (
                            <option key={`${p.chapter}-${p.start_para}-${idx}`} value={String(idx)}>
                              第{p.start_para}
                              {p.end_para !== p.start_para ? `–${p.end_para}` : ''}
                              段
                            </option>
                          ))}
                        </select>
                      ) : null}
                      {studioStage.cta?.isApply ? (
                        <button
                          type="button"
                          className="btn-primary btn-inline"
                          onClick={runStudioCta}
                          disabled={chatBusy}
                          title="将预览中的修订写入正文"
                        >
                          {chatBusy ? '进行中…' : '应用修改'}
                        </button>
                      ) : null}
                      <button
                        type="button"
                        className="btn-ghost btn-inline"
                        onClick={() => setDeskPatchHidden(true)}
                      >
                        收起对照
                      </button>
                    </div>
                  </div>
                  <div className="desk-patch-diff">
                    <div className="desk-patch-before">
                      <div className="desk-patch-label">原文</div>
                      <pre>{activeDeskPatch.before || '（空）'}</pre>
                    </div>
                    <div className="desk-patch-after">
                      <div className="desk-patch-label">修订后</div>
                      <pre>{activeDeskPatch.after || '（空）'}</pre>
                    </div>
                  </div>
                </div>
              ) : null}
              <div
                className={`reader-body${readerEditing ? ' reader-body-editing' : ''}${
                  !deskPatchHidden && activeDeskPatch && readerTab === 'draft' ? ' with-patch' : ''
                }${isVolumeWorkspace ? ' reader-body-workspace' : ''}`}
                ref={readerRef}
                onScroll={onReaderScroll}
              >
                {isVolumeWorkspace ? (
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
                  <textarea
                    className="reader-editor"
                    value={readerEditText}
                    onChange={(e) => setReaderEditText(e.target.value)}
                    spellCheck={false}
                    aria-label="编辑写作台内容"
                  />
                ) : (
                  <MarkdownView
                    variant="reader"
                    source={readerContent || '暂无内容'}
                  />
                )}
              </div>
              {(readerTab === 'draft' || readerTab === 'outline')
                && selectedChapter > 0
                && selectedChapterStatus ? (
                <div className="reader-publish-foot" aria-label="章节发布状态">
                  <span>
                    第{selectedChapter}章 · {selectedChapterStatus.statusLabel}
                    {selectedChapterStatus.bodyChars
                      ? ` · ${selectedChapterStatus.bodyChars}字`
                      : ''}
                  </span>
                  <span className="reader-publish-foot-meta">
                    {selectedChapterStatus.status === 'published'
                      ? '已计入已写章节'
                      : selectedChapterStatus.status === 'draft'
                        ? '草稿未计入已发布'
                        : `下一章目标：第${nextChapter}章`}
                  </span>
                </div>
              ) : null}
            </>
          )}
        </section>

        <NovelXChat
          project={project}
          sendRef={chatSendRef}
          onSetupGateChange={setChatSetupGateOpen}
          onBusyChange={handleChatBusyChange}
          onPreviewRefresh={handleChatPreviewRefresh}
          onProjectBound={handleProjectBound}
          onDraftPatchesChange={handleDraftPatchesChange}
          onAuditStateChange={handleAuditStateChange}
          compactDraftPatches={deskPatchDockVisible}
        />
        </div>
      </div>

      {engineOpen ? (
        <div className="engine-drawer-root" role="dialog" aria-label="引擎室">
          <button
            type="button"
            className="engine-drawer-backdrop"
            aria-label="关闭引擎室"
            onClick={() => setEngineOpen(false)}
          />
          <div className="engine-drawer">
            <ConfigPanel onClose={() => setEngineOpen(false)} title="引擎室" />
          </div>
        </div>
      ) : null}
    </div>
  )
}
