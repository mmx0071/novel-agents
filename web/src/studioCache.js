const STORAGE_KEY = 'novel-agents-studio-v2'

export const DRAFT_WORKSPACE_KEY = '__draft__'

function projectSessionId(projectId) {
  if (!projectId) return 'novel__draft'
  return `novel__${projectId}`
}

export function emptyWorkspace(projectId = '') {
  return {
    studioSessionId: projectSessionId(projectId),
    chatMessages: [],
    run: null,
    batchId: null,
    selectedChapter: null,
  }
}

export function migrateCache(raw) {
  if (!raw) {
    return { activeProject: '', projectSessions: {} }
  }
  if (raw.projectSessions) {
    return {
      activeProject: raw.activeProject || '',
      projectSessions: raw.projectSessions || {},
    }
  }
  const key = raw.project || DRAFT_WORKSPACE_KEY
  return {
    activeProject: raw.project || '',
    projectSessions: {
      [key]: {
        studioSessionId: raw.studioSessionId || projectSessionId(raw.project),
        chatMessages: raw.chatMessages || [],
        run: raw.run || null,
        batchId: raw.batchId || null,
        selectedChapter: raw.selectedChapter || 1,
      },
    },
  }
}

export function loadStudioCache() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return migrateCache(null)
    return migrateCache(JSON.parse(raw))
  } catch {
    return migrateCache(null)
  }
}

export function saveStudioCache(payload) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(payload))
  } catch {
    /* quota exceeded — ignore */
  }
}

function messagesForCache(messages) {
  return (messages || [])
    .filter((m) => m && !m.transient && !String(m.id || '').startsWith('think-'))
    .slice(-150)
}

export function snapshotWorkspace({
  project,
  studioSessionId,
  chatMessages,
  run,
  batchId,
  selectedChapter,
}) {
  const projectId = project || ''
  return {
    studioSessionId: studioSessionId || projectSessionId(projectId),
    chatMessages: messagesForCache(chatMessages),
    run: run
      ? {
          run_id: run.run_id,
          status: run.status,
          chapter_number: run.chapter_number,
          project: run.project,
          message: run.message,
        }
      : null,
    batchId: batchId || null,
    selectedChapter: selectedChapter || 1,
  }
}

export function workspaceKey(projectId) {
  return projectId || DRAFT_WORKSPACE_KEY
}
