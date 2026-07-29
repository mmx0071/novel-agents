import { describe, expect, it } from 'vitest'
import {
  STAGE_ORDER,
  buildCreateNovelMessage,
  chapterFromApprovalPrompt,
  deriveStudioStage,
  pickPrimaryApprovalOption,
  resolveApprovalChapter,
  resolveStudioCta,
} from './studioPhase.js'

describe('STAGE_ORDER', () => {
  it('lists four author-facing stages', () => {
    expect(STAGE_ORDER.map((s) => s.id)).toEqual([
      'setup',
      'volume',
      'chapter',
      'volume_end',
    ])
  })
})

describe('buildCreateNovelMessage', () => {
  it('builds genre-neutral create prompt from title/genre/brief', () => {
    expect(buildCreateNovelMessage({
      title: 'sample-novel',
      genre: '未定',
      brief: '主角在城中寻找线索',
    })).toBe('我想写一本小说，书名《sample-novel》，题材：未定。灵感：主角在城中寻找线索')
  })

  it('falls back when brief empty', () => {
    expect(buildCreateNovelMessage({ title: 'demo' })).toBe(
      '我想写一本小说，书名《demo》。请先帮我立项。',
    )
  })

  it('trims whitespace and ignores empty optional fields', () => {
    expect(buildCreateNovelMessage({ title: '  demo  ', genre: '  ', brief: '' }))
      .toBe('我想写一本小说，书名《demo》。请先帮我立项。')
  })
})

describe('deriveStudioStage', () => {
  it('returns setup empty state without preview', () => {
    const s = deriveStudioStage(null)
    expect(s.stageId).toBe('setup')
    expect(s.cta).toBeNull()
  })

  it('maps awaiting_confirm to setup confirm CTA', () => {
    const s = deriveStudioStage({
      setup_phase: 'awaiting_confirm',
      volume_phase: 'drafting_volume',
      has_master_outline: true,
      has_arc_outline: true,
    })
    expect(s.stageId).toBe('setup')
    expect(s.cta.id).toBe('confirm_setup')
    expect(s.cta.readerTab).toBe('master')
    expect(s.cta.message).toBeNull()
  })

  it('maps collecting to continue setup', () => {
    const s = deriveStudioStage({
      setup_phase: 'collecting',
      volume_phase: 'drafting_volume',
      has_master_outline: false,
      has_arc_outline: false,
      published_count: 0,
    })
    expect(s.stageId).toBe('setup')
    expect(s.cta.id).toBe('continue_setup')
  })

  it('maps awaiting_sync to volume_end', () => {
    const s = deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'awaiting_sync',
      has_arc_outline: true,
      published_count: 3,
      next_chapter: 4,
    })
    expect(s.stageId).toBe('volume_end')
    expect(s.stageIndex).toBe(3)
    expect(s.cta.message).toBe('同步设定库')
  })

  it('maps awaiting_next_arc / awaiting_next_plot to volume planning', () => {
    expect(deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'awaiting_next_arc',
      has_arc_outline: true,
    }).cta.id).toBe('design_arc')

    expect(deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'awaiting_next_plot',
      has_arc_outline: true,
    }).cta.id).toBe('design_plot')
  })

  it('prefers audit CTA when unpublished draft exists', () => {
    const s = deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'drafting_volume',
      has_arc_outline: true,
      published_count: 2,
      next_chapter: 3,
      chapters: [
        { number: 1, has_draft: true, body_chars: 5000 },
        { number: 2, has_draft: true, body_chars: 5200 },
        { number: 3, has_draft: true, body_chars: 1200 },
      ],
    })
    expect(s.stageId).toBe('chapter')
    expect(s.cta.id).toBe('audit_draft')
    expect(s.cta.chapter).toBe(3)
    expect(s.cta.label).toBe('审校第3章')
    expect(s.cta.message).toBe('审校第3章')
  })

  it('offers write_next when drafting with no unpublished draft', () => {
    const s = deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'drafting_volume',
      has_arc_outline: true,
      published_count: 2,
      next_chapter: 3,
      chapters: [
        { number: 1, has_draft: true, body_chars: 5000 },
        { number: 2, has_draft: true, body_chars: 5200 },
      ],
    })
    expect(s.cta.id).toBe('write_next')
    expect(s.cta.message).toBe('写第3章')
  })

  it('treats published_count>=1 as setup ready even if phase missing', () => {
    const s = deriveStudioStage({
      setup_phase: '',
      volume_phase: 'drafting_volume',
      has_arc_outline: true,
      published_count: 5,
      next_chapter: 6,
      chapters: [{ number: 5, has_draft: true, body_chars: 5000 }],
    })
    expect(s.stageId).toBe('chapter')
    expect(s.cta.id).toBe('write_next')
  })
})

