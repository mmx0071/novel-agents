/**
 * Derive author-facing studio stage + next CTA from preview phases.
 * Driven by server `setup_phase` / `volume_phase` — no story-specific logic.
 */

export const STAGE_ORDER = [
  { id: 'setup', label: '立项' },
  { id: 'volume', label: '卷规划' },
  { id: 'chapter', label: '写章' },
  { id: 'volume_end', label: '卷末' },
]

/** Author-facing stage strip labels by project mode. */
export function stageOrderForMode(mode) {
  if (String(mode || '') === 'short_drama') {
    return [
      { id: 'setup', label: '立项' },
      { id: 'volume', label: '系列规划' },
      { id: 'chapter', label: '写集' },
      { id: 'volume_end', label: '系列收束' },
    ]
  }
  return STAGE_ORDER
}

function unitWord(mode) {
  return String(mode || '') === 'short_drama' ? '集' : '章'
}

/**
 * @param {object|null} preview
 * @param {{ nextChapter?: number, publishedCount?: number }} [extra]
 */
export function deriveStudioStage(preview, extra = {}) {
  if (!preview) {
    return {
      stageId: 'setup',
      stageIndex: 0,
      setupPhase: '',
      volumePhase: '',
      detail: '选择或新建小说后开始',
      cta: null,
    }
  }

  const setupPhase = String(preview.setup_phase || '')
  const volumePhase = String(preview.volume_phase || '')
  const nextChapter = Number(extra.nextChapter ?? preview.next_chapter) || 1
  const publishedCount = Number(extra.publishedCount ?? preview.published_count) || 0
  const hasMaster = !!preview.has_master_outline
  const hasArc = !!preview.has_arc_outline
  const short = String(preview.project_mode || '') === 'short_drama'
  const unit = unitWord(preview.project_mode)

  if (setupPhase === 'awaiting_confirm') {
    return {
      stageId: 'setup',
      stageIndex: 0,
      setupPhase,
      volumePhase,
      detail: short
        ? '总纲与世界观已就绪，待确认定稿'
        : '总纲与卷纲已就绪，待确认定稿',
      cta: {
        id: 'confirm_setup',
        label: '去确认定稿',
        hint: '打开总纲后可一键确认或打回',
        message: null,
        readerTab: 'master',
        focusSetup: true,
      },
    }
  }

  // Collecting, or legacy preview without phase but not yet publishing.
  const setupReady = setupPhase === 'ready' || publishedCount >= 1
  if (!setupReady) {
    return {
      stageId: 'setup',
      stageIndex: 0,
      setupPhase,
      volumePhase,
      detail: short
        ? (hasMaster
          ? '总纲已有，可补世界观后确认定稿'
          : '收集灵感、生成系列总纲')
        : (hasMaster
          ? (hasArc ? '大纲已齐，可确认定稿或继续补世界观' : '总纲已有，请补齐卷纲与世界观后定稿')
          : '收集灵感、生成总纲与卷纲'),
      cta: {
        id: short
          ? (hasMaster ? 'confirm_setup' : 'continue_setup')
          : (hasMaster && hasArc ? 'confirm_setup' : 'continue_setup'),
        label: short
          ? (hasMaster ? '去确认定稿' : '继续立项')
          : (hasMaster && hasArc ? '去确认定稿' : (hasMaster ? '继续完善立项' : '继续立项')),
        hint: short
          ? (hasMaster ? '打开总纲后可一键确认或打回' : '在创作助手中补充灵感或总纲')
          : (hasMaster && hasArc
            ? '打开总纲后可一键确认或打回'
            : '在创作助手中补充灵感、总纲或卷纲'),
        message: short
          ? (hasMaster
            ? null
            : '请继续立项：锁定灵感并生成系列总纲')
          : (hasMaster && hasArc
            ? null
            : (hasMaster
              ? '请继续完善立项：补齐卷纲与世界观，准备确认定稿'
              : '请继续立项：锁定灵感并生成总纲与卷纲')),
        readerTab: short ? 'master' : (hasMaster ? (hasArc ? 'master' : 'arcs') : 'master'),
        focusSetup: short ? !!hasMaster : !!(hasMaster && hasArc),
      },
    }
  }

  // setup ready
  if (volumePhase === 'awaiting_sync') {
    return {
      stageId: 'volume_end',
      stageIndex: 3,
      setupPhase,
      volumePhase,
      detail: '本卷收束，待同步人物与设定进度',
      cta: {
        id: 'sync_volume',
        label: '同步本卷设定',
        hint: '卷末更新人物、地点、物品与本卷进度',
        message: '同步本卷设定',
        readerTab: 'arcs',
      },
    }
  }

  if (volumePhase === 'awaiting_next_arc') {
    return {
      stageId: 'volume',
      stageIndex: 1,
      setupPhase,
      volumePhase,
      detail: '开卷前需先有下一卷卷纲',
      cta: {
        id: 'design_arc',
        label: '生成下一卷卷纲',
        hint: '卷间交接：先卷纲，再剧情卡',
        message: '生成卷纲',
        readerTab: 'arcs',
      },
    }
  }

  if (volumePhase === 'awaiting_next_plot') {
    return {
      stageId: 'volume',
      stageIndex: 1,
      setupPhase,
      volumePhase,
      detail: '卷纲已有，需激活本卷剧情卡',
      cta: {
        id: 'design_plot',
        label: '设计剧情卡',
        hint: '为当前卷设计并激活主线剧情卡',
        message: '设计剧情卡',
        readerTab: 'arcs',
      },
    }
  }

  // drafting_volume (or unknown → treat as writing)
  const unpublishedDraft = (preview.chapters || []).some(
    (c) => Number(c.number) > publishedCount && (c.has_draft || (Number(c.body_chars) || 0) > 0),
  )

  if (unpublishedDraft) {
    const draftCh = [...(preview.chapters || [])]
      .filter((c) => Number(c.number) > publishedCount && (c.has_draft || (Number(c.body_chars) || 0) > 0))
      .sort((a, b) => Number(a.number) - Number(b.number))[0]
    const n = Number(draftCh?.number) || nextChapter
    // Default: re-audit unpublished draft (publish gate). resolveStudioCta upgrades to
    //「修正」only while a failed audit has not yet been followed by an applied revise.
    return {
      stageId: 'chapter',
      stageIndex: 2,
      setupPhase,
      volumePhase,
      detail: `第${n}${unit}草稿未发布，请先审校`,
      cta: {
        id: 'audit_draft',
        label: `审校第${n}${unit}`,
        hint: '审校通过后才能发布；刚改完请先审校',
        message: `审校第${n}${unit}`,
        readerTab: 'draft',
        chapter: n,
      },
    }
  }

  if (!short && !hasArc) {
    return {
      stageId: 'volume',
      stageIndex: 1,
      setupPhase,
      volumePhase,
      detail: '定稿后请设计卷纲与剧情卡',
      cta: {
        id: 'design_arc_ready',
        label: '生成卷纲',
        hint: '写章前需要卷纲与激活的剧情卡',
        message: '生成卷纲',
        readerTab: 'arcs',
      },
    }
  }

  const volumeQaPhase = String(preview.volume_qa_phase || '')
  const foreshadowPhase = String(preview.foreshadow_phase || '')

  // Soft gate: mid-volume QA due — prefer desk CTA, user can still chat to continue writing.
  if (!short && volumeQaPhase === 'mid_due') {
    return {
      stageId: 'chapter',
      stageIndex: 2,
      setupPhase,
      volumePhase,
      detail: `本卷已到建议复盘节点 · 下一${unit}第 ${nextChapter} ${unit}`,
      cta: {
        id: 'volume_mid_audit',
        label: '做卷中复盘',
        hint: '复盘本卷节奏与设定；也可在对话里说继续写',
        message: '对本卷做一次卷中复盘',
        readerTab: 'arcs',
      },
    }
  }

  let detail = publishedCount > 0
    ? `已发布 ${publishedCount} ${unit} · 下一${unit}第 ${nextChapter} ${unit}`
    : `准备写第 ${nextChapter} ${unit}`
  if (!short && foreshadowPhase === 'pressure_high') {
    detail += ' · 伏笔压力偏高，宜穿插回收'
  } else if (!short && foreshadowPhase === 'paydown') {
    detail += ' · 正在回收伏笔'
  }

  return {
    stageId: 'chapter',
    stageIndex: 2,
    setupPhase,
    volumePhase,
    detail,
    cta: {
      id: 'write_next',
      label: `写第${nextChapter}${unit}`,
      hint: short
        ? '按当前节拍继续写剧本'
        : (foreshadowPhase === 'pressure_high'
          ? '续写时可穿插回收近期伏笔'
          : '按当前剧情卡继续创作'),
      message: `写第${nextChapter}${unit}`,
      readerTab: 'draft',
      chapter: nextChapter,
    },
  }
}

