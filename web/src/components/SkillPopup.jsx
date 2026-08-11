import { useEffect, useMemo, useRef } from 'react'
import { skillLabelZh } from './toolLabels'

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
        const label = skillLabelZh(s.name).toLowerCase()
        return (
          s.name.toLowerCase().includes(q)
          || label.includes(q)
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
    <div className="nx-skill-popup" ref={ref} role="listbox" aria-label="常用动作">
      <div className="nx-skill-popup-hint">常用动作 · 选中后插入到输入框</div>
      {list.length === 0 ? (
        <div className="nx-skill-empty">没有匹配的动作</div>
      ) : (
        list.map((s) => (
          <button
            key={s.name}
            type="button"
            className="nx-skill-item"
            onClick={() => onPick(s)}
          >
            <span className="nx-skill-name">{skillLabelZh(s.name)}</span>
            <span className="nx-skill-desc">{s.description}</span>
          </button>
        ))
      )}
    </div>
  )
}
