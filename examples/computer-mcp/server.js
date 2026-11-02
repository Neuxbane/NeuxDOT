import http from 'node:http';
import fs from 'node:fs/promises';
import { existsSync, statSync } from 'node:fs';
import path from 'node:path';
import { exec, spawn } from 'node:child_process';
import crypto from 'node:crypto';

// Configuration
const PORT = parseInt(process.env.PORT || '3001', 10);
const HOST = process.env.HOST || '0.0.0.0';

// Generate a secure one-time API key for this server instance
const ONE_TIME_API_KEY =
  process.env.MCP_API_KEY || `mcp_live_${crypto.randomBytes(18).toString('hex')}`;

// Connected SSE clients for server push notifications
const sseClients = new Set();
const subscribedUris = new Set();

// Managed background and active terminal processes
const managedTerminals = new Map();

/**
 * Resources exposed for real-time Push MCP subscriptions
 */
const RESOURCES = [
  {
    uri: 'terminal://completed',
    name: 'Terminal Process Finished Events',
    description:
      'Real-time push notifications when a background or detached terminal command finishes execution',
    mimeType: 'application/json',
  },
  {
    uri: 'terminal://events',
    name: 'Terminal Process Lifecycle Stream',
    description:
      'Stream of all terminal lifecycle events including spawn, background detach, completion, and exit errors',
    mimeType: 'application/json',
  },
];

/**
 * Broadcasts a Push MCP notification over Server-Sent Events (SSE) to all connected clients
 */
function broadcastPushNotification(method, params) {
  const payload = JSON.stringify({
    jsonrpc: '2.0',
    method,
    params,
  });

  const eventMessage = `data: ${payload}\n\n`;

  for (const client of sseClients) {
    try {
      client.write(eventMessage);
    } catch {
      sseClients.delete(client);
    }
  }
}

/**
 * Parses duration strings like "120s", "5m", "500ms" into milliseconds.
 */
function parseTimeout(val, defaultMs = 120000) {
  if (val === undefined || val === null || val === '') return defaultMs;
  if (typeof val === 'number') return val;
  const match = String(val).trim().match(/^(\d+(?:\.\d+)?)\s*(ms|s|m|h)?$/i);
  if (!match) return defaultMs;
  const num = parseFloat(match[1]);
  const unit = (match[2] || 's').toLowerCase();
  switch (unit) {
    case 'ms':
      return Math.round(num);
    case 's':
      return Math.round(num * 1000);
    case 'm':
      return Math.round(num * 60 * 1000);
    case 'h':
      return Math.round(num * 60 * 60 * 1000);
    default:
      return Math.round(num * 1000);
  }
}

/**
 * Tool definitions exposed by this MCP server
 */
