<script lang="ts">
	import type { MemoryGraphNode } from '$lib/api';

	let { nodes }: { nodes: MemoryGraphNode[] } = $props();

	const WIDTH = 640;
	const HEIGHT = 340;
	const PAD = 26;
	/// Above this the spring layout stops being readable, so the graph is a sample
	/// rather than a hairball. The header states the real totals.
	const MAX_RENDER = 120;

	interface Point {
		x: number;
		y: number;
	}

	interface Rendered {
		key: string;
		node: MemoryGraphNode | undefined;
		degree: number;
		point: Point;
	}

	const TYPE_COLOURS: Record<string, string> = {
		skill: '#10b981',
		entity: '#0ea5e9',
		person: '#f59e0b',
		project: '#8b5cf6',
		note: '#ec4899',
		preference: '#14b8a6'
	};
	const FALLBACK_COLOUR = '#a1a1aa';

	function colourFor(type: string | undefined): string {
		if (!type) return FALLBACK_COLOUR;
		return TYPE_COLOURS[type] ?? FALLBACK_COLOUR;
	}

	/// Fruchterman-Reingold on the unit square. Deterministic: the initial
	/// placement is a phyllotaxis disc, so the same graph always renders the same
	/// way and there is no simulation state to leak between renders.
	function layout(keys: string[], edges: Array<[string, string]>): Map<string, Point> {
		const n = keys.length;
		const points = new Map<string, Point>();
		const centre = 0.5;

		if (n === 0) return points;
		if (n === 1) {
			points.set(keys[0], { x: centre, y: centre });
			return points;
		}

		const golden = Math.PI * (3 - Math.sqrt(5));
		keys.forEach((key, i) => {
			const radius = Math.sqrt((i + 0.5) / n) * 0.42;
			points.set(key, {
				x: centre + radius * Math.cos(i * golden),
				y: centre + radius * Math.sin(i * golden)
			});
		});

		const index = new Map(keys.map((key, i) => [key, i]));
		const springs: Array<[number, number]> = [];
		for (const [from, to] of edges) {
			const a = index.get(from);
			const b = index.get(to);
			if (a === undefined || b === undefined) continue;
			springs.push([a, b]);
		}

		const xs = new Float64Array(n);
		const ys = new Float64Array(n);
		const sync = () => {
			keys.forEach((key, i) => {
				const p = points.get(key);
				xs[i] = p ? p.x : centre;
				ys[i] = p ? p.y : centre;
			});
		};
		const flush = () => {
			keys.forEach((key, i) => {
				points.set(key, { x: xs[i], y: ys[i] });
			});
		};

		const k = Math.sqrt(1 / n);
		const iterations = 110;
		for (let step = 0; step < iterations; step++) {
			sync();
			const dx = new Float64Array(n);
			const dy = new Float64Array(n);

			for (let i = 0; i < n; i++) {
				for (let j = i + 1; j < n; j++) {
					let vx = xs[i] - xs[j];
					let vy = ys[i] - ys[j];
					let dist = Math.hypot(vx, vy);
					if (dist < 1e-4) {
						// Coincident points have no direction; nudge along a fixed
						// pattern so the result stays deterministic.
						vx = 1e-3 * (((i * 31 + j * 17) % 7) - 3);
						vy = 1e-3 * (((i * 13 + j * 29) % 5) - 2);
						dist = Math.hypot(vx, vy) || 1e-3;
					}
					const force = (k * k) / dist;
					const ux = (vx / dist) * force;
					const uy = (vy / dist) * force;
					dx[i] += ux;
					dy[i] += uy;
					dx[j] -= ux;
					dy[j] -= uy;
				}
			}

			for (const [a, b] of springs) {
				let vx = xs[a] - xs[b];
				let vy = ys[a] - ys[b];
				const dist = Math.hypot(vx, vy) || 1e-3;
				const force = (dist * dist) / k;
				const ux = (vx / dist) * force;
				const uy = (vy / dist) * force;
				dx[a] -= ux;
				dy[a] -= uy;
				dx[b] += ux;
				dy[b] += uy;
			}

			// Gravity keeps disconnected fragments from drifting to the rim.
			const temperature = 0.09 * (1 - step / iterations) + 0.004;
			for (let i = 0; i < n; i++) {
				dx[i] += (centre - xs[i]) * 0.6;
				dy[i] += (centre - ys[i]) * 0.6;
				const magnitude = Math.hypot(dx[i], dy[i]) || 1;
				const limit = Math.min(magnitude, temperature);
				xs[i] = Math.min(1, Math.max(0, xs[i] + (dx[i] / magnitude) * limit));
				ys[i] = Math.min(1, Math.max(0, ys[i] + (dy[i] / magnitude) * limit));
			}
			flush();
		}

		return points;
	}

	const byKey = $derived(new Map(nodes.map((node) => [node.key, node])));

	/// Every key that appears, including link targets the model has referenced but
	/// not written yet. Dangling targets are shown hollow rather than hidden: they
	/// are the TODO list the prompt tells the model to follow.
	const keys = $derived.by(() => {
		const seen = new Set<string>();
		for (const node of nodes) {
			seen.add(node.key);
			for (const target of node.outbound) seen.add(target);
		}
		return [...seen].sort();
	});

	const edges = $derived.by(() => {
		const seen = new Set<string>();
		const list: Array<[string, string]> = [];
		for (const node of nodes) {
			for (const target of node.outbound) {
				const id = `${node.key}\u0000${target}`;
				if (seen.has(id)) continue;
				seen.add(id);
				list.push([node.key, target]);
			}
		}
		return list;
	});

	const danglingCount = $derived(keys.filter((key) => !byKey.has(key)).length);
	const rendered = $derived(keys.slice(0, MAX_RENDER));

	const points = $derived(
		layout(
			rendered,
			edges.filter(([from, to]) => rendered.includes(from) && rendered.includes(to))
		)
	);

	const drawn = $derived.by(() => {
		const degree = new Map<string, number>();
		for (const [from, to] of edges) {
			if (!rendered.includes(from) || !rendered.includes(to)) continue;
			degree.set(from, (degree.get(from) ?? 0) + 1);
			degree.set(to, (degree.get(to) ?? 0) + 1);
		}
		const list: Rendered[] = [];
		for (const key of rendered) {
			const point = points.get(key);
			if (!point) continue;
			list.push({ key, node: byKey.get(key), degree: degree.get(key) ?? 0, point });
		}
		return list;
	});

	const drawnEdges = $derived.by(() =>
		edges
			.filter(([from, to]) => points.has(from) && points.has(to))
			.map(([from, to]) => {
				const a = points.get(from)!;
				const b = points.get(to)!;
				return { from, to, a, b };
			})
	);

	let selected = $state<string | null>(null);
	const selectedNode = $derived(selected ? (byKey.get(selected) ?? null) : null);
	const selectedKnown = $derived(selected !== null && byKey.has(selected));

	function neighboursOf(key: string): Set<string> {
		const set = new Set<string>([key]);
		for (const [from, to] of edges) {
			if (from === key) set.add(to);
			if (to === key) set.add(from);
		}
		return set;
	}

	const highlighted = $derived(selected ? neighboursOf(selected) : null);

	function isDimmed(key: string): boolean {
		return highlighted !== null && !highlighted.has(key);
	}

	function px(point: Point): number {
		return PAD + point.x * (WIDTH - PAD * 2);
	}
	function py(point: Point): number {
		return PAD + point.y * (HEIGHT - PAD * 2);
	}
	/// Dense graphs would otherwise render as one merged blob, so the markers
	/// shrink as the node count grows.
	const nodeScale = $derived(Math.min(1, Math.sqrt(60 / Math.max(rendered.length, 1))));

	function radius(degree: number): number {
		return (4 + Math.min(degree, 8) * 0.8) * nodeScale;
	}
