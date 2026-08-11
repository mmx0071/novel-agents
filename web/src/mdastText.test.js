import { describe, expect, it } from 'vitest'
import { mdastText, tableAsKvRows } from './mdastText'

describe('mdastText / tableAsKvRows', () => {
  it('flattens text nodes', () => {
    expect(mdastText({ type: 'text', value: '进度' })).toBe('进度')
    expect(mdastText({
      type: 'paragraph',
      children: [{ type: 'text', value: '已发布 ' }, { type: 'strong', children: [{ type: 'text', value: '20' }] }, { type: 'text', value: ' 章' }],
    })).toBe('已发布 20 章')
  })

  it('converts 2-col status tables to kv rows', () => {
    const node = {
      type: 'table',
      children: [
        {
          type: 'tableRow',
          children: [
            { type: 'tableCell', children: [{ type: 'text', value: '项目' }] },
            { type: 'tableCell', children: [{ type: 'text', value: '状态' }] },
          ],
        },
        {
          type: 'tableRow',
          children: [
            { type: 'tableCell', children: [{ type: 'text', value: '题材' }] },
            { type: 'tableCell', children: [{ type: 'text', value: '末世' }] },
          ],
        },
        {
          type: 'tableRow',
          children: [
            { type: 'tableCell', children: [{ type: 'text', value: '进度' }] },
            { type: 'tableCell', children: [{ type: 'text', value: '第21章' }] },
          ],
        },
      ],
    }
    expect(tableAsKvRows(node)).toEqual([
      { key: '题材', value: '末世' },
      { key: '进度', value: '第21章' },
    ])
  })

  it('ignores wider tables', () => {
    const node = {
      type: 'table',
      children: [
        {
          type: 'tableRow',
          children: [
            { type: 'tableCell', children: [{ type: 'text', value: 'a' }] },
            { type: 'tableCell', children: [{ type: 'text', value: 'b' }] },
            { type: 'tableCell', children: [{ type: 'text', value: 'c' }] },
          ],
        },
      ],
    }
    expect(tableAsKvRows(node)).toBeNull()
  })
})