const TOOLS = [
  {
    name: 'terminal',
    description:
      'Execute a bash command in the terminal environment. If execution exceeds the timeout, it automatically detaches into the background with a PID.',
    inputSchema: {
      type: 'object',
      properties: {
        bash: {
          type: 'string',
          description: 'The bash command to execute in shell environment',
        },
        runInBackground: {
          type: 'boolean',
          description: 'Whether to run the command in background without waiting (default: false)',
          default: false,
        },
        timeout: {
          type: 'string',
          description: 'Execution timeout duration, e.g. "120s", "30s", "5m" (default: "120s")',
          default: '120s',
        },
      },
      required: ['bash'],
    },
  },
  {
    name: 'list_terminal',
    description: 'List all managed terminal processes (running, detached, completed, or failed)',
    inputSchema: {
      type: 'object',
      properties: {
        activeOnly: {
          type: 'boolean',
          description: 'If true, only returns active/running or detached processes (default: false)',
          default: false,
        },
      },
    },
  },
  {
    name: 'check_terminal',
    description: 'Check status and latest output for a specific terminal process ID (PID)',
    inputSchema: {
      type: 'object',
      properties: {
        pid: {
          type: 'integer',
          description: 'The process ID of the terminal to inspect',
        },
      },
      required: ['pid'],
    },
  },
  {
    name: 'kill_terminal',
    description: 'Send a termination signal (SIGTERM/SIGKILL) to a managed terminal process',
    inputSchema: {
      type: 'object',
      properties: {
        pid: {
          type: 'integer',
          description: 'Process ID (PID) of the terminal process to kill',
        },
        signal: {
          type: 'string',
          description: 'Signal to send (e.g. SIGTERM, SIGKILL). Default: SIGTERM',
          default: 'SIGTERM',
        },
      },
      required: ['pid'],
    },
  },
  {
    name: 'list',
    description: 'List contents (files and directories) with metadata at the specified path',
    inputSchema: {
      type: 'object',
      properties: {
        path: {
          type: 'string',
          description: 'Path of the directory to list (default: current directory .)',
          default: '.',
        },
      },
    },
  },
  {
    name: 'read',
    description: 'Read file content with optional 1-based line range (from / to)',
    inputSchema: {
      type: 'object',
      properties: {
        path: {
          type: 'string',
          description: 'Relative or absolute file path to read',
        },
        from: {
          type: 'integer',
          description: 'Start line number (1-based, inclusive)',
        },
        to: {
          type: 'integer',
          description: 'End line number (1-based, inclusive)',
        },
      },
      required: ['path'],
    },
  },
  {
    name: 'set',
    description: 'Write complete content to a file (creates parent directories if needed)',
    inputSchema: {
      type: 'object',
      properties: {
        path: {
          type: 'string',
          description: 'Path of the file to create or overwrite',
        },
        content: {
          type: 'string',
          description: 'Full text content to write into the file',
        },
      },
      required: ['path', 'content'],
    },
  },
  {
    name: 'replace',
    description: 'Find and replace exact text string inside a file',
    inputSchema: {
      type: 'object',
      properties: {
        path: {
          type: 'string',
          description: 'Path of the file to modify',
        },
        search: {
          type: 'string',
          description: 'Exact text substring to search for',
        },
        content: {
          type: 'string',
          description: 'Replacement text content',
        },
      },
      required: ['path', 'search', 'content'],
    },
  },
];

/**
 * Executes a tool by name with specified arguments
 */
