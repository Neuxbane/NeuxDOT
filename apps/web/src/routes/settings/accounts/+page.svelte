<script lang="ts">
import { api, isRoot, type AccountRole, type ManagedAccount, type ServerAccount } from '$lib/api';
import { authState } from '$lib/stores/auth.svelte';
import Avatar from '$lib/components/Avatar.svelte';
import {
Plus,
Pencil,
Trash2,
X,
KeyRound,
Check,
ShieldCheck,
UserPlus,
LogOut,
CircleAlert,
Server
} from 'lucide-svelte';

const ROLE_OPTIONS: { id: AccountRole; label: string; hint: string }[] = [
{ id: 'member', label: 'Member', hint: 'Standard access' },
{ id: 'root', label: 'Root', hint: 'Full administration' }
];

const ROOT_COLOR = '#6366f1';
const MEMBER_COLOR = '#14b8a6';

// --- Remote account list (root only endpoint) ---
let managedAccounts = $state<ManagedAccount[]>([]);
let isLoadingAccounts = $state(false);
let listError = $state<string | null>(null);

// --- Add account (log in to another server profile) ---
let isAddModalOpen = $state(false);
let modalServerUrl = $state(api.getServerUrl() || 'http://localhost:8000');
let modalUsername = $state('');
let modalPassword = $state('');
let isSubmitting = $state(false);
let addError = $state<string | null>(null);

// --- Create user (root-only POST /api/accounts) ---
let isCreateModalOpen = $state(false);
let createUsername = $state('');
let createDisplayName = $state('');
let createPassword = $state('');
let createConfirm = $state('');
let createRole = $state<AccountRole>('member');
let isCreating = $state(false);
let createError = $state<string | null>(null);

// --- Edit account ---
let editingAccount = $state<ManagedAccount | null>(null);
let editUsername = $state('');
let editDisplayName = $state('');
let editRole = $state<AccountRole>('member');
let editAvatarUrl = $state('');
let isSavingEdit = $state(false);
let editError = $state<string | null>(null);

// --- Delete account ---
let deletingAccount = $state<ManagedAccount | null>(null);
let isDeleting = $state(false);
let deleteError = $state<string | null>(null);

// --- Change password (current account) ---
let oldPassword = $state('');
let newPassword = $state('');
let confirmPassword = $state('');
let isChangingPassword = $state(false);
let passwordError = $state<string | null>(null);
let passwordSaved = $state(false);

const rootCount = $derived(managedAccounts.filter((a) => a.role === 'root').length);
const canManage = $derived(isRoot(authState.role));

$effect(() => {
if (authState.isAuthenticated && canManage) {
loadAccounts();
}
});

function isSelf(acc: ManagedAccount): boolean {
return authState.activeAccount?.id === acc.id;
}

function isLastRoot(acc: ManagedAccount): boolean {
return acc.role === 'root' && rootCount <= 1;
}

function avatarShape(shape?: string): 'circle' | 'rounded' | 'square' {
return shape === 'circle' ? 'circle' : shape === 'square' ? 'square' : 'rounded';
}

async function loadAccounts() {
isLoadingAccounts = true;
listError = null;
try {
managedAccounts = await api.accounts.list();
} catch (e: any) {
listError = e?.message || 'Failed to load accounts.';
} finally {
isLoadingAccounts = false;
}
}

// ---------------------------------------------------------------- Add (log in)
function openAddModal() {
modalServerUrl = api.getServerUrl() || 'http://localhost:8000';
modalUsername = '';
modalPassword = '';
addError = null;
isAddModalOpen = true;
}

async function handleAddAccount() {
if (!modalUsername.trim() || !modalServerUrl.trim() || !modalPassword.trim()) return;
isSubmitting = true;
addError = null;

try {
api.setConfig(modalServerUrl.trim(), '');
const res = await api.auth.login(modalUsername.trim(), modalPassword.trim());

const newAcc: ServerAccount = {
id: res.account_id,
serverUrl: modalServerUrl.trim(),
username: res.username,
token: res.token,
role: res.role ?? 'member',
displayName: res.username,
avatarColor: '#3b82f6'
};

await authState.addAndSelectAccount(newAcc);
isAddModalOpen = false;
if (isRoot(newAcc.role)) await loadAccounts();
} catch (e: any) {
addError = e?.message || 'Login failed.';
} finally {
isSubmitting = false;
}
}