/**
 * Prefer the same primary action as the open chat approval card.
 * Setup / volume-sync / mutation gates are ignored so phase CTAs stay intact.
 */
export function pickPrimaryApprovalOption(options) {
  const list = Array.isArray(options) ? options.filter(Boolean) : []
  if (!list.length) return null

  const isDeskAction = (opt) => {
    const id = String(opt.id || '')
    const label = String(opt.label || '')
    // Setup / volume-sync / setting-bible gates — leave phase CTA alone.
    if (/^sc_/.test(id) || /^vs_/.test(id) || /^sbi_/.test(id) || /^vh_/.test(id)) {
      return false
    }
    // Mutation confirm (应用修改 / 放弃)
    if (id === 'cm_apply' || id === 'cm_discard' || /应用修改|放弃/.test(label)) return true
    if (/修正|修订|接受|重审|按建议|继续创作|连写|扩写/.test(label)) return true
    if (/^(cn_|aq_|ai_|cm_)/.test(id) || /revise|steer|audit|accept|apply/.test(id)) return true
    return false
  }

  const deskOpts = list.filter(isDeskAction)
  if (!deskOpts.length) return null

  // Prefer apply / revise over dismiss / accept when both are present.
  const apply = deskOpts.find((o) => {
    const id = String(o.id || '')
    const label = String(o.label || '')
    return id === 'cm_apply' || label === '应用修改'
  })
  if (apply) return apply

  const revise = deskOpts.find((o) => {
    const id = String(o.id || '')
    const label = String(o.label || '')
    return id === 'cn_revise' || /revise/.test(id) || /修正|修订|扩写/.test(label)
  })
  return revise || deskOpts[0]
}

