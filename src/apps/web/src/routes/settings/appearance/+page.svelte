<script lang="ts">
import { themeStore } from '$lib/stores/theme.svelte';
import { Sun, Moon } from 'lucide-svelte';
</script>

<section id="appearance" class="space-y-6">
<div>
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Appearance & Theme</h2>
<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">Toggle between Bright Mode and Dark Mode.</p>
</div>

<div class="grid grid-cols-1 sm:grid-cols-2 gap-4 max-w-md">
<button
type="button"
class="flex flex-col items-center justify-center rounded-2xl border p-5 transition-all {themeStore.isDark
? 'border-zinc-900 dark:border-white bg-white dark:bg-zinc-800 shadow-xs'
: 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50'}"
onclick={() => themeStore.set(true)}
>
<Moon class="h-6 w-6 mb-2 text-indigo-500" />
<span class="text-xs font-semibold text-zinc-900 dark:text-white">Dark Mode</span>
</button>

<button
type="button"
class="flex flex-col items-center justify-center rounded-2xl border p-5 transition-all {!themeStore.isDark
? 'border-zinc-900 dark:border-white bg-white dark:bg-zinc-800 shadow-xs ring-2 ring-zinc-900 dark:ring-white ring-offset-2 ring-offset-zinc-50 dark:ring-offset-zinc-950'
: 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50'}"
onclick={() => themeStore.set(false)}
>
<Sun class="h-6 w-6 mb-2 text-amber-500" />
<span class="text-xs font-semibold text-zinc-900 dark:text-white">Bright Mode</span>
</button>
</div>
</section>
