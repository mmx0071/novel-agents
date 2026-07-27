/** English tool / pipeline step id → Chinese label for chat UI. */
const TOOL_LABELS_ZH = {
  continue_writing: '继续创作',
  revise_chapter: '修订章节',
  revise_outline: '修订章纲',
  audit_chapter: '审校章节',
  audit_chapters: '审阅队列',
  audit_volume: '整卷复盘',
  apply_draft_patch: '局部补丁',
  steer_run: '门控续作',
  offer_decisions: '提出决策',
  chapter_planner: '章纲规划',
  lore_librarian: '设定检索',
  writer: '正文写作',
  local_reviser: '局部修订',
  scene_specialist: '场景专改',
  dialogue_specialist: '对话专改',
  consistency_auditor: '一致性审计',
  foreshadow_tracker: '伏笔追踪',
  pacing_reviewer: '节奏审查',
  literary_editor: '文学润色',
  nomenclature_curator: '名词管理',
  summarizer: '章节摘要',
  plot_acceptor: '剧情验收',
  autofix: '自动修复',
  read_chapter: '阅读章节',
  query_lore: '查询设定',
  query_memory: '查询记忆',
  list_entities: '浏览实体',
  list_plots: '剧情进度',
  list_expected_events: '预期列表',
  enqueue_expected_event: '登记预期',
  update_expected_event: '更新预期',
  review_expected_events: '检阅预期',
  resolve_expected_event: '处理预期',
  expectation_reviewer: '预期检阅',
  get_project_status: '项目状态',
  design_entity: '设计设定卡',
  design_plot: '设计剧情卡',
  update_plot: '更新剧情卡',
  design_master_outline: '设计总纲',
  design_arc_outline: '设计卷纲',
  sync_volume: '同步设定库',
  create_novel: '创建小说',
  init_novel: '创建小说',
  lock_brief: '锁定灵感',
  confirm_setup: '确认定稿',
  activate_agents: '激活 Agent',
  spawn_agent: '启动子 Agent',
  wait_agent: '等待子 Agent',
  list_agents: '列出子 Agent',
  interrupt_agent: '中断子 Agent',
  send_message: '发送消息',
  followup_task: '追加任务',
  upsert_setting: '写入设定',
  supplement_setting: '补充设定',
  audit_setting: '设定审计',
  delete_entity: '删除设定卡',
  list_projects: '列出项目',
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
  return '运行中'
}

export function toolLabelZh(name) {
  if (!name) return ''
  const key = String(name).trim()
  return TOOL_LABELS_ZH[key] || key
}

/** Normalize a progress step token (English id or Chinese label) to Chinese. */
export function stepLabelZh(token) {
  if (!token) return ''
  const t = String(token).trim()
  if (TOOL_LABELS_ZH[t]) return TOOL_LABELS_ZH[t]
  // Already Chinese (or unknown) — show as-is.
  return t
}