/** Extract「第N章」from approval / mutation prompt text. */
export function chapterFromApprovalPrompt(prompt) {
  const m = String(prompt || '').match(/第\s*(\d+)\s*章/)
  const n = m ? Number(m[1]) : 0
  return n > 0 ? n : 0
}

/**
 * Chapter for desk CTA when aligning with an open approval.
 * Apply / revise must not inherit phase「写下一章」over the chapter under review.
 */
export function resolveApprovalChapter({
  prompt,
  isApply,
  stageChapter,
  latestChapter,
  patchChapter,
} = {}) {
  const fromPrompt = chapterFromApprovalPrompt(prompt)
  const latest = Number(latestChapter) || 0
  const patch = Number(patchChapter) || 0
  const stage = Number(stageChapter) || 0
  if (isApply) {
    return fromPrompt || patch || latest || stage || undefined
  }
  return fromPrompt || latest || stage || undefined
}

/**
 * Align desk「下一步」with open chat approval / failed audit, so we don't keep
 * offering「审校第N章」after the real next step is「修正本章」.
 *
 * @param {ReturnType<typeof deriveStudioStage>} stage
 * @param {{ openApproval?: { prompt?: string, options?: object[] }|null, latest?: { chapter?: number, failed?: boolean }|null, patchChapter?: number, reviseAppliedAfterFail?: boolean, reviseChapter?: number }} [audit]
 */
