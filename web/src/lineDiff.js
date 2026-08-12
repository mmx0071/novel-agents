/** Line-level diff for writing-desk inline −/+ (Cursor-style hunks + context). */

const HUNK_CONTEXT = 3

/**
 * @param {string} text
 * @returns {string[]}
 */
export function splitLines(text) {
  const s = String(text ?? '')
  if (!s) return []
  const lines = s.split('\n')
  if (lines.length && lines[lines.length - 1] === '' && s.endsWith('\n')) {
    lines.pop()
  }
  return lines
}

/**
 * @typedef {{ type: 'equal' | 'del' | 'ins', text: string }} DiffOp
 * @typedef {{ kind: 'ctx' | 'del' | 'ins', text: string }} HunkLine
 * @typedef {{ type: 'equal', text: string }} EqualBlock
 * @typedef {{
 *   type: 'hunk',
 *   id: string,
 *   dels: string[],
 *   ins: string[],
 *   lines: HunkLine[],
 *   changeKind: 'insert' | 'delete' | 'replace',
 * }} HunkBlock
 * @typedef {EqualBlock | HunkBlock} DiffBlock
 */

/**
 * Strip common prefix/suffix, LCS-diff only the middle.
 * @param {string} before
 * @param {string} after
 * @returns {DiffOp[]}
 */
export function lineDiffOps(before, after) {
  const a = splitLines(before)
  const b = splitLines(after)
  if (!a.length && !b.length) return []
  if (!a.length) return b.map((text) => ({ type: 'ins', text }))
  if (!b.length) return a.map((text) => ({ type: 'del', text }))

  let pre = 0
  while (pre < a.length && pre < b.length && a[pre] === b[pre]) pre += 1

  let suf = 0
  while (
    suf < a.length - pre
    && suf < b.length - pre
    && a[a.length - 1 - suf] === b[b.length - 1 - suf]
  ) {
    suf += 1
  }

  /** @type {DiffOp[]} */
  const ops = []
  for (let i = 0; i < pre; i += 1) ops.push({ type: 'equal', text: a[i] })

  const midA = a.slice(pre, a.length - suf)
  const midB = b.slice(pre, b.length - suf)
  ops.push(...lcsLineOps(midA, midB))

  for (let i = a.length - suf; i < a.length; i += 1) {
    ops.push({ type: 'equal', text: a[i] })
  }
  return ops
}

/**
 * @param {string[]} a
 * @param {string[]} b
 * @returns {DiffOp[]}
 */
function lcsLineOps(a, b) {
  if (!a.length && !b.length) return []
  if (!a.length) return b.map((text) => ({ type: 'ins', text }))
  if (!b.length) return a.map((text) => ({ type: 'del', text }))

  const n = a.length
  const m = b.length
  if (n * m > 1_500_000) {
    return [
      ...a.map((text) => ({ type: 'del', text })),
      ...b.map((text) => ({ type: 'ins', text })),
    ]
  }

  /** @type {Int32Array[]} */
  const dp = Array.from({ length: n + 1 }, () => new Int32Array(m + 1))
  for (let i = n - 1; i >= 0; i -= 1) {
    for (let j = m - 1; j >= 0; j -= 1) {
      if (a[i] === b[j]) dp[i][j] = dp[i + 1][j + 1] + 1
      else dp[i][j] = Math.max(dp[i + 1][j], dp[i][j + 1])
    }
  }

  /** @type {DiffOp[]} */
  const ops = []
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      ops.push({ type: 'equal', text: a[i] })
      i += 1
      j += 1
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      ops.push({ type: 'del', text: a[i] })
      i += 1
    } else {
      ops.push({ type: 'ins', text: b[j] })
      j += 1
    }
  }
  while (i < n) {
    ops.push({ type: 'del', text: a[i] })
    i += 1
  }
  while (j < m) {
    ops.push({ type: 'ins', text: b[j] })
    j += 1
  }
  return ops
}