describe('resolveStudioCta', () => {
  const draftStage = () => deriveStudioStage({
    setup_phase: 'ready',
    volume_phase: 'drafting_volume',
    has_arc_outline: true,
    published_count: 4,
    next_chapter: 5,
    chapters: [
      { number: 4, has_draft: true, body_chars: 5000 },
      { number: 5, has_draft: true, body_chars: 4800 },
    ],
  })

  it('prefers open approval「修正本章」over audit_draft CTA', () => {
    const resolved = resolveStudioCta(draftStage(), {
      openApproval: {
        prompt: '第5章未发布，请选择下一步',
        options: [
          { id: 'cn_revise', label: '修正本章' },
        ],
      },
    })
    expect(resolved.cta.id).toBe('pending_choice')
    expect(resolved.cta.label).toBe('修正本章')
    expect(resolved.cta.message).toBe('cn_revise')
    expect(resolved.cta.fromApproval).toBe(true)
  })

  it('maps failed audit on draft chapter to 修正本章', () => {
    const resolved = resolveStudioCta(draftStage(), {
      latest: { chapter: 5, failed: true },
    })
    expect(resolved.cta.id).toBe('revise_draft')
    expect(resolved.cta.label).toBe('修正本章')
    expect(resolved.cta.message).toBe('修正本章')
  })

  it('ignores setup-gate options when aligning CTA', () => {
    const resolved = resolveStudioCta(draftStage(), {
      openApproval: {
        options: [
          { id: 'sc_approve', label: '确认定稿' },
          { id: 'sc_revise', label: '修改再生成' },
        ],
      },
    })
    expect(resolved.cta.id).toBe('audit_draft')
  })

  it('after applied revise, prefers 审校 over 修正', () => {
    const resolved = resolveStudioCta(draftStage(), {
      latest: { chapter: 5, failed: true },
      reviseAppliedAfterFail: true,
      reviseChapter: 5,
    })
    expect(resolved.cta.id).toBe('audit_draft')
    expect(resolved.cta.label).toBe('审校第5章')
    expect(resolved.cta.message).toBe('审校第5章')
  })

  it('pickPrimaryApprovalOption favors revise labels', () => {
    const opt = pickPrimaryApprovalOption([
      { id: '1', label: '接受现状' },
      { id: 'cn_revise', label: '修正本章' },
    ])
    expect(opt.id).toBe('cn_revise')
  })

  it('aligns desk CTA with mutation confirm「应用修改」', () => {
    const resolved = resolveStudioCta(draftStage(), {
      openApproval: {
        prompt: '请对照修订预览（原文 / 修订后）再选择：第5章局部修订（1 处）',
        options: [
          { id: 'cm_apply', label: '应用修改' },
          { id: 'cm_discard', label: '放弃' },
        ],
      },
    })
    expect(resolved.cta.id).toBe('pending_choice')
    expect(resolved.cta.label).toBe('应用修改')
    expect(resolved.cta.message).toBe('cm_apply')
    expect(resolved.cta.isApply).toBe(true)
    expect(resolved.cta.readerTab).toBe('draft')
    expect(resolved.cta.hint).toMatch(/对照|Apply/)
    expect(resolved.cta.chapter).toBe(5)
  })

  it('Apply CTA prefers revised chapter over phase nextChapter', () => {
    // Phase says「写第6章」, but mutation revises published第5章.
    const stage = deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'drafting_volume',
      has_arc_outline: true,
      published_count: 5,
      next_chapter: 6,
      chapters: [{ number: 5, has_draft: true, body_chars: 5000 }],
    })
    expect(stage.cta.chapter).toBe(6)
    const resolved = resolveStudioCta(stage, {
      openApproval: {
        prompt: '请对照修订预览（原文 / 修订后）再选择：第5章局部修订（2 处）',
        options: [
          { id: 'cm_apply', label: '应用修改' },
          { id: 'cm_discard', label: '放弃' },
        ],
      },
      latest: { chapter: 5, failed: true },
      patchChapter: 5,
    })
    expect(resolved.cta.isApply).toBe(true)
    expect(resolved.cta.chapter).toBe(5)
  })

  it('Apply CTA falls back to patchChapter when prompt has no chapter', () => {
    const stage = deriveStudioStage({
      setup_phase: 'ready',
      volume_phase: 'drafting_volume',
      has_arc_outline: true,
      published_count: 5,
      next_chapter: 6,
    })
    const resolved = resolveStudioCta(stage, {
      openApproval: {
        prompt: '待确认：局部修订',
        options: [{ id: 'cm_apply', label: '应用修改' }],
      },
      patchChapter: 4,
    })
    expect(resolved.cta.chapter).toBe(4)
  })
})

describe('chapterFromApprovalPrompt / resolveApprovalChapter', () => {
  it('parses chapter from prompt', () => {
    expect(chapterFromApprovalPrompt('第5章局部修订（1 处）')).toBe(5)
    expect(chapterFromApprovalPrompt('无章节')).toBe(0)
  })

  it('resolveApprovalChapter prioritizes prompt/patch over stage for apply', () => {
    expect(resolveApprovalChapter({
      prompt: '第3章',
      isApply: true,
      stageChapter: 6,
      latestChapter: 5,
      patchChapter: 4,
    })).toBe(3)
    expect(resolveApprovalChapter({
      prompt: '',
      isApply: true,
      stageChapter: 6,
      patchChapter: 4,
    })).toBe(4)
  })
})
