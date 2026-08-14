import { useMemo } from 'react'
import {
  parseLocationSubzones,
  sectionBody,
  splitMarkdownByH2,
  stableUnit,
} from '../../mdSections'

function entityKey(e) {
  return e?.slug || e?.id || e?.name || ''
}

function lightLevel(loc) {
  const status = String(loc?.status || '').toLowerCase()
  if (status === 'exited' || status === 'consumed') return 'faded'
  if (loc?.complete === true || status === 'active') return 'lit'
  return 'dim'
}

function layoutNodes(locations) {
  const parents = (locations || []).filter((e) => entityKey(e))
  if (!parents.length) return []

  const cx = 50
  const cy = 48
  const R = parents.length === 1 ? 0 : Math.min(36, 14 + parents.length * 3)
  const nodes = []

  parents.forEach((loc, i) => {
    const key = entityKey(loc)
    const seed = stableUnit(key)
    const angle = parents.length === 1
      ? 0
      : ((i / parents.length) * Math.PI * 2) + seed * 0.35
    const x = cx + R * Math.cos(angle - Math.PI / 2)
    const y = cy + R * Math.sin(angle - Math.PI / 2) * 0.88
    nodes.push({
      key,
      kind: 'parent',
      label: loc.name || key,
      x,
      y,
      light: lightLevel(loc),
      loc,
    })

    const { sections } = splitMarkdownByH2(loc.markdown || '')
    const overview = sectionBody(sections, ['概述', 'Overview'])
    const zones = parseLocationSubzones(overview).slice(0, 4)
    zones.forEach((z, zi) => {
      const zKey = `${key}::${z.name}`
      const zSeed = stableUnit(zKey)
      const za = angle + (zi - (zones.length - 1) / 2) * 0.42
      const zr = R + 12 + zSeed * 4
      nodes.push({
        key: zKey,
        kind: 'zone',
        parentKey: key,
        label: z.name,
        x: cx + zr * Math.cos(za - Math.PI / 2),
        y: cy + zr * Math.sin(za - Math.PI / 2) * 0.88,
        light: lightLevel(loc) === 'lit' ? 'dim' : 'dim',
        loc: null,
        selectKey: key,
      })
    })
  })

  return nodes
}

export default function LocationMapCanvas({
  locations = [],
  selectedKey = '',
  onSelect,
}) {
  const nodes = useMemo(() => layoutNodes(locations), [locations])

  if (!locations?.length) {
    return (
      <div className="location-map empty" aria-label="地点地图">
        <p>地图尚未展开。补全地点设定后，节点会在此点亮。</p>
      </div>
    )
  }

  return (
    <div className="location-map" aria-label="地点地图">
      <svg className="location-map-svg" viewBox="0 0 100 100" role="img">
        <defs>
          <radialGradient id="mapGlow" cx="50%" cy="50%" r="55%">
            <stop offset="0%" stopColor="rgba(180, 160, 120, 0.14)" />
            <stop offset="100%" stopColor="rgba(18, 22, 31, 0)" />
          </radialGradient>
        </defs>
        <rect width="100" height="100" fill="url(#mapGlow)" />
        {nodes.filter((n) => n.kind === 'zone').map((n) => {
          const parent = nodes.find((p) => p.key === n.parentKey)
          if (!parent) return null
          return (
            <line
              key={`e-${n.key}`}
              x1={parent.x}
              y1={parent.y}
              x2={n.x}
              y2={n.y}
              className="location-map-edge"
            />
          )
        })}
        {nodes.map((n) => {
          const active = selectedKey === (n.selectKey || n.key)
          const r = n.kind === 'zone' ? 1.6 : 2.8
          return (
            <g
              key={n.key}
              className={[
                'location-map-node',
                `is-${n.light}`,
                n.kind === 'zone' ? 'is-zone' : 'is-parent',
                active ? 'is-active' : '',
              ].filter(Boolean).join(' ')}
              transform={`translate(${n.x} ${n.y})`}
              onClick={() => onSelect?.(n.selectKey || n.key)}
              style={{ cursor: 'pointer' }}
            >
              <circle r={r + 1.2} className="location-map-halo" />
              <circle r={r} className="location-map-dot" />
              <text
                y={n.kind === 'zone' ? 4.2 : 5.5}
                textAnchor="middle"
                className="location-map-label"
              >
                {n.label.length > 8 ? `${n.label.slice(0, 8)}…` : n.label}
              </text>
            </g>
          )
        })}
      </svg>
      <p className="location-map-hint">点击节点查看地点详情 · 补全后节点更亮</p>
    </div>
  )
}
