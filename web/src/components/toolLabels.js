/** English tool / pipeline step / role id → Chinese label for chat UI. */
const TOOL_LABELS_ZH = {
  // Writing & revise
  continue_writing: '继续创作',
  continue_writing_batch: '批量续写',
  continue_episode: '续写短剧',
  revise_chapter: '修订章节',
  revise_local: '局部改稿',
  revise_outline: '修订章纲',
  revise_episode: '修订短剧',
  split_chapter: '拆成两章',
  apply_draft_patch: '局部改稿',
  replan_volume: '重排本卷计划',
  research_materials: '搜集素材',
  read_chapter: '阅读章节',
  read_episode: '阅读短剧',

  // Audit
  audit_chapter: '审校章节',
  audit_chapters: '审阅队列',
  audit_volume: '整卷复盘',
  audit_setting: '检查设定',

  // Decisions / steer
  steer_run: '按你的选择继续',
  offer_decisions: '请你选择下一步',

  // Pipeline agents / steps
  chapter_planner: '规划章纲',
  lore_librarian: '查阅设定',
  writer: '撰写正文',
  local_reviser: '局部改稿',
  scene_specialist: '打磨场景',
  dialogue_specialist: '打磨对白',
  consistency_auditor: '核对前后文',
  foreshadow_tracker: '梳理伏笔',
  pacing_reviewer: '检查节奏',
  literary_editor: '润色文笔',
  nomenclature_curator: '统一用词',
  summarizer: '整理章摘要',
  plot_acceptor: '核对剧情进度',
  autofix: '自动修正',
  expectation_reviewer: '检阅预期',

  // Lore / memory / status
  query_lore: '查询设定',
  query_memory: '查阅记忆',
  list_entities: '浏览设定卡',
  list_plots: '查看剧情进度',
  get_project_status: '查看项目进度',
  list_projects: '列出作品',

  // Expected events
  list_expected_events: '查看预期列表',
  enqueue_expected_event: '登记预期',
  update_expected_event: '更新预期',
  review_expected_events: '检阅预期',
  resolve_expected_event: '处理预期',

  // Design / setup
  design_entity: '设计设定卡',
  design_plot: '设计剧情卡',
  update_plot: '更新剧情卡',
  design_master_outline: '设计总纲',
  design_arc_outline: '设计卷纲',
  sync_volume: '同步设定库',
  confirm_volume_memory: '确认卷记忆',
  create_novel: '创建小说',
  init_novel: '创建小说',
  lock_brief: '锁定灵感',
  confirm_setup: '确认定稿',
  upsert_setting: '写入设定',
  supplement_setting: '补充设定',
  delete_entity: '删除设定卡',
  activate_agents: '启用写作角色',

  // Collaboration
  spawn_agent: '启动协作角色',
  wait_agent: '等待协作角色',
  list_agents: '查看协作角色',
  interrupt_agent: '中断协作角色',
  send_message: '发送消息',
  followup_task: '追加任务',

  // Versions
  list_version_nodes: '查看版本记录',
  restore_version_node: '回退到某版本',
}

/** Skill id → friendly Chinese name (no $ / path). */
const SKILL_LABELS_ZH = {
  studio: '创作助手总则',
  'novel-draft': '正文写作要点',
  'content-formats': '内容格式说明',
  'content-formats-script': '剧本格式说明',
  'activation-policy': '角色启用说明',
  'volume-lifecycle': '卷生命周期',
  'prose-pitfalls': '文笔避坑',
  // Agent role skills (kebab-case ids from config/skills)
  'chapter-planner': '规划章纲',
  'lore-librarian': '查阅设定',
  writer: '撰写正文',
  'scene-specialist': '打磨场景',
  'dialogue-specialist': '打磨对白',
  'consistency-auditor': '核对前后文',
  'foreshadow-tracker': '梳理伏笔',
  'pacing-reviewer': '检查节奏',
  'literary-editor': '润色文笔',
  'nomenclature-curator': '统一用词',
  summarizer: '整理章摘要',
  'plot-acceptor': '核对剧情进度',
  'expectation-reviewer': '检阅预期',
  'arc-planner': '规划卷纲',
  'master-planner': '规划总纲',
  'world-architect': '搭建世界观',
  'plot-designer': '设计剧情卡',
  'entity-designer': '设计设定卡',
  'volume-auditor': '整卷复盘',
  'material-researcher': '搜集素材',
  'decision-council': '决策评审',
  'episode-planner': '规划短剧集纲',
  'script-writer': '撰写剧本',
  'beat-acceptor': '核对短剧收束',
  'setting-auditor': '检查设定',
}

