import http from 'node:http';
import fs from 'node:fs/promises';
import { existsSync, statSync } from 'node:fs';
import path from 'node:path';
import { exec } from 'node:child_process';
import crypto from 'node:crypto';

// Configuration
const PORT = parseInt(process.env.PORT || '3001', 10);
const HOST = process.env.HOST || '0.0.0.0';

// Generate a secure one-time API key for this server instance
const ONE_TIME_API_KEY =
  process.env.MCP_API_KEY || `mcp_live_${crypto.randomBytes(18).toString('hex')}`;

// Connected SSE clients for server push notifications
const sseClients = new Set();

/**
 * Truncates long command output with head, body, and tail preserves for security and token economy.
 * Defaults to 1000 chars head, 1000 chars body, 1000 chars tail.
 */
function truncateOutput(text, head = 1000, body = 1000, tail = 1000) {
  const h = Math.max(0, parseInt(head ?? 1000, 10));
  const b = Math.max(0, parseInt(body ?? 1000, 10));
  const t = Math.max(0, parseInt(tail ?? 1000, 10));
  const total = text.length;
  const maxAllowed = h + b + t;

  if (total <= maxAllowed) {
    return text;
  }

  const headPart = text.slice(0, h);
  const tailPart = text.slice(Math.max(0, total - t));

  const middleStart = Math.max(h, Math.floor((total - b) / 2));
  const middleEnd = Math.min(total - t, middleStart + b);
  const bodyPart = text.slice(middleStart, middleEnd);

  const truncBefore = Math.max(0, middleStart - h);
  const truncAfter = Math.max(0, total - t - middleEnd);

  return [
    headPart,
    `\n\n[... Truncation notice: skipped ${truncBefore.toLocaleString()} characters ...]\n\n`,
    bodyPart,
    `\n\n[... Truncation notice: skipped ${truncAfter.toLocaleString()} characters ...]\n\n`,
    tailPart,
  ].join('');
}

/**
 * Tool definitions exposed by this MCP server
 */
const TOOLS = [
  {
    name: 'execute',
    description:
      'Execute a bash command with secure output truncation (head/body/tail security controls)',
    inputSchema: {
      type: 'object',
      properties: {
        bash: {
          type: 'string',
          description: 'The bash command to execute in shell environment',
        },
        head: {
          type: 'integer',
          description: 'Head character limit for truncation security (default: 1000)',
          default: 1000,
        },
        body: {
          type: 'integer',
          description: 'Middle body character limit for truncation security (default: 1000)',
          default: 1000,
        },
        tail: {
          type: 'integer',
          description: 'Tail character limit for truncation security (default: 1000)',
          default: 1000,
        },
      },
      required: ['bash'],
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
    case 'execute': {
      const command = args.bash;
      if (!command || typeof command !== 'string') {
        throw new Error("Missing required 'bash' command string");
      }
      const head = args.head ?? 1000;
      const body = args.body ?? 1000;
      const tail = args.tail ?? 1000;

      return new Promise((resolve) => {
        exec(
          command,
          {
            shell: '/bin/bash',
            timeout: 60000,
            maxBuffer: 10 * 1024 * 1024, // 10MB buffer
          },
          (err, stdout, stderr) => {
            const exitCode = err?.code ?? (err ? 1 : 0);
            const combined = [
              stdout ? stdout : '',
              stderr ? (stdout ? '\n[STDERR]\n' + stderr : stderr) : '',
            ].join('');

            const truncated = truncateOutput(combined || '(no output produced)', head, body, tail);
            resolve({
              exit_code: exitCode,
              output: truncated,
              is_truncated: combined.length > head + body + tail,
              raw_length: combined.length,
            });
          }
        );
      });
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
            resources: { subscribe: false, listChanged: false },
          },
        },
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
║   - execute(bash, head=1000, body=1000, tail=1000)                         ║
║   - list(path=".")                                                         ║
║   - read(path, from?, to?)                                                 ║
║   - set(path, content)                                                     ║
║   - replace(path, search, content)                                         ║
╚════════════════════════════════════════════════════════════════════════════╝
`);
});