async function callTool(name, args = {}) {
  switch (name) {
    case 'terminal':
    case 'execute': {
      const command = args.bash;
      if (!command || typeof command !== 'string') {
        throw new Error("Missing required 'bash' command string");
      }
      const runInBackground = Boolean(args.runInBackground);
      const timeoutMs = parseTimeout(args.timeout ?? '120s', 120000);

      const child = spawn('/bin/bash', ['-c', command], {
        detached: process.platform !== 'win32',
      });

      const entry = {
        pid: child.pid,
        command,
        startTime: Date.now(),
        endTime: null,
        status: 'running',
        exitCode: null,
        output: '',
        child,
      };
      managedTerminals.set(child.pid, entry);

      if (child.stdout) {
        child.stdout.on('data', (chunk) => {
          entry.output += chunk.toString();
          if (entry.output.length > 50 * 1024 * 1024) {
            entry.output = entry.output.slice(-50 * 1024 * 1024);
          }
        });
      }

      if (child.stderr) {
        child.stderr.on('data', (chunk) => {
          entry.output += chunk.toString();
          if (entry.output.length > 50 * 1024 * 1024) {
            entry.output = entry.output.slice(-50 * 1024 * 1024);
          }
        });
      }

      child.on('close', (code) => {
        entry.endTime = Date.now();
        entry.exitCode = code ?? 0;
        entry.status = code === 0 ? 'completed' : 'failed';

        const durationSeconds = Math.round(
          ((entry.endTime || Date.now()) - entry.startTime) / 1000
        );
        const notificationPayload = {
          uri: 'terminal://completed',
          pid: entry.pid,
          command: entry.command,
          status: entry.status,
          exit_code: entry.exitCode,
          duration_seconds: durationSeconds,
          output: entry.output,
          message: `Terminal PID ${entry.pid} ('${entry.command}') ${entry.status} with exit code ${entry.exitCode} in ${durationSeconds}s.`,
        };

        // 1. Standard MCP Resource update event
        broadcastPushNotification('notifications/resources/updated', {
          uri: 'terminal://completed',
        });

        // 2. Direct message push notification for agent wake-up & context ingestion
        broadcastPushNotification('notifications/message', notificationPayload);
      });

      child.on('error', (err) => {
        entry.endTime = Date.now();
        entry.exitCode = 1;
        entry.status = 'failed';
        entry.output += `\n[Process Error]: ${err.message}\n`;

        broadcastPushNotification('notifications/message', {
          uri: 'terminal://completed',
          pid: entry.pid,
          command: entry.command,
          status: 'failed',
          exit_code: 1,
          output: entry.output,
          message: `Terminal PID ${entry.pid} ('${entry.command}') failed: ${err.message}`,
        });
      });

      if (runInBackground) {
        return {
          status: 'running_in_background',
          pid: child.pid,
          command,
          message: `Process started in background with PID ${child.pid}. Use check_terminal(pid: ${child.pid}) to inspect output.`,
        };
      }

      return new Promise((resolve) => {
        let settled = false;

        const timer = setTimeout(() => {
          if (!settled) {
            settled = true;
            entry.status = 'detached_due_to_timeout';
            resolve({
              status: 'detached_due_to_timeout',
              pid: child.pid,
              command,
              timeout: args.timeout || '120s',
              message: `Command exceeded timeout of ${args.timeout || '120s'}. Process was automatically detached into the background with PID ${child.pid}. Use check_terminal(pid: ${child.pid}) to inspect its output or list_terminal() to view all active processes.`,
              output_so_far: entry.output || '(no output produced yet)',
            });
          }
        }, timeoutMs);

        child.on('close', (code) => {
          if (!settled) {
            settled = true;
            clearTimeout(timer);
            resolve({
              status: code === 0 ? 'completed' : 'failed',
              exit_code: code ?? 0,
              output: entry.output || '(no output produced)',
            });
          }
        });

        child.on('error', (err) => {
          if (!settled) {
            settled = true;
            clearTimeout(timer);
            resolve({
              status: 'failed',
              exit_code: 1,
              output: entry.output || err.message,
            });
          }
        });
      });
    }

    case 'list_terminal': {
      const activeOnly = Boolean(args.activeOnly);
      const list = [];
      const now = Date.now();

      for (const [pid, entry] of managedTerminals.entries()) {
        const isRunning =
          entry.status === 'running' || entry.status === 'detached_due_to_timeout';
        if (activeOnly && !isRunning) {
          continue;
        }

        const durationMs = (entry.endTime ?? now) - entry.startTime;
        list.push({
          pid,
          command: entry.command,
          status: entry.status,
          exit_code: entry.exitCode,
          duration_seconds: Math.round(durationMs / 1000),
          output_length_chars: entry.output.length,
          start_time: new Date(entry.startTime).toISOString(),
          end_time: entry.endTime ? new Date(entry.endTime).toISOString() : null,
        });
      }

      return {
        count: list.length,
        terminals: list,
      };
    }

    case 'check_terminal': {
      const pid = parseInt(args.pid, 10);
      if (isNaN(pid)) {
        throw new Error("Missing or invalid 'pid' parameter");
      }

      const entry = managedTerminals.get(pid);
      if (!entry) {
        throw new Error(
          `No managed terminal process found with PID ${pid}. Use list_terminal() to view available PIDs.`
        );
      }

      const now = Date.now();
      const durationMs = (entry.endTime ?? now) - entry.startTime;

      return {
        pid: entry.pid,
        command: entry.command,
        status: entry.status,
        exit_code: entry.exitCode,
        duration_seconds: Math.round(durationMs / 1000),
        output: entry.output || '(no output produced yet)',
        total_output_chars: entry.output.length,
        start_time: new Date(entry.startTime).toISOString(),
        end_time: entry.endTime ? new Date(entry.endTime).toISOString() : null,
      };
    }

    case 'kill_terminal': {
      const pid = parseInt(args.pid, 10);
      if (isNaN(pid)) {
        throw new Error("Missing or invalid 'pid' parameter");
      }
      const entry = managedTerminals.get(pid);
      if (!entry) {
        throw new Error(`No managed terminal process found with PID ${pid}`);
      }
      const signal = args.signal || 'SIGTERM';
      try {
        if (process.platform !== 'win32') {
          process.kill(-entry.pid, signal);
        } else {
          process.kill(entry.pid, signal);
        }
      } catch {
        try {
          process.kill(entry.pid, signal);
        } catch (err) {
          throw new Error(`Failed to kill process ${pid}: ${err.message}`);
        }
      }
      entry.status = 'killed';
      entry.endTime = Date.now();
      return {
        pid,
        status: 'killed',
        signal,
        message: `Signal ${signal} sent to process ${pid}`,
      };
    }

    case 'list': {
      const targetPath = path.resolve(args.path || '.');
      if (!existsSync(targetPath)) {
        throw new Error(`Directory path does not exist: ${targetPath}`);
      }
      const entries = await fs.readdir(targetPath, { withFileTypes: true });
      const items = entries.map((entry) => {
        const full = path.join(targetPath, entry.name);
        let size = null;
        let modified = null;
        try {
          const stat = statSync(full);
          size = entry.isFile() ? stat.size : null;
          modified = stat.mtime.toISOString();
        } catch {
          // ignore stat errors
        }
        return {
          name: entry.name,
          type: entry.isDirectory() ? 'directory' : entry.isFile() ? 'file' : 'other',
          size_bytes: size,
          modified,
        };
      });
      return {
        path: targetPath,
        count: items.length,
        items,
      };
    }

    case 'read': {
      const targetPath = path.resolve(args.path);
      if (!existsSync(targetPath)) {
        throw new Error(`File does not exist: ${targetPath}`);
      }
      const raw = await fs.readFile(targetPath, 'utf8');
      const lines = raw.split(/\r?\n/);
      const totalLines = lines.length;

      const fromLine = args.from ? Math.max(1, parseInt(args.from, 10)) : 1;
      const toLine = args.to ? Math.min(totalLines, parseInt(args.to, 10)) : totalLines;

      if (fromLine > toLine) {
        throw new Error(`'from' line (${fromLine}) cannot be greater than 'to' line (${toLine})`);
      }

      const selected = lines.slice(fromLine - 1, toLine);
      const numberedContent = selected
        .map((line, idx) => `${fromLine + idx} | ${line}`)
        .join('\n');

      return {
        path: targetPath,
        from: fromLine,
        to: toLine,
        total_lines: totalLines,
        content: numberedContent,
        raw_content: selected.join('\n'),
      };
    }

    case 'set': {
      const targetPath = path.resolve(args.path);
      const content = args.content ?? '';
      await fs.mkdir(path.dirname(targetPath), { recursive: true });
      await fs.writeFile(targetPath, content, 'utf8');
      return {
        success: true,
        path: targetPath,
        bytes_written: Buffer.byteLength(content, 'utf8'),
        message: `Successfully wrote file: ${targetPath}`,
      };
    }

    case 'replace': {
      const targetPath = path.resolve(args.path);
      if (!existsSync(targetPath)) {
        throw new Error(`File does not exist: ${targetPath}`);
      }
      const { search, content } = args;
      if (search === undefined || search === '') {
        throw new Error("Missing required 'search' target string");
      }
      if (content === undefined) {
        throw new Error("Missing required replacement 'content' string");
      }

      const original = await fs.readFile(targetPath, 'utf8');
      if (!original.includes(search)) {
        throw new Error(`Search string target not found in ${targetPath}`);
      }

      const occurrences = original.split(search).length - 1;
      const replaced = original.replaceAll(search, content);
      await fs.writeFile(targetPath, replaced, 'utf8');

      return {
        success: true,
        path: targetPath,
        occurrences_replaced: occurrences,
        message: `Replaced ${occurrences} occurrence(s) in ${targetPath}`,
      };
    }

    default:
      throw new Error(`Tool '${name}' not supported by Computer MCP v2 server`);
  }
}

