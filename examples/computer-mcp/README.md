# Computer MCP Server v2 (HTTP / SSE)

A standalone **Model Context Protocol (MCP) v2** server providing secure filesystem and shell execution tools over **HTTP / Server-Sent Events (SSE)** with **one-time generated API key authentication**.

Designed to connect with **NeuxDOT** and any standard MCP client.

---

## Features

- **Standalone HTTP/SSE Server**: Zero external dependencies. Runs on pure Node.js (Node 18+).
- **One-Time API Key Generation**: Every time the server starts, it generates a fresh cryptographically secure API key and prints the ready-to-paste NeuxDOT configuration to the console.
- **Strict Authentication**: Rejects unauthenticated requests with HTTP `401 Unauthorized` and `WWW-Authenticate: Bearer realm="computer-mcp"`.
- **Background Execution & Timeouts**: `terminal` supports detached background execution and configurable timeouts.

---

## Tools

| Tool | Parameters | Description |
|---|---|---|
| **`terminal`** | `bash: string`<br>`runInBackground?: boolean` *(default: false)*<br>`timeout?: string` *(default: "120s")* | Executes a bash command in the terminal environment with timeout or background execution support. Truncation is managed by the meta tool executor. |
| **`list`** | `path?: string` *(default: ".")* | Lists files and directories with entry type, byte size, and last modified timestamps. |
| **`read`** | `path: string`<br>`from?: number`<br>`to?: number` | Reads a file with optional 1-based line slicing (`from` to `to`) and line numbering. |
| **`set`** | `path: string`<br>`content: string` | Creates or overwrites a file with content (automatically creates parent directories). |
| **`replace`** | `path: string`<br>`search: string`<br>`content: string` | Replaces exact string occurrences in an existing file. |

---

## Quick Start

### 1. Run the Server

```bash
cd examples/computer-mcp
node server.js
```

Or specify a custom port:

```bash
PORT=3001 node server.js
```

### 2. Startup Banner

When launched, the server outputs its dynamic one-time credentials:

```text
╔════════════════════════════════════════════════════════════════════════════╗
║                   COMPUTER MCP SERVER v2 (HTTP / SSE)                      ║
╠════════════════════════════════════════════════════════════════════════════╣
║ Status:      ONLINE                                                        ║
║ Server URL:  http://localhost:3001/sse                                     ║
║                                                                            ║
║ ONE-TIME GENERATED API KEY:                                                ║
║   mcp_live_e89fa2bc837190827361548172901a8f3b21                            ║
╠════════════════════════════════════════════════════════════════════════════╣
║ NEUXDOT MCP CONFIGURATION:                                                 ║
║   1. Go to NeuxDOT -> MCP Settings (/mcp) -> "Add MCP Server"              ║
║   2. Server Name:  Computer MCP                                            ║
║   3. Transport:    http / SSE                                              ║
║   4. Endpoint URL: http://localhost:3001/sse                               ║
║   5. Alias:        computer                                                ║
║   6. Headers & Auth:                                                       ║
║      Authorization: Bearer mcp_live_e89fa2bc837190827361548172901a8f3b21    ║
╚════════════════════════════════════════════════════════════════════════════╝
```

---

## Connecting to NeuxDOT

1. Open NeuxDOT at `http://localhost:5173/mcp` (or your NeuxDOT desktop app).
2. Click **Add MCP Server**:
   - **Server Name:** `Computer MCP`
   - **Transport:** `http / SSE (Streamable URL)`
   - **SSE Endpoint URL:** `http://localhost:3001/sse`
   - **Unique Tool Call Alias:** `computer`
   - **Headers & Authentication:**
     ```http
     Authorization: Bearer <PASTE_YOUR_GENERATED_API_KEY>
     ```
3. Click **Add Server**.
4. Enable the server on your agent:
   - Your agent now has access to:
     - `computer.terminal`
     - `computer.list`
     - `computer.read`
     - `computer.set`
     - `computer.replace`

---

## Authentication Verification

You can verify the authentication using `curl`:

### Without API Key (Rejection)
```bash
curl -i http://localhost:3001/sse
# HTTP/1.1 401 Unauthorized
# WWW-Authenticate: Bearer realm="computer-mcp"
# {"jsonrpc":"2.0","error":{"code":-32000,"message":"Unauthorized: Invalid or missing API key"}}
```

### With API Key (Connected)
```bash
curl -i -H "Authorization: Bearer <API_KEY>" http://localhost:3001/sse
# HTTP/1.1 200 OK
# Content-Type: text/event-stream
# event: open
# data: {"status":"connected"}
```
