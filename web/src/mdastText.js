/** Flatten mdast / hast-ish node trees to plain text (for chat table → kv). */

export function mdastText(node) {
  if (node == null) return ''
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(mdastText).join('')
  if (typeof node !== 'object') return ''
  if (typeof node.value === 'string') return node.value
  if (Array.isArray(node.children)) return node.children.map(mdastText).join('')
  return ''
}

/**
 * If a GFM table is a 2-column key/value grid (common in status reports),
 * return [{ key, value }] rows (skipping header). Else null.
 */
export function tableAsKvRows(node) {
  if (!node || node.type !== 'table' || !Array.isArray(node.children)) return null
  const rows = node.children.filter((r) => r && r.type === 'tableRow')
  if (rows.length < 2) return null
  const cellsOf = (row) => (row.children || []).filter((c) => c && c.type === 'tableCell')
  const width = Math.max(...rows.map((r) => cellsOf(r).length), 0)
  if (width !== 2) return null
  // Skip header row when every body row looks like label → value.
  const body = rows.slice(1)
  if (!body.length) return null
  const out = body.map((row) => {
    const cells = cellsOf(row)
    return {
      key: mdastText(cells[0]).trim(),
      value: mdastText(cells[1]).trim(),
    }
  }).filter((r) => r.key || r.value)
  return out.length ? out : null
}