export function resolveStudioCta(stage, audit = {}) {
  if (!stage?.cta) return stage

  const primary = pickPrimaryApprovalOption(audit.openApproval?.options)
  if (primary) {
    const prompt = String(audit.openApproval?.prompt || '').trim()
    const id = String(primary.id || '')
    const isApply = id === 'cm_apply' || primary.label === '应用修改'
    // Keep hard-rule violation bullets visible in the desk CTA (not just「因硬规则未发布」).
    const detail = prompt
      ? prompt.split('\n').filter(Boolean).slice(0, 4).join(' · ').slice(0, 220)
      : (stage.detail || '请在创作助手中选择下一步')
    const chapter = resolveApprovalChapter({
      prompt,
      isApply,
      stageChapter: stage.cta.chapter,
      latestChapter: audit.latest?.chapter,
      patchChapter: audit.patchChapter,
    })
    return {
      ...stage,
      detail: isApply
        ? (detail.includes('对照') || detail.includes('修订')
          ? detail
          : '请先对照下方原文/修订，确认无误后再应用')
        : detail,
      cta: {
        id: 'pending_choice',
        label: primary.label || primary.id,
        hint: isApply
          ? '对照修订预览，确认无误后再应用'
          : '与创作助手待选项一致',
        message: String(primary.id || primary.label || ''),
        readerTab: 'draft',
        chapter,
        fromApproval: true,
        isApply,
      },
    }
  }

  const latest = audit.latest
  const draftCh = Number(stage.cta.chapter) || 0
  const reviseCh = Number(audit.reviseChapter) || 0
  const revisedAfterFail = !!audit.reviseAppliedAfterFail
    && draftCh > 0
    && (reviseCh === 0 || reviseCh === draftCh)

  if (
    (stage.cta.id === 'audit_draft' || stage.cta.id === 'revise_draft')
    && draftCh > 0
  ) {
    // Applied local revise after a failed audit → next is re-audit, not revise again.
    if (revisedAfterFail) {
      return {
        ...stage,
        detail: `第${draftCh}章修订已保存，请再审校一次`,
        cta: {
          id: 'audit_draft',
          label: `审校第${draftCh}章`,
          hint: '修订已应用，请审校确认后再发布',
          message: `审校第${draftCh}章`,
          readerTab: 'draft',
          chapter: draftCh,
        },
      }
    }
    if (
      latest?.failed
      && Number(latest.chapter) === draftCh
    ) {
      return {
        ...stage,
        detail: `第${draftCh}章审校未通过，待修正`,
        cta: {
          id: 'revise_draft',
          label: '修正本章',
          hint: '与创作助手一致：先按审校问题改稿，勿重复审校',
          message: '修正本章',
          readerTab: 'draft',
          chapter: draftCh,
        },
      }
    }
  }

  return stage
}

/** Build create-novel prompt from form fields (genre-neutral). */
export function buildCreateNovelMessage({ title, genre, brief, mode }) {
  const name = String(title || '').trim()
  const g = String(genre || '').trim()
  const b = String(brief || '').trim()
  const m = String(mode || 'longform').trim()
  const short = m === 'short_drama'
  const parts = [short ? '我想写一部 AI 漫剧短篇' : '我想写一本小说']
  if (name) parts.push(short ? `片名《${name}》` : `书名《${name}》`)
  if (g) parts.push(`题材：${g}`)
  let msg = parts.join('，')
  if (b) msg += `。灵感：${b}`
  else msg += '。请先帮我立项。'
  if (short) {
    msg += '。请按短剧剧本模式立项，按集产出剧本。'
  }
  return msg
}
