<script lang="ts">
import { api } from '$lib/api';
import { authState } from '$lib/stores/auth.svelte';
import Avatar from '$lib/components/Avatar.svelte';
	import AvatarCropModal from '$lib/components/AvatarCropModal.svelte';
import { Check, Upload, Trash2, Link2, Sparkles } from 'lucide-svelte';

const AVATAR_COLORS = [
'#3b82f6',
'#6366f1',
'#8b5cf6',
'#ec4899',
'#ef4444',
'#f59e0b',
'#10b981',
'#14b8a6',
'#0ea5e9',
'#64748b'
];

const AVATAR_SHAPES = [
{ id: 'circle', label: 'Circle' },
{ id: 'rounded', label: 'Rounded' },
{ id: 'square', label: 'Square' }
] as const;

// Source file ceiling; the cropper downscales + compresses well below this.
const MAX_AVATAR_SOURCE_BYTES = 8 * 1024 * 1024;
// Encoded size ceiling for the stored avatar.
const AVATAR_OUTPUT_BYTES = 64 * 1024;

// Profile form state (seeded from the loaded profile, then kept in sync)
let profileDisplayName = $state(authState.profile?.display_name || '');
let profileAvatarColor = $state(authState.profile?.avatar_color || '#3b82f6');
let profileAvatarShape = $state(authState.profile?.avatar_shape || 'circle');
let profileAvatarUrl = $state(authState.profile?.avatar_url || '');
let urlInput = $state('');
let isSavingProfile = $state(false);
let profileSavedBadge = $state(false);
let errorMessage = $state<string | null>(null);
let fileInputRef: HTMLInputElement | null = null;
// Source file awaiting crop/compress before it becomes the avatar.
let pendingAvatarFile = $state<File | null>(null);

$effect(() => {
if (authState.profile) {
profileDisplayName = authState.profile.display_name || '';
profileAvatarColor = authState.profile.avatar_color || '#3b82f6';
profileAvatarShape = authState.profile.avatar_shape || 'circle';
profileAvatarUrl = authState.profile.avatar_url || '';
urlInput = authState.profile.avatar_url?.startsWith('http') ? authState.profile.avatar_url : '';
}
});

const previewName = $derived(profileDisplayName.trim() || authState.username || 'Account');
const previewShape = $derived(
profileAvatarShape === 'circle' ? 'circle' : profileAvatarShape === 'square' ? 'square' : 'rounded'
);

function handleAvatarFile(event: Event) {
const target = event.target as HTMLInputElement;
const file = target.files?.[0];
if (!file) return;

if (!file.type.startsWith('image/')) {
errorMessage = 'Please choose an image file (PNG, JPEG, WebP, GIF or SVG).';
target.value = '';
return;
}
if (file.size > MAX_AVATAR_SOURCE_BYTES) {
errorMessage = `Image is too large (${Math.round(file.size / 1024)} KB). Maximum is ${MAX_AVATAR_SOURCE_BYTES / 1024} KB.`;
target.value = '';
return;
}

errorMessage = null;
target.value = '';
// Crop + compress before storing.
pendingAvatarFile = file;
}

function applyCroppedAvatar(dataUrl: string) {
pendingAvatarFile = null;
profileAvatarUrl = dataUrl;
urlInput = '';
}

function applyUrlInput() {
const value = urlInput.trim();
if (!value) return;
if (!/^(https?:)?\/\//i.test(value) && !value.startsWith('data:')) {
errorMessage = 'Avatar URL must start with http:// or https://';
return;
}
errorMessage = null;
profileAvatarUrl = value;
}

function clearAvatar() {
profileAvatarUrl = '';
urlInput = '';
errorMessage = null;
}

async function saveProfile() {
isSavingProfile = true;
errorMessage = null;
try {
const updated = await api.profile.update({
display_name: profileDisplayName.trim() || undefined,
avatar_color: profileAvatarColor,
avatar_shape: profileAvatarShape,
// Empty string clears the stored picture on the backend.
avatar_url: profileAvatarUrl.trim()
});
authState.applyProfile(updated);
profileSavedBadge = true;
setTimeout(() => (profileSavedBadge = false), 2000);
} catch (e: any) {
errorMessage = 'Failed to update profile: ' + (e?.message || e);
} finally {
isSavingProfile = false;
}
}
</script>