/**
 * Authentication check: checks Authorization: Bearer <key> or X-API-Key: <key>
 */
function isAuthorized(req) {
  const authHeader = req.headers['authorization'] || '';
  if (authHeader.startsWith('Bearer ')) {
    const token = authHeader.slice(7).trim();
    if (token === ONE_TIME_API_KEY) return true;
  }
  const xApiKey = req.headers['x-api-key'] || '';
  if (xApiKey && xApiKey.trim() === ONE_TIME_API_KEY) {
    return true;
  }
  return false;
}

/**
 * Handles JSON-RPC 2.0 requests
 */
async function handleJsonRpc(reqBody) {
  const { jsonrpc, id, method, params } = reqBody;

  switch (method) {
    case 'initialize': {
      return {
        jsonrpc: '2.0',
        id,
        result: {
          protocolVersion: '2024-11-05',
          serverInfo: {
            name: 'computer-mcp',
            version: '2.0.0',
          },
          capabilities: {
            tools: { listChanged: false },
            resources: { subscribe: true, listChanged: true },
          },
        },
      };
    }

    case 'resources/list': {
      return {
        jsonrpc: '2.0',
        id,
        result: {
          resources: RESOURCES,
        },
      };
    }

    case 'resources/subscribe': {
      const uri = params?.uri;
      if (uri) {
        subscribedUris.add(uri);
      }
      return {
        jsonrpc: '2.0',
        id,
        result: {},
      };
    }

    case 'resources/unsubscribe': {
      const uri = params?.uri;
      if (uri) {
        subscribedUris.delete(uri);
      }
      return {
        jsonrpc: '2.0',
        id,
        result: {},
      };
    }

    case 'notifications/initialized': {
      // Notification acknowledgment
      return null;
    }

    case 'ping': {
      return {
        jsonrpc: '2.0',
        id,
        result: {},
      };
    }

    case 'tools/list': {
      return {
        jsonrpc: '2.0',
        id,
        result: {
          tools: TOOLS,
        },
      };
    }

    case 'tools/call': {
      const toolName = params?.name;
      const toolArgs = params?.arguments || {};
      try {
        const toolResult = await callTool(toolName, toolArgs);
        return {
          jsonrpc: '2.0',
          id,
          result: {
            content: [
              {
                type: 'text',
                text:
                  typeof toolResult === 'string'
                    ? toolResult
                    : JSON.stringify(toolResult, null, 2),
              },
            ],
            isError: false,
          },
        };
      } catch (err) {
        return {
          jsonrpc: '2.0',
          id,
          result: {
            content: [
              {
                type: 'text',
                text: `Error executing ${toolName}: ${err.message}`,
              },
            ],
            isError: true,
          },
        };
      }
    }

    default:
      return {
        jsonrpc: '2.0',
        id,
        error: {
          code: -32601,
          message: `Method '${method}' not found`,
        },
      };
  }
}