/**
 * Cursor-style: change hunks carry ± lines + surrounding context.
 * Unchanged bulk remains equal (normal markdown).
 * @param {DiffOp[]} ops
 * @param {{ context?: number }} [opts]
 * @returns {DiffBlock[]}
 */
export function groupDiffBlocks(ops, opts = {}) {
  const context = Number.isFinite(opts.context) ? Math.max(0, opts.context) : HUNK_CONTEXT
  const list = Array.isArray(ops) ? ops : []
  if (!list.length) return []

  /** @type {Array<{ start: number, end: number }>} */
  const spans = []
  let i = 0
  while (i < list.length) {
    if (list[i].type === 'equal') {
      i += 1
      continue
    }
    const start = i
    while (i < list.length && list[i].type !== 'equal') i += 1
    spans.push({ start, end: i })
  }
  if (!spans.length) {
    return [{ type: 'equal', text: list.map((o) => o.text).join('\n') }]
  }

  /** @type {DiffBlock[]} */
  const blocks = []
  let cursor = 0
  let hunkIdx = 0

  for (const span of spans) {
    const ctxStart = Math.max(cursor, span.start - context)
    const ctxEnd = Math.min(list.length, span.end + context)

    // Equal text before this hunk's context window.
    if (cursor < ctxStart) {
      const text = list.slice(cursor, ctxStart).map((o) => o.text).join('\n')
      if (text !== '' || ctxStart > cursor) {
        blocks.push({ type: 'equal', text })
      }
    }

    hunkIdx += 1
    /** @type {HunkLine[]} */
    const lines = []
    const dels = []
    const ins = []
    for (let k = ctxStart; k < span.start; k += 1) {
      lines.push({ kind: 'ctx', text: list[k].text })
    }
    for (let k = span.start; k < span.end; k += 1) {
      const op = list[k]
      if (op.type === 'del') {
        lines.push({ kind: 'del', text: op.text })
        dels.push(op.text)
      } else if (op.type === 'ins') {
        lines.push({ kind: 'ins', text: op.text })
        ins.push(op.text)
      }
    }
    for (let k = span.end; k < ctxEnd; k += 1) {
      if (list[k].type === 'equal') {
        lines.push({ kind: 'ctx', text: list[k].text })
      }
    }

    let changeKind = 'replace'
    if (!dels.length && ins.length) changeKind = 'insert'
    else if (dels.length && !ins.length) changeKind = 'delete'

    blocks.push({
      type: 'hunk',
      id: `h${hunkIdx}`,
      dels,
      ins,
      lines,
      changeKind,
    })
    cursor = ctxEnd
  }

  if (cursor < list.length) {
    blocks.push({
      type: 'equal',
      text: list.slice(cursor).map((o) => o.text).join('\n'),
    })
  }
  return blocks
}

/**
 * @param {HunkBlock} hunk
 * @returns {string}
 */
export function hunkSummary(hunk) {
  const delN = hunk?.dels?.length || 0
  const insN = hunk?.ins?.length || 0
  if (hunk?.changeKind === 'insert' || (!delN && insN)) return `插入 +${insN}`
  if (hunk?.changeKind === 'delete' || (delN && !insN)) return `删除 −${delN}`
  return `替换 −${delN} / +${insN}`
}

/**
 * Build document after applying only selected hunks.
 * @param {DiffBlock[]} blocks
 * @param {Set<string> | string[]} selectedIds
 * @returns {string}
 */
export function buildPartialAfter(blocks, selectedIds) {
  const selected = selectedIds instanceof Set
    ? selectedIds
    : new Set(Array.isArray(selectedIds) ? selectedIds : [])
  const lines = []
  for (const block of blocks || []) {
    if (block.type === 'equal') {
      if (block.text !== '') lines.push(block.text)
      continue
    }
    const takeNew = selected.has(block.id)
    const hunkLines = Array.isArray(block.lines) ? block.lines : []
    if (hunkLines.length) {
      for (const hl of hunkLines) {
        if (hl.kind === 'ctx') lines.push(hl.text)
        else if (hl.kind === 'ins' && takeNew) lines.push(hl.text)
        else if (hl.kind === 'del' && !takeNew) lines.push(hl.text)
      }
      continue
    }
    const chunk = takeNew ? block.ins : block.dels
    if (chunk?.length) lines.push(chunk.join('\n'))
  }
  return lines.join('\n')
}