// ------------------------------------------------------------- Create account
function openCreateModal() {
createUsername = '';
createDisplayName = '';
createPassword = '';
createConfirm = '';
createRole = 'member';
createError = null;
isCreateModalOpen = true;
}

async function handleCreateAccount() {
createError = null;
if (!createUsername.trim()) return (createError = 'Username is required.');
if (createPassword.length < 8) return (createError = 'Password must be at least 8 characters.');
if (createPassword !== createConfirm) return (createError = 'Passwords do not match.');

isCreating = true;
try {
await api.accounts.create({
username: createUsername.trim(),
password: createPassword,
display_name: createDisplayName.trim() || undefined,
role: createRole
});
isCreateModalOpen = false;
await loadAccounts();
} catch (e: any) {
createError = e?.message || 'Failed to create account.';
} finally {
isCreating = false;
}
}

// ----------------------------------------------------------------------- Edit
function openEditModal(acc: ManagedAccount) {
editingAccount = acc;
editUsername = acc.username;
editDisplayName = acc.display_name || '';
editRole = acc.role;
editAvatarUrl = acc.avatar_url || '';
editError = null;
}

async function handleSaveEdit() {
if (!editingAccount) return;
editError = null;
if (!editUsername.trim()) return (editError = 'Username is required.');

isSavingEdit = true;
try {
const updated = await api.accounts.update(editingAccount.id, {
username: editUsername.trim(),
display_name: editDisplayName.trim(),
role: editRole,
avatar_url: editAvatarUrl.trim()
});
managedAccounts = managedAccounts.map((a) => (a.id === updated.id ? updated : a));
editingAccount = null;

// Keep the locally persisted identity in sync when editing ourselves.
if (isSelf(updated)) {
await authState.refreshIdentity();
await loadAccounts();
}
} catch (e: any) {
editError = e?.message || 'Failed to update account.';
} finally {
isSavingEdit = false;
}
}

// --------------------------------------------------------------------- Delete
function openDeleteConfirm(acc: ManagedAccount) {
deletingAccount = acc;
deleteError = null;
}

async function handleDeleteAccount() {
if (!deletingAccount) return;
const id = deletingAccount.id;
isDeleting = true;
deleteError = null;
try {
await api.accounts.delete(id);
managedAccounts = managedAccounts.filter((a) => a.id !== id);
deletingAccount = null;
} catch (e: any) {
deleteError = e?.message || 'Failed to delete account.';
} finally {
isDeleting = false;
}
}

// ----------------------------------------------------------- Change password
async function handleChangePassword() {
passwordError = null;
passwordSaved = false;
if (!oldPassword) return (passwordError = 'Enter your current password.');
if (newPassword.length < 8) return (passwordError = 'New password must be at least 8 characters.');
if (newPassword !== confirmPassword) return (passwordError = 'New passwords do not match.');

isChangingPassword = true;
try {
await authState.changePassword({
old_password: oldPassword,
new_password: newPassword,
confirm_password: confirmPassword
});
oldPassword = '';
newPassword = '';
confirmPassword = '';
passwordSaved = true;
setTimeout(() => (passwordSaved = false), 2500);
} catch (e: any) {
passwordError = e?.message || 'Failed to change password.';
} finally {
isChangingPassword = false;
}
}
</script>

