<script lang="ts">
	import { ArrowLeft, ShieldCheck, Zap, Lock, Code2 } from 'lucide-svelte';
</script>

<div id="docs-layout" class="flex h-full flex-col overflow-hidden">
	<header id="docs-header" class="flex items-center gap-3 border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5">
		<a href="/" class="text-zinc-400 hover:text-zinc-800 dark:hover:text-white transition-colors" aria-label="Back to Agent Playground">
			<ArrowLeft class="h-4 w-4" />
		</a>
		<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">Documentation: System Instruction & Prompt Presets</h1>
	</header>

	<main id="docs-content" class="flex-1 overflow-y-auto p-8 max-w-3xl mx-auto space-y-6 text-zinc-600 dark:text-zinc-300 text-xs leading-relaxed">
		<section id="section-safety" class="rounded-2xl border border-indigo-500/20 bg-indigo-500/5 p-5">
			<div class="flex items-center gap-2 font-semibold text-indigo-700 dark:text-indigo-300 text-sm mb-1">
				<ShieldCheck class="h-5 w-5" />
				Strict-Mode Handlebars Templating
			</div>
			<p class="text-zinc-500 dark:text-zinc-400">
				NeuxDOT uses Rust <code>handlebars</code> with strict validation to render system instructions. Presets are evaluated securely at execution time, allowing dynamic agent configuration without arbitrary code execution.
			</p>
		</section>

		<section id="section-variables">
			<h2 class="text-sm font-semibold text-zinc-900 dark:text-white mb-2">Structured Variables & Presets</h2>
			<div class="space-y-3">
				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 font-mono text-xs shadow-xs">
					<div class="text-indigo-600 dark:text-indigo-400 font-semibold mb-1">{'{{agent.name}}'}</div>
					<div class="text-zinc-500 dark:text-zinc-400 font-sans">
						Replaced with the exact name of the current agent (e.g. <code>Nexura</code>).
					</div>
				</div>

				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 font-mono text-xs shadow-xs">
					<div class="text-indigo-600 dark:text-indigo-400 font-semibold mb-1">{'{{agent.memory}}'} or {'{{agent.memory.content}}'}</div>
					<div class="text-zinc-500 dark:text-zinc-400 font-sans">
						Outputs the agent's persistent memory content. Empty if no memory has been stored yet.
					</div>
				</div>

				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 font-mono text-xs shadow-xs">
					<div class="text-indigo-600 dark:text-indigo-400 font-semibold mb-1">{'{{agent.memory.hash}}'}</div>
					<div class="text-zinc-500 dark:text-zinc-400 font-sans">
						The SHA-256 integrity hash of the current memory. Useful for optimistic locking with <code>setMemory</code> and <code>editMemory</code>.
					</div>
				</div>

				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 font-mono text-xs shadow-xs">
					<div class="text-indigo-600 dark:text-indigo-400 font-semibold mb-1">{'{{tools.preset}}'}</div>
					<div class="text-zinc-500 dark:text-zinc-400 font-sans">
						Outputs only the key names of all tools currently allowed for the agent (e.g. <code>chat_send, search_repo</code>). The agent fetches full schemas on-demand using <code>get_tool_schema(name)</code>.
					</div>
				</div>

				<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 font-mono text-xs shadow-xs">
					<div class="text-indigo-600 dark:text-indigo-400 font-semibold mb-1">{'{{time}}'} & {'{{context_remaining}}'}</div>
					<div class="text-zinc-500 dark:text-zinc-400 font-sans">
						Evaluates the current local time (e.g. <code>2:30 PM</code>) and remaining character buffer before <code>max_running_history_chars</code> is reached. Also available inside Response Hooks.
					</div>
				</div>
			</div>
		</section>

		<section id="section-example">
			<h2 class="text-sm font-semibold text-zinc-900 dark:text-white mb-2">Example System Prompt</h2>
			<pre class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-950 p-4 font-mono text-zinc-800 dark:text-zinc-200 shadow-xs">
You are {'{{agent.name}}'}, an autonomous AI developer.
Allowed tools: {'{{tools.preset}}'}.
Memory (Hash: {'{{agent.memory.hash}}'}):
{'{{agent.memory}}'}

Inspect tool schemas on-demand via get_tool_schema(name).</pre>
		</section>

		<section id="section-cache-guarantee">
			<h2 class="text-sm font-semibold text-zinc-900 dark:text-white mb-2">On-Demand Schemas & Cache Stability Guarantee</h2>
			<p class="text-zinc-500 dark:text-zinc-400 leading-relaxed">
				By providing only tool key names via <code>{'{{tools-preset}}'}</code> instead of inlining massive JSON schemas into the prompt, system instructions remain ultra-compact. Combined with the invariant 9 meta-tools (<code>search_tools</code>, <code>get_tool_schema</code>, <code>call_tool</code>, <code>list_servers</code>, <code>setMemory</code>, <code>editMemory</code>, <code>readMemory</code>, <code>compact</code>, <code>sleep</code>), prompt cache hit rates reach 100%, dramatically reducing latency and token costs.
			</p>
		</section>
	</main>
</div>
