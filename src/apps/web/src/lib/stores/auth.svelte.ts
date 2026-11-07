import {
api,
isRoot as apiIsRoot,
type ServerAccount,
type Profile,
type AccountRole,
type AuthMe,
type ChangePasswordPayload
} from '$lib/api';

class AuthState {
accounts = $state<ServerAccount[]>([]);
activeAccount = $state<ServerAccount | null>(null);
profile = $state<Profile | null>(null);
isAuthenticated = $state<boolean>(false);
isLoading = $state<boolean>(true);

/** Role of the currently authenticated server account (defaults to 'member' when unknown). */
role = $state<AccountRole>('member');
username = $state<string>('');

constructor() {
if (typeof window !== 'undefined') {
this.loadStorage();
}
}

// -- Derived convenience getters ------------------------------------------------
/** True when the active server account holds root privileges. */
get isRoot(): boolean {
return this.role === 'root';
}

/** Name to render in the UI: server profile display name → username. */
get displayName(): string {
return this.profile?.display_name?.trim() || this.username || this.activeAccount?.username || '';
}

/** Avatar picture (data URL or http URL) for the active account, if any. */
get avatarUrl(): string | undefined {
return this.profile?.avatar_url || this.activeAccount?.avatarUrl;
}

get avatarColor(): string {
return this.profile?.avatar_color || this.activeAccount?.avatarColor || '#3b82f6';
}

get avatarShape(): string {
return this.profile?.avatar_shape || this.activeAccount?.avatarShape || 'circle';
}

// -- Persistence ---------------------------------------------------------------
loadStorage() {
try {
const savedList = localStorage.getItem('neuxdot_accounts');
if (savedList) {
this.accounts = (JSON.parse(savedList) as ServerAccount[]).map((a) => ({
...a,
role: a.role ?? 'member'
}));
}

const activeStr = localStorage.getItem('neuxdot_active_account');
if (activeStr) {
const active = JSON.parse(activeStr) as ServerAccount;
this.switchAccount(active);
} else if (this.accounts.length > 0) {
this.switchAccount(this.accounts[0]);
} else {
this.isLoading = false;
}
} catch {
this.isLoading = false;
}
}

persist() {
if (typeof window === 'undefined') return;
localStorage.setItem('neuxdot_accounts', JSON.stringify(this.accounts));
if (this.activeAccount) {
localStorage.setItem('neuxdot_active_account', JSON.stringify(this.activeAccount));
} else {
localStorage.removeItem('neuxdot_active_account');
}
}

// -- Session -------------------------------------------------------------------
/**
 * Refresh the authenticated identity (`/api/auth/me`) for the active account and
 * keep the stored account row (role, display name, avatar) in sync.
 */
async refreshIdentity(): Promise<AuthMe | null> {
if (!this.activeAccount) return null;
try {
const me = await api.auth.me();
this.isAuthenticated = true;
this.role = me.role ?? 'member';
this.username = me.username || this.activeAccount.username;
this.profile = me.profile ?? null;
this.syncActiveAccountFromProfile();
return me;
} catch {
this.isAuthenticated = false;
return null;
}
}

async switchAccount(account: ServerAccount) {
this.activeAccount = account;
api.setConfig(account.serverUrl, account.token);
this.persist();

this.isLoading = true;
this.role = account.role ?? 'member';
this.username = account.username;
try {
await this.refreshIdentity();
} finally {
this.isLoading = false;
this.persist();
}
}

/** Copy profile fields and role back onto the persisted account record. */
syncActiveAccountFromProfile() {
if (!this.activeAccount) return;
const updated: ServerAccount = {
...this.activeAccount,
role: this.role,
username: this.username || this.activeAccount.username,
displayName: this.profile?.display_name || this.activeAccount.displayName,
avatarColor: this.profile?.avatar_color || this.activeAccount.avatarColor,
avatarShape: this.profile?.avatar_shape || this.activeAccount.avatarShape,
avatarUrl: this.profile?.avatar_url || this.activeAccount.avatarUrl
};
this.activeAccount = updated;
const idx = this.accounts.findIndex((a) => a.id === updated.id && a.serverUrl === updated.serverUrl);
if (idx >= 0) {
this.accounts[idx] = updated;
}
this.persist();
}

/** Called after PUT /api/profile succeeds so every surface re-renders. */
applyProfile(profile: Profile) {
this.profile = profile;
this.syncActiveAccountFromProfile();
}

async addAndSelectAccount(account: ServerAccount) {
const idx = this.accounts.findIndex(
(a) => a.serverUrl === account.serverUrl && a.username === account.username
);
if (idx >= 0) {
this.accounts[idx] = { ...account, role: account.role ?? this.accounts[idx].role ?? 'member' };
} else {
this.accounts.push({ ...account, role: account.role ?? 'member' });
}
await this.switchAccount(account);
}

removeAccount(id: string) {
this.accounts = this.accounts.filter((a) => a.id !== id);
if (this.activeAccount?.id === id) {
if (this.accounts.length > 0) {
this.switchAccount(this.accounts[0]);
} else {
this.activeAccount = null;
this.isAuthenticated = false;
this.role = 'member';
this.username = '';
this.profile = null;
this.persist();
}
} else {
this.persist();
}
}

logout() {
if (this.activeAccount) {
this.removeAccount(this.activeAccount.id);
}
}

// -- Account management ---------------------------------------------------------
/**
 * Change the password of the currently authenticated account.
 * Surfaces backend errors as-is (401 wrong old password, 400 mismatch).
 */
async changePassword(payload: ChangePasswordPayload) {
if (!this.activeAccount) throw new Error('No active account.');
if (payload.new_password !== payload.confirm_password) {
throw new Error('New password and confirmation do not match.');
}
return api.accounts.changePassword(this.activeAccount.id, payload);
}
}

export const authState = new AuthState();