/** Mutation preview kind → Chinese. */
const MUTATION_KIND_ZH = {
  mutation: '拟修改设定',
  entity: '拟修改设定卡',
  setting: '拟修改设定',
  plot: '拟修改剧情卡',
  outline: '拟修改大纲',
  chapter: '拟修改章节',
  draft: '拟修改正文',
}

/** Status verb on tool cells. */
export function toolVerbZh(status, { bulk = false } = {}) {
  if (status === 'failed') return '失败'
  if (status === 'cancelled') return '已取消'
  if (bulk) {
    if (status === 'completed') return '已查阅'
    return '查阅中'
  }
  if (status === 'completed') return '已完成'
  return '进行中'
}

function looksLikeInternalId(token) {
  const t = String(token || '').trim()
  if (!t) return false
  // snake_case / plain ascii tool ids — never show raw to users
  return /^[a-z][a-z0-9_]*$/i.test(t)
}

export function toolLabelZh(name) {
  if (!name) return ''
  const key = String(name).trim()
  if (TOOL_LABELS_ZH[key]) return TOOL_LABELS_ZH[key]
  if (/[\u4e00-\u9fff]/.test(key)) return key
  if (looksLikeInternalId(key)) return '后台步骤'
  return key
}

/** Normalize a progress step token (English id or Chinese label) to Chinese. */
export function stepLabelZh(token) {
  if (!token) return ''
  const t = String(token).trim()
  if (TOOL_LABELS_ZH[t]) return TOOL_LABELS_ZH[t]
  if (/[\u4e00-\u9fff]/.test(t)) return t
  if (looksLikeInternalId(t)) return '后台步骤'
  return t
}

/** Natural-language composer text when author picks a「常用动作」(no $ tokens). */
export function skillPromptForAuthor(name, description) {
  const key = String(name || '').trim().replace(/^\$/, '')
  const prompts = {
    studio: '请按创作流程帮我推进下一步',
    'novel-draft': '请按正文写作要点帮我写或改一章',
    'content-formats': '请按内容格式要求整理当前大纲或设定',
    'content-formats-script': '请按剧本格式帮我整理本集',
    'activation-policy': '请说明当前该启用哪些写作角色',
    'volume-lifecycle': '请按卷生命周期帮我判断下一步',
    'prose-pitfalls': '请按文笔避坑检查并改当前正文',
    writer: '请撰写或续写正文',
    'chapter-planner': '请规划下一章章纲',
    'literary-editor': '请润色当前正文文笔',
    'consistency-auditor': '请核对前后文一致性',
    'script-writer': '请撰写本集剧本',
    'episode-planner': '请规划下一集集纲',
  }
  if (prompts[key]) return prompts[key]
  const label = skillLabelZh(key, description)
  return `请参考「${label}」帮我推进创作`
}

/**
 * Friendly skill title. Prefer explicit map, then tool/role labels (kebab↔snake),
 * then a short slice of description — never collapse everything to「写作指南」.
 */
export function skillLabelZh(name, description) {
  if (!name && !description) return '未命名指南'
  const key = String(name || '').trim().replace(/^\$/, '')
  if (key && SKILL_LABELS_ZH[key]) return SKILL_LABELS_ZH[key]
  const snake = key.replace(/-/g, '_')
  if (snake && TOOL_LABELS_ZH[snake]) return TOOL_LABELS_ZH[snake]
  if (key && TOOL_LABELS_ZH[key]) return TOOL_LABELS_ZH[key]
  if (/[\u4e00-\u9fff]/.test(key)) return key
  const desc = String(description || '').trim()
  if (desc) {
    const head = desc
      .replace(/\s+/g, ' ')
      .split(/[。；;\n—–-]/)[0]
      .trim()
      .slice(0, 18)
    if (head) return head
  }
  // Last resort: readable id, not a generic bucket label
  if (key) return key.replace(/[-_]+/g, ' ')
  return '未命名指南'
}

export function mutationKindZh(kind) {
  if (!kind) return '拟修改内容'
  const key = String(kind).trim()
  if (MUTATION_KIND_ZH[key]) return MUTATION_KIND_ZH[key]
  if (/[\u4e00-\u9fff]/.test(key)) return key
  if (looksLikeInternalId(key)) return '拟修改内容'
  return key
}
