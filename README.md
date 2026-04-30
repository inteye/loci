# 脉络

`脉络` 是一个本地优先的代码库理解工作台，当前命令行与内部技术代号仍然使用 `loci`。

它会把仓库索引成图谱，把代码、提交历史、追溯结论和文档生成串成一条主链路，并通过下面几种形态提供出来：

- CLI：索引、问答、追溯、文档生成、评测
- 桌面端：Chat / Trace / Docs / Eval / Graph / Memory
- VS Code 插件：在编辑器里直接 Ask / Explain / Diff / Index
- 本地 HTTP API：给编辑器、脚本和外部集成使用

核心工作流是：

```text
index -> graph -> trace -> decision/concept -> ask/doc/eval
```

## 项目定位

很多代码助手更像“当前文件问答器”。

`脉络` 想做的是另一件事：让回答尽量建立在项目结构、Git 历史、追溯证据和已经沉淀下来的设计决策之上，而不是只靠一段局部上下文。

当前版本已经具备这些基础能力：

- 将仓库扫描为文件 / 符号 / 提交 / 决策 / 概念图谱
- 将 `explain` / `diff` 的结果沉淀回图谱中的 `Decision`
- 优先用 `Decision` / `Commit` 回答“为什么这样设计”“最近怎么演进”的问题
- 从图谱生成 onboarding / module / handoff 文档
- 对当前项目运行轻量评测，检查理解质量

## 当前能力

- `loci doctor`：检查项目是否已经具备完整工作流所需条件
- `loci index`：建立本地图谱和基础提交关联
- `loci ask`：围绕当前项目回答架构、职责、上手路径等问题
- `loci explain`：解释文件或符号，并生成 trace 决策沉淀
- `loci diff`：解释最近变更，并把结论写回图谱
- `loci trace`：查看决策链、证据边和相关提交
- `loci doc`：生成 onboarding / module / handoff 文档
- `loci eval`：对当前索引项目运行轻量评测
- 桌面端支持本地目录和 GitHub 仓库导入
- 本地 HTTP API 提供 `/api/v1/*` 版本化接口

## 支持的输入类型

当前已支持结构化代码解析的语言：

- Rust
- Python
- TypeScript / TSX
- JavaScript / JSX
- Go
- Java

当前也支持作为“文件级图谱源”纳入索引的内容：

- HTML
- Markdown
- TOML
- YAML

这意味着像原型仓库、文档仓库、产品说明仓库这类“代码不多但文件结构有价值”的项目，也可以先建立最小可用图谱。

## 当前状态

`脉络` 目前处于 Alpha 阶段。

已经可用的主链路：

- 索引项目
- 询问架构和职责问题
- 对文件和最近变更做追溯分析
- 生成项目文档
- 运行轻量评测

仍在继续打磨的部分：

- 更强的 trace 时间线重建和 commit 聚类
- 更广泛的 Windows 实机验证
- 更多语言支持
- 更完整的端到端集成测试

更细的进展和剩余事项见 [docs/ROADMAP.md](docs/ROADMAP.md)。

## 平台支持

| 能力 | macOS | Linux | Windows |
| --- | --- | --- | --- |
| 桌面端 | 支持 | 支持 | 支持 |
| CLI 主链路（`index / ask / trace / doc / eval`） | 支持 | 支持 | 基本可用 |
| shell-heavy 的工具和技能场景 | 支持 | 支持 | 仍需更多验证 |

说明：

- 桌面端 GitHub Actions 已支持 macOS / Linux / Windows 打包。
- CLI 现在已经补了 Windows 下的配置目录和 shell 执行分支适配。
- Windows 仍建议先保证 `git` 在 `PATH` 中，并把当前版本视为“可用但仍需更多真实项目验证”。

## 安装

### CLI

在仓库根目录构建：

```bash
cargo build --workspace
```

从当前仓库全局安装 CLI：

```bash
cargo install --path crates/cli
```

安装后验证：

```bash
loci --help
```

### 桌面端

本地开发：

```bash
cd apps/desktop
npm install
npm run tauri:dev
```

