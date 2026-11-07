<script lang="ts">
	import { eventStore } from '$lib/stores/events.svelte';
	import { Terminal, Trash2, Wifi, WifiOff } from 'lucide-svelte';

	let terminalContainer: HTMLDivElement | null = $state(null);

	$effect(() => {
		// Auto scroll on new logs
		if (eventStore.logs.length && terminalContainer) {
			terminalContainer.scrollTop = terminalContainer.scrollHeight;
		}
	});

	function getBadge(type: string, isMcpPush?: boolean) {
		if (isMcpPush) {
			return {
				label: 'MCP PUSH',
				class: 'bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/20'
			};
		}
		switch (type) {
			case 'provider':
				return { label: 'ROUTER', class: 'bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/20' };
			case 'tool_search':
				return { label: 'SEARCH', class: 'bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20' };
			case 'tool_call':
				return { label: 'EXEC', class: 'bg-cyan-500/10 text-cyan-600 dark:text-cyan-400 border-cyan-500/20' };
			case 'tool_result':
				return { label: 'RESULT', class: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20' };
			case 'memory':
				return { label: 'MEMORY', class: 'bg-pink-500/10 text-pink-600 dark:text-pink-400 border-pink-500/20' };
			case 'compact':
				return { label: 'COMPACT', class: 'bg-violet-500/10 text-violet-600 dark:text-violet-400 border-violet-500/20' };
			case 'sleep':
				return { label: 'SLEEP', class: 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border-indigo-500/20' };
			case 'wake':
				return { label: 'WAKE', class: 'bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20' };
			case 'text_disallowed':
				return { label: 'FLOW CONTROL', class: 'bg-rose-500/10 text-rose-600 dark:text-rose-400 border-rose-500/20' };
			case 'error':
				return { label: 'ERROR', class: 'bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20' };
			default:
				return { label: 'EVENT', class: 'bg-zinc-200 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 border-zinc-300 dark:border-zinc-700' };
		}
	}
</script>

<div class="flex h-72 flex-col rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-950 font-mono text-xs shadow-xs transition-colors">
	<!-- Terminal Header -->
	<div class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 px-4 py-2.5 bg-zinc-50/70 dark:bg-zinc-900/40 rounded-t-2xl">
		<div class="flex items-center gap-2">
			<Terminal class="h-3.5 w-3.5 text-zinc-400" />
			<span class="font-medium text-zinc-700 dark:text-zinc-300 text-xs">Live Runtime Terminal</span>
			<span class="flex items-center gap-1 text-[11px] {eventStore.isConnected ? 'text-emerald-600 dark:text-emerald-400' : 'text-zinc-400'}">
				{#if eventStore.isConnected}
					<Wifi class="h-3 w-3" /> Live
				{:else}
					<WifiOff class="h-3 w-3" /> Offline
				{/if}
			</span>
		</div>

		<div class="flex items-center gap-2">
			<button
				type="button"
				class="flex items-center gap-1 rounded-lg px-2 py-1 text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800 hover:text-zinc-700 dark:hover:text-zinc-200 transition-colors"
				onclick={() => eventStore.clear()}
				title="Clear logs"
			>
				<Trash2 class="h-3.5 w-3.5" />
				<span class="text-[11px]">Clear</span>
			</button>
		</div>
	</div>

	<!-- Terminal Body -->
	<div
		bind:this={terminalContainer}
		class="flex-1 overflow-y-auto p-3 space-y-2 select-text"
	>
		{#if eventStore.logs.length === 0}
			<div class="flex h-full items-center justify-center text-zinc-400 dark:text-zinc-600 italic text-[11px]">
				No events recorded. Perform an agent action or wait for server events.
			</div>
		{:else}
			{#each eventStore.logs as log (log.id)}
				{@const badge = getBadge(log.type, log.isMcpPush)}
				<div class="group rounded-xl border p-2.5 text-[11px] transition-colors {log.isMcpPush ? 'border-amber-500/30 bg-amber-500/5' : log.isError ? 'border-red-500/30 bg-red-500/5' : 'border-zinc-200/60 dark:border-zinc-800/80 bg-zinc-50/50 dark:bg-zinc-900/40 hover:bg-zinc-100/50 dark:hover:bg-zinc-900/80'}">
					<div class="flex items-start justify-between gap-2">
						<div class="flex items-center gap-2 flex-wrap">
							<span class="text-zinc-400 dark:text-zinc-500 text-[10px]">{log.timestamp}</span>
							<span class="rounded border px-1.5 py-0.5 text-[9px] font-bold uppercase tracking-wider {badge.class}">
								{badge.label}
							</span>
							<span class="font-medium {log.isMcpPush ? 'text-amber-700 dark:text-amber-300 font-semibold' : log.isError ? 'text-red-600 dark:text-red-400' : 'text-zinc-800 dark:text-zinc-200'}">
								{log.title}
							</span>
						</div>
					</div>

					{#if log.details}
						<div class="mt-1 text-zinc-500 dark:text-zinc-400 font-sans text-xs whitespace-pre-wrap">
							{log.details}
						</div>
					{/if}
				</div>
			{/each}
		{/if}
	</div>
</div>
