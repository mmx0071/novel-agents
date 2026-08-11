/** Icon toolbar for 状态 / 设置 — both open as side drawers. */

function IconStatus() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path
        fill="currentColor"
        d="M1 11.5a.5.5 0 0 1 .5-.5h2a.5.5 0 0 1 .5.5v3a.5.5 0 0 1-.5.5h-2a.5.5 0 0 1-.5-.5v-3zm4.5-5a.5.5 0 0 0-.5.5v8a.5.5 0 0 0 .5.5h2a.5.5 0 0 0 .5-.5v-8a.5.5 0 0 0-.5-.5h-2zm4-3a.5.5 0 0 0-.5.5v11a.5.5 0 0 0 .5.5h2a.5.5 0 0 0 .5-.5v-11a.5.5 0 0 0-.5-.5h-2zm4 6a.5.5 0 0 0-.5.5v5a.5.5 0 0 0 .5.5h2a.5.5 0 0 0 .5-.5v-5a.5.5 0 0 0-.5-.5h-2z"
      />
    </svg>
  )
}

function IconSettings() {
  return (
    <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <path
        fill="currentColor"
        d="M6.5 1.5h3l.35 1.4c.35.12.68.3.98.52l1.35-.5 1.5 2.6-1.1 1c.05.28.07.56.07.85s-.02.57-.07.85l1.1 1-1.5 2.6-1.35-.5a4 4 0 0 1-.98.52L9.5 14.5h-3l-.35-1.4a4 4 0 0 1-.98-.52l-1.35.5-1.5-2.6 1.1-1A4.2 4.2 0 0 1 3.35 8c0-.29.02-.57.07-.85l-1.1-1 1.5-2.6 1.35.5c.3-.22.63-.4.98-.52L6.5 1.5zM8 5.75A2.25 2.25 0 1 0 8 10.25 2.25 2.25 0 0 0 8 5.75z"
      />
    </svg>
  )
}

export default function StudioNavIcons({
  statusOpen = false,
  engineOpen = false,
  onOpenStatus,
  onOpenSettings,
  compact = false,
}) {
  return (
    <div
      className={`studio-nav-icons${compact ? ' is-compact' : ''}`}
      role="toolbar"
      aria-label="工作室导航"
    >
      <button
        type="button"
        className={`chat-icon-btn${statusOpen ? ' is-active' : ''}`}
        title="创作状态"
        aria-label="状态"
        aria-pressed={statusOpen}
        onClick={() => {
          if (typeof onOpenStatus === 'function') onOpenStatus()
        }}
      >
        <IconStatus />
      </button>
      <button
        type="button"
        className={`chat-icon-btn${engineOpen ? ' is-active' : ''}`}
        title="设置"
        aria-label="设置"
        aria-pressed={engineOpen}
        onClick={() => {
          if (typeof onOpenSettings === 'function') onOpenSettings()
        }}
      >
        <IconSettings />
      </button>
    </div>
  )
}
