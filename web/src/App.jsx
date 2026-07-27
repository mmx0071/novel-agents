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
import { buildVolumePlotGroups, sortPlotsByProgress } from './plotSort'

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
  /** `library` = 书库创作；`config` = 硬规则 / Skills */
  const [appMode, setAppMode] = useState('library')
  const [library, setLibrary] = useState([])
  const [novelRecord, setNovelRecord] = useState(null)
  const [project, setProject] = useState(initialProject)
  const [preview, setPreview] = useState(null)
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
      setReaderTab('arcs')
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
    if (opts.chapter != null && opts.chapter > 0) {
      setSelectedChapter(opts.chapter)
    } else if ((opts.focusLatestChapter || !opts.keepSelection) && chapters.length) {
      setSelectedChapter(chapters[chapters.length - 1].number)
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

  const chapterData = preview?.chapters?.find((c) => c.number === selectedChapter)
  const storyOutline = preview?.story_outline
  const nextChapter = novelRecord?.next_chapter ?? (preview?.chapters?.length || 0) + 1
  const publishedSet = new Set((preview?.chapters || []).map((c) => c.number))
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
  const volumeGroups = buildVolumePlotGroups(arcOutlines, plots)
  const isArcNav = readerTab === 'arcs' || readerTab === 'plots'

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

  const chapterList = (preview?.chapters || []).map((c) => ({
    number: c.number,
    title: c.title,
    summary: '',
  }))

  const progressLabel = (() => {
    if (!novelRecord) return ''
    const n = novelRecord.published_count || 0
    const vol = novelRecord.current_volume
    if (vol) return `已写 ${n} 章 · 当前卷：${vol}`
    return `已写 ${n} 章`
  })()

  const hasReaderMaterial = Boolean(
    chapterList.length
    || artifactTabs.length
    || arcOutlines.length
    || entities.characters?.length
    || entities.items?.length
    || entities.locations?.length
    || plots.length
    || preview?.artifacts?.master_outline
    || preview?.artifacts?.master_planner
    || storyOutline?.markdown
    || storyOutline?.acts?.length
  )

  const readerContent = (() => {
    if (readerTab === 'draft') return chapterData?.draft
    if (readerTab === 'outline') return chapterData?.outline
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
    { id: 'draft', label: '正文', show: Boolean(chapterData) },
    { id: 'outline', label: '章纲', show: Boolean(chapterData) },
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
    ...artifactTabs.map((k) => ({
      id: `art:${k}`, label: ARTIFACT_LABELS[k] || k, show: true,
    })),
  ].filter((t) => t.show)

  const readerCanEdit = Boolean(
    project
    && readerTab
    && readerTab !== 'entity_gaps',
  )

  const setupAwaitingConfirm = preview?.setup_phase === 'awaiting_confirm' || chatSetupGateOpen
  const setupOutlineTab = readerTab === 'master'
    || readerTab === 'arcs'
    || readerTab === 'art:arc_outline'
    || readerTab === 'art:arc_planner'
  const showSetupConfirmBar = Boolean(project && setupAwaitingConfirm && setupOutlineTab)

  const prevSetupGateRef = useRef(false)
  useEffect(() => {
    const wasOpen = prevSetupGateRef.current
    prevSetupGateRef.current = chatSetupGateOpen
    if (!project || !chatSetupGateOpen || wasOpen) return
    setReaderTab('master')
  }, [project, chatSetupGateOpen])

  const sendSetupGate = (actionId) => {
    if (typeof chatSendRef.current === 'function') {
      chatSendRef.current(actionId)
    }
  }

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
        <h1>NovelX</h1>
        {project && (
          <div className="header-meta">
            <span>{novelRecord?.display_title || project}</span>
            {novelRecord && progressLabel && (
              <span className="progress-chip">{progressLabel}</span>
            )}
          </div>
        )}
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
              aria-selected={appMode === 'config'}
              className={appMode === 'config' ? 'active' : ''}
              onClick={() => setAppMode('config')}
            >
              配置
            </button>
          </div>
          {appMode === 'library' ? (
            <>
              <h2 className="side-title">我的</h2>
              <p className="side-hint">每本小说独立 NovelX 对话，可切换并行创作</p>
              <ul className="novel-list">
                {library.length === 0 ? (
                  <li className="empty-list">暂无小说，在 NovelX 说「我想写一本小说」</li>
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
            </>
          ) : (
            <>
              <h2 className="side-title">框架配置</h2>
              <p className="side-hint">硬规则、禁名与 Skills 在右侧编辑；改动写入 config/。</p>
            </>
          )}
        </section>

        {appMode === 'config' ? (
          <div className="work-area work-area-config">
            <ConfigPanel />
          </div>
        ) : (
        <div className="work-area">
          <section className="panel reader-panel">
          <div className="reader-head">
            <h2>阅读</h2>
            {chapterList.length > 0 && (
              <div className="chapter-strip">
                {chapterList.map((ch) => (
                  <button
                    key={ch.number}
                    type="button"
                    className={[
                      'ch-pill',
                      selectedChapter === ch.number ? 'active' : '',
                      publishedSet.has(ch.number) ? 'published' : '',
                      ch.number === nextChapter ? 'next' : '',
                    ].join(' ')}
                    onClick={() => {
                      if (readerEditing) {
                        if (!window.confirm('正在编辑，切换章节将丢弃未保存修改，继续？')) return
                        cancelReaderEdit()
                      }
                      setSelectedChapter(ch.number)
                      setReaderTab(publishedSet.has(ch.number) ? 'draft' : 'master')
                    }}
                    title={ch.title || `第${ch.number}章`}
                  >
                    {ch.number}
                  </button>
                ))}
              </div>
            )}
          </div>

          {!hasReaderMaterial ? (
            <div className="empty small">在 NovelX 创建小说后，卷大纲、设定卡与正文将显示在此</div>
          ) : (
            <>
              <div className="reader-meta">
                <div className="reader-meta-row">
                  {chapterList.length > 0 && selectedChapter > 0 ? (
                    <span className="reader-ch-title">
                      第{selectedChapter}章
                      {chapterData?.title ? ` · ${chapterData.title}` : ''}
                    </span>
                  ) : (
                    <span className="reader-ch-title reader-ch-title-muted">阅读区</span>
                  )}
                  <div className="reader-head-actions">
                    {readerCanEdit && !readerEditing && (
                      <button
                        type="button"
                        className="btn-ghost btn-inline reader-edit-btn"
                        onClick={beginReaderEdit}
                        title="编辑当前阅读内容"
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
                <div className="reader-tabs" role="tablist" aria-label="阅读分类">
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
              {showSetupConfirmBar && (
                <div className="setup-confirm-bar" role="region" aria-label="定稿确认">
                  <div className="setup-confirm-copy">
                    <strong>总纲与卷纲已就绪</strong>
                    <span>确认后进入写章；或打回后重新生成大纲。</span>
                  </div>
                  <div className="setup-confirm-actions">
                    <button
                      type="button"
                      className="btn-primary btn-inline"
                      onClick={() => sendSetupGate('sc_approve')}
                    >
                      确认定稿
                    </button>
                    <button
                      type="button"
                      className="btn-ghost btn-inline"
                      onClick={() => sendSetupGate('sc_revise')}
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
                className={`reader-body${readerEditing ? ' reader-body-editing' : ''}`}
                ref={readerRef}
                onScroll={onReaderScroll}
              >
                {readerEditing ? (
                  <textarea
                    className="reader-editor"
                    value={readerEditText}
                    onChange={(e) => setReaderEditText(e.target.value)}
                    spellCheck={false}
                    aria-label="编辑阅读内容"
                  />
                ) : (
                  <MarkdownView
                    variant="reader"
                    source={readerContent || '暂无内容'}
                  />
                )}
              </div>
            </>
          )}
        </section>

        <NovelXChat
          project={project}
          sendRef={chatSendRef}
          onSetupGateChange={setChatSetupGateOpen}
          onPreviewRefresh={handleChatPreviewRefresh}
        />
        </div>
        )}
      </div>
    </div>
  )
}
