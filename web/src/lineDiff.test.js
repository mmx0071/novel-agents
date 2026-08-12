import { describe, expect, it } from 'vitest'
import {
  applyParaPatches,
  applyTextHunks,
  buildPartialAfter,
  groupDiffBlocks,
  hunkSummary,
  lineDiffOps,
  resolveInlineDiffPair,
} from './lineDiff'

describe('lineDiffOps', () => {
  it('keeps unchanged prefix/suffix as equal', () => {
    const ops = lineDiffOps('a\nb\nold\nc\n', 'a\nb\nnew\nc\n')
    expect(ops.filter((o) => o.type === 'equal').map((o) => o.text)).toEqual(['a', 'b', 'c'])
    expect(ops.find((o) => o.type === 'del')?.text).toBe('old')
    expect(ops.find((o) => o.type === 'ins')?.text).toBe('new')
  })

  it('does not mark whole doc as delete when only middle changes', () => {
    const head = Array.from({ length: 80 }, (_, i) => `line-${i}`).join('\n')
    const tail = Array.from({ length: 80 }, (_, i) => `tail-${i}`).join('\n')
    const ops = lineDiffOps(`${head}\nold-mid\n${tail}`, `${head}\nnew-mid\n${tail}`)
    expect(ops.filter((o) => o.type === 'del')).toHaveLength(1)
    expect(ops.filter((o) => o.type === 'ins')).toHaveLength(1)
    expect(ops.filter((o) => o.type === 'equal').length).toBeGreaterThan(100)
  })
})

describe('groupDiffBlocks (Cursor-style)', () => {
  it('attaches context lines and classifies insert hunks', () => {
    const before = 'keep-a\nkeep-b\nkeep-c\n'
    const after = 'keep-a\nkeep-b\nNEW\nkeep-c\n'
    const blocks = groupDiffBlocks(lineDiffOps(before, after), { context: 2 })
    const hunk = blocks.find((b) => b.type === 'hunk')
    expect(hunk.changeKind).toBe('insert')
    expect(hunk.dels).toEqual([])
    expect(hunk.ins).toEqual(['NEW'])
    expect(hunk.lines.some((l) => l.kind === 'ctx' && l.text === 'keep-b')).toBe(true)
    expect(hunk.lines.some((l) => l.kind === 'ctx' && l.text === 'keep-c')).toBe(true)
    expect(hunkSummary(hunk)).toBe('插入 +1')
  })

  it('shows both del and ins for replace', () => {
    const blocks = groupDiffBlocks(lineDiffOps('a\nold\nb\n', 'a\nnew\nb\n'), { context: 1 })
    const hunk = blocks.find((b) => b.type === 'hunk')
    expect(hunk.changeKind).toBe('replace')
    expect(hunk.dels).toEqual(['old'])
    expect(hunk.ins).toEqual(['new'])
    expect(hunkSummary(hunk)).toBe('替换 −1 / +1')
  })

  it('buildPartialAfter respects selection and keeps context', () => {
    const blocks = groupDiffBlocks(lineDiffOps('a\nold\nb\n', 'a\nnew\nb\n'), { context: 1 })
    const hunk = blocks.find((b) => b.type === 'hunk')
    expect(buildPartialAfter(blocks, [])).toContain('old')
    expect(buildPartialAfter(blocks, [])).not.toContain('new')
    expect(buildPartialAfter(blocks, [hunk.id])).toContain('new')
    expect(buildPartialAfter(blocks, [hunk.id])).not.toContain('old')
  })
})

describe('resolveInlineDiffPair / helpers', () => {
  it('splices truncated after via hunks onto full before', () => {
    const before = '# 世界观\n\nkeep\n\nold line\n\nend\n'
    const { after } = resolveInlineDiffPair({
      beforeFull: before,
      afterFull: '## 小节节选',
      diffs: [{ before: 'old line', after: 'new line' }],
    })
    expect(after).toContain('keep')
    expect(after).toContain('new line')
  })

  it('applyTextHunks / applyParaPatches', () => {
    expect(applyTextHunks('a\nold\nb', [{ before: 'old', after: 'new' }])).toBe('a\nnew\nb')
    const { after } = applyParaPatches('p1\n\np2 old\n\np3', [{
      start_para: 2,
      end_para: 2,
      before: 'p2 old',
      after: 'p2 new',
    }])
    expect(after).toBe('p1\n\np2 new\n\np3')
  })
})
