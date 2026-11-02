<script lang="ts">
interface Props {
/** Picture URL (data URL or http(s) URL). Falls back to the colored shape when absent/broken. */
src?: string | null;
/** Name used for the initials fallback and the accessible label. */
name?: string | null;
size?: 'xs' | 'sm' | 'md' | 'lg' | 'xl';
color?: string;
shape?: 'circle' | 'rounded' | 'square';
class?: string;
}

let {
src = null,
name = '',
size = 'md',
color = '#3b82f6',
shape = 'circle',
class: className = ''
}: Props = $props();

let failed = $state(false);

// Reset the broken-image flag whenever the source changes.
$effect(() => {
void src;
failed = false;
});

const sizes = {
xs: 'h-6 w-6 text-[10px]',
sm: 'h-8 w-8 text-[11px]',
md: 'h-10 w-10 text-xs',
lg: 'h-14 w-14 text-base',
xl: 'h-20 w-20 text-xl'
};

const radius = $derived(
shape === 'circle' ? 'rounded-full' : shape === 'square' ? 'rounded-md' : 'rounded-xl'
);

const initials = $derived(
(name || '?')
.trim()
.split(/\s+/)
.slice(0, 2)
.map((part) => part.charAt(0))
.join('')
.toUpperCase() || '?'
);

const label = $derived(name ? `${name} avatar` : 'User avatar');
</script>

{#if src && !failed}
<img
{src}
alt={label}
class="{sizes[size]} {radius} shrink-0 object-cover border border-zinc-200/80 dark:border-zinc-700/80 bg-zinc-100 dark:bg-zinc-800 {className}"
onerror={() => (failed = true)}
/>
{:else}
<div
class="{sizes[size]} {radius} shrink-0 flex items-center justify-center font-bold text-white shadow-2xs select-none {className}"
style="background-color: {color};"
role="img"
aria-label={label}
title={label}
>
{initials}
</div>
{/if}