// HTTP Server
const server = http.createServer(async (req, res) => {
  // CORS Headers
  res.setHeader('Access-Control-Allow-Origin', '*');
  res.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
  res.setHeader('Access-Control-Allow-Headers', 'Content-Type, Authorization, X-API-Key');

  if (req.method === 'OPTIONS') {
    res.writeHead(204);
    res.end();
    return;
  }

  const url = new URL(req.url, `http://${req.headers.host || 'localhost'}`);

  // Authentication enforcement
  if (!isAuthorized(req)) {
    res.writeHead(401, {
      'Content-Type': 'application/json',
      'WWW-Authenticate': 'Bearer realm="computer-mcp"',
    });
    res.end(
      JSON.stringify({
        jsonrpc: '2.0',
        error: {
          code: -32000,
          message: 'Unauthorized: Invalid or missing API key',
        },
      })
    );
    return;
  }

  // SSE Stream Listener Endpoint
  if (req.method === 'GET' && url.pathname.endsWith('/sse')) {
    res.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
    });

    res.write('event: open\ndata: {"status":"connected"}\n\n');
    sseClients.add(res);

    req.on('close', () => {
      sseClients.delete(res);
    });
    return;
  }

  // JSON-RPC POST Endpoint
  if (req.method === 'POST') {
    let bodyData = '';
    req.on('data', (chunk) => {
      bodyData += chunk;
    });

    req.on('end', async () => {
      try {
        const parsed = JSON.parse(bodyData || '{}');
        const response = await handleJsonRpc(parsed);
        if (response) {
          res.writeHead(200, { 'Content-Type': 'application/json' });
          res.end(JSON.stringify(response));
        } else {
          res.writeHead(204);
          res.end();
        }
      } catch (err) {
        res.writeHead(400, { 'Content-Type': 'application/json' });
        res.end(
          JSON.stringify({
            jsonrpc: '2.0',
            error: {
              code: -32700,
              message: `Parse error: ${err.message}`,
            },
          })
        );
      }
    });
    return;
  }

  res.writeHead(404, { 'Content-Type': 'application/json' });
  res.end(JSON.stringify({ error: 'Endpoint not found' }));
});