构建当前平台的桌面安装包：

```bash
cd apps/desktop
npm install
npm run tauri:build
```

## 配置

`脉络` 会按下面的顺序寻找 provider 配置：

- 项目级：`.bs/config.toml`
- macOS / Linux 全局：`~/.config/bs/config.toml`
- Windows 全局：`%APPDATA%/bs/config.toml`

建议从 [config.example.toml](config.example.toml) 开始：

```bash
mkdir -p .bs
cp config.example.toml .bs/config.toml
```

示例：

```toml
default_provider = "litellm"

[[providers]]
name = "litellm"
protocol = "litellm"
base_url = "http://localhost:4000/v1"
model = "gpt-4o-mini"
api_key_env = "LITELLM_API_KEY"
```

当前支持的 provider 形态：

- OpenAI 兼容接口
- Anthropic
- LiteLLM / litellm-rs 网关
- 本地 OpenAI 兼容网关，例如 Ollama

## 5 分钟快速开始

进入你想分析的仓库后：

```bash
# 1. 先看当前项目还缺什么
loci doctor --path .

# 2. 建立图谱
loci index --path .

# 3. 测一下模型连接
loci model test --path .

# 4. 直接提问
loci ask "这个项目的核心模块是什么？" --path .

# 5. 跑追溯链路
loci explain path/to/file --path .
loci diff --path .
loci trace path/to/file --path .

# 6. 生成文档和评测
loci doc onboarding --path .
loci eval --path .
```

重要约束只有一条：

- `index` 和后续 `ask / explain / trace / doc / eval` 必须指向同一个项目路径。

## 桌面端使用方式

桌面端是目前体验最完整的入口。

你可以直接：

- 选择本地项目目录
- 导入 GitHub 仓库地址
- 自动切换到导入后的项目目录
- 建立索引
- 提问
- 查看 Trace 决策和提交证据
- 生成文档
- 运行评测

GitHub 导入的本地仓库默认放在：

```text
~/.loci/projects
```

## HTTP API

启动本地服务：

```bash
cargo run -p loci-server
```

或者：

```bash
loci serve --path .
```

默认地址：

```text
http://127.0.0.1:3000
```

核心接口包括：

- `GET /health`
- `GET /meta`
- `GET /openapi.json`
- `POST /api/v1/ask`
- `POST /api/v1/explain`
- `POST /api/v1/diff`
- `POST /api/v1/trace`
- `POST /api/v1/doc`
- `POST /api/v1/eval`
- `GET /api/v1/projects`

## 仓库结构

```text
crates/
  agent/       trace 和 agent 逻辑
  cli/         loci CLI
  codebase/    扫描、解析、Git 历史
  core/        共享类型和错误定义
  graph/       图谱存储和向量索引
  knowledge/   外部材料导入
  llm/         模型配置和 provider 客户端
  memory/      短期记忆
  skills/      内置技能
  storage/     存储辅助层
  tools/       工具执行层

apps/
  desktop/           React + Tauri 桌面端
  server/            本地 HTTP API
  vscode-extension/  VS Code 插件
```

## 开发

工作区检查：

```bash
cargo fmt --all
cargo test --workspace
```

桌面端：

```bash
cd apps/desktop
npm install
npm run build
npm run tauri:dev
```

VS Code 插件：

```bash
cd apps/vscode-extension
npm install
npm run compile
```

## 发布自动化

桌面端打包工作流位于：

```text
.github/workflows/desktop-bundles.yml
```

当前会为下面三个平台原生构建桌面端 bundle：

- macOS
- Linux
- Windows

每个平台的产物都会作为 GitHub Actions artifact 上传。

## 参与贡献

欢迎贡献，尤其是这些方向：

- 更多语言支持
- trace 质量提升
- Windows 兼容性验证
- 桌面端体验打磨
- 更完整的集成测试

提交 PR 前建议至少执行：

```bash
cargo fmt --all
cargo test --workspace
```

如果改动了桌面端，再额外执行：

```bash
cd apps/desktop
npm run build
```