<div class="space-y-10">
<!-- ACCOUNTS ON THIS SERVER -->
<section id="accounts" class="space-y-6">
<div class="flex items-center justify-between gap-3">
<div>
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Server Accounts</h2>
<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
Accounts registered on
<span class="font-mono">{api.getServerUrl().replace(/^https?:\/\//, '')}</span>.
{#if !canManage}Only root accounts can create, edit or delete accounts.{/if}
</p>
</div>
<div class="flex items-center gap-2">
<button
type="button"
class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98"
onclick={openAddModal}
>
<Plus class="h-3.5 w-3.5" />
<span>Add Account</span>
</button>
{#if canManage}
<button
type="button"
class="flex items-center gap-1.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-50 dark:hover:bg-zinc-800 transition-colors shadow-2xs"
onclick={openCreateModal}
>
<UserPlus class="h-3.5 w-3.5" />
<span>New User</span>
</button>
{/if}
</div>
</div>

{#if !canManage}
<div class="rounded-2xl border border-amber-200 dark:border-amber-900/60 bg-amber-50 dark:bg-amber-950/30 p-4 flex items-start gap-2.5 text-xs text-amber-800 dark:text-amber-300">
<ShieldCheck class="h-4 w-4 shrink-0 mt-0.5" />
<span>
You are signed in as <strong>{authState.username}</strong> with the <strong>{authState.role}</strong> role.
Account administration is restricted to root accounts.
</span>
</div>
{:else}
{#if listError}
<div class="rounded-2xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-3 text-xs text-rose-600 dark:text-rose-400 flex items-center justify-between">
<span class="flex items-center gap-1.5"><CircleAlert class="h-3.5 w-3.5" /> {listError}</span>
<button type="button" class="underline hover:no-underline" onclick={loadAccounts}>Retry</button>
</div>
{/if}

<div class="space-y-2">
{#if isLoadingAccounts && managedAccounts.length === 0}
<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
Loading accounts...
</div>
{:else if managedAccounts.length === 0}
<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
No accounts returned by the server.
</div>
{:else}
{#each managedAccounts as acc (acc.id)}
{@const self = isSelf(acc)}
{@const lastRoot = isLastRoot(acc)}
<div
class="flex items-center justify-between gap-3 rounded-2xl border p-4 transition-all {self
? 'border-zinc-400 dark:border-zinc-600 bg-white dark:bg-zinc-900 shadow-xs'
: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50 dark:hover:bg-zinc-800'}"
>
<div class="flex items-center gap-3 min-w-0">
<Avatar
src={acc.avatar_url}
name={acc.display_name || acc.username}
color={acc.role === 'root' ? ROOT_COLOR : MEMBER_COLOR}
shape="rounded"
size="md"
/>
<div class="min-w-0">
<div class="flex items-center gap-2 flex-wrap">
<span class="text-sm font-semibold text-zinc-900 dark:text-white truncate">{acc.username}</span>
<span
class="rounded-full px-2 py-0.5 text-[10px] font-medium border {acc.role === 'root'
? 'bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border-indigo-500/20'
: 'bg-zinc-100 dark:bg-zinc-800 text-zinc-500 dark:text-zinc-400 border-zinc-200 dark:border-zinc-700'}"
>
{acc.role}
</span>
{#if self}
<span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
You
</span>
{/if}
{#if lastRoot}
<span class="rounded-full bg-amber-500/10 px-2 py-0.5 text-[10px] font-medium text-amber-600 dark:text-amber-400 border border-amber-500/20">
Last root
</span>
{/if}
</div>
<div class="flex items-center gap-2 mt-0.5">
{#if acc.display_name}
<span class="text-xs text-zinc-500 dark:text-zinc-400 truncate">{acc.display_name}</span>
<span class="text-zinc-300 dark:text-zinc-700">·</span>
{/if}
<span class="text-[11px] font-mono text-zinc-400 dark:text-zinc-500">
{acc.created_at ? new Date(acc.created_at).toLocaleDateString() : '—'}
</span>
</div>
</div>
</div>

<div class="flex items-center gap-1.5 shrink-0">
<button
type="button"
class="rounded-lg p-1.5 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-zinc-800 dark:hover:text-white transition-colors"
onclick={() => openEditModal(acc)}
title="Edit account"
aria-label="Edit account {acc.username}"
>
<Pencil class="h-4 w-4" />
</button>
<button
type="button"
class="rounded-lg p-1.5 transition-colors {self || lastRoot
? 'text-zinc-300 dark:text-zinc-700 cursor-not-allowed'
: 'text-zinc-400 hover:bg-rose-500/10 hover:text-rose-500'}"
disabled={self || lastRoot}
onclick={() => openDeleteConfirm(acc)}
title={self
? 'You cannot delete your own account'
: lastRoot
? 'The last root account cannot be deleted'
: 'Delete account'}
aria-label="Delete account {acc.username}"
>
<Trash2 class="h-4 w-4" />
</button>
</div>
</div>
{/each}
{/if}
</div>
{/if}
</section>

<hr class="border-zinc-200/80 dark:border-zinc-800/80" />

<!-- CONNECTED SERVER PROFILES (multi-server) -->
<section id="connected-profiles" class="space-y-6">
<div>
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Connected Server Profiles</h2>
<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
Manage multiple server profiles. Switching seamlessly updates the base URL and secure enclave credentials.
</p>
</div>

<div class="space-y-2">
{#if authState.accounts.length === 0}
<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
No accounts connected yet. Click <strong>Add Account</strong> to log in with your credentials from the server CLI.
</div>
{:else}
{#each authState.accounts as acc (acc.id + acc.serverUrl)}
{@const active = authState.activeAccount?.id === acc.id && authState.activeAccount?.serverUrl === acc.serverUrl}
<div
class="flex items-center justify-between gap-3 rounded-2xl border p-4 transition-all {active
? 'border-zinc-400 dark:border-zinc-600 bg-white dark:bg-zinc-900 shadow-xs'
: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50 dark:hover:bg-zinc-800'}"
>
<div class="flex items-center gap-3 min-w-0">
<Avatar
src={active ? authState.avatarUrl : acc.avatarUrl}
name={acc.displayName || acc.username}
color={acc.avatarColor || '#3b82f6'}
shape={avatarShape(acc.avatarShape)}
size="md"
/>
<div class="min-w-0">
<div class="flex items-center gap-2">
<span class="text-sm font-semibold text-zinc-900 dark:text-white truncate">
{acc.displayName || acc.username}
</span>
<span class="rounded-full bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-medium text-zinc-500 dark:text-zinc-400">
{acc.role ?? 'member'}
</span>
{#if active}
<span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
Active
</span>
{/if}
</div>
<span class="text-xs font-mono text-zinc-400 dark:text-zinc-500 truncate block">{acc.serverUrl}</span>
</div>
</div>

<div class="flex items-center gap-2 shrink-0">
{#if !active}
<button
type="button"
class="rounded-xl bg-zinc-100 dark:bg-zinc-800 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors shadow-2xs"
onclick={() => authState.switchAccount(acc)}
>
Switch To
</button>
<button
type="button"
class="rounded-lg p-1.5 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
onclick={() => authState.removeAccount(acc.id)}
title="Remove this server profile from the browser"
aria-label="Remove account"
>
<Trash2 class="h-4 w-4" />
</button>
{:else}
<button
type="button"
class="rounded-xl bg-zinc-100 dark:bg-zinc-800 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors shadow-2xs"
onclick={() => authState.logout()}
title="Sign out of this server"
>
<LogOut class="h-3.5 w-3.5" />
</button>
{/if}
</div>
</div>
{/each}
{/if}
</div>
</section>

<hr class="border-zinc-200/80 dark:border-zinc-800/80" />

<!-- CHANGE PASSWORD (current account) -->
<section id="change-password" class="space-y-6">
<div>
<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Change Password</h2>
<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
Update the password for the account you are currently signed in as
{#if authState.username}<span class="font-mono font-medium">{authState.username}</span>{/if}.
</p>
</div>

<form
class="space-y-4 max-w-md"
onsubmit={(e) => {
e.preventDefault();
handleChangePassword();
}}
>
<div>
<label for="old-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
Current Password
</label>
<input
id="old-password"
type="password"
bind:value={oldPassword}
autocomplete="current-password"
placeholder="••••••••"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>
<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
<div>
<label for="new-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
New Password
</label>
<input
id="new-password"
type="password"
bind:value={newPassword}
autocomplete="new-password"
placeholder="At least 8 characters"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>
<div>
<label for="confirm-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
Confirm New Password
</label>
<input
id="confirm-password"
type="password"
bind:value={confirmPassword}
autocomplete="new-password"
placeholder="Repeat password"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>
</div>

{#if passwordError}
<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400 flex items-center gap-1.5">
<CircleAlert class="h-3.5 w-3.5 shrink-0" />
<span>{passwordError}</span>
</div>
{/if}

<div class="flex items-center gap-3 pt-1">
<button
type="submit"
class="flex items-center gap-1.5 rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
disabled={!oldPassword || !newPassword || !confirmPassword || isChangingPassword}
>
<KeyRound class="h-3.5 w-3.5" />
<span>{isChangingPassword ? 'Updating...' : 'Update Password'}</span>
</button>
{#if passwordSaved}
<span class="text-xs text-emerald-600 dark:text-emerald-400 flex items-center gap-1 font-medium">
<Check class="h-3.5 w-3.5" /> Password updated
</span>
{/if}
</div>
</form>
</section>
</div>

<!-- ============================== MODALS ============================== -->

<!-- Modal: Add Account (log in to a server profile) -->
{#if isAddModalOpen}
<div
id="modal-add-account"
class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
>
<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">Log In to Account</h3>
<button
type="button"
class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
onclick={() => (isAddModalOpen = false)}
aria-label="Close modal"
>
<X class="h-4 w-4" />
</button>
</div>

<form
onsubmit={(e) => {
e.preventDefault();
handleAddAccount();
}}
class="mt-4 space-y-4"
>
<div>
<label for="server-url" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Server URL</label>
<input
id="server-url"
type="text"
bind:value={modalServerUrl}
placeholder="http://localhost:8000"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>

<div>
<label for="username" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Username</label>
<input
id="username"
type="text"
bind:value={modalUsername}
placeholder="Your account username"
autocomplete="username"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>

<div>
<label for="login-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Password</label>
<input
id="login-password"
type="password"
bind:value={modalPassword}
autocomplete="current-password"
placeholder="Enter your account password"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>

<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950/60 p-3 text-[11px] text-zinc-600 dark:text-zinc-400">
🔑 The server CLI displays the initial <strong>root</strong> user and password once per run. Other users are created by a root account via <strong>New User</strong>.
</div>

{#if addError}
<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400">
{addError}
</div>
{/if}

<div class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3">
<button
type="button"
class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
onclick={() => (isAddModalOpen = false)}
>
Cancel
</button>
<button
type="submit"
class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
disabled={!modalUsername.trim() || !modalServerUrl.trim() || !modalPassword.trim() || isSubmitting}
>
{isSubmitting ? 'Logging In...' : 'Log In'}
</button>
</div>
</form>
</div>
</div>
{/if}

<!-- Modal: Create user (root only) -->
{#if isCreateModalOpen}
<div
id="modal-create-account"
class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
>
<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
<div>
<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">Create Account</h3>
<p class="text-[11px] text-zinc-500 dark:text-zinc-400">Register a new user on this server.</p>
</div>
<button
type="button"
class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
onclick={() => (isCreateModalOpen = false)}
aria-label="Close modal"
>
<X class="h-4 w-4" />
</button>
</div>

<form
onsubmit={(e) => {
e.preventDefault();
handleCreateAccount();
}}
class="mt-4 space-y-4"
>
<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
<div>
<label for="create-username" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Username</label>
<input
id="create-username"
type="text"
bind:value={createUsername}
placeholder="e.g. alice"
autocomplete="off"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>
<div>
<label for="create-display-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Display Name</label>
<input
id="create-display-name"
type="text"
bind:value={createDisplayName}
placeholder="Optional"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>
</div>

<div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
<div>
<label for="create-password" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Password</label>
<input
id="create-password"
type="password"
bind:value={createPassword}
autocomplete="new-password"
placeholder="At least 8 characters"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>
<div>
<label for="create-confirm" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Confirm Password</label>
<input
id="create-confirm"
type="password"
bind:value={createConfirm}
autocomplete="new-password"
placeholder="Repeat password"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>
</div>

<div>
<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Role</span>
<div class="grid grid-cols-2 gap-2">
{#each ROLE_OPTIONS as role (role.id)}
<button
type="button"
class="rounded-xl border px-3 py-2 text-left transition-all {createRole === role.id
? 'border-zinc-900 dark:border-white bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
: 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-50 dark:hover:bg-zinc-800'}"
onclick={() => (createRole = role.id)}
>
<span class="block text-xs font-semibold capitalize">{role.label}</span>
<span class="block text-[10px] opacity-70">{role.hint}</span>
</button>
{/each}
</div>
</div>

{#if createError}
<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400">
{createError}
</div>
{/if}

<div class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3">
<button
type="button"
class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
onclick={() => (isCreateModalOpen = false)}
>
Cancel
</button>
<button
type="submit"
class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
disabled={!createUsername.trim() || !createPassword || !createConfirm || isCreating}
>
{isCreating ? 'Creating...' : 'Create Account'}
</button>
</div>
</form>
</div>
</div>
{/if}

<!-- Modal: Edit account -->
{#if editingAccount}
<div
id="modal-edit-account"
class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
>
<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
<div class="flex items-center gap-2">
<Server class="h-4 w-4 text-cyan-500" />
<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">Edit Account</h3>
</div>
<button
type="button"
class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white p-1"
onclick={() => (editingAccount = null)}
aria-label="Close modal"
>
<X class="h-4 w-4" />
</button>
</div>

<form
onsubmit={(e) => {
e.preventDefault();
handleSaveEdit();
}}
class="mt-4 space-y-4"
>
<div>
<label for="edit-username" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Username</label>
<input
id="edit-username"
type="text"
bind:value={editUsername}
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
required
/>
</div>

<div>
<label for="edit-display-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Display Name</label>
<input
id="edit-display-name"
type="text"
bind:value={editDisplayName}
placeholder="Optional"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>

<div>
<label for="edit-avatar-url" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Avatar URL</label>
<input
id="edit-avatar-url"
type="url"
bind:value={editAvatarUrl}
placeholder="https://.../avatar.png (optional)"
class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
/>
</div>

<div>
<span class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">Role</span>
<div class="grid grid-cols-2 gap-2">
{#each ROLE_OPTIONS as role (role.id)}
{@const blocked = role.id !== 'root' && isLastRoot(editingAccount)}
<button
type="button"
disabled={blocked}
class="rounded-xl border px-3 py-2 text-left transition-all {editRole === role.id
? 'border-zinc-900 dark:border-white bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
: 'border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 text-zinc-600 dark:text-zinc-400 hover:bg-zinc-50 dark:hover:bg-zinc-800'} {blocked
? 'opacity-40 cursor-not-allowed'
: ''}"
onclick={() => (editRole = role.id)}
title={blocked ? 'The last root account must stay root' : role.hint}
>
<span class="block text-xs font-semibold capitalize">{role.label}</span>
<span class="block text-[10px] opacity-70">{role.hint}</span>
</button>
{/each}
</div>
</div>

{#if editError}
<div class="rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400">
{editError}
</div>
{/if}

<div class="mt-6 flex justify-end gap-2 border-t border-zinc-100 dark:border-zinc-800 pt-3">
<button
type="button"
class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
onclick={() => (editingAccount = null)}
>
Cancel
</button>
<button
type="submit"
class="rounded-xl bg-zinc-900 dark:bg-white px-4 py-2 text-xs font-medium text-white dark:text-zinc-950 hover:bg-zinc-800 dark:hover:bg-zinc-100 disabled:opacity-40 transition-all shadow-xs"
disabled={!editUsername.trim() || isSavingEdit}
>
{isSavingEdit ? 'Saving...' : 'Save Changes'}
</button>
</div>
</form>
</div>
</div>
{/if}

<!-- Modal: Delete account confirmation -->
{#if deletingAccount}
<div
id="modal-delete-account"
class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
>
<div class="w-full max-w-sm rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
<div class="flex items-start gap-3">
<div class="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-rose-500/10 text-rose-500 border border-rose-500/20">
<Trash2 class="h-4 w-4" />
</div>
<div>
<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">Delete account?</h3>
<p class="mt-1 text-xs text-zinc-500 dark:text-zinc-400">
This permanently removes <strong>{deletingAccount.username}</strong> from the server. This action cannot be undone.
</p>
</div>
</div>

{#if deleteError}
<div class="mt-3 rounded-xl border border-rose-200 dark:border-rose-900/60 bg-rose-50 dark:bg-rose-950/40 p-2.5 text-[11px] text-rose-600 dark:text-rose-400">
{deleteError}
</div>
{/if}

<div class="mt-5 flex justify-end gap-2">
<button
type="button"
class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
onclick={() => (deletingAccount = null)}
>
Cancel
</button>
<button
type="button"
class="rounded-xl bg-rose-600 hover:bg-rose-500 px-4 py-2 text-xs font-medium text-white transition-all shadow-xs disabled:opacity-40"
disabled={isDeleting}
onclick={handleDeleteAccount}
>
{isDeleting ? 'Deleting...' : 'Delete Account'}
</button>
</div>
</div>
</div>
{/if}
