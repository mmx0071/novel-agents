import { useCallback, useEffect, useState } from 'react'

const API = '/api'

async function api(path, options = {}) {
  const res = await fetch(`${API}${path}`, {
    headers: { 'Content-Type': 'application/json' },
    ...options,
  })
  let data = {}
  try {
    data = await res.json()
  } catch {
    data = {}
  }
  if (!res.ok) {
    return { error: data.error || `请求失败 (${res.status})`, ...data }
  }
  return data
}

const TABS = [
  { id: 'llm', label: '模型' },
  { id: 'rules', label: '硬规则' },
  { id: 'naming', label: '禁名' },
  { id: 'skills', label: 'Skills' },
]

const EMPTY_LLM = {
  profile: 'dev',
  dev_model: 'deepseek-v4-flash',
  default_provider: 'deepseek',
  providers: [],
  tasks: [],
  retry: { max_retries: 3, base_delay_ms: 800, max_delay_ms: 10000 },
}

export default function ConfigPanel() {
  const [tab, setTab] = useState('llm')
  const [yaml, setYaml] = useState('')
  const [catalog, setCatalog] = useState([])
  const [skills, setSkills] = useState([])
  const [skillName, setSkillName] = useState('')
  const [skillContent, setSkillContent] = useState('')
  const [skillEditable, setSkillEditable] = useState(false)
  const [skillMeta, setSkillMeta] = useState(null)
  const [llm, setLlm] = useState(EMPTY_LLM)
  const [llmRuntime, setLlmRuntime] = useState(null)
  const [apiKeyDraft, setApiKeyDraft] = useState('')
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [status, setStatus] = useState('')

  const loadRules = useCallback(async () => {
    setLoading(true)
    setError('')
    setStatus('')
    const data = await api('/config/content_rules')
    setLoading(false)
    if (data.error) {
      setError(data.error)
      return
    }
    setYaml(data.yaml || '')
    setCatalog(Array.isArray(data.rules) ? data.rules : [])
  }, [])

  const loadNaming = useCallback(async () => {
    setLoading(true)
    setError('')
    setStatus('')
    const data = await api('/config/naming_rules')
    setLoading(false)
    if (data.error) {
      setError(data.error)
      return
    }
    setYaml(data.yaml || '')
  }, [])

  const loadLlm = useCallback(async () => {
    setLoading(true)
    setError('')
    setStatus('')
    const data = await api('/config/llm')
    setLoading(false)
    if (data.error) {
      setError(data.error)
      return
    }
    const settings = data.settings || EMPTY_LLM
    setLlm({
      profile: settings.profile || 'dev',
      dev_model: settings.dev_model || '',
      default_provider: settings.default_provider || 'deepseek',
      providers: Array.isArray(settings.providers) ? settings.providers : [],
      tasks: Array.isArray(settings.tasks) ? settings.tasks : [],
      retry: settings.retry || EMPTY_LLM.retry,
    })
    setLlmRuntime(data.runtime || null)
    // Write-only: never populate key from server.
    setApiKeyDraft('')
  }, [])

  const loadSkills = useCallback(async () => {
    setLoading(true)
    setError('')
    setStatus('')
    const data = await api('/skills/list')
    setLoading(false)
    if (data.error) {
      setError(data.error)
      return
    }
    const list = Array.isArray(data.skills) ? data.skills : []
    setSkills(list)
    if (!skillName && list.length) {
      setSkillName(list[0].name)
    }
  }, [skillName])

  const loadSkill = useCallback(async (name) => {
    if (!name) return
    setLoading(true)
    setError('')
    setStatus('')
    const data = await api(`/skills/${encodeURIComponent(name)}`)
    setLoading(false)
    if (data.error) {
      setError(data.error)
      setSkillContent('')
      setSkillEditable(false)
      setSkillMeta(null)
      return
    }
    setSkillContent(data.content || '')
    setSkillEditable(!!data.editable)
    setSkillMeta({
      path: data.path,
      scope: data.scope,
      description: data.description,
    })
  }, [])

  useEffect(() => {
    if (tab === 'llm') loadLlm()
    else if (tab === 'rules') loadRules()
    else if (tab === 'naming') loadNaming()
    else loadSkills()
  }, [tab, loadLlm, loadRules, loadNaming, loadSkills])

  useEffect(() => {
    if (tab === 'skills' && skillName) {
      loadSkill(skillName)
    }
  }, [tab, skillName, loadSkill])

  async function saveYaml(endpoint) {
    setSaving(true)
    setError('')
    setStatus('')
    const data = await api(endpoint, {
      method: 'PUT',
      body: JSON.stringify({ content: yaml }),
    })
    setSaving(false)
    if (data.error || data.ok === false) {
      setError(data.error || '保存失败')
      return
    }
    setStatus('已保存（下次写章/审校即生效）')
    if (Array.isArray(data.rules)) setCatalog(data.rules)
  }

  async function toggleRuleFlag(id, patch) {
    setSaving(true)
    setError('')
    setStatus('')
    const data = await api('/config/content_rules/flags', {
      method: 'PUT',
      body: JSON.stringify({ id, ...patch }),
    })
    setSaving(false)
    if (data.error || data.ok === false) {
      setError(data.error || '切换失败')
      return
    }
    if (typeof data.yaml === 'string') setYaml(data.yaml)
    if (Array.isArray(data.rules)) setCatalog(data.rules)
    const label = patch.enabled === false
      ? '已关闭'
      : patch.enabled === true
        ? '已启用'
        : patch.blocking === false
          ? '已改为警告（不阻断发布）'
          : '已改为阻断'
    setStatus(`${label}（下次写章/审校即生效）`)
  }

  async function saveSkill() {
    if (!skillName || !skillEditable) return
    setSaving(true)
    setError('')
    setStatus('')
    const data = await api(`/skills/${encodeURIComponent(skillName)}`, {
      method: 'PUT',
      body: JSON.stringify({ content: skillContent }),
    })
    setSaving(false)
    if (data.error || data.ok === false) {
      setError(data.error || '保存失败')
      return
    }
    setStatus('已保存并热重载 Skills')
  }

  async function saveLlm() {
    setSaving(true)
    setError('')
    setStatus('')
    const body = {
      profile: llm.profile,
      dev_model: llm.dev_model,
      default_provider: llm.default_provider,
      providers: llm.providers.map((p) => ({
        name: p.name,
        base_url: p.base_url ?? null,
        api_key_env: p.api_key_env,
      })),
      tasks: llm.tasks.map((t) => ({
        name: t.name,
        description: t.description || '',
        model: t.model,
        max_tokens: Number(t.max_tokens) || 256,
      })),
      retry: {
        max_retries: Number(llm.retry.max_retries) || 0,
        base_delay_ms: Number(llm.retry.base_delay_ms) || 100,
        max_delay_ms: Number(llm.retry.max_delay_ms) || 1000,
      },
    }
    const key = apiKeyDraft.trim()
    if (key) body.api_key = key
    const data = await api('/config/llm', {
      method: 'PUT',
      body: JSON.stringify(body),
    })
    setSaving(false)
    if (data.error || data.ok === false) {
      setError(data.error || '保存失败')
      return
    }
    setApiKeyDraft('')
    if (data.settings) {
      setLlm({
        profile: data.settings.profile || llm.profile,
        dev_model: data.settings.dev_model || '',
        default_provider: data.settings.default_provider || 'deepseek',
        providers: Array.isArray(data.settings.providers) ? data.settings.providers : [],
        tasks: Array.isArray(data.settings.tasks) ? data.settings.tasks : [],
        retry: data.settings.retry || llm.retry,
      })
    }
    setLlmRuntime(data.runtime || null)
    setStatus('已保存并热重载 LLM 配置')
  }

  const activeProvider =
    llm.providers.find((p) => p.name === llm.default_provider) || llm.providers[0] || null

  function updateActiveProvider(patch) {
    if (!activeProvider) return
    setLlm((prev) => ({
      ...prev,
      providers: prev.providers.map((p) =>
        p.name === activeProvider.name ? { ...p, ...patch } : p,
      ),
    }))
  }

  function updateTask(index, patch) {
    setLlm((prev) => ({
      ...prev,
      tasks: prev.tasks.map((t, i) => (i === index ? { ...t, ...patch } : t)),
    }))
  }

  const keyStatusLabel = (() => {
    const has = activeProvider?.has_api_key || llmRuntime?.has_api_key
    const suffix = activeProvider?.api_key_suffix || llmRuntime?.api_key_suffix
    if (has && suffix) return `已配置 ·•••${suffix}`
    if (has) return '已配置'
    return '未配置'
  })()

  return (
    <section className="panel config-panel">
      <div className="config-head">
        <h2>配置</h2>
        <p className="side-hint">编辑模型、硬规则、禁名与 Skills；落盘后热重载，无需重启。</p>
      </div>

      <div className="panel-tabs" role="tablist">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={tab === t.id}
            className={tab === t.id ? 'active' : ''}
            onClick={() => setTab(t.id)}
          >
            {t.label}
          </button>
        ))}
      </div>

      {error ? <div className="config-banner config-banner-error">{error}</div> : null}
      {status ? <div className="config-banner config-banner-ok">{status}</div> : null}
      {loading ? <div className="config-muted">加载中…</div> : null}

      {tab === 'llm' && (
        <div className="config-body config-llm">
          <div className="config-llm-grid">
            <label className="config-field">
              <span className="config-label">Profile</span>
              <select
                value={llm.profile}
                onChange={(e) => setLlm((p) => ({ ...p, profile: e.target.value }))}
              >
                <option value="dev">dev（全部用 dev_model）</option>
                <option value="prod">prod（按任务分模型）</option>
              </select>
            </label>
            <label className="config-field">
              <span className="config-label">dev_model</span>
              <input
                type="text"
                value={llm.dev_model}
                onChange={(e) => setLlm((p) => ({ ...p, dev_model: e.target.value }))}
                autoComplete="off"
              />
            </label>
            <label className="config-field">
              <span className="config-label">Provider</span>
              <select
                value={llm.default_provider}
                onChange={(e) => setLlm((p) => ({ ...p, default_provider: e.target.value }))}
              >
                {llm.providers.map((p) => (
                  <option key={p.name} value={p.name}>
                    {p.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="config-field">
              <span className="config-label">Base URL</span>
              <input
                type="text"
                value={activeProvider?.base_url || ''}
                onChange={(e) => updateActiveProvider({ base_url: e.target.value })}
                autoComplete="off"
                spellCheck={false}
              />
            </label>
            <label className="config-field config-field-wide">
              <span className="config-label">
                API Key <span className="config-muted-inline">（{keyStatusLabel} · 只写不读）</span>
              </span>
              <input
                type="password"
                value={apiKeyDraft}
                onChange={(e) => setApiKeyDraft(e.target.value)}
                placeholder="粘贴新 Key（留空则不修改）"
                autoComplete="new-password"
                spellCheck={false}
              />
            </label>
          </div>

          <div className="config-llm-section">
            <div className="config-label">任务模型</div>
            <div className="config-task-table">
              <div className="config-task-head">
                <span>任务</span>
                <span>模型</span>
                <span>max_tokens</span>
              </div>
              {llm.tasks.map((t, i) => (
                <div key={t.name} className="config-task-row">
                  <div className="config-task-name" title={t.description || t.name}>
                    {t.name}
                  </div>
                  <input
                    type="text"
                    value={t.model}
                    onChange={(e) => updateTask(i, { model: e.target.value })}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <input
                    type="number"
                    min={256}
                    max={384000}
                    value={t.max_tokens}
                    onChange={(e) => updateTask(i, { max_tokens: e.target.value })}
                  />
                </div>
              ))}
            </div>
          </div>

          <div className="config-llm-grid config-llm-retry">
            <label className="config-field">
              <span className="config-label">max_retries</span>
              <input
                type="number"
                min={0}
                max={8}
                value={llm.retry.max_retries}
                onChange={(e) =>
                  setLlm((p) => ({
                    ...p,
                    retry: { ...p.retry, max_retries: e.target.value },
                  }))
                }
              />
            </label>
            <label className="config-field">
              <span className="config-label">base_delay_ms</span>
              <input
                type="number"
                min={100}
                value={llm.retry.base_delay_ms}
                onChange={(e) =>
                  setLlm((p) => ({
                    ...p,
                    retry: { ...p.retry, base_delay_ms: e.target.value },
                  }))
                }
              />
            </label>
            <label className="config-field">
              <span className="config-label">max_delay_ms</span>
              <input
                type="number"
                min={100}
                value={llm.retry.max_delay_ms}
                onChange={(e) =>
                  setLlm((p) => ({
                    ...p,
                    retry: { ...p.retry, max_delay_ms: e.target.value },
                  }))
                }
              />
            </label>
          </div>

          {llmRuntime ? (
            <div className="config-muted">
              运行时：{llmRuntime.default_model}
              {llmRuntime.api_key_env ? ` · env=${llmRuntime.api_key_env}` : ''}
              {llmRuntime.has_api_key
                ? llmRuntime.api_key_suffix
                  ? ` · Key ·•••${llmRuntime.api_key_suffix}`
                  : ' · Key 已配置'
                : ' · Key 未配置'}
            </div>
          ) : null}

          <div className="config-actions">
            <button type="button" onClick={loadLlm} disabled={loading || saving}>
              刷新
            </button>
            <button
              type="button"
              className="primary"
              onClick={saveLlm}
              disabled={loading || saving}
            >
              {saving ? '保存中…' : '保存'}
            </button>
          </div>
        </div>
      )}

      {tab === 'rules' && (
        <div className="config-body">
          <p className="config-muted">
            点击「启用/关闭」或「阻断/警告」会立即保存。关闭=不再扫描；警告=仅提示、不阻断发布。
          </p>
          <ul className="config-rule-list">
            {catalog.map((r) => (
              <li key={r.id}>
                <div className="config-rule-title">
                  <span>{r.title || r.id}</span>
                  <span className="config-rule-flags">
                    <button
                      type="button"
                      className={`config-flag-btn ${r.enabled ? 'on' : 'off'}`}
                      disabled={saving}
                      onClick={() => toggleRuleFlag(r.id, { enabled: !r.enabled })}
                      title="切换是否扫描本规则"
                    >
                      {r.enabled ? '启用' : '关闭'}
                    </button>
                    <button
                      type="button"
                      className={`config-flag-btn ${r.blocking ? 'block' : 'warn'}`}
                      disabled={saving || !r.enabled}
                      onClick={() => toggleRuleFlag(r.id, { blocking: !r.blocking })}
                      title="阻断=不发布；警告=仅提示仍可发布"
                    >
                      {r.blocking ? '阻断' : '警告'}
                    </button>
                  </span>
                </div>
                <p>{r.description || '—'}</p>
              </li>
            ))}
          </ul>
          <label className="config-label" htmlFor="content-rules-yaml">content_rules.yaml</label>
          <textarea
            id="content-rules-yaml"
            className="config-editor"
            value={yaml}
            onChange={(e) => setYaml(e.target.value)}
            spellCheck={false}
          />
          <div className="config-actions">
            <button type="button" onClick={loadRules} disabled={loading || saving}>刷新</button>
            <button
              type="button"
              className="primary"
              onClick={() => saveYaml('/config/content_rules')}
              disabled={loading || saving}
            >
              {saving ? '保存中…' : '保存'}
            </button>
          </div>
        </div>
      )}

      {tab === 'naming' && (
        <div className="config-body">
          <label className="config-label" htmlFor="naming-rules-yaml">naming_rules.yaml</label>
          <textarea
            id="naming-rules-yaml"
            className="config-editor"
            value={yaml}
            onChange={(e) => setYaml(e.target.value)}
            spellCheck={false}
          />
          <div className="config-actions">
            <button type="button" onClick={loadNaming} disabled={loading || saving}>刷新</button>
            <button
              type="button"
              className="primary"
              onClick={() => saveYaml('/config/naming_rules')}
              disabled={loading || saving}
            >
              {saving ? '保存中…' : '保存'}
            </button>
          </div>
        </div>
      )}

      {tab === 'skills' && (
        <div className="config-body config-skills">
          <div className="config-skill-list">
            {skills.map((s) => (
              <button
                key={s.name}
                type="button"
                className={skillName === s.name ? 'active' : ''}
                onClick={() => setSkillName(s.name)}
                title={s.description}
              >
                <span className="config-skill-name">{s.name}</span>
                <span className="config-skill-scope">{s.scope}</span>
              </button>
            ))}
          </div>
          <div className="config-skill-editor">
            {skillMeta ? (
              <div className="config-muted">
                {skillMeta.path}
                {skillMeta.description ? ` · ${skillMeta.description}` : ''}
                {!skillEditable ? ' · 只读（项目 Skill）' : ''}
              </div>
            ) : null}
            <textarea
              className="config-editor"
              value={skillContent}
              onChange={(e) => setSkillContent(e.target.value)}
              spellCheck={false}
              disabled={!skillEditable}
              aria-label="Skill 正文"
            />
            <div className="config-actions">
              <button type="button" onClick={() => loadSkill(skillName)} disabled={loading || saving}>
                刷新
              </button>
              <button
                type="button"
                className="primary"
                onClick={saveSkill}
                disabled={!skillEditable || loading || saving}
              >
                {saving ? '保存中…' : '保存'}
              </button>
            </div>
          </div>
        </div>
      )}
    </section>
  )
}
