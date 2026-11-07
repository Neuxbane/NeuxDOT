import { api } from '$lib/api';

export interface TerminalEntry {
	id: string;
	timestamp: string;
	type: string;
	agent_id: string;
	title: string;
	details?: string;
	isError?: boolean;
	isMcpPush?: boolean;
}

class EventStore {
	logs = $state<TerminalEntry[]>([]);
	isConnected = $state<boolean>(false);
	currentAgentId = $state<string | null>(null);
/**
 * Bumped whenever an event implies the agent's persisted running history
 * changed. Views can react to it to refetch from the server, so the UI
 * reflects real turn/tool activity instead of a one-off snapshot.
 */
historyRevision = $state<number>(0);
	private ws: WebSocket | null = null;
	private reconnectTimer: number | null = null;

	setAgent(agentId: string | null) {
		if (this.currentAgentId === agentId && this.ws) return;
		this.currentAgentId = agentId;
		this.connect();
	}

	/** Signals that persisted history may have changed. */
bumpHistory() {
this.historyRevision++;
}

connect() {
		if (typeof window === 'undefined') return;
		if (this.ws) {
			this.ws.close();
			this.ws = null;
		}

		const serverUrl = api.getServerUrl();
		const wsProto = serverUrl.startsWith('https') ? 'wss' : 'ws';
		const host = serverUrl.replace(/^https?:\/\//, '');

		const query = this.currentAgentId ? `?agent_id=${encodeURIComponent(this.currentAgentId)}` : '';
		const url = `${wsProto}://${host}/ws/events${query}`;

		try {
			this.ws = new WebSocket(url);

			this.ws.onopen = () => {
				this.isConnected = true;
				this.addLog({
					type: 'system',
					title: 'WebSocket connected to NeuxDOT event stream',
					details: `Listening for agent: ${this.currentAgentId || 'all'}`
				});
			};

			this.ws.onmessage = (event) => {
				try {
					const envelope = JSON.parse(event.data);
					this.handleEventEnvelope(envelope);
				} catch (e) {
					console.error('Failed to parse WS message:', e);
				}
			};

			this.ws.onclose = () => {
				this.isConnected = false;
				this.scheduleReconnect();
			};

			this.ws.onerror = () => {
				this.isConnected = false;
			};
		} catch {
			this.scheduleReconnect();
		}
	}

	private scheduleReconnect() {
		if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
		this.reconnectTimer = window.setTimeout(() => {
			if (this.currentAgentId) {
				this.connect();
			}
		}, 3000);
	}

	private handleEventEnvelope(envelope: any) {
		const ev = envelope.event;
		const type = ev.type;
		const payload = ev.payload;
		const time = new Date(envelope.timestamp).toLocaleTimeString();

		switch (type) {
			case 'provider_selected':
				this.addLog({
					timestamp: time,
					type: 'provider',
					title: `Provider Selected: ${payload.provider_name} (${payload.model})`,
					details: `Strategy: ${payload.strategy}`
				});
				break;

			case 'tool_search':
				this.addLog({
					timestamp: time,
					type: 'tool_search',
					title: `Meta-Tool: search_tools("${payload.query}")`,
					details: `Matches returned: ${payload.results_count}`
				});
				break;

			case 'tool_schema_fetched':
				this.addLog({
					timestamp: time,
					type: 'tool_schema',
					title: `Meta-Tool: get_tool_schema("${payload.tool_name}")`
				});
				break;

			case 'tool_call_started':
				this.addLog({
					timestamp: time,
					type: 'tool_call',
					title: `Executing: ${payload.tool_name}()`,
					details: JSON.stringify(payload.arguments, null, 2)
				});
				break;

			case 'tool_call_completed':
				this.bumpHistory();
				this.addLog({
					timestamp: time,
					type: 'tool_result',
					title: `Result: ${payload.tool_name} (success=${payload.success})`,
					details: JSON.stringify(payload.result, null, 2),
					isError: !payload.success
				});
				break;

			case 'mcp_pushed_notification':
				this.bumpHistory();
				this.addLog({
					timestamp: time,
					type: 'mcp_push',
					title: `🔔 [MCP PUSH] ${payload.mcp_name} pushed: ${payload.method}`,
					details: JSON.stringify(payload.payload, null, 2),
					isMcpPush: true
				});
				break;

			case 'memory_updated':
				this.addLog({
					timestamp: time,
					type: 'memory',
					title: `🧠 Memory Updated (hash: ${payload.updated_hash.substring(0, 10)}...)`,
					details: `Character count: ${payload.char_count} / 5,000`
				});
				break;

			case 'memory_compacted':
				this.bumpHistory();
				this.addLog({
					timestamp: time,
					type: 'compact',
					title: `📦 History Compacted to Prompt + Memory`,
					details: `Current hash: ${payload.current_hash.substring(0, 10)}...`
				});
				break;

			case 'memory_read':
				this.addLog({
					timestamp: time,
					type: 'memory',
					title: `📖 Memory Read (chars: ${payload.char_count})`,
					details: `Current hash: ${payload.current_hash.substring(0, 10)}...`
				});
				break;

			case 'agent_sleeping':
				this.addLog({
					timestamp: time,
					type: 'sleep',
					title: `💤 Agent Sleeping for ${payload.duration} (interruptible=${payload.interruptible})`,
					details: `Sleep meta-tool executing...`
				});
				break;

			case 'agent_woke_up':
				this.addLog({
					timestamp: time,
					type: 'wake',
					title: `⏰ Agent Woke Up (${payload.reason})`
				});
				break;

			case 'text_disallowed':
				this.addLog({
					timestamp: time,
					type: 'text_disallowed',
					title: `⛔ Flow Control: Plain Output Text Blocked`,
					details: `Prompted agent: "${payload.error_message}"`,
					isError: true
				});
				break;

			case 'turn_started':
				this.bumpHistory();
				this.addLog({
					timestamp: time,
					type: 'turn_started',
					title: `Agent turn started (${payload.turn_id.substring(0, 8)})`
				});
				break;

			case 'turn_completed':
				this.bumpHistory();
				this.addLog({
					timestamp: time,
					type: 'turn_completed',
					title: `Agent turn finished`,
					details: payload.content
				});
				break;

			case 'error':
				this.addLog({
					timestamp: time,
					type: 'error',
					title: `Error: ${payload.message}`,
					isError: true
				});
				break;

			case 'info':
				this.addLog({
					timestamp: time,
					type: 'info',
					title: payload.message
				});
				break;
		}
	}

	addLog(entry: {
		timestamp?: string;
		type: string;
		title: string;
		details?: string;
		isError?: boolean;
		isMcpPush?: boolean;
	}) {
		const newEntry: TerminalEntry = {
			id: Math.random().toString(36).substring(2, 9),
			timestamp: entry.timestamp || new Date().toLocaleTimeString(),
			type: entry.type,
			agent_id: this.currentAgentId || '',
			title: entry.title,
			details: entry.details,
			isError: entry.isError,
			isMcpPush: entry.isMcpPush
		};

		this.logs.push(newEntry);
		if (this.logs.length > 200) {
			this.logs.shift();
		}
	}

	clear() {
		this.logs = [];
	}
}

export const eventStore = new EventStore();
