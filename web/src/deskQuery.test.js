import { describe, expect, it } from 'vitest'
import { deskPageUrl, deskQueryFromSearch, deskSearch } from './deskQuery'

describe('deskQuery', () => {
  it('parses project chapter and tab', () => {
    expect(deskQueryFromSearch('?project=sample-novel&chapter=2&tab=outline')).toEqual({
      project: 'sample-novel',
      chapter: 2,
      tab: 'outline',
    })
  })

  it('ignores empty or invalid chapter', () => {
    expect(deskQueryFromSearch('project=demo&chapter=0')).toEqual({
      project: 'demo',
      chapter: 0,
      tab: '',
    })
  })

  it('builds a follow URL without a trailing empty query', () => {
    expect(deskSearch({})).toBe('')
    expect(deskPageUrl('http://127.0.0.1:8765', {
      project: 'sample-novel',
      chapter: 3,
      tab: 'draft',
    })).toBe('http://127.0.0.1:8765/?project=sample-novel&chapter=3&tab=draft')
  })
})
