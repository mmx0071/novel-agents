import { useEffect, useMemo, useRef } from 'react'

export default function SkillPopup({
  open,
  query,
  skills,
  onPick,
  onClose,
}) {
  const list = useMemo(() => {
    const q = (query || '').toLowerCase()
    return (skills || [])
      .filter((s) => {
        if (!q) return true
        return (
          s.name.toLowerCase().includes(q)
          || (s.description || '').toLowerCase().includes(q)
        )
      })
      .slice(0, 12)
  }, [skills, query])

  const ref = useRef(null)
  useEffect(() => {
    if (!open) return undefined
    const onKey = (e) => {
      if (e.key === 'Escape') onClose?.()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (!open) return null

  return (
    <div className="nx-skill-popup" ref={ref} role="listbox" aria-label="能力列表">
      <div className="nx-skill-popup-hint">能力 · 选中后插入 $name</div>
      {list.length === 0 ? (
        <div className="nx-skill-empty">无匹配能力</div>
      ) : (
        list.map((s) => (
          <button
            key={s.name}
            type="button"
            className="nx-skill-item"
            onClick={() => onPick(s)}
          >
            <span className="nx-skill-name">${s.name}</span>
            <span className="nx-skill-desc">{s.description}</span>
          </button>
        ))
      )}
    </div>
  )
}
