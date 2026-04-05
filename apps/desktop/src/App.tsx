import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'

type Tab = 'chat' | 'graph' | 'memory'

interface GraphNode { id: string; label: string; kind: string; description?: string }
interface GraphEdge { from: string; to: string; kind: string }
interface GraphData  { nodes: GraphNode[]; edges: GraphEdge[] }

export default function App() {
  const [tab, setTab] = useState<Tab>('chat')
  const [projectPath, setProjectPath] = useState('.')
  const [question, setQuestion] = useState('')
  const [answer, setAnswer] = useState('')
  const [loading, setLoading] = useState(false)
  const [graph, setGraph] = useState<GraphData | null>(null)
  const [memories, setMemories] = useState<string[]>([])

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
      const g = await invoke<GraphData>('get_graph', { projectPath })
      setGraph(g)
    } catch (e) {
      console.error(e)
    }
  }

  async function loadMemories() {
    try {
      const m = await invoke<string[]>('get_memories', { projectPath })
      setMemories(m)
    } catch (e) {
      console.error(e)
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
    if (tab === 'graph') loadGraph()
    if (tab === 'memory') loadMemories()
  }, [tab])

  return (
    <div style={{ fontFamily: 'sans-serif', maxWidth: 900, margin: '0 auto', padding: 20 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 20 }}>
        <h2 style={{ margin: 0 }}>🧠 Sage</h2>
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

      {/* Tabs */}
      <div style={{ display: 'flex', gap: 8, marginBottom: 16 }}>
        {(['chat', 'graph', 'memory'] as Tab[]).map(t => (
          <button key={t} onClick={() => setTab(t)}
            style={{ ...btnStyle, background: tab === t ? '#0066cc' : '#eee', color: tab === t ? '#fff' : '#333' }}>
            {t.charAt(0).toUpperCase() + t.slice(1)}
          </button>
        ))}
      </div>

      {/* Chat */}
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

      {/* Graph */}
      {tab === 'graph' && (
        <div>
          {graph ? (
            <>
              <p style={{ color: '#666' }}>{graph.nodes.length} nodes · {graph.edges.length} edges</p>
              <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(200px, 1fr))', gap: 8 }}>
                {graph.nodes.slice(0, 100).map(n => (
                  <div key={n.id} style={{ background: kindColor(n.kind), padding: '8px 12px', borderRadius: 6, fontSize: 12 }}>
                    <div style={{ fontWeight: 600 }}>{n.label}</div>
                    <div style={{ color: '#666', fontSize: 11 }}>{n.kind}</div>
                    {n.description && <div style={{ marginTop: 4, color: '#444' }}>{n.description.slice(0, 80)}</div>}
                  </div>
                ))}
              </div>
              {graph.nodes.length > 100 && <p style={{ color: '#999' }}>Showing 100 of {graph.nodes.length} nodes</p>}
            </>
          ) : <p>Loading graph...</p>}
        </div>
      )}

      {/* Memory */}
      {tab === 'memory' && (
        <div>
          {memories.length === 0 ? <p style={{ color: '#999' }}>No memories yet. Start asking questions.</p> : (
            memories.map((m, i) => (
              <div key={i} style={{ background: '#f9f9f9', padding: '10px 14px', borderRadius: 6, marginBottom: 8, fontSize: 13 }}>
                {m.slice(0, 300)}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  )
}

const btnStyle: React.CSSProperties = {
  padding: '6px 14px', border: 'none', borderRadius: 4,
  cursor: 'pointer', background: '#eee', fontSize: 13
}

function kindColor(kind: string): string {
  const map: Record<string, string> = {
    File: '#e8f4fd', Function: '#e8fde8', AsyncFunction: '#d4f5d4',
    Struct: '#fdf3e8', Enum: '#fde8f3', Trait: '#f3e8fd', Module: '#f0f0f0',
  }
  return map[kind] ?? '#f5f5f5'
}
