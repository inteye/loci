import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'

type Panel = 'chat' | 'trace' | 'docs' | 'eval' | 'graph' | 'memory'
type Operation = 'project' | 'index' | 'ask' | 'trace' | 'docs' | 'eval' | 'graph' | 'memory'
type AsyncState = 'idle' | 'loading' | 'success' | 'error'

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
  trace: {
    title: '追溯分析',
    description: '按文件路径或符号查看决策节点、提交记录和证据边，定位“为什么这样设计”。',
  },
  docs: {
    title: '文档生成',
    description: '根据当前图谱、概念和决策生成 onboarding、模块说明或交接文档。',
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
      '先选择项目并完成索引，然后在这里提问架构、追溯原因、设计决策或新同事上手问题。',
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
  const [statuses, setStatuses] = useState<Record<Operation, StatusEntry>>(statusDefaults)

  const activeStatus = useMemo(() => {
    const order: Operation[] = ['project', 'index', 'ask', 'trace', 'docs', 'eval', 'graph', 'memory']
    return order.find((key) => statuses[key].state === 'loading')
  }, [statuses])

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

  async function handlePickProjectPath() {
    updateStatus('project', 'loading', '正在打开项目目录选择器...')
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: '选择项目目录',
      })
      if (!selected || Array.isArray(selected)) {
        updateStatus('project', 'idle', '已取消项目目录选择。')
        return
      }
      setProjectPath(selected)
      setGraph(null)
      setTrace(null)
      setDoc(null)
      setEvalData(null)
      setMemories([])
      setMessages((prev) => [
        prev[0],
        makeMessage('system', `项目路径已更新为 \`${selected}\`。\n\n如果这是一个新项目，请先重新执行索引。`, '项目已切换'),
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
    updateStatus('ask', 'loading', '正在结合当前索引和上下文生成回答...')

    try {
      const answer = await invoke<string>('ask', { projectPath, question: trimmed })
      setMessages((prev) => [...prev, makeMessage('assistant', answer, '回答')])
      updateStatus('ask', 'success', '已基于本地图谱和记忆上下文生成回答。')
    } catch (error) {
      const message = `问答失败：${String(error)}`
      setMessages((prev) => [...prev, makeMessage('system', message, '错误')])
      updateStatus('ask', 'error', message)
    }
  }

  async function loadTrace(target = traceTarget) {
    updateStatus('trace', 'loading', target.trim()
      ? `正在加载 “${target.trim()}” 的 Trace 视图...`
      : '正在加载最近的决策 Trace...')
    try {
      const data = await invoke<TraceData>('get_trace', { projectPath, target })
      setTrace(data)
      updateStatus(
        'trace',
        'success',
        `已加载 ${data.decisions.length} 个决策、${data.commits.length} 个提交、${data.evidence.length} 条证据边。`,
      )
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
                选择项目
              </button>
            </div>
            <p>选择要建立索引和进行问答的仓库目录。</p>
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

        <div className="workspace-grid">
          <section className="main-panel">
            {panel === 'chat' && (
              <div className="chat-layout">
                <div className="helper-strip">
                  <div>
                    <strong>如何使用</strong>
                    <p>先对当前项目建立索引，然后直接提问架构、职责、设计原因或新人上手路径。</p>
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
                    <p>按 Enter 发送，按 Shift+Enter 换行。</p>
                    <button type="button" className="primary-button" onClick={handleAsk} disabled={disabled}>
                      {statuses.ask.state === 'loading' ? '生成中...' : '发送'}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {panel === 'trace' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>追溯目标</strong>
                    <p>留空时查看最近的决策节点，也可以输入文件路径或符号名称进行追溯。</p>
                  </div>
                  <div className="toolbar-actions">
                    <input
                      value={traceTarget}
                      onChange={(event) => setTraceTarget(event.target.value)}
                      onKeyDown={(event) => event.key === 'Enter' && void loadTrace()}
                      placeholder="例如：crates/cli/src/main.rs 或某个符号名"
                    />
                    <button type="button" className="primary-button" onClick={() => loadTrace()} disabled={disabled}>
                      {statuses.trace.state === 'loading' ? '加载中...' : '刷新追溯结果'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.trace} empty={!trace}>
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
                </PanelState>
              </div>
            )}

            {panel === 'docs' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>文档生成器</strong>
                    <p>需要入门文档、模块概览或交接文档时，直接从当前图谱生成。</p>
                  </div>
                  <div className="toolbar-actions">
                    <select value={docKind} onChange={(event) => setDocKind(event.target.value)}>
                      <option value="onboarding">入门文档</option>
                      <option value="module">模块概览</option>
                      <option value="handoff">交接文档</option>
                    </select>
                    <button type="button" className="primary-button" onClick={() => loadDoc()} disabled={disabled}>
                      {statuses.docs.state === 'loading' ? '生成中...' : '生成文档'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.docs} empty={!doc}>
                  {doc ? <MarkdownCard title={`${doc.kind} 文档`} content={doc.content} /> : null}
                </PanelState>
              </div>
            )}

            {panel === 'eval' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>评测执行器</strong>
                    <p>运行一组内置评测问题，检查当前本地索引对核心代码库问题的支持情况。</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadEval()} disabled={disabled}>
                      {statuses.eval.state === 'loading' ? '运行中...' : '运行评测'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.eval} empty={!evalData}>
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
                      <div className="result-list">
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
                </PanelState>
              </div>
            )}

            {panel === 'graph' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>图谱浏览器</strong>
                    <p>这是检查索引是否正确捕获文件、符号、决策和提交的最快方式。</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadGraph()} disabled={disabled}>
                      {statuses.graph.state === 'loading' ? '加载中...' : '刷新图谱'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.graph} empty={!graph}>
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
                </PanelState>
              </div>
            )}

            {panel === 'memory' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>近期记忆</strong>
                    <p>这里展示的是桌面端问答过程中沉淀下来的近期短期记忆内容。</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadMemories()} disabled={disabled}>
                      {statuses.memory.state === 'loading' ? '加载中...' : '刷新记忆'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.memory} empty={memories.length === 0}>
                  {memories.length > 0 ? (
                    <div className="result-list">
                      {memories.map((memory, index) => (
                        <div key={index} className="memory-card">
                          {memory}
                        </div>
                      ))}
                    </div>
                  ) : null}
                </PanelState>
              </div>
            )}
          </section>

          <aside className="context-panel">
            <div className="context-card">
              <div className="context-label">当前项目</div>
              <p>{projectPath}</p>
            </div>

            <div className="context-card">
              <div className="context-label">当前面板</div>
              <p>{panelHelp[panel].description}</p>
            </div>

            <div className="context-card">
              <div className="context-label">操作提示</div>
              <ul>
                <li>切换项目路径后，建议先重新建立索引。</li>
                <li>先在聊天区问大问题，再切到追溯或文档面板做进一步确认。</li>
                <li>桌面端操作全部走本地能力，不依赖外部 HTTP 服务。</li>
              </ul>
            </div>
          </aside>
        </div>
      </main>
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
  children: React.ReactNode
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
  children: React.ReactNode
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
