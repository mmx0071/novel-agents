const GROUP_TAB = {
  characters: 'ent:characters',
  character: 'ent:characters',
  人物: 'ent:characters',
  items: 'ent:items',
  item: 'ent:items',
  物品: 'ent:items',
  locations: 'ent:locations',
  location: 'ent:locations',
  地点: 'ent:locations',
}

const GROUP_LABEL = {
  'ent:characters': '人物',
  'ent:items': '物品',
  'ent:locations': '地点',
}

/** Parse gaps like `[characters]「甲」待补全…` or plain strings. */
export function parseGapLine(raw) {
  const text = String(raw || '').trim()
  if (!text) return null
  const m = text.match(/^\[([^\]]+)\]\s*[「"']?([^」"']+)[」"']?\s*(.*)$/)
  if (m) {
    const groupRaw = m[1].trim().toLowerCase()
    const tab = GROUP_TAB[groupRaw] || GROUP_TAB[m[1].trim()] || ''
    return {
      raw: text,
      group: m[1].trim(),
      name: m[2].trim(),
      rest: (m[3] || '').trim(),
      tab,
      jumpable: Boolean(tab),
    }
  }
  return { raw: text, group: '', name: '', rest: text, tab: '', jumpable: false }
}

export default function EntityGapsDesk({ gaps = [], onJump }) {
  const rows = (gaps || []).map(parseGapLine).filter(Boolean)

  if (!rows.length) {
    return (
      <div className="gaps-desk desk-empty">
        <p>设定卡与世界观暂无明显缺口。</p>
      </div>
    )
  }

  return (
    <div className="gaps-desk" aria-label="设定缺口待办">
      <header className="gaps-desk-head">
        <h3>设定缺口</h3>
        <p className="gaps-desk-note">软提示，不妨碍继续写。点条目可跳到对应设定卡。</p>
      </header>
      <ul className="gaps-list">
        {rows.map((row, i) => {
          const label = row.tab ? GROUP_LABEL[row.tab] : row.group
          const title = row.name || row.raw
          const canJump = row.jumpable && typeof onJump === 'function'
          return (
            <li key={`${i}-${row.raw.slice(0, 40)}`}>
              {canJump ? (
                <button
                  type="button"
                  className="gaps-item"
                  onClick={() => onJump({ tab: row.tab, name: row.name })}
                >
                  {label ? <span className="gaps-group">{label}</span> : null}
                  <span className="gaps-title">{title}</span>
                  {row.rest ? <span className="gaps-rest">{row.rest}</span> : null}
                </button>
              ) : (
                <div className="gaps-item is-static">
                  <span className="gaps-title">{row.raw}</span>
                </div>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