/**
 * @param {DiffBlock[]} blocks
 * @returns {string}
 */
export function buildFullAfter(blocks) {
  const all = (blocks || [])
    .filter((b) => b.type === 'hunk')
    .map((b) => b.id)
  return buildPartialAfter(blocks, all)
}

/**
 * @param {string} doc
 * @param {Array<{ before?: string, after?: string }>} hunks
 * @returns {string}
 */
export function applyTextHunks(doc, hunks) {
  let out = String(doc ?? '')
  for (const h of hunks || []) {
    const b = String(h?.before ?? '')
    const a = String(h?.after ?? '')
    if (!b && !a) continue
    if (!b) {
      if (a && !out.includes(a)) out = out.endsWith('\n') ? `${out}${a}` : `${out}\n${a}`
      continue
    }
    const idx = out.indexOf(b)
    if (idx >= 0) {
      out = out.slice(0, idx) + a + out.slice(idx + b.length)
    }
  }
  return out
}

/**
 * @param {{ beforeFull?: string, afterFull?: string, before?: string, after?: string, diffs?: Array<{before?:string,after?:string}> }} patch
 * @param {string} diskText
 * @returns {{ before: string, after: string }}
 */
export function resolveInlineDiffPair(patch, diskText = '') {
  let before = String(patch?.beforeFull || patch?.before || '')
  let after = String(patch?.afterFull || patch?.after || '')
  const disk = String(diskText || '')
  const usableDisk = disk && !/^（尚无|暂无内容|加载/.test(disk)

  if (!before && usableDisk) before = disk
  if (!after && before) {
    const hunks = Array.isArray(patch?.diffs) ? patch.diffs : []
    if (hunks.length) after = applyTextHunks(before, hunks)
    else after = before
  }

  if (
    before
    && after
    && after.length < before.length * 0.55
    && !after.startsWith(before.slice(0, Math.min(40, before.length)))
  ) {
    const hunks = Array.isArray(patch?.diffs) ? patch.diffs : []
    if (hunks.length) {
      after = applyTextHunks(before, hunks)
    }
  }

  if (!before && after && usableDisk) before = disk
  if (!after && before) after = before
  return { before, after }
}

/**
 * @param {string} content
 * @param {Array<{ start_para?: number, end_para?: number, before?: string, after?: string }>} patches
 * @returns {{ before: string, after: string }}
 */
export function applyParaPatches(content, patches) {
  const raw = String(content ?? '')
  const list = Array.isArray(patches) ? patches : []
  if (!list.length) return { before: raw, after: raw }
  const paras = raw.split(/\n{2,}/)
  const afterParas = paras.slice()
  const ordered = [...list].sort(
    (x, y) => (Number(y.start_para) || 1) - (Number(x.start_para) || 1),
  )
  for (const p of ordered) {
    const start = Math.max(1, Number(p.start_para) || 1)
    const end = Math.max(start, Number(p.end_para) || start)
    const beforeText = String(p.before ?? '')
    const afterText = String(p.after ?? '')
    let idx = start - 1
    if (idx < 0 || idx >= afterParas.length || (beforeText && afterParas[idx] !== beforeText)) {
      const found = afterParas.findIndex((para) => para === beforeText)
      if (found >= 0) idx = found
    }
    if (idx < 0 || idx >= afterParas.length) continue
    const span = Math.max(1, end - start + 1)
    afterParas.splice(idx, span, afterText)
  }
  return { before: raw, after: afterParas.join('\n\n') }
}
