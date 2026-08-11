import { stepLabelZh } from './toolLabels'
import {
  isAgentActive,
  lifecycleLabelZh,
  sortSubAgentsForRail,
} from '../subAgents'

/**
 * Compact SubAgent list — background visibility; click opens dedicated page.
 * Does not auto-open when agents spawn.
 */
export default function SubAgentRail({ agents, openThreadId, onOpen, collapsed = false }) {
  const list = sortSubAgentsForRail(agents).slice(0, 12)
  if (!list.length) return null

  const running = list.filter((a) => isAgentActive(a.lifecycle)).length

  if (collapsed) {
    return (
      <div className="subagent-rail subagent-rail-collapsed" title="协作角色">
        <span className="subagent-rail-pulse-wrap">
          {running > 0 ? <span className="subagent-rail-pulse" aria-label="进行中" /> : null}
          <span className="subagent-rail-count">{running || list.length}</span>
        </span>
      </div>
    )
  }

  return (
    <div className="subagent-rail" aria-label="协作角色">
      <div className="subagent-rail-head">
        <span className="subagent-rail-title">协作角色</span>
        <span className="subagent-rail-meta">
          {running > 0 ? `${running} 运行中` : `${list.length} 个`}
          <span className="subagent-rail-hint">后台任务 · 点开查看</span>
        </span>
      </div>
      <ul className="subagent-rail-list">
        {list.map((a) => {
          const active = isAgentActive(a.lifecycle)
          const selected = openThreadId && openThreadId === a.threadId
          const label = stepLabelZh(a.role) || a.role || '协作'
          return (
            <li key={a.threadId}>
              <button
                type="button"
                className={[
                  'subagent-rail-item',
                  active ? 'is-active' : '',
                  selected ? 'is-selected' : '',
                  a.lifecycle === 'failed' ? 'is-failed' : '',
                ].filter(Boolean).join(' ')}
                onClick={() => onOpen?.(a)}
                title={a.summary || label}
              >
                <span className={`subagent-rail-dot life-${a.lifecycle}`} aria-hidden="true" />
                <span className="subagent-rail-role">{label}</span>
                <span className="subagent-rail-life">{lifecycleLabelZh(a.lifecycle)}</span>
              </button>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
