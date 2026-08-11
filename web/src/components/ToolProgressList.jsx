import { stepLabelZh } from './toolLabels'

/**
 * Nested step list parsed from tool ▶/✓ streams.
 * Only the tip running step may spin — never historical ▶ rows.
 */
export default function ToolProgressList({ entries, live = false }) {
  const list = Array.isArray(entries) ? entries : []
  if (!list.length) return null

  let tipRunning = -1
  if (live) {
    for (let i = list.length - 1; i >= 0; i -= 1) {
      const e = list[i]
      if (e.status !== 'running') continue
      // Chapter milestones are macro todos — never the micro spinner tip.
      if (/^第\d+章/.test(String(e.name || '').trim())) continue
      tipRunning = i
      break
    }
  }

  return (
    <ul className={`nx-prog${live ? ' is-live' : ''}`} aria-label="执行步骤">
      {list.map((e, i) => {
        const spinning = i === tipRunning
        const label = stepLabelZh(e.name) || e.name
        return (
          <li
            key={`${e.kind}:${e.name}:${i}`}
            className={`nx-prog-item nx-prog-${e.kind} nx-prog-${e.status || 'note'}`}
          >
            <span className="nx-prog-mark" aria-hidden="true">{markFor(e, spinning)}</span>
            <span className="nx-prog-main">
              <span className="nx-prog-name">{label}</span>
              {e.summary ? (
                <span className="nx-prog-summary"> · {e.summary}</span>
              ) : null}
              {spinning && e.live ? (
                <span className="nx-prog-live"> · {e.live}</span>
              ) : null}
            </span>
            {spinning ? (
              <span className="nx-toolcell-spin" aria-label="进行中" />
            ) : null}
          </li>
        )
      })}
    </ul>
  )
}

function markFor(e, spinning) {
  if (spinning) return '▶'
  if (e.mark) return e.mark
  if (e.status === 'done') return '✓'
  if (e.status === 'failed') return '✕'
  if (e.status === 'paused') return '⏸'
  if (e.status === 'running') return '▶'
  return '•'
}
