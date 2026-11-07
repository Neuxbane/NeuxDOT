<script lang="ts">
import { onMount } from 'svelte';
import { api, type InstructionPreset } from '$lib/api';
import { authState } from '$lib/stores/auth.svelte';
import {
BookOpen,
Plus,
Pencil,
Trash2,
Save,
X,
Loader2,
AlertCircle
} from 'lucide-svelte';

let presets = $state<InstructionPreset[]>([]);
let isLoading = $state(true);
let error = $state<string | null>(null);
let info = $state<string | null>(null);

// Editor state (create when editingId is null)
let isEditorOpen = $state(false);
let editingId = $state<string | null>(null);
let formName = $state('');
let formDescription = $state('');
let formContent = $state('');
let isSaving = $state(false);

let deletingId = $state<string | null>(null);

// Refresh when the active account changes: presets are per-account, so a
// stale list would leak the previous account's instructions into the UI.
const accountScope = $derived(
authState.isAuthenticated
? `${authState.activeAccount?.serverUrl ?? ''}|${authState.activeAccount?.id ?? ''}`
: ''
);

let requestId = 0;

async function loadPresets() {
const mine = ++requestId;
const scope = accountScope;
isLoading = true;
error = null;
try {
const list = await api.instructionPresets.list();
if (mine !== requestId || scope !== accountScope) return;
presets = list;
} catch (e: any) {
if (mine !== requestId) return;
error = e?.message || 'Failed to load instruction presets.';
presets = [];
} finally {
if (mine === requestId) isLoading = false;
}
}

$effect(() => {
if (!accountScope) {
presets = [];
isLoading = false;
return;
}
loadPresets();
});

function openCreate() {
editingId = null;
formName = '';
formDescription = '';
formContent = '';
error = null;
isEditorOpen = true;
}

function openEdit(preset: InstructionPreset) {
editingId = preset.id;
formName = preset.name;
formDescription = preset.description ?? '';
formContent = preset.content;
error = null;
isEditorOpen = true;
}

function closeEditor() {
if (isSaving) return;
isEditorOpen = false;
editingId = null;
}

async function save() {
error = null;
if (!formName.trim()) {
error = 'Name is required.';
return;
}
if (!formContent.trim()) {
error = 'Instructions cannot be empty.';
return;
}

isSaving = true;
try {
if (editingId) {
const updated = await api.instructionPresets.update(editingId, {
name: formName.trim(),
description: formDescription.trim(),
content: formContent
});
presets = presets.map((p) => (p.id === updated.id ? updated : p));
info = 'Preset updated.';
} else {
const created = await api.instructionPresets.create({
name: formName.trim(),
description: formDescription.trim(),
content: formContent
});
presets = [...presets, created];
info = 'Preset created.';
}
isEditorOpen = false;
editingId = null;
} catch (e: any) {
error = e?.message || 'Failed to save the preset.';
} finally {
isSaving = false;
}
}

async function remove(preset: InstructionPreset) {
if (!confirm(`Delete the preset "${preset.name}"?\n\nAgents using it keep their current instructions; they just stop being linked to this preset.`))
return;
deletingId = preset.id;
error = null;
try {
await api.instructionPresets.delete(preset.id);
presets = presets.filter((p) => p.id !== preset.id);
} catch (e: any) {
error = e?.message || 'Failed to delete the preset.';
} finally {
deletingId = null;
}
}
</script>

