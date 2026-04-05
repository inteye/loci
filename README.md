# loci

本地优先的技术人员 AI Agent 系统。

## 架构

```
crates/
  core/        # 共享类型：Memory, Knowledge, Task, Message 等
  llm/         # LLM 客户端抽象（OpenAI 兼容协议，支持 Ollama/Claude 等）
  memory/      # 记忆系统（短期/项目/全局，SQLite + 向量）
  knowledge/   # 知识库（文件/URL/对话自动提取，向量检索）
  agent/       # Planner + Executor（动态任务分解 + 工具调用循环）
  tools/       # 工具注册表（shell, file, http, knowledge_search, memory_recall）
  storage/     # SQLite 持久化层
  cli/         # CLI 入口

apps/
  server/      # 本地 HTTP server（所有端共享）
  desktop/     # Tauri 桌面应用（TODO）
```

## 快速开始

```bash
# 安装 Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 配置环境变量
export OPENAI_API_KEY=sk-...
export LLM_BASE_URL=http://localhost:11434/v1  # Ollama（可选）
export LLM_MODEL=gpt-4o

# 启动本地 server
cargo run -p loci-server

# 调用
curl -X POST http://localhost:3000/run \
  -H 'Content-Type: application/json' \
  -d '{"goal": "列出当前目录的文件并统计行数", "working_dir": "/tmp"}'
```

## 环境变量

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `OPENAI_API_KEY` | API Key（必填） | - |
| `LLM_BASE_URL` | 自定义 endpoint（Ollama 等） | OpenAI 官方 |
| `LLM_MODEL` | 模型名称 | `gpt-4o` |

## 自定义 Provider

复制 `config.example.toml` 到 `.bs/config.toml`（项目级）或 `~/.config/bs/config.toml`（全局）：

```toml
default_provider = "ollama"

[[providers]]
name = "ollama"
base_url = "http://localhost:11434/v1"
model = "qwen2.5-coder:7b"
api_key = "ollama"

[[providers]]
name = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
```

使用指定 provider：

```bash
loci ask "这个模块是做什么的" --provider ollama
loci ask "帮我分析架构" --provider deepseek
```

## 开发路线

详见 [docs/ROADMAP.md](docs/ROADMAP.md)

**已完成：**
- [x] 核心类型定义
- [x] LLM 客户端（OpenAI 兼容，多 provider 配置）
- [x] 工具注册表 + 基础工具（shell/file/http）
- [x] 动态 Planner（LLM 任务分解 + DAG 执行）
- [x] 本地 HTTP Server
- [x] Rust / Python / TypeScript AST 解析
- [x] 项目扫描器 + Git 历史分析
- [x] 知识图谱（节点/边 + SQLite 持久化）
- [x] 向量索引（余弦相似度语义检索）
- [x] 记忆系统（三层：session/project/global）
- [x] 知识库（文件/URL 导入 + 目录监听）
- [x] CLI 完整命令集（index/embed/ask/graph/history/memory/knowledge）
- [x] 多轮对话交互模式
- [x] 增量索引

**待完成（P0）：**
- [ ] 真实项目验证 + bug 修复
- [ ] `loci explain <file|symbol>`
- [ ] `loci diff [commit]`

## CLI 使用

```bash
export OPENAI_API_KEY=sk-...

# 索引当前项目
loci index .

# 问问题
loci ask "这个项目的核心模块是什么？"
loci ask "LlmClient trait 在哪里定义，有哪些实现？"

# 查看知识图谱
loci graph

# 查看文件 git 历史
loci history crates/agent/src/executor.rs
```
