<div align="center">

<img src="assets/banner.svg" alt="NeuxDOT - Cache-Invariant Zero-Trust MCP Agent Runtime &amp; Desktop" width="100%" />

<br/>

[![Rust](https://img.shields.io/badge/Rust-1.80%2B-F97316.svg?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/Tauri-v2-24C8DB.svg?style=for-the-badge&logo=tauri&logoColor=white)](https://tauri.app/)
[![Svelte](https://img.shields.io/badge/SvelteKit-5%20(Runes)-FF3E00.svg?style=for-the-badge&logo=svelte&logoColor=white)](https://svelte.dev/)
[![Wasmtime](https://img.shields.io/badge/Wasmtime-WASM%20Sandbox-8B5CF6.svg?style=for-the-badge&logo=webassembly&logoColor=white)](https://wasmtime.dev/)
[![SQLite](https://img.shields.io/badge/SQLite-WAL%20Mode-003B57.svg?style=for-the-badge&logo=sqlite&logoColor=white)](https://www.sqlite.org/)
[![MCP](https://img.shields.io/badge/MCP-Native%20Stdio%2FSSE-EC4899.svg?style=for-the-badge&logo=sparkles&logoColor=white)](https://modelcontextprotocol.io/)
[![License](https://img.shields.io/badge/License-MIT-10B981.svg?style=for-the-badge)](LICENSE)
[![Kawaii](https://img.shields.io/badge/Kawaii-100%25%20Inside-F43F5E.svg?style=for-the-badge&logo=heart&logoColor=white)](#-the-neuxdot-paradigm)

<br/>

> ### 🌸 (｡•̀ᴗ-)✧ *“Zero-trust determinism never looked so cute!”*
> **NeuxDOT** is an ultra-fast, cache-invariant, zero-trust MCP-native agent runtime & desktop platform. Built for developers who demand **100% deterministic prompt prefix caching**, surgical **invariant meta-tools**, sandboxed **Wasmtime WebAssembly extensions**, and real-time **unprompted event streaming**.

<br/>

**[ 🌸 The Paradigm ](#-the-neuxdot-paradigm)** &nbsp;•&nbsp;
**[ 🏛️ Architecture ](#-system-architecture)** &nbsp;•&nbsp;
**[ ✨ 11 Meta-Tools ](#1--11-invariant-meta-tools-cache-stability-guarantee)** &nbsp;•&nbsp;
**[ 🧩 Unified Plugins ](#2--unified-plugin-system)** &nbsp;•&nbsp;
**[ 🧠 Instruction Presets ](#3--agent-instruction-presets)** &nbsp;•&nbsp;
**[ 🛡️ Multi-Account & Roles ](#4--multi-account--role-based-access)** &nbsp;•&nbsp;
**[ 🧩 WASM Sandbox ](#5--sandboxed-webassembly-wasm-extensions)** &nbsp;•&nbsp;
**[ 🪝 Response Hooks ](#6--transparent-response-hooks--handlebars)** &nbsp;•&nbsp;
**[ 🚀 Quick Start ](#-quick-start)** &nbsp;•&nbsp;
**[ 🛠️ Extension Guide ](#-building-wasm-extensions)**

<br/>

</div>

---

## ⚡ The NeuxDOT Paradigm

Traditional agent architectures dump volatile tool JSON schemas, dynamic server updates, and unformatted state directly into the system prompt. Every turn breaks LLM prefix caches, ballooning token bills and injecting multi-second latency penalties.

```
Traditional Framework:
[ Volatile System Prompt (Tool Schemas + Server State) ] ➔ 💥 Cache Miss Every Turn!

NeuxDOT Architecture:
[ ❄️ Invariant Static Prompt + 9 Meta-Tools ] ➔ ⚡ 100% Cache Hit Across All Turns!
```

NeuxDOT eliminates this flaw at the runtime kernel layer:

* ⚡ **100% Cache-Invariance**: System prompts and tool signatures never change across turns. The model pulls external schemas dynamically on-demand using invariant meta-tools.
* 🛡️ **Zero-Trust Tool Discovery**: External MCP endpoints and tools disabled by policy are completely masked from the LLM. Zero prompt leakage.
* 🌸 **Unprompted Event Push Streams**: True asynchronous MCP push notifications (`notifications/resources/updated`) stream live updates directly into context without busy-wait polling.
* 🧩 **Sandboxed WASM Extensibility**: Isolated WebAssembly extensions running inside Wasmtime with host-managed Key-Value stores and seamless data migration between versions.
* ⏳ **Single-Timer Deterministic Sleeping**: Native `sleep(duration)` with timer supersedence, cancellation guarantees, and instant wake-up on push events.
* 🎀 **Pure Handlebars Templating**: No hardcoded system prompt injections. Complete user control over prompt structure and memory formatting.
* 🪝 **Transparent Response Hooks**: Custom message telemetry evaluated with `{{time}}` and `{{context_remaining}}` injected into every tool execution event.

---

## 🏛️ System Architecture

```mermaid
graph TD
    classDef client fill:#1E1B4B,stroke:#6366F1,stroke-width:2px,color:#E0E7FF;
    classDef server fill:#09090B,stroke:#A855F7,stroke-width:2px,color:#F3E8FF;
    classDef core fill:#18181B,stroke:#EC4899,stroke-width:2px,color:#FDF2F8;
    classDef plugin fill:#022C22,stroke:#10B981,stroke-width:2px,color:#D1FAE5;
    classDef storage fill:#172554,stroke:#38BDF8,stroke-width:2px,color:#E0F2FE;
    classDef auth fill:#3B0764,stroke:#C084FC,stroke-width:2px,color:#F3E8FF;

    User(["🌸 User / Web UI / Tauri App"]):::client -->|WebSocket / REST API| Server["🌐 NeuxDOT Server (Axum)"]:::server
    Server --> Auth["🛡️ Auth &amp; Roles<br/>root / member"]:::auth
    Server --> Core["⚡ NeuxDOT Core Runtime Engine"]:::core

    subgraph CoreEngine ["🐾 Core Runtime Kernel"]
        Core --> Router["🔀 Provider Router<br/>(Fallback / Round-Robin / Random)"]:::core
        Core --> TM["⏳ Single-Timer Manager<br/>(Interruptible Sleep)"]:::core
        Core --> Meta["🛡️ 11 Invariant Meta-Tools"]:::core
        Core --> Hooks["🪝 Response Hooks Engine"]:::core
        Core --> Presets["🧠 Instruction Presets<br/>(per-account)"]:::core
        Core --> Trunc["✂️ Head/Body/Tail Truncation<br/>+ ctx header"]:::core
    end

    Router --> LLM["🤖 LLM Providers<br/>(OpenAI / Ollama / Anthropic)"]:::client

    subgraph UnifiedPlugins ["🌸 Unified Plugin System (MCP + WASM + HTTP)"]
        Core --> P_Wasm["🧩 WASM / Wasmtime Sandbox"]:::plugin
        Core --> P_Stdio["🔌 MCP Stdio Transport"]:::plugin
        Core --> P_Http["🌐 MCP SSE / HTTP Transport"]:::plugin
        Core --> P_Custom["🪄 Custom HTTP Tools<br/>(curl-style + captures)"]:::plugin
        P_Wasm --> MCPApps["🖼️ MCP Apps<br/>(sandboxed iframe UI)"]:::plugin
    end

    subgraph Memory ["🧠 Agent Memory & History"]
        Core --> H_Work[("📜 agent_history<br/>working context — prunable")]:::storage
        Core --> H_Arch[("🗄️ agent_transcript<br/>append-only — distillation")]:::storage
        H_Arch -.->|history_search / history_inspect| Core
    end

    P_Wasm -->|Isolated KV| SQLite[("🗄️ SQLite (WAL)")]:::storage
    P_Wasm -->|Isolated FS| DiskStorage["📁 data/hash/"]:::storage
    Auth --> SQLite
    Core -->|Live Event Envelopes| WS["📡 WebSocket Event Broadcaster"]:::server
```

### 💡 Data Plane: Working Context vs. Append-Only Transcript

NeuxDOT keeps two deliberately separate views of an agent's history:

| | `agent_history` | `agent_transcript` |
| :--- | :--- | :--- |
| **Purpose** | The live working context sent to the LLM | Immutable long-horizon record |
| **Mutation** | Pruned and compacted | Append-only, never deleted |
| **Content** | Head/body/tail truncated | Full fidelity |
| **Ordering** | `created_at, rowid` | `seq` (monotonic per agent) |
| **Extra** | — | `origin`, `turn`, `metadata_json` |

The transcript is the source of truth for distillation and for `history_search` / `history_inspect`, so nothing a model can no longer see is ever lost.

---

## 🎯 Sparkling Features

### 1. 🛡️ 11 Invariant Meta-Tools (Cache-Stability Guarantee)

NeuxDOT never inlines sprawling JSON schemas. The model communicates with the world through 11 immutable meta-tools:

| 🎀 Meta-Tool | 📋 Purpose | ⚡ Invariant Guarantee |
| :--- | :--- | :--- |
| `search_tools` | Search external MCP tools by keyword or tag | Filtered by zero-trust agent permissions |
| `get_tool_schema` | Pull parameter specifications on-demand | Dynamically pulled only when needed |
| `call_tool` | Execute verified external MCP tools | Strict JSON schema parameter validation |
| `list_servers` | Inspect connected MCP servers and statuses | Real-time connectivity inspection |
| `setMemory` | Overwrite persistent agent memory with SHA-256 check | Optimistic concurrency locking |
| `editMemory` | Surgical search-and-replace update to memory | Protected by hash matching |
| `readMemory` | Retrieve current untruncated memory content and hash | Integrity-safe memory inspection |
| `compact` | Rebuild the working context and re-install a freshly rendered system instruction | Cache-safe: applies changed presets/memory at a context-rebuild point |
| `sleep` | Pause execution for human durations (`10s`, `1m`, `2h`) | Interruptible via incoming push events |
| `history_search` | Regex search across the full append-only transcript | Recovers text lost to truncation or compaction |
| `history_inspect` | Read an exact character range of one transcript entry | Pairs with the `<<truncated:from-to>>` markers |

Every tool response and event is prefixed with a context header (`2026-11-02T07:49:33.124Z ctx:30%`) reporting the UTC timestamp and live context pressure.

Large tool results are rendered as a **head / body / tail** view with explicit `<<truncated:from-to>>` markers; the full payload always lives in the append-only transcript and can be paged back with `history_inspect`. `call_tool` accepts optional `head`/`body`/`tail` overrides per call.

---

### 2. 🧩 Unified Plugin System

MCP servers, WASM extensions and plain HTTP tools are all *plugins* behind one transport
abstraction, so an agent never needs to know how a capability is implemented:

| Transport | Description | Default Permissions |
| :--- | :--- | :--- |
| `wasm` / `wasm_stdio` | Wasmtime-sandboxed extension served over an MCP shim | ui, storage, agent_control |
| `stdio` | Classic MCP server over stdio | ui, process, agent_control |
| `http` | Remote MCP server over SSE / HTTP | ui, network, agent_control |
| `custom_tools` / `curl` | Declarative HTTP tools with session captures | network, storage |

* **MCP Apps** — WASM plugins can ship UI as a sandboxed `<iframe>`, mounted inside the agent workspace via an `app://` resource URI.
* **Custom HTTP Tools** — define a request (method, URL, headers, body) plus *captures* that persist `Set-Cookie` headers or JSON fields into session variables (`TOKEN`, `COOKIES`) for later calls.
* **Per-plugin permissions** (`ui`, `storage`, `process`, `network`, `agent_control`, `identity`) are declared per transport and enforced by the host.
* `/api/plugins` is the canonical endpoint; `/api/mcp` and `/api/extensions` remain as aliases for compatibility.

---

### 3. 🧠 Agent Instruction Presets

System prompts can be reused across agents without duplicating text:

* Presets live in `agent_instruction_presets` and are **strictly per-account** — every query filters by `account_id`, and another account's preset returns `404`, never `403`, so existence is not leaked.
* An agent points at a preset via `instruction_preset_id`; `NULL` means **Custom (default)** and restores the free-text editor.
* Variables (`{{agent.name}}`, `{{agent.memory}}`, `{{tools.preset}}`) work exactly as in a custom prompt.
* Deleting a preset **detaches** agents but keeps their current instruction text, so nothing silently breaks.

> **Cache-safety contract.** Selecting a preset only writes `agents.system_prompt`. A running conversation is never mutated mid-flight — the new instruction is installed when the agent **compacts** (the designated context-rebuild point), so the prompt prefix cache stays warm until then.

---

### 4. 🛡️ Multi-Account & Role-Based Access

* Accounts carry a `role` of `root` or `member`, embedded in the JWT and enforced via `require_root()` on account management.
* **Authority is resolved from the database on every request**, not trusted from the 30-day token — a demoted or deleted account loses access immediately.
* Guardrails: the **last remaining root** cannot be demoted or deleted, and no one may delete their own account.
* Password changes take `old_password` / `new_password` / `confirm_password` (wrong current password → `401`, mismatch → `400`, weak → `400`).
* Profiles support a display name plus a cropped, compressed avatar (`avatar_url`).

---

### 5. 🧩 Sandboxed WebAssembly (WASM) Extensions

Extend agent capabilities safely with pre-compiled WebAssembly:
* **Isolated Host-Managed Storage**: Each extension gets a dedicated Key-Value namespace and isolated filesystem sandbox in `data/<wasm_hash>/`.
* **Zero-Downtime Data Migration**: Updating an extension automatically migrates existing storage files to the new build and removes stale directories.
* **Manifest Inspection Modal**: Preview incoming tool definitions, UI panels, and storage requirements before committing updates.

```
┌────────────────────────────────────────────────────────┐
│  🧩 WASM Sandbox (Wasmtime)                            │
│  ├── 📦 Isolated Storage: data/<hash>/                  │
│  ├── 🔑 Host-Managed KV: extension_kv table             │
│  ├── 🛡️ Safe Host ABI: Memory-bounded syscalls         │
│  └── 🌸 Background Dispatch: host_agent_run_background │
└────────────────────────────────────────────────────────┘
```

---

### 6. 🪝 Transparent Response Hooks & Handlebars

System prompt variables are structured with zero hidden magic:

* `{{agent.name}}`: The agent's configured display name.
* `{{agent.id}}`: Unique Agent UUID.
* `{{agent.memory}}` / `{{agent.memory.content}}`: Current persistent memory string.
* `{{agent.memory.hash}}`: SHA-256 integrity hash of memory.
* `{{tools.preset}}`: Comma-separated list of enabled tool names.
* `{{time}}`: Current formatted local timestamp (e.g. `4:15 PM`).
* `{{context_remaining}}`: Estimated remaining characters before buffer capacity.

#### Transparent Response Hooks Example
Extensions or users can register hooks that automatically append contextual telemetry to every tool execution response:

```json
{"content":[{"text":"Message posted to chat console.","type":"text"}]}
```
```text
[Response Hook]
Current time: 4:15 PM | Buffer remaining: 294,820 chars
```

---

### 7. 📜 Running History Buffer, Truncation & Compaction

* Configurable `max_running_history_chars` per agent (defaults to **300,000** characters), with live progress in the agent dashboard.
* Pruning removes **whole tool-call groups**, never a half-group, so an `assistant` message with `tool_calls` is always followed by one `tool` reply per id — otherwise providers reject the request.
* An interrupted or corrupted transcript is repaired once on load (`normalize_history_transcript`): unanswered calls get a synthetic reply, misplaced replies become context rather than dangling rows.
* `compact` clears the working context, reinstalls a freshly rendered system instruction, and leaves the append-only transcript untouched.

---

## 🚀 Quick Start

### Prerequisites
* 🦀 **Rust** 1.80+ ([Install Rust](https://www.rust-lang.org/tools/install))
* 📦 **Node.js** 20+ & **pnpm** ([Install pnpm](https://pnpm.io/))
* 🗄️ **SQLite3**

### 1. Clone & Install
```bash
git clone https://github.com/NeuxDOT/NeuxDOT.git
cd NeuxDOT
pnpm install
```

### 2. Run the Full Stack in Development

#### Option A: Web Mode (Server + SvelteKit Frontend)
```bash
# Terminal 1: Start the Rust backend server (port 8000)
cargo run -p neuxdot-server

# Terminal 2: Start the modern SvelteKit web client
pnpm --filter web dev
```
Open [http://localhost:5173](http://localhost:5173) in your browser.

#### Option B: Native Desktop App (Tauri v2)
```bash
pnpm --filter web tauri dev
```

---

## 🛠️ Building WASM Extensions

NeuxDOT extensions compile directly to `wasm32-unknown-unknown` without heavy runtime dependencies.

See the included **Agent Chat Console** extension in [`examples/chat-extension/`](examples/chat-extension/):

```bash
# Compile the example extension
cargo build --release --target wasm32-unknown-unknown \
  --manifest-path examples/chat-extension/Cargo.toml

# Upload the resulting binary via the NeuxDOT Settings UI:
# target/wasm32-unknown-unknown/release/chat_extension.wasm
```

### Extension Host ABI Interface
Extensions declare safe host imports for logging, isolated storage, and unprompted background execution:
```rust
#[link(wasm_import_module = "env")]
extern "C" {
    fn host_log(msg_ptr: *const u8, msg_len: usize);
    fn host_kv_get(key_ptr: *const u8, key_len: usize, out_ptr: *mut u8, max_out: usize) -> i32;
    fn host_kv_set(key_ptr: *const u8, key_len: usize, val_ptr: *const u8, val_len: usize) -> i32;
    fn host_fs_write(path_ptr: *const u8, path_len: usize, data_ptr: *const u8, data_len: usize) -> i32;
    fn host_agent_run_background(agent_id_ptr: *const u8, agent_id_len: usize, input_ptr: *const u8, input_len: usize) -> i32;
    fn host_get_time_str(out_ptr: *mut u8, max_out: usize) -> i32;
}
```

---

## 🧪 Testing & Verification

NeuxDOT includes an extensive integration test suite covering meta-tools, prompt interpolation, memory hashing, WASM runners, timer invariants, and zero-trust permissions:

```bash
# Run all workspace unit and E2E integration tests
cargo test --workspace

# Validate web client TypeScript and Svelte runes
pnpm --filter web check

# Verify production frontend bundle
pnpm --filter web build
```

---

## 📂 Repository Structure

```
.
├── apps/
│   └── web/                # SvelteKit 5 (Runes) frontend + Tailwind CSS
├── assets/
│   ├── banner.svg          # Stylized hero banner & typography
│   └── logo.svg            # Chibi mascot SVG logo
├── crates/
│   ├── neuxdot-core/       # Core runtime engine, MCP client, WASM runner, memory
│   └── neuxdot-server/     # Axum REST & WebSocket streaming server
├── examples/
│   ├── chat-extension/     # Reference WASM extension (Chat Console & unprompted turns)
│   └── computer-mcp/       # Reference headless MCP server (terminal / file / desktop tools)
├── src-tauri/              # Tauri v2 desktop shell
├── Cargo.toml              # Cargo workspace manifest
├── pnpm-workspace.yaml     # PNPM monorepo workspace configuration
└── .gitignore              # Security-first ignore rules (zero-data leakage)
```

---

## 🔒 Security & Privacy

* **Zero-Leakage Guarantee**: `.sqlite` files, runtime chat logs, extension storage caches, and `.env` credentials are strictly excluded from version control.
* **Sandboxed Execution**: WASM extensions have no direct access to system memory, sockets, or host files outside their provisioned `data/<hash>/` directory.
* **Zero-Trust Tool Discovery**: Tools cannot be called unless explicitly authorized on an agent level.

---

<div align="center">

```text
✧･ﾟ: *✧･ﾟ:* 🌸 *:･ﾟ✧*:･ﾟ✧ ─── ･ ｡ﾟ☆: *.☽ .* :☆ﾟ. ─── ✧･ﾟ: *✧･ﾟ:* 🌸 *:･ﾟ✧*:･ﾟ✧
```

<img src="assets/logo.svg" alt="NeuxDOT Mascot" width="120" />

### (◕‿◕✿) Stay deterministic, stay kawaii!

Crafted with 💖, 🌸, and lots of green tea.

Licensed under the [MIT License](LICENSE).

</div>
