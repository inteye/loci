import { useEffect, useMemo, useState, type ReactNode } from 'react'
import { invoke } from '@tauri-apps/api/core'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

type Panel = 'chat' | 'settings' | 'trace' | 'docs' | 'eval' | 'graph' | 'memory'
type Operation = 'project' | 'index' | 'ask' | 'settings' | 'trace' | 'docs' | 'eval' | 'graph' | 'memory'
type AsyncState = 'idle' | 'loading' | 'success' | 'error'
type ProviderProtocol = 'openai' | 'litellm' | 'anthropic'
type BuiltInProviderId = 'openai' | 'anthropic' | 'ollama' | 'qwen_coding_plan' | 'litellm' | 'custom'

interface GraphNode {
  id: string
  label: string
  kind: string
  description?: string
  file_path?: string
}

interface GraphEdge {
  from: string
  to: string
  kind: string
}

interface GraphData {
  nodes: GraphNode[]
  edges: GraphEdge[]
}

interface TraceData {
  anchors: GraphNode[]
  decisions: GraphNode[]
  commits: GraphNode[]
  evidence: GraphEdge[]
  related: GraphNode[]
}

interface DocData {
  kind: string
  content: string
}

interface EvalScore {
  score: number
  rationale: string
}

interface EvalResult {
  category: string
  prompt: string
  answer: string
  score: EvalScore
}

interface EvalData {
  average_score: number
  results: EvalResult[]
  drift_check: string[]
}

interface ProviderSettingsData {
  name: string
  protocol: ProviderProtocol
  base_url: string
  api_key: string
  api_key_env: string
  model: string
  preset?: BuiltInProviderId
}

interface ModelSettingsData {
  config_path: string
  default_provider?: string | null
  providers: ProviderSettingsData[]
}

interface StatusEntry {
  state: AsyncState
  message: string
}

interface ChatMessage {
  id: string
  role: 'user' | 'assistant' | 'system'
  content: string
  title?: string
}

const panelHelp: Record<Panel, { title: string; description: string }> = {
  chat: {
    title: '代码库问答',
    description: '索引完成后，在主聊天区直接提问架构、职责、设计原因和上手路径。',
  },
  settings: {
    title: '模型设置',
    description: '管理默认模型、协议类型、Base URL 和密钥来源。这里是桌面端能否正常工作的基础设置。',
  },
  trace: {
    title: '追溯分析',
    description: '按文件路径或符号查看决策节点、提交记录和证据边，定位“为什么这样设计”。',
  },
  docs: {
    title: '文档生成',
    description: '根据当前图谱、概念和决策生成入门文档、模块说明或交接文档。',
  },
  eval: {
    title: '质量评测',
    description: '用内置评测问题检查当前索引对架构理解、Trace 和上手引导的支持效果。',
  },
  graph: {
    title: '图谱浏览',
    description: '查看当前索引生成了哪些节点和边，确认图谱到底知道什么。',
  },
  memory: {
    title: '近期记忆',
    description: '查看桌面端问答过程中沉淀下来的近期短期记忆内容。',
  },
}

const statusDefaults: Record<Operation, StatusEntry> = {
  project: { state: 'idle', message: '先选择项目目录，再执行索引和问答。' },
  index: { state: 'idle', message: '开始提问前，先为当前项目建立索引。' },
  ask: { state: 'idle', message: '直接输入关于当前代码库的问题。' },
  settings: { state: 'idle', message: '先配置可用模型，再执行问答、文档或评测。' },
  trace: { state: 'idle', message: '输入文件路径或符号名称查看 Trace 证据。' },
  docs: { state: 'idle', message: '生成当前项目的内置文档视图。' },
  eval: { state: 'idle', message: '运行评测问题，检查当前索引质量。' },
  graph: { state: 'idle', message: '加载图谱，查看节点和边的情况。' },
  memory: { state: 'idle', message: '加载桌面端问答生成的近期记忆。' },
}

const suggestedQuestions = [
  '这个项目的核心模块是什么？',
  '为什么这里会采用当前的设计？',
  '如果我是新同事，应该先看哪几个文件？',
]

const builtInProviders: Array<{
  id: Exclude<BuiltInProviderId, 'custom'>
  label: string
  description: string
  protocol: ProviderProtocol
  base_url: string
  model: string
  api_key_env: string
  api_key: string
  requiresApiKey: boolean
}> = [
  {
    id: 'openai',
    label: 'OpenAI',
    description: '官方 OpenAI 服务。内置协议和默认地址，通常只需要填写 API Key。',
    protocol: 'openai',
    base_url: '',
    model: 'gpt-4o-mini',
    api_key_env: 'OPENAI_API_KEY',
    api_key: '',
    requiresApiKey: true,
  },
  {
    id: 'anthropic',
    label: 'Anthropic',
    description: '官方 Anthropic Messages API。内置协议和默认地址，通常只需要填写 API Key。',
    protocol: 'anthropic',
    base_url: 'https://api.anthropic.com/v1',
    model: 'claude-3-7-sonnet-latest',
    api_key_env: 'ANTHROPIC_API_KEY',
    api_key: '',
    requiresApiKey: true,
  },
  {
    id: 'ollama',
    label: 'Ollama',
    description: '本地 Ollama 服务。默认直连本机地址，不需要额外 API Key。',
    protocol: 'openai',
    base_url: 'http://localhost:11434/v1',
    model: 'qwen2.5-coder:7b',
    api_key_env: '',
    api_key: 'ollama',
    requiresApiKey: false,
  },
  {
    id: 'qwen_coding_plan',
    label: 'Qwen Coding Plan',
    description: '内置 Qwen 编码模型入口，默认按 OpenAI 兼容接口接入，只需要填写 API Key。',
    protocol: 'openai',
    base_url: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    model: 'qwen-plus',
    api_key_env: 'DASHSCOPE_API_KEY',
    api_key: '',
    requiresApiKey: true,
  },
  {
    id: 'litellm',
    label: 'litellm-rs Gateway',
    description: '推荐模式。基于 litellm-rs 统一管理多模型，loci 只连接一个 OpenAI 兼容网关地址。',
    protocol: 'litellm',
    base_url: 'http://localhost:4000/v1',
    model: 'gpt-4o-mini',
    api_key_env: 'LITELLM_API_KEY',
    api_key: '',
    requiresApiKey: true,
  },
]

