/** Deep-link the writing desk from dsh (`?project=&chapter=&tab=`). Genre-neutral. */

export function deskQueryFromSearch(search) {
  const raw = String(search || '')
  const q = new URLSearchParams(raw.startsWith('?') ? raw.slice(1) : raw)
  const project = String(q.get('project') || '').trim()
  const chapter = Number(q.get('chapter')) || 0
  const tab = String(q.get('tab') || '').trim()
  return { project, chapter: chapter > 0 ? chapter : 0, tab }
}

export function deskSearch({ project, chapter, tab } = {}) {
  const q = new URLSearchParams()
  const dir = String(project || '').trim()
  if (dir) q.set('project', dir)
  const n = Number(chapter) || 0
  if (n > 0) q.set('chapter', String(n))
  const t = String(tab || '').trim()
  if (t) q.set('tab', t === 'plots' ? 'arcs' : t)
  const s = q.toString()
  return s ? `?${s}` : ''
}

export function deskPageUrl(origin, opts) {
  const base = String(origin || '').replace(/\/$/, '')
  const search = deskSearch(opts)
  return search ? `${base}/${search}` : `${base}/`
}
