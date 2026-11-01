<script lang="ts">
	import { api, type Agent, type ExtensionItem, type ExtensionUiHook } from '$lib/api';
	import {
		MessageSquare,
		Send,
		Trash2,
		ShieldCheck,
		Info,
		Bot,
		User,
		Cpu,
		HardDrive,
		Database,
		CheckCircle2,
		CornerDownLeft
	} from 'lucide-svelte';

	interface Props {
		agent: Agent;
		extension: ExtensionItem;
		hook: ExtensionUiHook;
	}

	let { agent, extension, hook }: Props = $props();

	interface ChatMessage {
		id: string;
		role: 'user' | 'assistant';
		text: string;
		timestamp: string;
	}

	let messages = $state<ChatMessage[]>([]);
	let userInput = $state('');
	let isClearing = $state(false);
	let showTransparency = $state(false);
	let scrollContainer = $state<HTMLDivElement | null>(null);

	// Load history when agent or extension changes, and poll continuously for unprompted AI messages
	$effect(() => {
		if (!agent?.id || !extension?.id) return;
		loadHistory(true);

		const timer = setInterval(() => {
			loadHistory(false);
		}, 1200);

		return () => clearInterval(timer);
	});

	function scrollToBottom() {
		if (typeof window !== 'undefined') {
			setTimeout(() => {
				if (scrollContainer) {
					scrollContainer.scrollTop = scrollContainer.scrollHeight;
				}
			}, 30);
		}
	}

	async function loadHistory(autoScroll = false) {
		try {
			const res = await api.extensions.executeAction<ChatMessage[]>(extension.id, 'get_history', {
				agent_id: agent.id
			});
			if (Array.isArray(res)) {
				const valid = res.filter((m) => m && m.text && m.text.trim().length > 0);
				// Detect if new messages arrived
				const hasNewMessages = valid.length !== messages.length ||
					(valid.length > 0 && valid[valid.length - 1].id !== messages[messages.length - 1]?.id);
				if (hasNewMessages) {
					messages = valid;
					scrollToBottom();
				}
			}
		} catch (e) {
			console.error('Failed to load extension chat history:', e);
		}
	}

	function formatTime(): string {
		return new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit', hour12: true });
	}

	async function handleSend() {
		const text = userInput.trim();
		if (!text) return;

		// Immediately clear input so user can type subsequent messages without waiting!
		userInput = '';
		const currentTime = formatTime();
		const tempId = 'usr_' + Date.now();

		// Optimistic append
		messages = [
			...messages,
			{
				id: tempId,
				role: 'user',
				text,
				timestamp: currentTime
			}
		];
		scrollToBottom();

		try {
			const res = await api.extensions.executeAction<ChatMessage[]>(extension.id, 'send_message', {
				agent_id: agent.id,
				message: text,
				timestamp: currentTime
			});
			if (Array.isArray(res)) {
				const valid = res.filter((m) => m && m.text && m.text.trim().length > 0);
				if (valid.length > 0) {
					messages = valid;
					scrollToBottom();
				}
			}
		} catch (e) {
			console.error('Failed to send message:', e);
		}
	}

	async function handleClear() {
		if (!confirm(`Clear extension conversation history for ${agent.name}?`)) return;
		isClearing = true;
		try {
			await api.extensions.executeAction(extension.id, 'clear_history', {
				agent_id: agent.id
			});
			messages = [];
		} catch (e) {
			alert('Failed to clear extension history: ' + e);
		} finally {
			isClearing = false;
		}
	}
</script>