const protocolOptions: Array<{ value: ProviderProtocol; label: string; helper: string }> = [
  {
    value: 'openai',
    label: 'OpenAI 协议',
    helper: '适用于 OpenAI、OpenRouter、Ollama、Groq、DeepSeek、LM Studio 等兼容 `/v1` 接口的服务。',
  },
  {
    value: 'litellm',
    label: 'litellm-rs 网关',
    helper: '推荐生产接入方式。通过 litellm-rs 统一接入多模型，再由 loci 只对接一个 OpenAI 兼容网关地址。',
  },
  {
    value: 'anthropic',
    label: 'Anthropic 协议',
    helper: '适用于 Anthropic Messages API 或兼容其消息协议的服务。',
  },
]

function emptyProvider(index: number): ProviderSettingsData {
  return {
    name: `provider-${index + 1}`,
    protocol: 'openai',
    base_url: '',
    api_key: '',
    api_key_env: '',
    model: '',
    preset: 'custom',
  }
}

function inferPreset(provider: ProviderSettingsData): BuiltInProviderId {
  if (provider.name === 'openai') return 'openai'
  if (provider.name === 'anthropic') return 'anthropic'
  if (provider.name === 'ollama') return 'ollama'
  if (provider.name === 'qwen_coding_plan') return 'qwen_coding_plan'
  if (provider.name === 'litellm' || provider.protocol === 'litellm') return 'litellm'
  return 'custom'
}

function createBuiltInProvider(id: Exclude<BuiltInProviderId, 'custom'>): ProviderSettingsData {
  const preset = builtInProviders.find((item) => item.id === id)!
  return {
    name: preset.id,
    protocol: preset.protocol,
    base_url: preset.base_url,
    api_key: preset.api_key,
    api_key_env: preset.api_key_env,
    model: preset.model,
    preset: preset.id,
  }
}

function builtInLabel(provider: ProviderSettingsData): string {
  if (provider.preset && provider.preset !== 'custom') {
    return builtInProviders.find((item) => item.id === provider.preset)?.label ?? provider.name
  }
  return provider.name || '自定义 provider'
}

function isBuiltInProvider(provider: ProviderSettingsData): boolean {
  return Boolean(provider.preset && provider.preset !== 'custom')
}

