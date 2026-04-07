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
    title: 'Ask the codebase',
    description: 'Use the main chat to ask architecture, ownership, and design questions after indexing the project.',
  },
  trace: {
    title: 'Inspect trace evidence',
    description: 'Look up a file path or symbol to inspect decisions, commits, and evidence edges captured in the graph.',
  },
  docs: {
    title: 'Generate working docs',
    description: 'Create onboarding, module, or handoff docs from the current graph, decisions, and concepts.',
  },
  eval: {
    title: 'Run quality checks',
    description: 'Evaluate how well the current index supports architecture, traceability, and onboarding questions.',
  },
  graph: {
    title: 'Browse graph structure',
    description: 'Inspect indexed nodes and edges to see what the codebase graph currently knows.',
  },
  memory: {
    title: 'Review recent memory',
    description: 'See the most recent short-term memories captured from desktop question answering.',
  },
}

const statusDefaults: Record<Operation, StatusEntry> = {
  project: { state: 'idle', message: 'Choose a project folder to start.' },
  index: { state: 'idle', message: 'Index the selected project before relying on graph-driven answers.' },
  ask: { state: 'idle', message: 'Ask a direct question about the indexed project.' },
  trace: { state: 'idle', message: 'Enter a file path or symbol to inspect trace evidence.' },
  docs: { state: 'idle', message: 'Generate one of the built-in documentation views.' },
  eval: { state: 'idle', message: 'Run evaluation prompts against the current index.' },
  graph: { state: 'idle', message: 'Load the current graph to inspect nodes and edges.' },
  memory: { state: 'idle', message: 'Load recent memory entries from desktop question answering.' },
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
      'Select a project, run indexing, then ask architecture, trace, or onboarding questions here.',
      'Welcome',
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
          project: { state: 'success', message: `Loaded default project: ${path}` },
        }))
      })
      .catch((error) => {
        setStatuses((prev) => ({
          ...prev,
          project: { state: 'error', message: `Could not load default project: ${String(error)}` },
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
    updateStatus('project', 'loading', 'Opening project picker...')
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: 'Select project directory',
      })
      if (!selected || Array.isArray(selected)) {
        updateStatus('project', 'idle', 'Project selection cancelled.')
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
        makeMessage('system', `Project path updated to \`${selected}\`. Re-index before asking if this is a new project.`, 'Project changed'),
      ])
      updateStatus('project', 'success', `Selected project: ${selected}`)
    } catch (error) {
      updateStatus('project', 'error', `Could not open project picker: ${String(error)}`)
    }
  }

  async function handleIndex() {
    updateStatus('index', 'loading', 'Indexing project and rebuilding the local graph...')
    try {
      const result = await invoke<string>('index_project', { projectPath })
      setGraph(null)
      setTrace(null)
      setDoc(null)
      setEvalData(null)
      setMemories([])
      setMessages((prev) => [
        ...prev,
        makeMessage('system', `Index completed.\n\n${result}`, 'Index finished'),
      ])
      updateStatus('index', 'success', result)
    } catch (error) {
      updateStatus('index', 'error', `Index failed: ${String(error)}`)
    }
  }

  async function handleAsk() {
    const trimmed = question.trim()
    if (!trimmed) {
      updateStatus('ask', 'error', 'Enter a question before sending.')
      return
    }

    const userMessage = makeMessage('user', trimmed)
    setMessages((prev) => [...prev, userMessage])
    setQuestion('')
    updateStatus('ask', 'loading', 'Thinking through the indexed project...')

    try {
      const answer = await invoke<string>('ask', { projectPath, question: trimmed })
      setMessages((prev) => [...prev, makeMessage('assistant', answer, 'Answer')])
      updateStatus('ask', 'success', 'Answer generated from the local graph and memory context.')
    } catch (error) {
      const message = `Ask failed: ${String(error)}`
      setMessages((prev) => [...prev, makeMessage('system', message, 'Error')])
      updateStatus('ask', 'error', message)
    }
  }

  async function loadTrace(target = traceTarget) {
    updateStatus('trace', 'loading', target.trim()
      ? `Loading trace view for "${target.trim()}"...`
      : 'Loading recent decision trace...')
    try {
      const data = await invoke<TraceData>('get_trace', { projectPath, target })
      setTrace(data)
      updateStatus(
        'trace',
        'success',
        `${data.decisions.length} decisions, ${data.commits.length} commits, ${data.evidence.length} evidence edges loaded.`,
      )
    } catch (error) {
      setTrace(null)
      updateStatus('trace', 'error', `Trace failed: ${String(error)}`)
    }
  }

  async function loadDoc(kind = docKind) {
    updateStatus('docs', 'loading', `Generating ${kind} document from the local graph...`)
    try {
      const data = await invoke<DocData>('get_doc', { projectPath, kind, provider: null })
      setDoc(data)
      updateStatus('docs', 'success', `${kind} document generated.`)
    } catch (error) {
      setDoc(null)
      updateStatus('docs', 'error', `Document generation failed: ${String(error)}`)
    }
  }

  async function loadEval() {
    updateStatus('eval', 'loading', 'Running evaluation prompts against the indexed project...')
    try {
      const data = await invoke<EvalData>('get_eval', { projectPath, provider: null })
      setEvalData(data)
      updateStatus('eval', 'success', `Average score ${data.average_score.toFixed(2)}/5 across ${data.results.length} prompts.`)
    } catch (error) {
      setEvalData(null)
      updateStatus('eval', 'error', `Evaluation failed: ${String(error)}`)
    }
  }

  async function loadGraph() {
    updateStatus('graph', 'loading', 'Loading graph nodes and edges...')
    try {
      const data = await invoke<GraphData>('get_graph', { projectPath })
      setGraph(data)
      updateStatus('graph', 'success', `${data.nodes.length} nodes and ${data.edges.length} edges loaded.`)
    } catch (error) {
      setGraph(null)
      updateStatus('graph', 'error', `Graph loading failed: ${String(error)}`)
    }
  }

  async function loadMemories() {
    updateStatus('memory', 'loading', 'Loading recent desktop memories...')
    try {
      const data = await invoke<string[]>('get_memories', { projectPath })
      setMemories(data)
      updateStatus('memory', 'success', data.length === 0 ? 'No memories yet.' : `${data.length} memory entries loaded.`)
    } catch (error) {
      setMemories([])
      updateStatus('memory', 'error', `Memory loading failed: ${String(error)}`)
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
            <p>Local-first codebase understanding workspace</p>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="sidebar-section-label">Workspace</div>
          <div className="project-card">
            <label htmlFor="project-path">Project path</label>
            <div className="project-actions">
              <input
                id="project-path"
                value={projectPath}
                onChange={(event) => setProjectPath(event.target.value)}
                placeholder="/path/to/project"
              />
              <button type="button" className="secondary-button" onClick={handlePickProjectPath} disabled={statuses.project.state === 'loading'}>
                Choose project
              </button>
            </div>
            <p>Select the repository folder you want to index and query.</p>
            <button type="button" className="primary-button wide-button" onClick={handleIndex} disabled={disabled}>
              {statuses.index.state === 'loading' ? 'Indexing...' : 'Index project'}
            </button>
          </div>
        </div>

        <div className="sidebar-section">
          <div className="sidebar-section-label">Tools</div>
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
            <h2>{panel === 'chat' ? 'Chat with the indexed codebase' : panelHelp[panel].title}</h2>
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
                    <strong>How to use this view</strong>
                    <p>Index the selected project first, then ask direct questions about architecture, ownership, design rationale, or onboarding.</p>
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
                  <label htmlFor="chat-question">Question</label>
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
                    placeholder="Ask what this system does, why a design exists, or where to start."
                    rows={4}
                  />
                  <div className="composer-footer">
                    <p>Press Enter to send. Press Shift+Enter for a new line.</p>
                    <button type="button" className="primary-button" onClick={handleAsk} disabled={disabled}>
                      {statuses.ask.state === 'loading' ? 'Thinking...' : 'Send'}
                    </button>
                  </div>
                </div>
              </div>
            )}

            {panel === 'trace' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>Trace target</strong>
                    <p>Leave it empty to inspect the latest decision nodes, or enter a file path or symbol name.</p>
                  </div>
                  <div className="toolbar-actions">
                    <input
                      value={traceTarget}
                      onChange={(event) => setTraceTarget(event.target.value)}
                      onKeyDown={(event) => event.key === 'Enter' && void loadTrace()}
                      placeholder="crates/cli/src/main.rs or some_symbol"
                    />
                    <button type="button" className="primary-button" onClick={() => loadTrace()} disabled={disabled}>
                      {statuses.trace.state === 'loading' ? 'Loading...' : 'Refresh trace'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.trace} empty={!trace}>
                  {trace ? (
                    <>
                      <MetricRow
                        items={[
                          ['Anchors', trace.anchors.length],
                          ['Decisions', trace.decisions.length],
                          ['Commits', trace.commits.length],
                          ['Evidence', trace.evidence.length],
                        ]}
                      />
                      <Section title="Anchors" empty="No matching file or symbol nodes yet.">
                        {trace.anchors.map((node) => <NodeCard key={node.id} node={node} />)}
                      </Section>
                      <Section title="Decisions" empty="No decision nodes linked to this target yet. Run explain or diff to create them first.">
                        {trace.decisions.map((node) => <NodeCard key={node.id} node={node} />)}
                      </Section>
                      <Section title="Commits" empty="No commit evidence found for this target.">
                        {trace.commits.map((node) => <NodeCard key={node.id} node={node} compact />)}
                      </Section>
                      <Section title="Evidence" empty="No structured evidence edges recorded yet.">
                        {trace.evidence.map((edge, index) => (
                          <div key={`${edge.from}-${edge.to}-${index}`} className="evidence-card">
                            <strong>{edge.kind}</strong>
                            <div>{edge.from}{' -> '}{edge.to}</div>
                          </div>
                        ))}
                      </Section>
                      <Section title="Related nodes" empty="No related nodes were found.">
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
                    <strong>Document generator</strong>
                    <p>Use this when you need onboarding notes, a module summary, or a handoff document from the current graph.</p>
                  </div>
                  <div className="toolbar-actions">
                    <select value={docKind} onChange={(event) => setDocKind(event.target.value)}>
                      <option value="onboarding">onboarding</option>
                      <option value="module">module</option>
                      <option value="handoff">handoff</option>
                    </select>
                    <button type="button" className="primary-button" onClick={() => loadDoc()} disabled={disabled}>
                      {statuses.docs.state === 'loading' ? 'Generating...' : 'Generate'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.docs} empty={!doc}>
                  {doc ? <MarkdownCard title={`${doc.kind} document`} content={doc.content} /> : null}
                </PanelState>
              </div>
            )}

            {panel === 'eval' && (
              <div className="panel-stack">
                <div className="toolbar">
                  <div className="toolbar-copy">
                    <strong>Evaluation runner</strong>
                    <p>Run a small built-in evaluation set to see how useful the current local index is for core codebase questions.</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadEval()} disabled={disabled}>
                      {statuses.eval.state === 'loading' ? 'Running...' : 'Run eval'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.eval} empty={!evalData}>
                  {evalData ? (
                    <>
                      <MetricRow
                        items={[
                          ['Average score', `${evalData.average_score.toFixed(2)}/5`],
                          ['Prompts', evalData.results.length],
                          ['Drift notes', evalData.drift_check.length],
                        ]}
                      />
                      <Section title="Drift check" empty="No drift-check notes were returned.">
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
                            <MarkdownCard title="Answer" content={result.answer} />
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
                    <strong>Graph browser</strong>
                    <p>This is the quickest way to see whether indexing captured files, symbols, decisions, and commits the way you expect.</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadGraph()} disabled={disabled}>
                      {statuses.graph.state === 'loading' ? 'Loading...' : 'Refresh graph'}
                    </button>
                  </div>
                </div>

                <PanelState status={statuses.graph} empty={!graph}>
                  {graph ? (
                    <>
                      <MetricRow
                        items={[
                          ['Nodes', graph.nodes.length],
                          ['Edges', graph.edges.length],
                        ]}
                      />
                      <div className="node-grid">
                        {graph.nodes.slice(0, 120).map((node) => (
                          <NodeCard key={node.id} node={node} compact />
                        ))}
                      </div>
                      {graph.nodes.length > 120 && (
                        <p className="footnote">Showing 120 of {graph.nodes.length} nodes. Use Trace for a narrower investigation.</p>
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
                    <strong>Recent memory</strong>
                    <p>These are the recent short-term memory entries captured from desktop ask interactions.</p>
                  </div>
                  <div className="toolbar-actions">
                    <button type="button" className="primary-button" onClick={() => loadMemories()} disabled={disabled}>
                      {statuses.memory.state === 'loading' ? 'Loading...' : 'Refresh memory'}
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
              <div className="context-label">Current project</div>
              <p>{projectPath}</p>
            </div>

            <div className="context-card">
              <div className="context-label">Current panel</div>
              <p>{panelHelp[panel].description}</p>
            </div>

            <div className="context-card">
              <div className="context-label">Operator notes</div>
              <ul>
                <li>Index after changing the project path.</li>
                <li>Use chat for broad questions, then switch to Trace or Docs for focused follow-up.</li>
                <li>All desktop actions run locally; no external HTTP service is required.</li>
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
        <span>{message.title ?? (message.role === 'user' ? 'You' : message.role === 'assistant' ? 'loci' : 'System')}</span>
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
    return <div className="empty-state">Loading...</div>
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
      <strong>{tone === 'loading' ? 'Working' : tone === 'error' ? 'Attention' : 'Status'}</strong>
      <span>{latest.message}</span>
    </div>
  )
}

function StatusPill({ status }: { status: StatusEntry }) {
  return <span className={`status-pill ${status.state}`}>{status.state}</span>
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