<div id="settings-instructions" class="space-y-10">
	<section id="agent-instructions" class="space-y-6">
		<div class="flex items-center justify-between gap-3">
			<div>
				<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Agent Instructions</h2>
				<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
					Reusable system prompt presets for this account. Presets are private to you and can be
					selected on any agent under its System Prompt.
				</p>
			</div>
			<div class="flex items-center gap-2">
				<button
					type="button"
					onclick={openCreate}
					class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98 cursor-pointer"
					>
					<Plus class="h-3.5 w-3.5" />
					<span>New Preset</span>
				</button>
			</div>
		</div>

		<div class="space-y-4">
			{#if error}
			<div class="flex items-start gap-2 rounded-xl border border-rose-500/20 bg-rose-500/10 px-3 py-2 text-[11px] text-rose-600 dark:text-rose-400">
				<AlertCircle class="h-3.5 w-3.5 shrink-0 mt-0.5" />
				<span>{error}</span>
			</div>
			{/if}
			{#if info}
			<div class="rounded-xl border border-emerald-500/20 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-600 dark:text-emerald-400">
				{info}
			</div>
			{/if}

			{#if isEditorOpen}
			<section class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 space-y-3 shadow-xs">
				<div class="flex items-center justify-between">
					<h2 class="text-[11px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
						{editingId ? 'Edit preset' : 'New preset'}
					</h2>
					<button type="button" onclick={closeEditor} class="rounded-lg p-1 text-zinc-400 hover:text-zinc-900 dark:hover:text-white cursor-pointer" aria-label="Close editor">
						<X class="h-4 w-4" />
					</button>
				</div>

				<div>
					<label for="preset-name" class="block text-[11px] font-medium text-zinc-500 dark:text-zinc-400 mb-1">Name</label>
					<input
						id="preset-name"
						type="text"
						bind:value={formName}
						placeholder="e.g. Senior Rust Engineer"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:outline-none"
						/>
					</div>

					<div>
						<label for="preset-description" class="block text-[11px] font-medium text-zinc-500 dark:text-zinc-400 mb-1">Description <span class="text-zinc-400">(optional)</span></label>
						<input
							id="preset-description"
							type="text"
							bind:value={formDescription}
							placeholder="What is this preset for?"
							class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:outline-none"
							/>
						</div>

						<div>
							<label for="preset-content" class="block text-[11px] font-medium text-zinc-500 dark:text-zinc-400 mb-1">Instructions</label>
							<textarea
								id="preset-content"
								rows="10"
								bind:value={formContent}
								placeholder={'You are {{agent.name}}.\nRules:\n- ...'}
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:outline-none"
								></textarea>
								<p class="mt-1 text-[10px] text-zinc-400 dark:text-zinc-500">
									Supports the same variables as a custom prompt: {'{{agent.name}}'}, {'{{agent.memory}}'}, {'{{tools.preset}}'}.
								</p>
							</div>

							<div class="flex items-center justify-end gap-2">
								<button type="button" onclick={closeEditor} disabled={isSaving} class="rounded-xl px-3 py-1.5 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 cursor-pointer">
									Cancel
								</button>
								<button
									type="button"
									onclick={save}
									disabled={isSaving}
									class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 disabled:opacity-50 cursor-pointer"
									>
									{#if isSaving}
									<Loader2 class="h-3.5 w-3.5 animate-spin" />
									{:else}
									<Save class="h-3.5 w-3.5" />
									{/if}
									<span>{editingId ? 'Save changes' : 'Create preset'}</span>
								</button>
							</div>
						</section>
						{/if}

						{#if isLoading}
						<div class="flex items-center justify-center gap-2 rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-10 text-xs text-zinc-400">
							<Loader2 class="h-4 w-4 animate-spin" /> Loading presets…
							</div>
							{:else if presets.length === 0 && !isEditorOpen}
							<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-10 text-center">
								<BookOpen class="mx-auto mb-2 h-6 w-6 text-zinc-300 dark:text-zinc-700" />
								<p class="text-xs text-zinc-500 dark:text-zinc-400">
									No instruction presets yet. Create one to reuse the same system prompt across agents.
								</p>
							</div>
							{:else}
							<div class="space-y-2">
								{#each presets as preset (preset.id)}
								<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 p-4">
									<div class="flex items-start justify-between gap-3">
										<div class="min-w-0">
											<div class="flex items-center gap-2">
												<BookOpen class="h-3.5 w-3.5 shrink-0 text-violet-400 dark:text-violet-500" />
												<h3 class="truncate text-xs font-semibold text-zinc-900 dark:text-white">{preset.name}</h3>
											</div>
											{#if preset.description}
											<p class="mt-1 text-[11px] text-zinc-500 dark:text-zinc-400">{preset.description}</p>
											{/if}
											<pre class="mt-2 max-h-24 overflow-hidden whitespace-pre-wrap break-words rounded-lg bg-zinc-50 dark:bg-zinc-950 px-2.5 py-2 font-mono text-[10px] leading-relaxed text-zinc-500 dark:text-zinc-400">{preset.content}</pre>
										</div>
										<div class="flex shrink-0 items-center gap-1">
											<button type="button" onclick={() => openEdit(preset)} title="Edit preset" aria-label="Edit preset" class="rounded-lg p-1.5 text-zinc-400 hover:text-zinc-900 dark:hover:text-white hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors cursor-pointer">
												<Pencil class="h-3.5 w-3.5" />
											</button>
											<button type="button" onclick={() => remove(preset)} disabled={deletingId === preset.id} title="Delete preset" aria-label="Delete preset" class="rounded-lg p-1.5 text-zinc-400 hover:text-rose-500 hover:bg-rose-500/10 transition-colors cursor-pointer disabled:opacity-50">
												{#if deletingId === preset.id}
												<Loader2 class="h-3.5 w-3.5 animate-spin" />
												{:else}
												<Trash2 class="h-3.5 w-3.5" />
												{/if}
											</button>
										</div>
									</div>
								</div>
								{/each}
							</div>
							{/if}
						</div>
					</section>
				</div>