</script>

<div class="rounded-xl border border-zinc-200 bg-white p-3 dark:border-zinc-800 dark:bg-zinc-950">
	<div class="mb-2 flex flex-wrap items-center justify-between gap-2">
		<div class="flex items-center gap-2 text-[11px] font-medium text-zinc-600 dark:text-zinc-400">
			<span>Wiki-link graph</span>
			<span class="font-mono text-[10px] text-zinc-400">
				{nodes.length} node{nodes.length === 1 ? '' : 's'} · {edges.length} link{edges.length === 1
					? ''
					: 's'}
			</span>
			{#if danglingCount > 0}
				<span
					class="rounded-full border border-amber-500/30 bg-amber-500/10 px-1.5 py-0.5 font-mono text-[10px] text-amber-700 dark:text-amber-300"
					title="Link targets the model referenced but has not written yet"
				>
					{danglingCount} dangling
				</span>
			{/if}
		</div>
		{#if keys.length > MAX_RENDER}
			<span class="text-[10px] text-zinc-400">showing {MAX_RENDER} of {keys.length}</span>
		{/if}
	</div>

	{#if keys.length === 0}
		<p class="py-6 text-center text-[11px] italic text-zinc-400">
			No memory nodes yet. The agent writes them with the <code>memoryWrite</code> tool.
		</p>
	{:else}
		<svg
			viewBox="0 0 {WIDTH} {HEIGHT}"
			class="w-full select-none"
			role="img"
			aria-label="Memory graph: {nodes.length} nodes, {edges.length} links"
		>
			<g>
				{#each drawnEdges as edge (edge.from + '\u0000' + edge.to)}
					<line
						x1={px(edge.a)}
						y1={py(edge.a)}
						x2={px(edge.b)}
						y2={py(edge.b)}
						stroke={selected && (edge.from === selected || edge.to === selected)
							? '#f472b6'
							: 'currentColor'}
						stroke-width={selected && (edge.from === selected || edge.to === selected) ? 1.6 : 0.7}
						stroke-opacity={selected
							? edge.from === selected || edge.to === selected
								? 0.9
								: 0.08
							: 0.18}
						class="text-zinc-500"
					/>
				{/each}
			</g>
			<g>
				{#each drawn as item (item.key)}
					{@const known = item.node !== undefined}
					<circle
						cx={px(item.point)}
						cy={py(item.point)}
						r={radius(item.degree)}
						fill={known ? colourFor(item.node?.type) : 'none'}
						stroke={known ? 'none' : '#71717a'}
						stroke-width={known ? 0 : 1.2}
						stroke-dasharray={known ? '' : '2 2'}
						opacity={isDimmed(item.key) ? 0.22 : 1}
						class="cursor-pointer transition-opacity"
						role="button"
						tabindex="-1"
						aria-label={known
							? `${item.key}, ${item.node?.type}, ${item.degree} links`
							: `${item.key}, dangling`}
						onclick={() => (selected = selected === item.key ? null : item.key)}
						onkeydown={(event) => {
							if (event.key === 'Enter' || event.key === ' ') {
								event.preventDefault();
								selected = selected === item.key ? null : item.key;
							}
						}}
					>
						<title>{item.key}{known ? ` (${item.node?.type})` : ' — dangling'}</title>
					</circle>
				{/each}
			</g>
		</svg>

		<div class="mt-2 flex flex-wrap items-center gap-2">
			<label
				for="memory-graph-select"
				class="text-[11px] font-medium text-zinc-600 dark:text-zinc-400">Inspect</label
			>
			<select
				id="memory-graph-select"
				class="rounded-lg border border-zinc-200 bg-white px-2 py-1 text-[11px] text-zinc-800 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-200"
				value={selected ?? ''}
				onchange={(event) => (selected = event.currentTarget.value || null)}
			>
				<option value="">—</option>
				{#each rendered as key (key)}
					<option value={key}>{key}{byKey.has(key) ? '' : ' (dangling)'}</option>
				{/each}
			</select>
			{#if selected}
				<span
					class="rounded-full px-1.5 py-0.5 text-[10px] font-medium"
					style="background-color: {colourFor(selectedNode?.type)}22; color: {colourFor(
						selectedNode?.type
					)}"
				>
					{selectedKnown ? (selectedNode?.type ?? 'note') : 'dangling'}
				</span>
			{/if}
		</div>

		{#if selected && !selectedKnown}
			<div
				class="mt-2 rounded-lg border border-dashed border-amber-500/40 bg-amber-500/5 p-2 text-[11px] text-amber-800 dark:text-amber-200"
			>
				<code
					class="font-mono text-[11px] font-semibold text-amber-900 dark:text-amber-100"
					>{selected}</code
				>
				is referenced by
				{#each nodes.filter((node) => node.outbound.includes(selected!)) as source (source.key)}
					<code class="font-mono">{source.key}</code>
				{/each}
				but has no node yet. Write it with <code>memoryWrite</code>.
			</div>
		{:else if selectedNode}
			<div class="mt-2 space-y-1.5 rounded-lg bg-zinc-50 p-2 dark:bg-zinc-900">
				<div class="flex items-center justify-between gap-2">
					<code class="font-mono text-[11px] font-semibold text-zinc-900 dark:text-zinc-100"
						>{selectedNode.key}</code
					>
					{#if selectedNode.updated_at}
						<span class="text-[10px] text-zinc-400"
							>{new Date(selectedNode.updated_at).toLocaleString()}</span
						>
					{/if}
				</div>
				{#if selectedNode.description}
					<p class="text-[11px] text-zinc-600 dark:text-zinc-400">{selectedNode.description}</p>
				{/if}
				<pre
					class="max-h-32 overflow-y-auto whitespace-pre-wrap break-words font-mono text-[11px] text-zinc-700 dark:text-zinc-300">{selectedNode.value}</pre
				>
				<div class="flex flex-wrap gap-3 text-[10px] text-zinc-500 dark:text-zinc-400">
					{#if selectedNode.outbound.length > 0}
						<span>
							links to
							{#each selectedNode.outbound as target (target)}
								<button
									type="button"
									class="font-mono text-pink-600 hover:underline dark:text-pink-400"
									onclick={() => (selected = target)}>{target}</button
								>
							{/each}
						</span>
					{/if}
					{#if selectedNode.backlinks.length > 0}
						<span>
							linked from
							{#each selectedNode.backlinks as source (source)}
								<button
									type="button"
									class="font-mono text-sky-600 hover:underline dark:text-sky-400"
									onclick={() => (selected = source)}>{source}</button
								>
							{/each}
						</span>
					{/if}
				</div>
			</div>
		{/if}
	{/if}
</div>