<section id="server-profile" class="space-y-6">
<div>
	{#if pendingAvatarFile}
		<AvatarCropModal
			file={pendingAvatarFile}
			size={256}
			maxBytes={AVATAR_OUTPUT_BYTES}
			onCancel={() => (pendingAvatarFile = null)}
			onApply={applyCroppedAvatar}
		/>
	{/if}
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Server Profile Settings</h2>
<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
Customize the display name and avatar for this account. Stored on the backend server and shown across the app.
</p>
</div>

<div class="grid grid-cols-1 lg:grid-cols-[minmax(0,1fr)_240px] gap-6 items-start">
<!-- Editor -->
<div class="space-y-5 rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
<!-- Display Name -->
<div>
<label for="display-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
Display Name
</label>
<input
id="display-name"
type="text"
bind:value={profileDisplayName}
placeholder={authState.username || 'Your Name or Handle'}
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>

<!-- Profile picture -->
<div class="space-y-2">
<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300">Profile Picture</span>

<div class="grid grid-cols-1 md:grid-cols-2 gap-3">
<div class="rounded-xl border border-dashed border-zinc-300 dark:border-zinc-750 p-3 bg-zinc-50/50 dark:bg-zinc-950/40 space-y-2">
<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
<Upload class="h-4 w-4 text-emerald-500" />
<span>Upload image</span>
</div>
<p class="text-[11px] text-zinc-500 dark:text-zinc-400">
Cropped, downscaled to 256×256 and compressed before storing (≈{Math.round(AVATAR_OUTPUT_BYTES / 1024)} KB).
</p>
<input
type="file"
accept="image/*"
class="hidden"
bind:this={fileInputRef}
onchange={handleAvatarFile}
/>
<button
type="button"
class="w-full flex items-center justify-center gap-1.5 rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all"
onclick={() => fileInputRef?.click()}
>
<Upload class="h-3.5 w-3.5" />
<span>Choose Image</span>
</button>
</div>

<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-50/50 dark:bg-zinc-950/40 space-y-2">
<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
<Link2 class="h-4 w-4 text-cyan-500" />
<span>Use image URL</span>
</div>
<p class="text-[11px] text-zinc-500 dark:text-zinc-400">Reference a remote http(s) image.</p>
<div class="flex gap-2">
<input
type="url"
bind:value={urlInput}
placeholder="https://.../avatar.png"
class="flex-1 rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none"
onkeydown={(e) => e.key === 'Enter' && applyUrlInput()}
/>
<button
type="button"
class="rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-40"
onclick={applyUrlInput}
disabled={!urlInput.trim()}
>
Set
</button>
</div>
</div>
</div>

{#if profileAvatarUrl}
<button
type="button"
class="inline-flex items-center gap-1.5 rounded-lg px-2 py-1 text-[11px] font-medium text-zinc-500 hover:text-rose-500 hover:bg-rose-500/10 transition-colors"
onclick={clearAvatar}
>
<Trash2 class="h-3.5 w-3.5" />
Remove picture (fall back to colored initials)
</button>
{/if}
</div>

<!-- Fallback color -->
<div class="space-y-2">
<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300">
Fallback Avatar Color
</span>
<div class="flex flex-wrap items-center gap-2">
{#each AVATAR_COLORS as color (color)}
<button
type="button"
class="h-7 w-7 rounded-full border-2 transition-all {profileAvatarColor === color
? 'border-zinc-900 dark:border-white scale-110'
: 'border-transparent hover:scale-105'}"
style="background-color: {color};"
onclick={() => (profileAvatarColor = color)}
aria-label="Avatar color {color}"
title={color}
></button>
{/each}
<label class="ml-1 inline-flex items-center gap-1.5 text-[11px] text-zinc-500 dark:text-zinc-400">
<input
type="color"
bind:value={profileAvatarColor}
class="h-7 w-8 cursor-pointer rounded border border-zinc-200 dark:border-zinc-700 bg-transparent p-0.5"
aria-label="Custom avatar color"
/>
<span class="font-mono">{profileAvatarColor}</span>
</label>
</div>
</div>

<!-- Fallback shape -->
<div class="space-y-2">
<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300">Fallback Avatar Shape</span>
<div class="flex flex-wrap gap-2">
{#each AVATAR_SHAPES as shape (shape.id)}
<button
type="button"
class="rounded-xl border px-3 py-1.5 text-xs font-medium transition-all {profileAvatarShape === shape.id
? 'border-zinc-900 dark:border-white bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
: 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-50 dark:hover:bg-zinc-800'}"
onclick={() => (profileAvatarShape = shape.id)}
>
{shape.label}
</button>
{/each}
</div>
</div>

{#if errorMessage}
<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400">
{errorMessage}
</div>
{/if}

<div class="flex items-center gap-3 pt-1">
<button
type="button"
class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
onclick={saveProfile}
disabled={isSavingProfile}
>
{isSavingProfile ? 'Saving...' : 'Save Profile'}
</button>
{#if profileSavedBadge}
<span class="text-xs text-emerald-600 dark:text-emerald-400 flex items-center gap-1 font-medium">
<Check class="h-3.5 w-3.5" /> Saved!
</span>
{/if}
</div>
</div>

<!-- Live preview -->
<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 shadow-xs">
<div class="flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500 mb-3">
<Sparkles class="h-3.5 w-3.5 text-amber-500" />
Live Preview
</div>
<div class="flex flex-col items-center text-center gap-2">
<Avatar
src={profileAvatarUrl || null}
name={previewName}
color={profileAvatarColor}
shape={previewShape}
size="xl"
/>
<div class="flex flex-col">
<span class="text-sm font-semibold text-zinc-900 dark:text-white truncate max-w-[180px]">
{previewName}
</span>
<span class="text-[11px] font-mono text-zinc-400 dark:text-zinc-500">
@{authState.username || 'account'}
</span>
<span class="mt-1 inline-flex items-center justify-center rounded-full bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-zinc-500 dark:text-zinc-400">
{authState.role}
</span>
</div>
</div>
<p class="mt-3 text-[11px] text-zinc-400 dark:text-zinc-500 text-center">
The picture is used when set; otherwise the colored {profileAvatarShape} with initials is rendered.
</p>
</div>
</div>
</section>
