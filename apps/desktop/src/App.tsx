import type { CSSProperties, ReactNode } from 'react'
import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'

type Tab = 'chat' | 'trace' | 'docs' | 'eval' | 'graph' | 'memory'

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

export default function App() {
  const [tab, setTab] = useState<Tab>('chat')
  const [projectPath, setProjectPath] = useState('.')
  const [question, setQuestion] = useState('')
  const [answer, setAnswer] = useState('')
  const [loading, setLoading] = useState(false)
  const [graph, setGraph] = useState<GraphData | null>(null)
  const [memories, setMemories] = useState<string[]>([])
  const [traceTarget, setTraceTarget] = useState('')
  const [trace, setTrace] = useState<TraceData | null>(null)
  const [traceError, setTraceError] = useState('')
  const [docKind, setDocKind] = useState('onboarding')
  const [doc, setDoc] = useState<DocData | null>(null)
  const [docError, setDocError] = useState('')
  const [evalData, setEvalData] = useState<EvalData | null>(null)
  const [evalError, setEvalError] = useState('')

  useEffect(() => {
    invoke<string>('get_default_project_path')
      .then(path => setProjectPath(path))
      .catch(() => {})
  }, [])

  async function handleAsk() {
    if (!question.trim()) return
    setLoading(true)
    setAnswer('')
    try {
      const res = await invoke<string>('ask', { projectPath, question })
      setAnswer(res)
    } catch (e) {
      setAnswer(`Error: ${e}`)
    }
    setLoading(false)
  }

  async function loadGraph() {
    try {
      const data = await invoke<GraphData>('get_graph', { projectPath })
      setGraph(data)
    } catch (e) {
      console.error(e)
    }
  }

  async function loadMemories() {
    try {
      const data = await invoke<string[]>('get_memories', { projectPath })
      setMemories(data)
    } catch (e) {
      console.error(e)
    }
  }

  async function loadTrace(target = traceTarget) {
    setTraceError('')
    try {
      const data = await invoke<TraceData>('get_trace', { projectPath, target })
      setTrace(data)
    } catch (e) {
      setTrace(null)
      setTraceError(String(e))
    }
  }

  async function loadDoc(kind = docKind) {
    setDocError('')
    try {
      const data = await invoke<DocData>('get_doc', { projectPath, kind, provider: null })
      setDoc(data)
    } catch (e) {
      setDoc(null)
      setDocError(String(e))
    }
  }

  async function loadEval() {
    setEvalError('')
    try {
      const data = await invoke<EvalData>('get_eval', { projectPath, provider: null })
      setEvalData(data)
    } catch (e) {
      setEvalData(null)
      setEvalError(String(e))
    }
  }

  async function handleIndex() {
    setLoading(true)
    try {
      const res = await invoke<string>('index_project', { projectPath })
      alert(`Indexed: ${res}`)
    } catch (e) {
      alert(`Error: ${e}`)
    }
    setLoading(false)
  }

  useEffect(() => {
    if (tab === 'trace') loadTrace()
    if (tab === 'docs') loadDoc()
    if (tab === 'eval') loadEval()
    if (tab === 'graph') loadGraph()
    if (tab === 'memory') loadMemories()
  }, [tab])

  return (
    <div style={{ fontFamily: '"IBM Plex Sans", sans-serif', maxWidth: 960, margin: '0 auto', padding: 20 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 20 }}>
        <h2 style={{ margin: 0 }}>loci desktop</h2>
        <input
          value={projectPath}
          onChange={e => setProjectPath(e.target.value)}
          placeholder="Project path"
          style={{ flex: 1, padding: '4px 8px', border: '1px solid #ccc', borderRadius: 4 }}
        />
        <button onClick={handleIndex} disabled={loading} style={btnStyle}>
          Index
        </button>
      </div>

      <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
        {(['chat', 'trace', 'docs', 'eval', 'graph', 'memory'] as Tab[]).map(t => (
          <button
            key={t}
            onClick={() => setTab(t)}
            style={{ ...btnStyle, background: tab === t ? '#0066cc' : '#eee', color: tab === t ? '#fff' : '#333' }}
          >
            {t.charAt(0).toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>

      {tab === 'chat' && (
        <div>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <input
              value={question}
              onChange={e => setQuestion(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && handleAsk()}
              placeholder="Ask about the codebase..."
              style={{ flex: 1, padding: '8px 12px', border: '1px solid #ccc', borderRadius: 4, fontSize: 14 }}
            />
            <button onClick={handleAsk} disabled={loading} style={{ ...btnStyle, background: '#0066cc', color: '#fff' }}>
              {loading ? '...' : 'Ask'}
            </button>
          </div>
          {answer && (
            <pre style={{ background: '#f5f5f5', padding: 16, borderRadius: 6, whiteSpace: 'pre-wrap', fontSize: 13 }}>
              {answer}
            </pre>
          )}
        </div>
      )}

      {tab === 'trace' && (
        <div>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <input
              value={traceTarget}
              onChange={e => setTraceTarget(e.target.value)}
              onKeyDown={e => e.key === 'Enter' && loadTrace()}
              placeholder="File path or symbol name for trace lookup"
              style={{ flex: 1, padding: '8px 12px', border: '1px solid #ccc', borderRadius: 4, fontSize: 14 }}
            />
            <button onClick={() => loadTrace()} style={{ ...btnStyle, background: '#0b6e4f', color: '#fff' }}>
              Refresh Trace
            </button>
          </div>

          {traceError && <div style={errorCardStyle}>{traceError}</div>}

          {trace && (
            <>
              <p style={{ color: '#666', marginBottom: 16 }}>
                {trace.anchors.length} anchors · {trace.decisions.length} decisions · {trace.commits.length} commits · {trace.evidence.length} evidence edges
              </p>
              <Section title="Anchors" empty="No matching file or symbol nodes yet. Leave the input empty to inspect the latest decision nodes.">
                {trace.anchors.map(node => <NodeCard key={node.id} node={node} />)}
              </Section>
              <Section title="Decisions" empty="No decision nodes linked to this target yet. Run explain or diff first.">
                {trace.decisions.map(node => <NodeCard key={node.id} node={node} />)}
              </Section>
              <Section title="Commits" empty="No commit evidence found for this target.">
                {trace.commits.map(node => <NodeCard key={node.id} node={node} compact />)}
              </Section>
              <Section title="Evidence" empty="No structured evidence edges recorded yet.">
                {trace.evidence.map((edge, index) => (
                  <div key={`${edge.from}-${edge.to}-${index}`} style={edgeCardStyle}>
                    <strong>{edge.kind}</strong>
                    <div style={{ color: '#666', fontSize: 12 }}>{edge.from} → {edge.to}</div>
                  </div>
                ))}
              </Section>
              <Section title="Related Nodes" empty="No additional related nodes were found.">
                {trace.related.map(node => <NodeCard key={node.id} node={node} compact />)}
              </Section>
            </>
          )}
        </div>
      )}

      {tab === 'docs' && (
        <div>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <select
              value={docKind}
              onChange={e => setDocKind(e.target.value)}
              style={{ padding: '8px 12px', border: '1px solid #ccc', borderRadius: 4, fontSize: 14 }}
            >
              <option value="onboarding">onboarding</option>
              <option value="module">module</option>
              <option value="handoff">handoff</option>
            </select>
            <button onClick={() => loadDoc()} style={{ ...btnStyle, background: '#8a5a00', color: '#fff' }}>
              Generate Doc
            </button>
          </div>

          {docError && <div style={errorCardStyle}>{docError}</div>}

          {doc && (
            <div>
              <p style={{ color: '#666', marginBottom: 12 }}>
                Using current graph decisions, concepts, and files to generate a {doc.kind} document.
              </p>
              <pre style={outputCardStyle}>{doc.content}</pre>
            </div>
          )}
        </div>
      )}

      {tab === 'eval' && (
        <div>
          <div style={{ display: 'flex', gap: 8, marginBottom: 12 }}>
            <button onClick={() => loadEval()} style={{ ...btnStyle, background: '#7a2cff', color: '#fff' }}>
              Run Eval
            </button>
          </div>

          {evalError && <div style={errorCardStyle}>{evalError}</div>}

          {evalData && (
            <div>
              <p style={{ color: '#666', marginBottom: 12 }}>
                Average score: {evalData.average_score.toFixed(2)}/5 across {evalData.results.length} evaluation prompts.
              </p>
              <Section title="Drift Check" empty="No drift check notes were returned.">
                {evalData.drift_check.map((line, index) => (
                  <div key={index} style={edgeCardStyle}>{line}</div>
                ))}
              </Section>
              <Section title="Results" empty="No evaluation results were returned.">
                {evalData.results.map((result, index) => (
                  <div key={`${result.category}-${index}`} style={resultCardStyle}>
                    <div style={{ fontWeight: 600, marginBottom: 6 }}>{result.category}</div>
                    <div style={{ color: '#444', marginBottom: 6 }}>{result.prompt}</div>
                    <div style={{ color: '#666', fontSize: 12, marginBottom: 8 }}>
                      Score: {result.score.score}/5
                    </div>
                    <div style={{ color: '#555', fontSize: 12, marginBottom: 8 }}>
                      {result.score.rationale}
                    </div>
                    <pre style={resultAnswerStyle}>{result.answer}</pre>
                  </div>
                ))}
              </Section>
            </div>
          )}
        </div>
      )}

      {tab === 'graph' && (
        <div>
          {graph ? (
            <>
              <p style={{ color: '#666' }}>{graph.nodes.length} nodes · {graph.edges.length} edges</p>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))', gap: 8 }}>
                {graph.nodes.slice(0, 100).map(node => (
                  <NodeCard key={node.id} node={node} compact />
                ))}
              </div>
              {graph.nodes.length > 100 && <p style={{ color: '#999' }}>Showing 100 of {graph.nodes.length} nodes</p>}
            </>
          ) : <p>Loading graph...</p>}
        </div>
      )}

      {tab === 'memory' && (
        <div>
          {memories.length === 0 ? <p style={{ color: '#999' }}>No memories yet. Start asking questions.</p> : (
            memories.map((memory, index) => (
              <div key={index} style={{ background: '#f9f9f9', padding: '10px 14px', borderRadius: 6, marginBottom: 8, fontSize: 13 }}>
                {memory.slice(0, 300)}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  )
}

function Section({ title, empty, children }: { title: string; empty: string; children: ReactNode }) {
  const hasItems = Array.isArray(children) ? children.length > 0 : Boolean(children)
  return (
    <div style={{ marginBottom: 18 }}>
      <h3 style={{ marginBottom: 10 }}>{title}</h3>
      {hasItems ? (
        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(220px, 1fr))', gap: 10 }}>
          {children}
        </div>
      ) : (
        <p style={{ color: '#888' }}>{empty}</p>
      )}
    </div>
  )
}

function NodeCard({ node, compact = false }: { node: GraphNode; compact?: boolean }) {
  return (
    <div style={{ background: kindColor(node.kind), padding: '10px 12px', borderRadius: 8, fontSize: 12 }}>
      <div style={{ fontWeight: 600 }}>{node.label}</div>
      <div style={{ color: '#666', fontSize: 11 }}>{node.kind}</div>
      {node.file_path && <div style={{ color: '#777', fontSize: 11, marginTop: 4 }}>{node.file_path}</div>}
      {!compact && node.description && <div style={{ marginTop: 6, color: '#444' }}>{node.description.slice(0, 140)}</div>}
    </div>
  )
}

const btnStyle: CSSProperties = {
  padding: '6px 14px',
  border: 'none',
  borderRadius: 4,
  cursor: 'pointer',
  background: '#eee',
  fontSize: 13,
}

const edgeCardStyle: CSSProperties = {
  background: '#f5f5f5',
  padding: '10px 12px',
  borderRadius: 8,
  fontSize: 12,
}

const errorCardStyle: CSSProperties = {
  background: '#fdecec',
  color: '#8c1d18',
  padding: '10px 12px',
  borderRadius: 8,
  marginBottom: 12,
}

const outputCardStyle: CSSProperties = {
  background: '#f8f6f0',
  padding: 16,
  borderRadius: 8,
  whiteSpace: 'pre-wrap',
  fontSize: 13,
  lineHeight: 1.5,
}

const resultCardStyle: CSSProperties = {
  background: '#f7f4ff',
  padding: 12,
  borderRadius: 8,
  fontSize: 12,
}

const resultAnswerStyle: CSSProperties = {
  background: '#ffffff',
  padding: 12,
  borderRadius: 6,
  whiteSpace: 'pre-wrap',
  fontSize: 12,
  lineHeight: 1.45,
}

function kindColor(kind: string): string {
  const map: Record<string, string> = {
    File: '#e8f4fd',
    Function: '#e8fde8',
    AsyncFunction: '#d4f5d4',
    Struct: '#fdf3e8',
    Enum: '#fde8f3',
    Trait: '#f3e8fd',
    Module: '#f0f0f0',
    Concept: '#fff4d6',
    Decision: '#ffe1cc',
    Commit: '#dde7ff',
  }
  return map[kind] ?? '#f5f5f5'
}
