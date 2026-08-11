import { useEffect, useId } from 'react'
import { createPortal } from 'react-dom'

/**
 * Centered create-novel dialog. Keeps the header chrome clear of form clutter.
 */
export default function NewNovelModal({
  open,
  title,
  genre,
  brief,
  mode,
  busy = false,
  onTitleChange,
  onGenreChange,
  onBriefChange,
  onModeChange,
  onCancel,
  onSubmit,
}) {
  const titleId = useId()
  const nameId = useId()
  const canSubmit = Boolean(String(title || '').trim()) && !busy

  useEffect(() => {
    if (!open) return undefined
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
    <div className="new-novel-modal-root" role="dialog" aria-modal="true" aria-labelledby={titleId}>
      <button
        type="button"
        className="new-novel-modal-backdrop"
        aria-label="取消"
        disabled={busy}
        onClick={() => {
          if (!busy) onCancel?.()
        }}
      />
      <div className="new-novel-modal">
        <header className="new-novel-modal-head">
          <h2 id={titleId}>新建小说</h2>
          <p>填好书名后，创作助手会帮你立项并继续往下走。</p>
        </header>
        <form
          className="new-novel-modal-form"
          onSubmit={(e) => {
            e.preventDefault()
            if (canSubmit) onSubmit?.()
          }}
        >
          <label className="new-novel-field" htmlFor={nameId}>
            <span>书名</span>
            <input
              id={nameId}
              type="text"
              value={title}
              onChange={(e) => onTitleChange?.(e.target.value)}
              placeholder="例如：雨夜来信"
              autoFocus
              disabled={busy}
              autoComplete="off"
            />
          </label>
          <label className="new-novel-field">
            <span>题材 <em>可选</em></span>
            <input
              type="text"
              value={genre}
              onChange={(e) => onGenreChange?.(e.target.value)}
              placeholder="悬疑 / 言情 / 奇幻…"
              disabled={busy}
              autoComplete="off"
            />
          </label>
          <fieldset className="new-novel-mode" disabled={busy}>
            <legend>模式</legend>
            <div className="new-novel-mode-options" role="radiogroup" aria-label="创作模式">
              <label className={mode === 'longform' ? 'is-active' : ''}>
                <input
                  type="radio"
                  name="new-novel-mode"
                  value="longform"
                  checked={mode === 'longform'}
                  onChange={() => onModeChange?.('longform')}
                />
                <span className="new-novel-mode-title">长篇小说</span>
                <span className="new-novel-mode-desc">分章连载，适合长篇故事</span>
              </label>
              <label className={mode === 'short_drama' ? 'is-active' : ''}>
                <input
                  type="radio"
                  name="new-novel-mode"
                  value="short_drama"
                  checked={mode === 'short_drama'}
                  onChange={() => onModeChange?.('short_drama')}
                />
                <span className="new-novel-mode-title">短剧剧本</span>
                <span className="new-novel-mode-desc">按集推进，适合短剧脚本</span>
              </label>
            </div>
          </fieldset>
          <label className="new-novel-field">
            <span>灵感 <em>可选</em></span>
            <textarea
              value={brief}
              onChange={(e) => onBriefChange?.(e.target.value)}
              placeholder="一两句卖点、人物或开场设定"
              rows={3}
              disabled={busy}
            />
          </label>
          <div className="new-novel-modal-actions">
            <button
              type="button"
              className="btn-ghost btn-inline"
              disabled={busy}
              onClick={() => onCancel?.()}
            >
              取消
            </button>
            <button
              type="submit"
              className="btn-primary btn-inline"
              disabled={!canSubmit}
            >
              {busy ? '创建中…' : '开始创作'}
            </button>
          </div>
        </form>
      </div>
    </div>,
    document.body,
  )
}
