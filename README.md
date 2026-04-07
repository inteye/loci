# loci

`loci` 是一个本地优先的代码库理解系统，当前覆盖三类使用方式：

- 命令行：索引代码库、追问架构、解释文件和变更、生成文档、跑评测
- 桌面端：查看问答、Trace、Docs、Eval、Graph、Memory
- VS Code 插件：在编辑器里直接触发问答、文件解释和变更解释

它的核心不是通用 Agent 外壳，而是围绕代码库理解构建的主链路：`index -> graph -> trace -> decision/concept -> ask/doc/eval`。

## 当前能力

- 代码索引与语义检索
- Git 历史、blame、trace 决策沉淀
- `Decision` / `Concept` / `Commit` 图谱
- 文档生成：`onboarding`、`module`、`handoff`
- 评测入口：固定样本、评分、结果落盘
- 本地 HTTP API、桌面端、VS Code 插件

## 项目结构

```text
crates/
  cli/         loci CLI
  agent/       trace 与 agent 能力
  codebase/    索引、git 历史、代码解析
  graph/       知识图谱与向量索引
  memory/      会话记忆
  knowledge/   外部材料导入与检索
  llm/         模型 provider 配置与客户端
  skills/      技能系统

apps/
  server/            loci-server HTTP API
  desktop/           React + Tauri 桌面端
  vscode-extension/  VS Code 插件
```

## 安装与配置

先准备 Rust 1.78+，然后在仓库根目录执行：

```bash
cargo build --workspace
```

如果你希望像 `npm install -g` 一样全局直接使用 `loci`，Rust 下对应的方式是 `cargo install`：

```bash
# 从当前仓库全局安装 loci
cargo install --path crates/cli

# 或使用仓库里的快捷入口
make install
```

安装完成后，二进制会进入 `~/.cargo/bin/loci`。确保 `~/.cargo/bin` 在 `PATH` 里，然后就可以直接执行：

```bash
loci --help
loci index .
loci ask "这个项目的核心模块是什么？" --path .
```

如果更新了本地源码并希望覆盖安装：

```bash
cargo install --path crates/cli --force
```

LLM provider 可以通过环境变量或配置文件提供。推荐复制 [config.example.toml](config.example.toml) 到：

- 项目级：`.bs/config.toml`
- 全局：`~/.config/bs/config.toml`

示例：

```toml
default_provider = "openai"

[[providers]]
name = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
```

也可以直接使用环境变量：

```bash
export OPENAI_API_KEY=sk-...
```

## 命令行快速开始

`loci` 是主入口二进制。下面示例优先使用全局安装后的形式；如果你还没安装，也可以把 `loci` 替换成 `cargo run -p loci-cli --`。

最重要的约束是：`index` 和后续 `ask / explain / trace / doc / eval` 必须指向同一个项目路径。最稳妥的做法有两种：

1. 始终显式传 `--path`
2. 先注册并切换 active project，再直接运行命令

示例一：显式路径

```bash
loci index --path /path/to/repo
loci ask "这个项目的核心模块是什么？" --path /path/to/repo
loci trace crates/cli/src/main.rs --path /path/to/repo
```

示例二：使用项目注册表

```bash
loci project add myrepo --path /path/to/repo
loci project use myrepo
loci index
loci ask "这个项目的核心模块是什么？"
```

```bash
# 索引项目
loci index .

# 询问代码库
loci ask "这个项目的核心模块是什么？" --path .

# 解释文件 / 追溯原因
loci explain crates/cli/src/main.rs --path .
loci trace crates/cli/src/main.rs --path .

# 解释最近变更
loci diff --path .

# 生成文档
loci doc onboarding --path .
loci doc module --path .

# 跑评测
loci eval --path .

# 项目、记忆、知识库
loci project list
loci memory list --path .
loci knowledge list --path .
```

完整命令列表：

```bash
loci --help
```

## HTTP API

本地服务是独立入口，主要给 CLI 的 `serve` 模式、外部集成和 VS Code 插件使用；桌面端本身不再要求先启动它：

```bash
loci-server
# 或
loci serve --path .
```

默认监听 `http://localhost:3000`。当前 API 同时提供兼容根路径和版本化路径：

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
- `POST /api/v1/knowledge/search`
- `POST /api/v1/history`

## 桌面端

桌面端位于 [apps/desktop](/root/inteye/loci/apps/desktop)，当前已经覆盖 `Chat`、`Trace`、`Docs`、`Eval`、`Graph`、`Memory`。

开发模式：

```bash
cd apps/desktop
npm install
npm run tauri dev
```

桌面端现在直接调用内嵌的本地图谱、问答和评测逻辑，不再要求用户先手动启动 `loci serve`。

开发与打包：

```bash
cd apps/desktop
npm install
npm run tauri:dev
npm run tauri:build
```

`tauri build` 会按当前平台输出安装包；在 macOS 上会生成 `.app` 和 `.dmg`。

## VS Code 插件

插件位于 [apps/vscode-extension](/root/inteye/loci/apps/vscode-extension)，当前命令包括：

- `loci: Ask a question`
- `loci: Explain this file`
- `loci: Explain recent changes`
- `loci: Index project`

本地开发：

```bash
cd apps/vscode-extension
npm install
npm run compile
```

然后在 VS Code 里运行 Extension Host。插件默认连接 `http://localhost:3000`，可通过 `loci.serverUrl` 配置覆盖。

## 当前状态

README 现在描述的是当前 alpha 阶段的真实入口，而不是早期原型。更细的演进路线和剩余工作见 [docs/ROADMAP.md](/root/inteye/loci/docs/ROADMAP.md)。