// Start Server
server.listen(PORT, HOST, () => {
  const localUrl = `http://localhost:${PORT}/sse`;
  console.log(`
╔════════════════════════════════════════════════════════════════════════════╗
║                   COMPUTER MCP SERVER v2 (HTTP / SSE)                      ║
╠════════════════════════════════════════════════════════════════════════════╣
║ Status:      ONLINE                                                        ║
║ Server URL:  ${localUrl.padEnd(58)}║
║                                                                            ║
║ ONE-TIME GENERATED API KEY:                                                ║
║   ${ONE_TIME_API_KEY.padEnd(71)}║
╠════════════════════════════════════════════════════════════════════════════╣
║ NEUXDOT MCP CONFIGURATION:                                                 ║
║   1. Go to NeuxDOT -> MCP Settings (/mcp) -> "Add MCP Server"              ║
║   2. Server Name:  Computer MCP                                            ║
║   3. Transport:    http / SSE                                              ║
║   4. Endpoint URL: ${localUrl.padEnd(54)}║
║   5. Alias:        computer                                                ║
║   6. Headers & Auth:                                                       ║
║      Authorization: Bearer ${ONE_TIME_API_KEY.padEnd(46)}║
╠════════════════════════════════════════════════════════════════════════════╣
║ Tools available:                                                           ║
║   - terminal(bash, runInBackground=false, timeout="120s")                  ║
║   - list_terminal(activeOnly=false)                                        ║
║   - check_terminal(pid)                                                    ║
║   - kill_terminal(pid, signal="SIGTERM")                                   ║
║   - list(path=".")                                                         ║
║   - read(path, from?, to?)                                                 ║
║   - set(path, content)                                                     ║
║   - replace(path, search, content)                                         ║
╠════════════════════════════════════════════════════════════════════════════╣
║ Push MCP Subscriptions:                                                    ║
║   - terminal://completed (auto push when background command finishes)      ║
║   - terminal://events    (full process lifecycle events stream)            ║
╚════════════════════════════════════════════════════════════════════════════╝
`);
});
