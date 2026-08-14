/** Map Host SSE events onto the writing desk (no chat). */

export function readerTabFromHostEvent(ev) {
  const tab = String(ev?.readerTab || '').trim()
  if (tab) return tab
  return 'volume'
}

export function applyHostDeskEvent(ev, handlers) {
  if (!ev || typeof ev !== 'object') return
  const kind = String(ev.kind || '')
  const chapter = Number(ev.chapter) || 0
  const tab = readerTabFromHostEvent(ev)
  if (typeof handlers.onFocus === 'function' && (chapter > 0 || tab)) {
    handlers.onFocus({ readerTab: tab, chapter })
  }
  if (kind === 'discarded' && typeof handlers.onClearPatches === 'function') {
    handlers.onClearPatches()
  }
  if (typeof handlers.onRefresh === 'function') {
    handlers.onRefresh(String(ev.project || ''))
  }
}