function makeMessage(role: ChatMessage['role'], content: string, title?: string): ChatMessage {
  return {
    id: `${role}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
    role,
    content,
    title,
  }
}

export default function App() {
  const [panel, setPanel] = useState<Panel>('chat')
  const [projectPath, setProjectPath] = useState('.')
  const [question, setQuestion] = useState('')
  const [messages, setMessages] = useState<ChatMessage[]>([
    makeMessage(
      'system',
      '先选择项目并完成索引，再确认顶部的模型设置已经可用，然后在这里提问架构、追溯原因、设计决策或新人上手问题。',
      '欢迎使用',
    ),
  ])
  const [traceTarget, setTraceTarget] = useState('')
  const [docKind, setDocKind] = useState('onboarding')
  const [graph, setGraph] = useState<GraphData | null>(null)
  const [trace, setTrace] = useState<TraceData | null>(null)
  const [doc, setDoc] = useState<DocData | null>(null)
  const [evalData, setEvalData] = useState<EvalData | null>(null)
  const [memories, setMemories] = useState<string[]>([])
  const [settings, setSettings] = useState<ModelSettingsData | null>(null)
  const [settingsExpanded, setSettingsExpanded] = useState(true)
  const [settingsTestResult, setSettingsTestResult] = useState<string>('')
  const [statuses, setStatuses] = useState<Record<Operation, StatusEntry>>(statusDefaults)

  const activeStatus = useMemo(() => {
    const order: Operation[] = ['project', 'index', 'settings', 'ask', 'trace', 'docs', 'eval', 'graph', 'memory']
    return order.find((key) => statuses[key].state === 'loading')
  }, [statuses])

  const defaultProviderName = settings?.default_provider ?? settings?.providers[0]?.name ?? '未设置'
  const needsSettingsAttention = !settings || settings.providers.length === 0 || defaultProviderName === '未设置'

  useEffect(() => {
    invoke<string>('get_default_project_path')
      .then((path) => {
        setProjectPath(path)
        setStatuses((prev) => ({
          ...prev,
          project: { state: 'success', message: `已加载默认项目：${path}` },
        }))
      })
      .catch((error) => {
        setStatuses((prev) => ({
          ...prev,
          project: { state: 'error', message: `加载默认项目失败：${String(error)}` },
        }))
      })
  }, [])

  useEffect(() => {
    void loadSettings()
  }, [projectPath])

  useEffect(() => {
    if (needsSettingsAttention) {
      setSettingsExpanded(true)
    }
  }, [needsSettingsAttention])

  useEffect(() => {
    if (panel === 'trace' && !trace && statuses.trace.state === 'idle') {
      void loadTrace(traceTarget)
    }
    if (panel === 'docs' && !doc && statuses.docs.state === 'idle') {
      void loadDoc(docKind)
    }
    if (panel === 'eval' && !evalData && statuses.eval.state === 'idle') {
      void loadEval()
    }
    if (panel === 'graph' && !graph && statuses.graph.state === 'idle') {
      void loadGraph()
    }
    if (panel === 'memory' && memories.length === 0 && statuses.memory.state === 'idle') {
      void loadMemories()
    }
  }, [panel])

  function updateStatus(operation: Operation, state: AsyncState, message: string) {
    setStatuses((prev) => ({
      ...prev,
      [operation]: { state, message },
    }))
  }

  function resetProjectViews() {
    setGraph(null)
    setTrace(null)
    setDoc(null)
    setEvalData(null)
    setMemories([])
    setSettings(null)
  }

  async function handlePickProjectPath() {
    updateStatus('project', 'loading', '正在打开项目目录选择器...')
    try {
      const selected = await invoke<string | null>('pick_project_directory')
      if (!selected) {
        updateStatus('project', 'idle', '已取消项目目录选择。')
        return
      }
      setProjectPath(selected)
      resetProjectViews()
      setMessages((prev) => [
        prev[0],
        makeMessage('system', `项目路径已更新为 \`${selected}\`。\n\n如果这是一个新项目，请先重新执行索引，并确认模型设置。`, '项目已切换'),
      ])
      updateStatus('project', 'success', `已选择项目：${selected}`)
    } catch (error) {
      updateStatus('project', 'error', `打开项目选择器失败：${String(error)}`)
    }
  }

  async function handleIndex() {
    updateStatus('index', 'loading', '正在索引项目并重建本地图谱...')
    try {
      const result = await invoke<string>('index_project', { projectPath })
      setGraph(null)
      setTrace(null)
      setDoc(null)
      setEvalData(null)
      setMemories([])
      setMessages((prev) => [
        ...prev,
        makeMessage('system', `索引完成。\n\n${result}`, '索引完成'),
      ])
      updateStatus('index', 'success', result)
    } catch (error) {
      updateStatus('index', 'error', `索引失败：${String(error)}`)
    }
  }

  async function handleAsk() {
    const trimmed = question.trim()
    if (!trimmed) {
      updateStatus('ask', 'error', '请输入问题后再发送。')
      return
    }

    const userMessage = makeMessage('user', trimmed)
    setMessages((prev) => [...prev, userMessage])
    setQuestion('')
    updateStatus('ask', 'loading', '正在结合当前索引和模型设置生成回答...')

    try {
      const answer = await invoke<string>('ask', { projectPath, question: trimmed })
      setMessages((prev) => [...prev, makeMessage('assistant', answer, '回答')])
      updateStatus('ask', 'success', `已使用默认模型「${defaultProviderName}」生成回答。`)
    } catch (error) {
      const message = `问答失败：${String(error)}`
      setMessages((prev) => [...prev, makeMessage('system', message, '错误')])
      updateStatus('ask', 'error', message)
    }
  }

  async function loadSettings() {
    updateStatus('settings', 'loading', '正在读取当前项目的模型设置...')
    try {
      const data = await invoke<ModelSettingsData>('get_model_settings', { projectPath })
      setSettings({
        ...data,
        providers: (data.providers.length > 0 ? data.providers : [emptyProvider(0)]).map((provider) => ({
          ...provider,
          preset: inferPreset(provider),
        })),
      })
      updateStatus('settings', 'success', `已加载 ${data.providers.length} 个模型配置。`)
    } catch (error) {
      setSettings(null)
      updateStatus('settings', 'error', `设置加载失败：${String(error)}`)
    }
  }

  async function saveSettings() {
    if (!settings) {
      updateStatus('settings', 'error', '当前没有可保存的设置。')
      return
    }

    updateStatus('settings', 'loading', '正在保存模型设置...')
    try {
      const message = await invoke<string>('save_model_settings', {
        projectPath,
        settings,
      })
      updateStatus('settings', 'success', message)
      setSettingsExpanded(false)
      setMessages((prev) => [
        ...prev,
        makeMessage('system', `${message}\n\n默认模型：${settings.default_provider ?? settings.providers[0]?.name ?? '未设置'}`, '设置已保存'),
      ])
    } catch (error) {
      updateStatus('settings', 'error', `设置保存失败：${String(error)}`)
    }
  }

  async function testSettingsConnection(provider?: string | null) {
    updateStatus('settings', 'loading', '正在测试模型连接...')
    try {
      const result = await invoke<string>('test_model_connection', {
        projectPath,
        provider: provider ?? null,
      })
      setSettingsTestResult(result)
      updateStatus('settings', 'success', '模型连接测试成功。')
    } catch (error) {
      setSettingsTestResult('')
      updateStatus('settings', 'error', `连接测试失败：${String(error)}`)
    }
  }

  function updateProvider(index: number, field: keyof ProviderSettingsData, value: string) {
    setSettings((prev) => {
      if (!prev) return prev
      const providers = [...prev.providers]
      const current = providers[index]
      if (!current) return prev
      const nextName = field === 'name' ? value : current.name
      providers[index] = {
        ...current,
        [field]: field === 'protocol' ? value as ProviderProtocol : value,
      }

      let defaultProvider = prev.default_provider
      if (current.name === prev.default_provider && field === 'name') {
        defaultProvider = nextName
      }
      if (defaultProvider && !providers.some((provider) => provider.name === defaultProvider)) {
        defaultProvider = providers[0]?.name ?? null
      }

      return {
        ...prev,
        providers,
        default_provider: defaultProvider,
      }
    })
    updateStatus('settings', 'idle', '模型设置已修改，记得保存。')
  }

  function addBuiltInProvider(id: Exclude<BuiltInProviderId, 'custom'>) {
    setSettings((prev) => {
      const next = prev ?? {
        config_path: `${projectPath}/.bs/config.toml`,
        default_provider: null,
        providers: [],
      }
      const provider = createBuiltInProvider(id)
      const existingIndex = next.providers.findIndex((item) => item.name === provider.name)
      const providers = existingIndex >= 0
        ? next.providers.map((item, index) => index === existingIndex ? { ...item, ...provider } : item)
        : [...next.providers, provider]
      return {
        ...next,
        providers,
        default_provider: next.default_provider ?? providers[0]?.name ?? null,
      }
    })
    setSettingsExpanded(true)
    updateStatus('settings', 'idle', `已加入 ${builtInProviders.find((item) => item.id === id)?.label}，填写后记得保存。`)
  }

  function addCustomProvider() {
    setSettings((prev) => {
      const next = prev ?? {
        config_path: `${projectPath}/.bs/config.toml`,
        default_provider: null,
        providers: [],
      }
      const providers = [...next.providers, emptyProvider(next.providers.length)]
      return {
        ...next,
        providers,
        default_provider: next.default_provider ?? providers[0]?.name ?? null,
      }
    })
    setSettingsExpanded(true)
    updateStatus('settings', 'idle', '已新增自定义 provider，填写协议、接口地址和 API Key 后记得保存。')
  }

  function removeProvider(index: number) {
    setSettings((prev) => {
      if (!prev) return prev
      const provider = prev.providers[index]
      const providers = prev.providers.filter((_, current) => current !== index)
      const nextProviders = providers.length > 0 ? providers : [emptyProvider(0)]
      return {
        ...prev,
        providers: nextProviders,
        default_provider:
          prev.default_provider === provider?.name
            ? nextProviders[0]?.name ?? null
            : prev.default_provider,
      }
    })
    updateStatus('settings', 'idle', '已移除一个模型配置项。')
  }

  async function loadTrace(target = traceTarget) {
    updateStatus(
      'trace',
      'loading',
      target.trim() ? `正在加载 “${target.trim()}” 的 Trace 视图...` : '正在加载最近的决策 Trace...',
    )
    try {
      const data = await invoke<TraceData>('get_trace', { projectPath, target })
      setTrace(data)
      updateStatus('trace', 'success', `已加载 ${data.decisions.length} 个决策、${data.commits.length} 个提交、${data.evidence.length} 条证据边。`)
    } catch (error) {
      setTrace(null)
      updateStatus('trace', 'error', `Trace 加载失败：${String(error)}`)
    }
  }

  async function loadDoc(kind = docKind) {
    updateStatus('docs', 'loading', `正在根据本地图谱生成 ${kind} 文档...`)
    try {
      const data = await invoke<DocData>('get_doc', { projectPath, kind, provider: null })
      setDoc(data)
      updateStatus('docs', 'success', `${kind} 文档已生成。`)
    } catch (error) {
      setDoc(null)
      updateStatus('docs', 'error', `文档生成失败：${String(error)}`)
    }
  }

  async function loadEval() {
    updateStatus('eval', 'loading', '正在针对当前索引运行评测问题...')
    try {
      const data = await invoke<EvalData>('get_eval', { projectPath, provider: null })
      setEvalData(data)
      updateStatus('eval', 'success', `平均得分 ${data.average_score.toFixed(2)}/5，共 ${data.results.length} 个评测问题。`)
    } catch (error) {
      setEvalData(null)
      updateStatus('eval', 'error', `评测失败：${String(error)}`)
    }
  }

  async function loadGraph() {
    updateStatus('graph', 'loading', '正在加载图谱节点和边...')
    try {
      const data = await invoke<GraphData>('get_graph', { projectPath })
      setGraph(data)
      updateStatus('graph', 'success', `已加载 ${data.nodes.length} 个节点和 ${data.edges.length} 条边。`)
    } catch (error) {
      setGraph(null)
      updateStatus('graph', 'error', `图谱加载失败：${String(error)}`)
    }
  }

  async function loadMemories() {
    updateStatus('memory', 'loading', '正在加载近期记忆...')
    try {
      const data = await invoke<string[]>('get_memories', { projectPath })
      setMemories(data)
      updateStatus('memory', 'success', data.length === 0 ? '当前还没有记忆。' : `已加载 ${data.length} 条记忆。`)
    } catch (error) {
      setMemories([])
      updateStatus('memory', 'error', `记忆加载失败：${String(error)}`)
    }
  }

  const disabled = Boolean(activeStatus)

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-block">
          <div className="brand-mark">l</div>
          <div>
            <h1>loci desktop</h1>
            <p>本地优先的代码库理解工作台</p>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="sidebar-section-label">工作区</div>
          <div className="project-card">
            <label htmlFor="project-path">项目路径</label>
            <div className="project-actions">
              <input
                id="project-path"
                value={projectPath}
                onChange={(event) => setProjectPath(event.target.value)}
                placeholder="/path/to/project"
              />
              <button type="button" className="secondary-button" onClick={handlePickProjectPath} disabled={statuses.project.state === 'loading'}>
                {statuses.project.state === 'loading' ? '选择中...' : '选择项目'}
              </button>
            </div>
            <p>先选择仓库目录，再建立索引、配置模型和执行问答。</p>
            <button type="button" className="primary-button wide-button" onClick={handleIndex} disabled={disabled}>
              {statuses.index.state === 'loading' ? '索引中...' : '建立索引'}
            </button>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="sidebar-section-label">工具面板</div>
          <nav className="tool-nav">
            {(Object.keys(panelHelp) as Panel[]).map((key) => (
              <button
                key={key}
                type="button"
                className={panel === key ? 'tool-button active' : 'tool-button'}
                onClick={() => setPanel(key)}
              >
                <span>{panelHelp[key].title}</span>
                <small>{panelHelp[key].description}</small>
              </button>
            ))}
          </nav>
        </div>
      </aside>

      <main className="workspace">
        <header className="workspace-header">
          <div>
            <div className="eyebrow">{panelHelp[panel].title}</div>
            <h2>{panel === 'chat' ? '和当前代码库对话' : panelHelp[panel].title}</h2>
            <p>{panelHelp[panel].description}</p>
          </div>
          <StatusPill status={statuses[panel === 'chat' ? 'ask' : panel]} />
        </header>

        <StatusBanner statuses={statuses} />

        <section className={`settings-rail ${needsSettingsAttention ? 'attention' : ''}`}>
          <div className="settings-rail-header">
            <div className="settings-rail-copy">
              <div className="eyebrow">关键设置</div>
              <h3>模型设置</h3>
              <p>
                当前默认模型：<strong>{defaultProviderName}</strong>
                {settings?.providers.length ? `，共 ${settings.providers.length} 个 provider。` : '，当前还没有有效配置。'}
              </p>
            </div>
            <div className="settings-rail-actions">
              <button type="button" className="secondary-button" onClick={() => setPanel('settings')}>
                打开完整设置
              </button>
              <button type="button" className="secondary-button" onClick={() => setSettingsExpanded((prev) => !prev)}>
                {settingsExpanded ? '收起设置' : '展开设置'}
              </button>
            </div>
          </div>

          {settingsExpanded && (
            <div className="settings-rail-body">
              <div className="settings-rail-toolbar">
                <p>先从内置服务商里启用一个 provider。OpenAI、Anthropic、Ollama、Qwen Coding Plan、LiteLLM 都可以一键加入；只有自定义 provider 才需要填写协议和接口地址。</p>
                <div className="settings-rail-actions">
                  <button type="button" className="secondary-button" onClick={() => loadSettings()} disabled={disabled}>
                    {statuses.settings.state === 'loading' ? '读取中...' : '重新读取'}
                  </button>
                  <button type="button" className="secondary-button" onClick={() => testSettingsConnection(settings?.default_provider)} disabled={disabled}>
                    {statuses.settings.state === 'loading' ? '测试中...' : '测试连接'}
                  </button>
                  <button type="button" className="primary-button" onClick={() => saveSettings()} disabled={disabled}>
                    {statuses.settings.state === 'loading' ? '保存中...' : '保存设置'}
                  </button>
                </div>
              </div>

              <PanelState status={statuses.settings} empty={!settings}>
                {settings ? (
                  <div className="settings-rail-grid">
                    <div className="settings-card settings-catalog-card">
                      <div className="settings-card-header">
                        <div>
                          <strong>内置服务商</strong>
                          <p>内置项已经带好协议、默认地址和推荐模型，通常只需要补 API Key。</p>
                        </div>
                      </div>
                      <div className="provider-catalog">
                        {builtInProviders.map((preset) => (
                          <button
                            key={preset.id}
                            type="button"
                            className="provider-preset-card"
                            onClick={() => addBuiltInProvider(preset.id)}
                          >
                            <strong>{preset.label}</strong>
                            <span>{preset.description}</span>
                            <small>{preset.protocol} / {preset.model}</small>
                          </button>
                        ))}
                        <button
                          type="button"
                          className="provider-preset-card custom"
                          onClick={addCustomProvider}
                        >
                          <strong>自定义 Provider</strong>
                          <span>用于接入通用 OpenAI 类或 Anthropic 类协议的服务。</span>
                          <small>需填写协议、接口地址、模型和 API Key</small>
                        </button>
                      </div>
                    </div>

                    <div className="settings-card">
                      <label htmlFor="rail-default-provider">默认 provider</label>
                      <select
                        id="rail-default-provider"
                        value={settings.default_provider ?? ''}
                        onChange={(event) => setSettings((prev) => prev ? ({
                          ...prev,
                          default_provider: event.target.value || null,
                        }) : prev)}
                      >
                        {settings.providers.map((provider) => (
                          <option key={provider.name} value={provider.name}>
                            {builtInLabel(provider)}
                          </option>
                        ))}
                      </select>
                      <p>问答、文档、评测都会优先使用这里指定的默认 provider。</p>
                    </div>

                    <div className="settings-card settings-card-summary">
                      <div className="settings-card-header">
                        <div>
                          <strong>配置摘要</strong>
                          <p>{settings.config_path}</p>
                        </div>
                      </div>
                      {settingsTestResult ? <div className="settings-test-result">{settingsTestResult}</div> : null}
                      <div className="provider-summary-list">
                        {settings.providers.map((provider, index) => (
                          <button
                            key={`${provider.name}-${index}`}
                            type="button"
                            className="provider-summary-chip"
                            onClick={() => setPanel('settings')}
                          >
                            <span>{builtInLabel(provider)}</span>
                            <small>{provider.preset === 'custom' ? 'custom' : 'built-in'} / {provider.model || '未设置模型'}</small>
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>
                ) : null}
              </PanelState>
            </div>
          )}
        </section>

        <div className="workspace-grid">
          <section className="main-panel">
            {panel === 'chat' && (
              <div className="chat-layout">
                <div className="helper-strip">
                  <div>
                    <strong>如何使用</strong>
                    <p>先建立索引，再确认顶部“模型设置”已经可用，然后直接提问架构、职责、设计原因或新人上手路径。</p>
                  </div>
                  <div className="suggestion-list">
                    {suggestedQuestions.map((item) => (
                      <button
                        key={item}
                        type="button"
                        className="suggestion-chip"
                        onClick={() => setQuestion(item)}
                      >
                        {item}
                      </button>
                    ))}
                  </div>
                </div>

                <div className="chat-history">
                  {messages.map((message) => (
                    <MessageCard key={message.id} message={message} />
                  ))}
                </div>

                <div className="composer">
                  <label htmlFor="chat-question">问题输入</label>
                  <textarea
                    id="chat-question"
                    value={question}
                    onChange={(event) => setQuestion(event.target.value)}
                    onKeyDown={(event) => {
                      if (event.key === 'Enter' && !event.shiftKey) {
                        event.preventDefault()
                        void handleAsk()
                      }
                    }}
                    placeholder="输入你想了解的问题，例如：这个系统做什么、为什么这样设计、应该从哪里开始看。"
                    rows={4}
                  />
                  <div className="composer-footer">
                    <p>当前默认模型：{defaultProviderName}。按 Enter 发送，按 Shift+Enter 换行。</p>
                    <button type="button" className="primary-button" onClick={handleAsk} disabled={disabled}>
                      {statuses.ask.state === 'loading' ? '生成中...' : '发送'}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {panel === 'settings' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>完整模型设置</strong>
                    <p>这里编辑的是当前项目的 `.bs/config.toml`。内置服务商默认只需要填写 API Key；自定义 provider 才需要填写协议和接口地址。</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="secondary-button" onClick={() => loadSettings()} disabled={disabled}>
                      {statuses.settings.state === 'loading' ? '读取中...' : '重新读取'}
                    </button>
                    <button type="button" className="secondary-button" onClick={() => testSettingsConnection(settings?.default_provider)} disabled={disabled}>
                      {statuses.settings.state === 'loading' ? '测试中...' : '测试默认连接'}
                    </button>
                    <button type="button" className="secondary-button" onClick={addCustomProvider} disabled={disabled}>
                      新增自定义 provider
                    </button>
                    <button type="button" className="primary-button" onClick={() => saveSettings()} disabled={disabled}>
                      {statuses.settings.state === 'loading' ? '保存中...' : '保存设置'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.settings} empty={!settings}>
                  {settings ? (
                    <div className="panel-stack">
                      <div className="settings-banner">
                        <div>
                          <strong>配置文件</strong>
                          <p>{settings.config_path}</p>
                        </div>
                        <div>
                          <strong>默认模型</strong>
                          <p>{settings.default_provider ?? '未设置'}</p>
                        </div>
                      </div>

                      {settings.providers.map((provider, index) => (
                        <div key={`${provider.name}-${index}`} className="settings-card">
                          <div className="settings-card-header">
                            <div>
                              <strong>{builtInLabel(provider) || `模型配置 ${index + 1}`}</strong>
                              <p>
                                {isBuiltInProvider(provider)
                                  ? '内置服务商已带好协议和默认地址，通常只需要填写 API Key。'
                                  : '自定义 provider 需要手动填写协议、接口地址、模型和 API Key。'}
                              </p>
                            </div>
                            <button type="button" className="ghost-button" onClick={() => removeProvider(index)} disabled={disabled}>
                              删除
                            </button>
                          </div>

                          <div className="settings-grid">
                            {provider.preset === 'custom' ? (
                              <>
                                <div className="field-group">
                                  <label>显示名称</label>
                                  <input
                                    value={provider.name}
                                    onChange={(event) => updateProvider(index, 'name', event.target.value)}
                                    placeholder="例如 openai-proxy / claude-gateway / local-dev"
                                  />
                                </div>

                                <div className="field-group">
                                  <label>协议类型</label>
                                  <select
                                    value={provider.protocol}
                                    onChange={(event) => updateProvider(index, 'protocol', event.target.value)}
                                  >
                                    {protocolOptions.filter((option) => option.value !== 'litellm').map((option) => (
                                      <option key={option.value} value={option.value}>
                                        {option.label}
                                      </option>
                                    ))}
                                  </select>
                                  <p>{protocolOptions.find((option) => option.value === provider.protocol)?.helper}</p>
                                </div>

                                <div className="field-group span-2">
                                  <label>接口地址</label>
                                  <input
                                    value={provider.base_url}
                                    onChange={(event) => updateProvider(index, 'base_url', event.target.value)}
                                    placeholder={provider.protocol === 'anthropic' ? 'https://api.anthropic.com/v1' : 'https://api.openai.com/v1'}
                                  />
                                  <p>自定义 provider 必须填写接口地址。Anthropic 协议会自动补 `/messages`。</p>
                                </div>

                                <div className="field-group span-2">
                                  <label>模型名</label>
                                  <input
                                    value={provider.model}
                                    onChange={(event) => updateProvider(index, 'model', event.target.value)}
                                    placeholder={provider.protocol === 'anthropic' ? 'claude-3-7-sonnet-latest' : 'gpt-4o-mini'}
                                  />
                                </div>

                                <div className="field-group span-2">
                                  <label>API Key</label>
                                  <input
                                    type="password"
                                    value={provider.api_key}
                                    onChange={(event) => updateProvider(index, 'api_key', event.target.value)}
                                    placeholder="填写可直接访问该服务的 API Key"
                                  />
                                </div>
                              </>
                            ) : (
                              <>
                                <div className="field-group span-2">
                                  <label>接口信息</label>
                                  <div className="provider-readonly-block">
                                    <div><strong>协议</strong><span>{protocolOptions.find((option) => option.value === provider.protocol)?.label}</span></div>
                                    <div><strong>地址</strong><span>{provider.base_url || '使用协议默认地址'}</span></div>
                                    <div><strong>模型</strong><span>{provider.model}</span></div>
                                  </div>
                                </div>

                                {provider.name !== 'ollama' ? (
                                  <div className="field-group span-2">
                                    <label>API Key</label>
                                    <input
                                      type="password"
                                      value={provider.api_key}
                                      onChange={(event) => updateProvider(index, 'api_key', event.target.value)}
                                      placeholder={`填写 ${builtInLabel(provider)} 的 API Key`}
                                    />
                                  </div>
                                ) : (
                                  <div className="field-group span-2">
                                    <label>本地服务说明</label>
                                    <div className="provider-readonly-block">
                                      <div><strong>认证</strong><span>Ollama 默认不需要额外 API Key，系统会自动使用内置占位值。</span></div>
                                    </div>
                                  </div>
                                )}
                              </>
                            )}

                            <div className="field-group span-2">
                              <label>是否作为默认 provider</label>
                              <div className="field-group-inline">
                                <button
                                  type="button"
                                  className={settings.default_provider === provider.name ? 'primary-button' : 'secondary-button'}
                                  onClick={() => setSettings((prev) => prev ? ({ ...prev, default_provider: provider.name }) : prev)}
                                  disabled={disabled}
                                >
                                  {settings.default_provider === provider.name ? '当前默认' : '设为默认'}
                                </button>
                                <button
                                  type="button"
                                  className="secondary-button"
                                  onClick={() => testSettingsConnection(provider.name)}
                                  disabled={disabled}
                                >
                                  测试此 provider
                                </button>
                              </div>
                            </div>
                          </div>
                        </div>
                      ))}
                    </div>
                  ) : null}
                </PanelState>
              </div>
            )}

            {panel === 'trace' && (
              <ToolPanel
                status={statuses.trace}
                title="追溯目标"
                description="留空时查看最近的决策节点，也可以输入文件路径或符号名称进行追溯。"
                actionArea={(
                  <>
                    <input
                      value={traceTarget}
                      onChange={(event) => setTraceTarget(event.target.value)}
                      onKeyDown={(event) => event.key === 'Enter' && void loadTrace()}
                      placeholder="例如：crates/cli/src/main.rs 或某个符号名"
                    />
                    <button type="button" className="primary-button" onClick={() => loadTrace()} disabled={disabled}>
                      {statuses.trace.state === 'loading' ? '加载中...' : '刷新追溯结果'}
                    </button>
                  </>
                )}
              >
                {trace ? (
                  <>
                    <MetricRow
                      items={[
                        ['锚点', trace.anchors.length],
                        ['决策', trace.decisions.length],
                        ['提交', trace.commits.length],
                        ['证据', trace.evidence.length],
                      ]}
                    />
                    <Section title="锚点节点" empty="当前还没有匹配到文件或符号节点。">
                      {trace.anchors.map((node) => <NodeCard key={node.id} node={node} />)}
                    </Section>
                    <Section title="决策节点" empty="当前目标还没有关联到决策节点，通常需要先运行 explain 或 diff。">
                      {trace.decisions.map((node) => <NodeCard key={node.id} node={node} />)}
                    </Section>
                    <Section title="提交记录" empty="当前目标还没有关联到提交证据。">
                      {trace.commits.map((node) => <NodeCard key={node.id} node={node} compact />)}
                    </Section>
                    <Section title="证据边" empty="当前还没有记录结构化证据边。">
                      {trace.evidence.map((edge, index) => (
                        <div key={`${edge.from}-${edge.to}-${index}`} className="evidence-card">
                          <strong>{edge.kind}</strong>
                          <div>{edge.from}{' -> '}{edge.to}</div>
                        </div>
                      ))}
                    </Section>
                    <Section title="相关节点" empty="当前没有发现额外的相关节点。">
                      {trace.related.map((node) => <NodeCard key={node.id} node={node} compact />)}
                    </Section>
                  </>
                ) : null}
              </ToolPanel>
            )}

            {panel === 'docs' && (
              <ToolPanel
                status={statuses.docs}
                title="文档生成器"
                description="需要入门文档、模块概览或交接文档时，直接从当前图谱生成。"
                actionArea={(
                  <>
                    <select value={docKind} onChange={(event) => setDocKind(event.target.value)}>
                      <option value="onboarding">入门文档</option>
                      <option value="module">模块概览</option>
                      <option value="handoff">交接文档</option>
                    </select>
                    <button type="button" className="primary-button" onClick={() => loadDoc()} disabled={disabled}>
                      {statuses.docs.state === 'loading' ? '生成中...' : '生成文档'}
                    </button>
                  </>
                )}
              >
                {doc ? <MarkdownCard title={`${doc.kind} 文档`} content={doc.content} /> : null}
              </ToolPanel>
            )}

            {panel === 'eval' && (
              <ToolPanel
                status={statuses.eval}
                title="评测执行器"
                description="运行一组内置评测问题，检查当前本地索引对核心代码库问题的支持情况。"
                actionArea={(
                  <button type="button" className="primary-button" onClick={() => loadEval()} disabled={disabled}>
                    {statuses.eval.state === 'loading' ? '运行中...' : '运行评测'}
                  </button>
                )}
              >
                {evalData ? (
                  <>
                    <MetricRow
                      items={[
                        ['平均分', `${evalData.average_score.toFixed(2)}/5`],
                        ['问题数', evalData.results.length],
                        ['偏移检查', evalData.drift_check.length],
                      ]}
                    />
                    <Section title="偏移检查" empty="当前没有返回偏移检查说明。">
                      {evalData.drift_check.map((line, index) => (
                        <div key={index} className="result-card">
                          <p>{line}</p>
                        </div>
                      ))}
                    </Section>
                    <div className="result-list result-list-column">
                      {evalData.results.map((result, index) => (
                        <div key={`${result.category}-${index}`} className="result-card">
                          <div className="result-header">
                            <div>
                              <strong>{result.category}</strong>
                              <p>{result.prompt}</p>
                            </div>
                            <span>{result.score.score}/5</span>
                          </div>
                          <p className="result-rationale">{result.score.rationale}</p>
                          <MarkdownCard title="回答内容" content={result.answer} />
                        </div>
                      ))}
                    </div>
                  </>
                ) : null}
              </ToolPanel>
            )}

            {panel === 'graph' && (
              <ToolPanel
                status={statuses.graph}
                title="图谱浏览器"
                description="这是检查索引是否正确捕获文件、符号、决策和提交的最快方式。"
                actionArea={(
                  <button type="button" className="primary-button" onClick={() => loadGraph()} disabled={disabled}>
                    {statuses.graph.state === 'loading' ? '加载中...' : '刷新图谱'}
                  </button>
                )}
              >
                {graph ? (
                  <>
                    <MetricRow
                      items={[
                        ['节点', graph.nodes.length],
                        ['边', graph.edges.length],
                      ]}
                    />
                    <div className="node-grid">
                      {graph.nodes.slice(0, 120).map((node) => (
                        <NodeCard key={node.id} node={node} compact />
                      ))}
                    </div>
                    {graph.nodes.length > 120 && (
                      <p className="footnote">当前仅展示前 120 个节点。需要进一步缩小范围时，请切换到 Trace 面板。</p>
                    )}
                  </>
                ) : null}
              </ToolPanel>
            )}

            {panel === 'memory' && (
              <ToolPanel
                status={statuses.memory}
                title="近期记忆"
                description="这里展示的是桌面端问答过程中沉淀下来的近期短期记忆内容。"
                actionArea={(
                  <button type="button" className="primary-button" onClick={() => loadMemories()} disabled={disabled}>
                    {statuses.memory.state === 'loading' ? '加载中...' : '刷新记忆'}
                  </button>
                )}
              >
                {memories.length > 0 ? (
                  <div className="result-list result-list-column">
                    {memories.map((memory, index) => (
                      <div key={index} className="memory-card">
                        {memory}
                      </div>
                    ))}
                  </div>
                ) : null}
              </ToolPanel>
            )}
          </section>

          <aside className="context-panel">
            <div className="context-card">
              <div className="context-label">当前项目</div>
              <p>{projectPath}</p>
            </div>

            <div className="context-card">
              <div className="context-label">当前默认模型</div>
              <p>{defaultProviderName}</p>
            </div>

            <div className="context-card">
              <div className="context-label">当前面板</div>
              <p>{panelHelp[panel].description}</p>
            </div>

            <div className="context-card">
              <div className="context-label">操作提示</div>
              <ul>
                <li>切换项目路径后，建议先重新建立索引，再检查顶部模型设置。</li>
                <li>模型设置是基础入口，但保存成功后可以收起，减少界面干扰。</li>
                <li>OpenAI 协议适合兼容 `/v1` 接口的服务，Anthropic 协议适合 Messages API。</li>
                <li>桌面端操作全部走本地能力，不依赖外部 HTTP 服务。</li>
              </ul>
            </div>
          </aside>
        </div>
      </main>
    </div>
  )
}

function ToolPanel({
  status,
  title,
  description,
  actionArea,
  children,
}: {
  status: StatusEntry
  title: string
  description: string
  actionArea: ReactNode
  children: ReactNode
}) {
  return (
    <div className="panel-stack">
      <div className="toolbar">
        <div className="toolbar-copy">
          <strong>{title}</strong>
          <p>{description}</p>
        </div>
        <div className="toolbar-actions">{actionArea}</div>
      </div>

      <PanelState status={status} empty={!children}>
        {children}
      </PanelState>
    </div>
  )
}

function MessageCard({ message }: { message: ChatMessage }) {
  return (
    <div className={`message-card ${message.role}`}>
      <div className="message-meta">
        <span>{message.title ?? (message.role === 'user' ? '你' : message.role === 'assistant' ? 'loci' : '系统')}</span>
      </div>
      <Markdown content={message.content} />
    </div>
  )
}

function MarkdownCard({ title, content }: { title: string; content: string }) {
  return (
    <div className="markdown-card">
      <div className="markdown-title">{title}</div>
      <Markdown content={content} />
    </div>
  )
}

function Markdown({ content }: { content: string }) {
  return (
    <div className="markdown-body">
      <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
    </div>
  )
}

function PanelState({
  status,
  empty,
  children,
}: {
  status: StatusEntry
  empty: boolean
  children: ReactNode
}) {
  if (status.state === 'loading') {
    return <div className="empty-state">加载中...</div>
  }
  if (status.state === 'error') {
    return <div className="error-panel">{status.message}</div>
  }
  if (empty) {
    return <div className="empty-state">{status.message}</div>
  }
  return <>{children}</>
}

function MetricRow({ items }: { items: Array<[string, string | number]> }) {
  return (
    <div className="metric-row">
      {items.map(([label, value]) => (
        <div key={label} className="metric-card">
          <span>{label}</span>
          <strong>{value}</strong>
        </div>
      ))}
    </div>
  )
}

function StatusBanner({ statuses }: { statuses: Record<Operation, StatusEntry> }) {
  const active = Object.values(statuses).find((entry) => entry.state === 'loading')
  const error = Object.values(statuses).find((entry) => entry.state === 'error')
  const latest = active ?? error ?? statuses.project
  const tone = active ? 'loading' : error ? 'error' : latest.state === 'success' ? 'success' : 'idle'

  return (
    <div className={`status-banner ${tone}`}>
      <strong>{tone === 'loading' ? '处理中' : tone === 'error' ? '需要关注' : '当前状态'}</strong>
      <span>{latest.message}</span>
    </div>
  )
}

function StatusPill({ status }: { status: StatusEntry }) {
  const text = status.state === 'loading'
    ? '处理中'
    : status.state === 'success'
      ? '完成'
      : status.state === 'error'
        ? '错误'
        : '空闲'
  return <span className={`status-pill ${status.state}`}>{text}</span>
}

function Section({
  title,
  empty,
  children,
}: {
  title: string
  empty: string
  children: ReactNode
}) {
  const hasItems = Array.isArray(children) ? children.length > 0 : Boolean(children)
  return (
    <div className="section-block">
      <div className="section-heading">
        <h3>{title}</h3>
      </div>
      {hasItems ? <div className="node-grid">{children}</div> : <p className="footnote">{empty}</p>}
    </div>
  )
}

function NodeCard({ node, compact = false }: { node: GraphNode; compact?: boolean }) {
  return (
    <div className={`node-card kind-${node.kind.toLowerCase()}`}>
      <strong>{node.label}</strong>
      <span>{node.kind}</span>
      {node.file_path ? <small>{node.file_path}</small> : null}
      {!compact && node.description ? <p>{node.description.slice(0, 180)}</p> : null}
    </div>
  )
}
