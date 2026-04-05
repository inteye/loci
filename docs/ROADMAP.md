# 开发路线图

## 当前状态（2026-04-05）

整体完成度约 60%，基础链路可用：`loci index → loci embed → loci ask`。

当前最大的工作不是继续铺命令，而是把产品收敛成真正的“代码库理解 Agent”：

- 统一项目认知层，避免 graph / memory / knowledge 三套事实源并存
- 用专用 agent 承载 `explore / trace / navigate / document`
- 把 Git 溯源补成闭环，而不是只停留在最近提交摘要

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

### P0 — 收紧主链路

- [ ] **统一项目认知层**
  - 定义 graph 为唯一项目事实主存
  - memory 仅保存会话上下文和用户偏好
  - knowledge 作为外部材料层，最终沉淀为 graph 节点/边

- [ ] **专用 agent 路由**
  - `ask` -> Explore / Navigate
  - `explain` -> Explore / Trace
  - `diff` -> Trace
  - `doc` -> Document

- [x] **真实项目验证**（待持续补案例）
- [x] **`loci explain <file|symbol>`**
- [x] **`loci diff [commit]`**

### P1 — 补齐差异化能力

- [x] **Trace 基础闭环**
  - `git blame` 摘要已接入
  - `TraceAgent` 已落地
  - `explain` / `diff` 已复用统一 trace report
  - graph 已支持 `commit` / `decision` 节点与 trace 写回

- [ ] **Trace 深化**
  - commit 聚类与更稳定的时间线重建
  - file / symbol / commit / decision 的细粒度证据关联
  - 输出证据、结论、置信度的稳定评测

- [x] **跨文件调用关系**
  - AST 解析时提取函数调用关系
  - 知识图谱中建立 `Calls` 边
  - 支持“谁调用了这个函数”类查询

- [x] **问答沉淀回写图谱**
  - 自动知识提取不再只写 knowledge store
  - 已支持 `concept` / `decision` 节点写入
  - 已建立基础证据边和来源追踪

- [ ] **Decision 优先检索与消费**
  - `ask` 的 trace 类问题优先召回 `decision`
  - 增加显式 `trace` 入口，直接查看决策链/证据链
  - 让文档生成优先消费 `decision` / `concept`

- [x] **`loci serve` 后台模式**
- [x] **HTTP Server 基础端点**

### P2 — 产品化

- [x] **Tauri 桌面 UI** — Chat / Graph / Memory 三个 Tab，调用 Tauri commands
- [x] **VS Code 插件** — Ask / Explain / Diff / Index 四个命令，右键菜单集成

- [x] **多项目管理** — `loci project add/list/use/remove`，注册表存 `~/.config/bs/projects.json`
- [x] **Skills 系统** — `loci skill`，内置 code_review / commit_message / doc_generate / pr_description
- [x] **Harness 执行沙箱** — 危险命令拦截（rm/DROP/shutdown 等），审计日志写 `.bs/audit.log`

- [ ] **文档产出统一化**
  - 模块说明
  - onboarding 文档
  - 交接文档
  - 输出中区分事实和推断

### P3 — 长期

- [ ] **语言能力分层**
  - L1：扫描与基础符号提取
  - L2：导航与依赖分析
  - L3：溯源与设计决策理解
  - L4：架构文档生成

- [ ] **Go / Java / C++ 深度支持**（按语言逐步补齐，而非统一承诺）
- [ ] **本地 embedding 模型**（llama.cpp，不依赖云端 API）
- [ ] **团队共享模式**（共享知识图谱 + 权限控制）
- [ ] **移动端**（iOS / Android，连接局域网内的 loci serve）

---

## 技术债

- [ ] `graph / memory / knowledge` 三层边界不清，存在事实源分裂
- [ ] `crates/agent` 仍偏通用 planner/executor，和垂直产品定位不一致
- [ ] `Trace` 仍缺少稳定的时间线归并和更细粒度证据边类型
- [x] 知识图谱重建时未清理旧节点（已修复：`store.clear()` on re-index）
- [x] 问答后的知识沉淀已开始回写图谱（`concept` / `decision`）
- [ ] 向量索引在节点数 >10 万时性能未验证
- [ ] `loci knowledge watch` 使用了 `block_in_place`，在单线程 runtime 下会 panic
- [ ] 缺少单元测试和集成测试

---

## 下一步

优先按下面顺序推进：

1. `loci trace <file|commit>`：显式暴露决策链和证据链
2. 把 `Decision` 的 evidence 从 `RelatedTo` 细化为更明确的边类型
3. 让文档生成优先消费 `Decision` / `Concept`
4. 补真实项目评测和基础测试
