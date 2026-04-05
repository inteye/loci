# Repository Guidelines

## Project Structure & Module Organization
`loci` is a Rust workspace. Core libraries live in `crates/`: `core` defines shared types, `llm` wraps model providers, `agent` handles planning/execution, and `tools`, `memory`, `knowledge`, `graph`, `storage`, `codebase`, and `skills` provide supporting subsystems. User-facing binaries live in `crates/cli` (`loci`) and `apps/server` (`loci-server`).

Additional apps are under `apps/desktop` (React + Tauri) and `apps/vscode-extension` (TypeScript). Documentation and planning notes live in `docs/`, and sample provider config is in `config.example.toml`.

## Build, Test, and Development Commands
Use Cargo from the repo root for workspace work:

- `cargo build --workspace`: build all Rust crates and binaries.
- `cargo test --workspace`: run Rust tests across the workspace.
- `cargo run -p loci-cli -- --help`: inspect CLI commands.
- `cargo run -p loci-server`: start the local HTTP server on port `3000`.
- `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`: format and lint before opening a PR.

Frontend app commands are local to each app:

- `cd apps/desktop && npm run dev`
- `cd apps/vscode-extension && npm run compile`

## Coding Style & Naming Conventions
Follow standard Rust 2021 conventions and let `rustfmt` control formatting: 4-space indentation, `snake_case` for functions/modules, `CamelCase` for types, and clear crate names with the `loci-` prefix. Keep modules focused; prefer adding logic to the matching domain crate instead of expanding binaries.

For TypeScript apps, keep filenames descriptive and use the existing script/toolchain (`tsc`, Vite, Tauri) rather than ad hoc build steps.

## Testing Guidelines
There are currently few dedicated test files, so new features should add targeted unit tests close to the affected crate or integration tests when behavior crosses crate boundaries. Name tests after observable behavior, for example `planner_builds_task_dag`. Run `cargo test --workspace` before submitting changes.

## Commit & Pull Request Guidelines
Git history is minimal, but the existing commit uses a short prefix style (`init: initial commit`). Continue with concise, imperative summaries such as `cli: add provider override` or `graph: fix vector similarity`.

Pull requests should explain the motivation, summarize behavior changes, list validation steps, and include screenshots or terminal output when touching `apps/desktop`, `apps/vscode-extension`, or API responses.
