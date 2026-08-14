import MarkdownView from '../MarkdownView'
import { sectionBody, splitMarkdownByH2 } from '../../mdSections'
import LocationMapCanvas from './LocationMapCanvas'

const STATUS_ZH = {
  active: '在场',
  background: '背景',
  exited: '退场',
  consumed: '已消耗',
}

function LocationDetail({ entity }) {
  if (!entity) {
    return <p className="desk-muted">在地图上选择一个地点。</p>
  }
  const name = entity.name || '未命名'
  const status = String(entity.status || '').trim()
  const statusLabel = STATUS_ZH[status] || status
  const { sections } = splitMarkdownByH2(entity.markdown || '')
  const overview = sectionBody(sections, ['概述', 'Overview'])
  const factions = sectionBody(sections, ['此地势力', '势力', 'Factions'])
  const production = sectionBody(sections, ['产出资源', '产出', 'Production'])

  return (
    <div className="location-detail">
      <header className="location-detail-head">
        <h3>{name}</h3>
        <div className="location-badges">
          {statusLabel ? (
            <span className={`location-badge status-${status || 'active'}`}>{statusLabel}</span>
          ) : null}
          {!entity.complete ? <span className="location-badge is-gap">待补全</span> : null}
        </div>
      </header>
      <section className="location-detail-section">
        <h4>概述</h4>
        {overview ? (
          <MarkdownView source={overview} variant="reader" />
        ) : (
          <p className="desk-muted">（待补）</p>
        )}
      </section>
      <div className="location-detail-split">
        <section>
          <h4>此地势力</h4>
          {factions ? (
            <MarkdownView source={factions} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
        <section>
          <h4>产出资源</h4>
          {production ? (
            <MarkdownView source={production} variant="reader" />
          ) : (
            <p className="desk-muted">（待补）</p>
          )}
        </section>
      </div>
    </div>
  )
}

export default function LocationDesk({
  locations = [],
  selected,
  selectedKey = '',
  onSelect,
}) {
  if (!locations?.length) {
    return (
      <div className="location-desk desk-empty">
        <p>地图尚未展开。可在创作助手中补地点设定。</p>
      </div>
    )
  }

  return (
    <div className="location-desk" aria-label="地点地图与详情">
      <LocationMapCanvas
        locations={locations}
        selectedKey={selectedKey}
        onSelect={onSelect}
      />
      <LocationDetail entity={selected} />
    </div>
  )
}
