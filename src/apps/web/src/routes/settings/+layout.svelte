<script lang="ts">
import { page } from '$app/state';
import { authState } from '$lib/stores/auth.svelte';
import { navState } from '$lib/stores/nav.svelte';
import Avatar from '$lib/components/Avatar.svelte';
import { Settings, ArrowLeft, User, Sun, Users, BookOpen, Menu } from 'lucide-svelte';

let { children } = $props();

interface NavItem {
href: string;
label: string;
hint: string;
icon: typeof Settings;
iconColor: string;
}

const navItems: NavItem[] = [
{
href: '/settings/accounts',
label: 'Accounts',
hint: 'Users, roles & passwords',
icon: Users,
iconColor: 'text-cyan-400 dark:text-cyan-600'
},
{
href: '/settings/instructions',
label: 'Agent Instructions',
hint: 'Reusable system prompt presets',
icon: BookOpen,
iconColor: 'text-violet-400 dark:text-violet-600'
},
{
href: '/settings/profile',
label: 'Server Profile',
hint: 'Display name & avatar',
icon: User,
iconColor: 'text-indigo-400 dark:text-indigo-600'
},
{
href: '/settings/appearance',
label: 'Appearance',
hint: 'Theme & interface styling',
icon: Sun,
iconColor: 'text-amber-400 dark:text-amber-600'
}
];

const pathname = $derived(page.url.pathname.replace(/\/+$/, '') || '/');

// Mobile drill-down: the settings section list is level 2.
let navOpen = $state(true);

// Close the section list only when the user actually navigates between
// settings pages (not on the initial render, which must show level 2).
	// Close the section list only when the user navigates between settings pages.
	// `prevPath` is a plain variable so the first render keeps level 2 visible.
	let prevPath: string | null = null;
	$effect(() => {
		const current = pathname;
		if (prevPath !== null && current !== prevPath) {
			navOpen = false;
		}
		prevPath = current;
	});

const onHub = $derived(pathname === '/settings');

// A section is only "current" while its content is actually on screen. When the
// section list is open (level 2), nothing is selected — otherwise going "back"
// would leave the previous item highlighted.
let isWide = $state(false);
$effect(() => {
if (typeof window === 'undefined') return;
const mq = window.matchMedia('(min-width: 1024px)');
isWide = mq.matches;
const onChange = (e: MediaQueryListEvent) => (isWide = e.matches);
mq.addEventListener('change', onChange);
return () => mq.removeEventListener('change', onChange);
});

function isCurrent(href: string) {
// On mobile the section list is its own screen, so nothing is selected while
// it is open. On desktop the list is always visible beside the content.
return (isWide || !navOpen) && pathname === href;
}
</script>

<div id="settings-page" class="flex h-full flex-col overflow-hidden">
<!-- Header -->
<header
id="settings-header"
class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5"
>
<div class="flex items-center gap-3">
<button
type="button"
onclick={() => {
if (onHub) {
// On the section list: leave Settings entirely (level 1).
navOpen = true;
navState.showLevel1();
} else if (!navOpen) {
// Viewing a section: go back to the list (level 2).
navOpen = true;
} else {
// The list is already open but the URL is still a section:
// treat this as leaving Settings.
navState.showLevel1();
}
}}
class="cursor-pointer text-zinc-400 transition-colors hover:text-zinc-800 dark:hover:text-white"
aria-label={onHub ? 'Back to navigation' : 'Back to settings sections'}
>
<ArrowLeft class="h-4 w-4" />
</button>
<div class="flex items-center gap-2">
<Settings class="h-4 w-4 text-cyan-500" />
<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">
Platform Settings
</h1>
</div>
</div>

{#if authState.activeAccount}
<div class="flex items-center gap-2.5">
<Avatar
src={authState.avatarUrl}
name={authState.displayName}
color={authState.avatarColor}
shape={authState.avatarShape === 'circle' ? 'circle' : authState.avatarShape === 'square' ? 'square' : 'rounded'}
size="sm"
/>
<div class="flex flex-col leading-tight">
<span class="text-xs font-semibold text-zinc-800 dark:text-zinc-200 truncate max-w-[160px]">
{authState.displayName}
</span>
<span class="text-[10px] font-mono uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
{authState.role}
</span>
</div>
</div>
{/if}
</header>

<!-- Settings Layout: Left Route Navigation + Routed Content Area -->
<div id="settings-layout" class="flex flex-1 overflow-hidden">

<aside
id="settings-sidebar"
class="h-full w-full shrink-0 flex-col justify-between border-r border-zinc-200/80 bg-zinc-100/50 p-4 dark:border-zinc-800/80 dark:bg-zinc-900/40 lg:flex lg:w-64 {navOpen ? 'flex' : 'hidden'}"
>
<nav id="settings-nav" class="space-y-1" aria-label="Settings sections">
<div class="px-2.5 pb-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
Navigation
</div>

{#each navItems as item (item.href)}
{@const current = isCurrent(item.href)}
<a
href={item.href}
aria-current={current ? 'page' : undefined}
class="flex items-center gap-3 rounded-xl px-3 py-2 text-xs font-medium transition-all {current
? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-white'}"
>
<item.icon
class="h-4 w-4 shrink-0 {current ? item.iconColor : 'text-zinc-400 dark:text-zinc-500'}"
/>
<div class="flex flex-col text-left">
<span>{item.label}</span>
<span class="text-[10px] font-normal opacity-70">{item.hint}</span>
</div>
</a>
{/each}
</nav>

<div class="px-2 pt-4 border-t border-zinc-200/70 dark:border-zinc-800/80 text-[11px] text-zinc-400 dark:text-zinc-500">
NeuxDOT System Configuration
</div>
</aside>

<main
id="settings-content"
class="flex-1 flex-col overflow-y-auto px-4 py-6 sm:px-8 sm:py-8 md:px-12 lg:flex {onHub ? 'hidden' : 'flex'}"
>
<div class="max-w-3xl">
{@render children()}
</div>
</main>
</div>
</div>
