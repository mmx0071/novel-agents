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
  { id: 'rules', label: '硬规则' },
  { id: 'naming', label: '禁名' },
  { id: 'skills', label: 'Skills' },
]

export default function ConfigPanel() {
  const [tab, setTab] = useState('rules')
  const [yaml, setYaml] = useState('')
  const [catalog, setCatalog] = useState([])
  const [skills, setSkills] = useState([])
  const [skillName, setSkillName] = useState('')
  const [skillContent, setSkillContent] = useState('')
  const [skillEditable, setSkillEditable] = useState(false)
  const [skillMeta, setSkillMeta] = useState(null)
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
    if (tab === 'rules') loadRules()
    else if (tab === 'naming') loadNaming()
    else loadSkills()
  }, [tab, loadRules, loadNaming, loadSkills])

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

  return (
    <section className="panel config-panel">
      <div className="config-head">
        <h2>配置</h2>
        <p className="side-hint">编辑框架硬规则、禁名与 Skills；落盘后热重载，无需重启。</p>
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

      {tab === 'rules' && (
        <div className="config-body">
          <ul className="config-rule-list">
            {catalog.map((r) => (
              <li key={r.id}>
                <div className="config-rule-title">
                  <span>{r.title || r.id}</span>
                  <span className="config-rule-flags">
                    {r.enabled ? '启用' : '关闭'}
                    {r.blocking ? ' · 阻断' : ' · 警告'}
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
