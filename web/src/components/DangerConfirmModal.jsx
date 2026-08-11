import { useEffect, useId, useState } from 'react'
import { createPortal } from 'react-dom'

/**
 * In-app danger confirm. When `confirmText` is set, user must type it exactly.
 * Portaled to document.body so parent overflow/stacking cannot swallow it.
 */
export default function DangerConfirmModal({
  open,
  title = '确认删除',
  message = '',
  confirmText = '',
  confirmLabel = '确认删除',
  cancelLabel = '取消',
  busy = false,
  onCancel,
  onConfirm,
}) {
  const [typed, setTyped] = useState('')
  const titleId = useId()
  const inputId = useId()
  const needMatch = Boolean(confirmText)
  const matched = !needMatch || typed.trim() === String(confirmText).trim()

  useEffect(() => {
    if (!open) {
      setTyped('')
      return undefined
    }
    setTyped('')
    const onKey = (e) => {
      if (e.key === 'Escape' && !busy) onCancel?.()
    }
    window.addEventListener('keydown', onKey)
    const prevOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      window.removeEventListener('keydown', onKey)
      document.body.style.overflow = prevOverflow
    }
  }, [open, busy, onCancel])

  if (!open || typeof document === 'undefined') return null

  return createPortal(
    <div className="danger-confirm-root" role="dialog" aria-modal="true" aria-labelledby={titleId}>
      <button
        type="button"
        className="danger-confirm-backdrop"
        aria-label="取消"
        disabled={busy}
        onClick={() => {
          if (!busy) onCancel?.()
        }}
      />
      <div className="danger-confirm-card">
        <h2 className="danger-confirm-title" id={titleId}>{title}</h2>
        {message ? (
          <p className="danger-confirm-message">{message}</p>
        ) : null}
        {needMatch ? (
          <label className="danger-confirm-field" htmlFor={inputId}>
            <span>
              请输入
              <strong> {confirmText} </strong>
              以确认
            </span>
            <input
              id={inputId}
              type="text"
              value={typed}
              autoFocus
              disabled={busy}
              placeholder={confirmText}
              autoComplete="off"
              onChange={(e) => setTyped(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && matched && !busy) {
                  e.preventDefault()
                  onConfirm?.()
                }
              }}
            />
          </label>
        ) : null}
        <div className="danger-confirm-actions">
          <button
            type="button"
            className="btn-ghost btn-inline"
            disabled={busy}
            onClick={() => onCancel?.()}
          >
            {cancelLabel}
          </button>
          <button
            type="button"
            className="danger-confirm-ok"
            disabled={busy || !matched}
            onClick={() => onConfirm?.()}
          >
            {busy ? '处理中…' : confirmLabel}
          </button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
