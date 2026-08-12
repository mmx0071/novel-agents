/** Git-style line hunks for confirm UI (mirrors novelx-tools::mutation::text_hunk_diffs). */

const HUNK_LINE_CAP = 48

/**
 * @param {string} before
 * @param {string} after
 * @returns {{ before: string, after: string }[]}
 */
export function textHunkDiffs(before, after) {
  const b = String(before ?? '')
  const a = String(after ?? '')
  if (b === a) return []

  const bl = b.split('\n')
  const al = a.split('\n')
  // Drop trailing empty line from split when source ends with \n
  if (bl.length && bl[bl.length - 1] === '' && b.endsWith('\n')) bl.pop()
  if (al.length && al[al.length - 1] === '' && a.endsWith('\n')) al.pop()

  let pre = 0
  while (pre < bl.length && pre < al.length && bl[pre] === al[pre]) pre += 1

  let suf = 0
  while (
    suf < bl.length - pre
    && suf < al.length - pre
    && bl[bl.length - 1 - suf] === al[al.length - 1 - suf]
  ) {
    suf += 1
  }

  const bChangedEnd = bl.length - suf
  const aChangedEnd = al.length - suf
  const bStart = Math.max(0, pre - (pre > 0 ? 1 : 0))
  const aStart = Math.max(0, pre - (pre > 0 ? 1 : 0))
  const bEnd = Math.min(bl.length, bChangedEnd + (suf > 0 ? 1 : 0))
  const aEnd = Math.min(al.length, aChangedEnd + (suf > 0 ? 1 : 0))

  const bSlice = bStart < bEnd ? bl.slice(bStart, bEnd) : []
  const aSlice = aStart < aEnd ? al.slice(aStart, aEnd) : []

  const [bText, bTrunc] = joinCapped(bSlice, HUNK_LINE_CAP)
  const [aText, aTrunc] = joinCapped(aSlice, HUNK_LINE_CAP)

  return [{
    before: bTrunc ? `${bText}\n…（后续删减已省略）` : bText,
    after: aTrunc ? `${aText}\n…（后续新增已省略）` : aText,
  }]
}

function joinCapped(lines, cap) {
  if (!lines.length) return ['', false]
  if (lines.length <= cap) return [lines.join('\n'), false]
  return [lines.slice(0, cap).join('\n'), true]
}
