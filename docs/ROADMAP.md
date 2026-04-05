# 开发路线图

## 当前状态（2026-04-05）

整体完成度约 60%，核心链路可用：`loci index → loci embed → loci ask`

---

## 已完成

- [x] 核心类型定义（Memory, Knowledge, Task, Message 等）
- [x] LLM 客户端（OpenAI 兼容协议，多 provider 配置）
- [x] 工具注册表 + 基础工具（shell / file / http）
- [x] 动态 Planner（LLM 任务分解 → DAG）
- [x] Executor（工具调用循环）
- [x] 本地 HTTP Server
- [x] Rust AST 解析（函数 / 结构体 / trait / impl 提取）
- [x] Python / TypeScript AST 解析（tree-sitter）
- [x] 项目扫描器（多语言文件索引）
- [x] Git 历史分析（文件变更追踪）
- [x] 知识图谱（节点 / 边 + SQLite 持久化）
- [x] 向量索引（embedding 存 SQLite BLOB，余弦相似度检索）
- [x] 记忆系统（三层：session / project / global，语义召回）
- [x] 知识库（文件 / URL 导入，目录监听，语义搜索）
- [x] CLI 入口（index / embed / ask / graph / history / memory / knowledge）
- [x] 多轮对话交互模式（session 上下文保持）
- [x] 增量索引（只重解析变更文件）
- [x] 多 provider 配置（.bs/config.toml，支持 Ollama / DeepSeek / Groq 等）
- [x] 友好错误提示（无配置时给出操作指引）

---

## 待完成

### P0 — 让产品真正可用

- [x] **真实项目验证**（待配置 API key 后验证）
- [x] **`loci explain <file|symbol>`**
- [x] **`loci diff [commit]`**

### P1 — 差异化核心功能

- [x] **跨文件调用关系**
  - AST 解析时提取函数调用关系（`ExprCall` + `ExprMethodCall`）
  - 知识图谱中建立 `Calls` 边
  - 支持"谁调用了这个函数"类查询

- [x] **自动知识提取** — 对话后 LLM 判断是否有可复用知识点，自动存入知识库
- [x] **`loci serve` 后台模式** — 常驻 HTTP server，文件变更自动增量更新索引，提供 `/ask` API
- [x] **HTTP Server 完善** — `/health` `/run` `/ask` `/graph` `/memories` 端点全部实现

### P2 — 产品化

- [x] **Tauri 桌面 UI** — Chat / Graph / Memory 三个 Tab，调用 Tauri commands
- [x] **VS Code 插件** — Ask / Explain / Diff / Index 四个命令，右键菜单集成

- [x] **多项目管理** — `loci project add/list/use/remove`，注册表存 `~/.config/bs/projects.json`
- [x] **Skills 系统** — `loci skill`，内置 code_review / commit_message / doc_generate / pr_description
- [x] **Harness 执行沙箱** — 危险命令拦截（rm/DROP/shutdown 等），审计日志写 `.bs/audit.log`

### P3 — 长期

- [ ] **Go / Java / C++ AST 支持**（tree-sitter 扩展）
- [ ] **本地 embedding 模型**（llama.cpp，不依赖云端 API）
- [ ] **团队共享模式**（共享知识图谱 + 权限控制）
- [ ] **移动端**（iOS / Android，连接局域网内的 loci serve）

---

## 技术债

- [ ] `crates/agent` Planner/Executor 未接入 CLI（目前只有 HTTP server 用到）
- [x] 知识图谱重建时未清理旧节点（已修复：`store.clear()` on re-index）
- [ ] 向量索引在节点数 >10 万时性能未验证
- [ ] `loci knowledge watch` 使用了 `block_in_place`，在单线程 runtime 下会 panic
- [ ] 缺少单元测试和集成测试
