import { useCallback, useEffect, useRef, useState } from 'react'
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
import { sortPlotsByProgress } from './plotSort'

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
  // master_outline / master_planner → single reader tab「总纲」(not listed here as artifact)
  arc_outline: '卷纲',
  arc_planner: '卷纲', // legacy filename
}

const ENTITY_TAB_LABELS = {
  characters: '人物',
  items: '物品',
  locations: '地点',
}

function entityCardKey(e) {
  return e?.slug || e?.id || e?.name || ''
}

function plotCardKey(p) {
  return p?.slug || p?.id || p?.title || ''
}

function formatEntityCard(e, emptyLabel = '设定卡') {
  if (!e) return `（暂无${emptyLabel}）`
  const mark = e.complete ? '' : '（待补全）'
  const gaps = e.gaps?.length ? `\n> 缺口：${e.gaps.join('、')}\n` : ''
  return `${e.markdown || `# ${e.name}`}${gaps}${mark ? `\n${mark}` : ''}`
}

function formatPlotCard(p) {
  if (!p) {
    return (
      '（暂无剧情卡）\n\n'
      + '剧情卡服务于大纲某卷的全部或局部情节，须含：概览、剧情走向、出场人物/物品/设定。\n'
      + '节奏：剧情收束 → 衔接章 → 下一剧情卡。\n'
      + '可说「第一幕局部剧情卡：…」'
    )
  }
  const joinList = (arr) => (arr?.length ? arr.join('、') : '—')
  const mark = p.complete ? '' : '（待补全）'
  const gaps = p.gaps?.length ? `\n> 缺口：${p.gaps.join('、')}` : ''
  const scope = p.scope === 'volume' ? '整卷' : '卷内局部'
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
  const [library, setLibrary] = useState([])
  const [novelRecord, setNovelRecord] = useState(null)
  const [project, setProject] = useState(initialProject)
  const [preview, setPreview] = useState(null)
  const [selectedChapter, setSelectedChapter] = useState(() => initialWorkspace.selectedChapter || 1)
  const [readerTab, setReaderTab] = useState('draft')
  const [readerCardKey, setReaderCardKey] = useState('')
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
    } else if (data.preview?.plots?.length && !data.preview?.chapters?.length) {
      setReaderTab('plots')
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
      setReaderTab(opts.readerTab)
    }
    const lib = await api(`/library/${name}`)
    if (!lib.error) setNovelRecord(lib.novel)
    refreshLibrary()
  }, [refreshLibrary])

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
  const artifactTabs = Object.keys(preview?.artifacts || {}).filter((k) => {
    if (k === 'world_architect' && preview?.artifacts?.bible) return false
    if (
      k === 'master_outline'
      || k === 'master_planner'
      || k === 'story_outline'
      || k === 'relation_graph'
      || k === 'nomenclature'
    ) {
      return false
    }
    return true
  })
  const entities = preview?.entities || {}
  const plots = sortPlotsByProgress(preview?.plots || [])
  const entityGaps = preview?.entity_gaps || []

  const readerCardList = (() => {
    if (readerTab === 'plots') {
      return plots.map((p) => ({
        key: plotCardKey(p),
        label: p.title || '未命名',
        complete: !!p.complete,
        raw: p,
      }))
    }
    if (readerTab.startsWith('ent:')) {
      const group = readerTab.slice(4)
      return (entities[group] || []).map((e) => ({
        key: entityCardKey(e),
        label: e.name || '未命名',
        complete: !!e.complete,
        raw: e,
      }))
    }
    return []
  })()

  const selectedReaderCard = readerCardList.find((c) => c.key === readerCardKey)?.raw
    || readerCardList[0]?.raw
    || null

  const readerCardKeysSig = readerCardList.map((c) => c.key).join('\0')

  useEffect(() => {
    if (!readerCardKeysSig) {
      if (readerCardKey) setReaderCardKey('')
      return
    }
    const keys = readerCardKeysSig.split('\0')
    if (!keys.includes(readerCardKey)) {
      setReaderCardKey(keys[0])
    }
  }, [readerTab, readerCardKeysSig, readerCardKey])

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
    if (readerTab === 'plots') {
      if (!readerCardList.length) return formatPlotCard(null)
      return formatPlotCard(selectedReaderCard)
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

  useEffect(() => {
    const el = readerRef.current
    if (!el || readerEditing) return
    const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 160
    if (nearBottom) el.scrollTop = el.scrollHeight
  }, [readerContent, readerEditing])

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
      id: 'plots',
      label: '剧情卡',
      show: Boolean(plots.length),
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
    setReaderEditText(readerContent || '')
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
    const activeCardKey = readerCardList.length
      ? (readerCardKey || readerCardList[0]?.key || '')
      : ''
    const res = await api(`/projects/${encodeURIComponent(project)}/content`, {
      method: 'PUT',
      body: JSON.stringify({
        tab: readerTab,
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
    setReaderTab(id)
    setReaderCardKey('')
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
        </section>

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
                {chapterList.length > 0 && selectedChapter > 0 && (
                  <span className="reader-ch-title">
                    第{selectedChapter}章
                    {chapterData?.title ? ` · ${chapterData.title}` : ''}
                  </span>
                )}
                <div className="reader-tabs">
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
              {readerCardList.length > 0 && (
                <div className="reader-card-strip" role="tablist" aria-label="设定卡列表">
                  {readerCardList.map((c) => {
                    const active = (readerCardKey || readerCardList[0]?.key) === c.key
                    return (
                      <button
                        key={c.key}
                        type="button"
                        role="tab"
                        aria-selected={active}
                        className={active ? 'active' : ''}
                        onClick={() => switchReaderCard(c.key)}
                        title={c.complete ? c.label : `${c.label}（待补全）`}
                      >
                        {c.label}
                        {!c.complete ? <span className="reader-card-gap">·</span> : null}
                      </button>
                    )
                  })}
                </div>
              )}
              <div className={`reader-body${readerEditing ? ' reader-body-editing' : ''}`} ref={readerRef}>
                {readerEditing ? (
                  <textarea
                    className="reader-editor"
                    value={readerEditText}
                    onChange={(e) => setReaderEditText(e.target.value)}
                    spellCheck={false}
                    aria-label="编辑阅读内容"
                  />
                ) : (
                  <pre>{readerContent || '暂无内容'}</pre>
                )}
              </div>
            </>
          )}
        </section>

        <NovelXChat
          project={project}
          sendRef={chatSendRef}
          onSetupGateChange={setChatSetupGateOpen}
          onPreviewRefresh={(name, opts) => {
            if (name) refreshPreview(name, opts)
          }}
        />
        </div>
      </div>
    </div>
  )
}
