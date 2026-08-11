import { describe, expect, it } from 'vitest'
import {
  isAgentActive,
  lifecycleLabelZh,
  mergeSubAgentList,
  normalizeLifecycle,
  normalizeSubAgent,
  sortSubAgentsForRail,
  upsertSubAgent,
} from './subAgents'

describe('subAgents', () => {
  it('normalizes camelCase AgentStatusChanged payload', () => {
    const a = normalizeSubAgent({
      type: 'agent_status_changed',
      status: {
        threadId: 't1',
        role: 'writer',
        agentPath: '/root/writer',
        parentThreadId: 'root',
        lifecycle: 'Running',
        summary: '写第1章',
      },
    })
    expect(a.threadId).toBe('t1')
    expect(a.lifecycle).toBe('running')
    expect(a.summary).toBe('写第1章')
    expect(isAgentActive(a.lifecycle)).toBe(true)
  })

  it('upserts without losing prior summary', () => {
    let list = upsertSubAgent([], {
      threadId: 't1',
      role: 'writer',
      lifecycle: 'spawned',
      summary: 'task A',
    })
    list = upsertSubAgent(list, {
      threadId: 't1',
      lifecycle: 'running',
      summary: '',
    })
    expect(list).toHaveLength(1)
    expect(list[0].summary).toBe('task A')
    expect(list[0].lifecycle).toBe('running')
  })

  it('merges hydrate list and sorts active first', () => {
    const list = mergeSubAgentList([], [
      { threadId: 'a', role: 'writer', lifecycle: 'completed' },
      { threadId: 'b', role: 'consistency_auditor', lifecycle: 'running' },
    ])
    const sorted = sortSubAgentsForRail(list)
    expect(sorted[0].threadId).toBe('b')
    expect(normalizeLifecycle('Completed')).toBe('completed')
    expect(lifecycleLabelZh('failed')).toBe('失败')
  })
})