<div id="extension-panel-{hook.panel_id || 'chat'}" class="flex flex-col h-full bg-zinc-50/50 dark:bg-zinc-950/40">
	<!-- Top Bar -->
	<header class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/80 dark:bg-zinc-900/60 backdrop-blur-md px-6 py-3.5">
		<div class="flex items-center gap-3">
			<div class="flex h-8 w-8 items-center justify-center rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20">
				<MessageSquare class="h-4 w-4" />
			</div>
			<div>
				<div class="flex items-center gap-2">
					<h2 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">{hook.title || 'Interactive Chat'}</h2>
					<span class="rounded-full px-2 py-0.5 text-[9px] font-mono bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20">
						WASM Sandboxed
					</span>
				</div>
				<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
					Powered by {extension.name} <span class="font-mono text-[10px]">v{extension.version}</span>
				</p>
			</div>
		</div>

		<div class="flex items-center gap-2">
			<!-- Transparency Info Button -->
			<button
				type="button"
				class="flex items-center gap-1.5 rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-2.5 py-1 text-xs font-medium text-zinc-600 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors shadow-2xs"
				onclick={() => (showTransparency = !showTransparency)}
				title="View radical transparency & host permissions"
			>
				<ShieldCheck class="h-3.5 w-3.5 text-emerald-500" />
				<span>Transparency</span>
			</button>

			<!-- Clear History -->
			{#if messages.length > 0}
				<button
					type="button"
					class="flex items-center gap-1.5 rounded-lg border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-2.5 py-1 text-xs font-medium text-zinc-500 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-950/20 transition-colors shadow-2xs"
					onclick={handleClear}
					disabled={isClearing}
					title="Clear conversation history from isolated KV"
				>
					<Trash2 class="h-3.5 w-3.5" />
					<span>Clear</span>
				</button>
			{/if}
		</div>
	</header>

	<!-- Radical Transparency Drawer (Linux-style transparent consent) -->
	{#if showTransparency}
		<div class="border-b border-indigo-500/20 bg-indigo-500/5 px-6 py-3 text-xs space-y-2">
			<div class="flex items-center justify-between">
				<div class="flex items-center gap-1.5 font-semibold text-indigo-700 dark:text-indigo-300">
					<Info class="h-4 w-4" />
					<span>Radical Transparency: Extension Sandbox Contract</span>
				</div>
				<span class="text-[10px] font-mono text-zinc-400">
					Hash: {extension.wasm_hash.substring(0, 12)}...
				</span>
			</div>
			<div class="grid grid-cols-1 md:grid-cols-3 gap-3 pt-1">
				<div class="rounded-lg border border-indigo-200/50 dark:border-indigo-900/50 bg-white/60 dark:bg-zinc-900/60 p-2.5">
					<div class="flex items-center gap-1.5 text-[11px] font-semibold text-zinc-700 dark:text-zinc-300 mb-1">
						<Database class="h-3.5 w-3.5 text-indigo-500" />
						<span>Isolated Host KV</span>
					</div>
					<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
						Partitioned by WASM hash. Storing <code>chat_history_{agent.id}</code> in SQLite.
					</p>
				</div>

				<div class="rounded-lg border border-indigo-200/50 dark:border-indigo-900/50 bg-white/60 dark:bg-zinc-900/60 p-2.5">
					<div class="flex items-center gap-1.5 text-[11px] font-semibold text-zinc-700 dark:text-zinc-300 mb-1">
						<HardDrive class="h-3.5 w-3.5 text-emerald-500" />
						<span>Host Filesystem Storage</span>
					</div>
					<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
						Sandboxed in <code>data/extensions/{extension.wasm_hash.substring(0, 8)}.../</code>. Transcript audit logs.
					</p>
				</div>

				<div class="rounded-lg border border-indigo-200/50 dark:border-indigo-900/50 bg-white/60 dark:bg-zinc-900/60 p-2.5">
					<div class="flex items-center gap-1.5 text-[11px] font-semibold text-zinc-700 dark:text-zinc-300 mb-1">
						<Cpu class="h-3.5 w-3.5 text-amber-500" />
						<span>Data Scope & Permissions</span>
					</div>
					<p class="text-[11px] text-zinc-500 dark:text-zinc-400 font-mono text-[10px]">
						{JSON.stringify(extension.manifest.data_access || ['agent.id', 'agent.name'])}
					</p>
				</div>
			</div>
		</div>
	{/if}

	<!-- Messages Scroll Area -->
	<div
		bind:this={scrollContainer}
		id="extension-chat-messages"
		class="flex-1 overflow-y-auto px-6 py-6 space-y-4"
	>
		{#if messages.length === 0}
			<!-- Empty State -->
			<div class="flex h-full flex-col items-center justify-center text-center max-w-md mx-auto py-12">
				<div class="flex h-12 w-12 items-center justify-center rounded-2xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 mb-3 border border-indigo-500/20">
					<Bot class="h-6 w-6" />
				</div>
				<h3 class="text-sm font-semibold text-zinc-800 dark:text-zinc-200">
					Interactive Chat with {agent.name}
				</h3>
				<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-1">
					Send a prompt to execute actions inside the WASM extension runtime. All turns and state are securely preserved in host-managed storage.
				</p>
			</div>
		{:else}
			{#each messages as msg (msg.id)}
				{#if msg.text && msg.text.trim().length > 0}
					<div class="flex flex-col {msg.role === 'user' ? 'items-end' : 'items-start'}">
						<div class="flex items-center gap-1.5 mb-1 text-[10px] text-zinc-400 px-1">
							{#if msg.role === 'user'}
								<span class="font-medium text-zinc-600 dark:text-zinc-400">You</span>
							{:else}
								<Bot class="h-3 w-3 text-indigo-500" />
								<span class="font-medium text-zinc-600 dark:text-zinc-400">{agent.name}</span>
							{/if}
							<span>•</span>
							<span class="font-mono">{msg.timestamp}</span>
						</div>

						<div
							class="max-w-[80%] rounded-2xl px-4 py-2.5 text-xs leading-relaxed shadow-xs {msg.role === 'user'
								? 'bg-zinc-900 text-white dark:bg-zinc-100 dark:text-zinc-950 rounded-tr-xs'
								: 'border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/90 text-zinc-800 dark:text-zinc-200 rounded-tl-xs whitespace-pre-wrap'}"
						>
							{msg.text}
						</div>
					</div>
				{/if}
			{/each}
		{/if}
	</div>

	<!-- Floating Prompt Input Bar -->
	<div class="p-4 border-t border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/50 backdrop-blur-md">
		<div class="max-w-3xl mx-auto flex items-center gap-2 rounded-full border border-zinc-200 dark:border-zinc-800 bg-white/90 dark:bg-zinc-900/90 backdrop-blur-xl p-1.5 pl-4 shadow-lg focus-within:ring-2 focus-within:ring-zinc-400 dark:focus-within:ring-zinc-600 transition-all">
			<input
				type="text"
				bind:value={userInput}
				placeholder="Message {agent.name} via {extension.name}..."
				class="flex-1 bg-transparent px-2 text-xs text-zinc-900 dark:text-white placeholder-zinc-400 focus:outline-none"
				onkeydown={(e) => e.key === 'Enter' && handleSend()}
			/>
			<button
				type="button"
				class="flex h-8 w-8 items-center justify-center rounded-full bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 text-white dark:text-zinc-950 disabled:opacity-30 shadow-xs transition-all active:scale-95"
				onclick={handleSend}
				disabled={!userInput.trim()}
				aria-label="Send message"
			>
				<CornerDownLeft class="h-4 w-4" />
			</button>
		</div>
	</div>
</div>
