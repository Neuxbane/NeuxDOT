import { api, type ServerAccount, type Profile } from '$lib/api';

class AuthState {
	accounts = $state<ServerAccount[]>([]);
	activeAccount = $state<ServerAccount | null>(null);
	profile = $state<Profile | null>(null);
	isAuthenticated = $state<boolean>(false);
	isLoading = $state<boolean>(true);

	constructor() {
		if (typeof window !== 'undefined') {
			this.loadStorage();
		}
	}

	loadStorage() {
		try {
			const savedList = localStorage.getItem('neuxdot_accounts');
			if (savedList) {
				this.accounts = JSON.parse(savedList);
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

	async switchAccount(account: ServerAccount) {
		this.activeAccount = account;
		api.setConfig(account.serverUrl, account.token);
		this.persist();

		this.isLoading = true;
		try {
			const me = await api.auth.me();
			this.isAuthenticated = true;
			this.profile = me.profile;
		} catch {
			// Token might be invalid or server down
			this.isAuthenticated = false;
		} finally {
			this.isLoading = false;
		}
	}

	async addAndSelectAccount(account: ServerAccount) {
		const idx = this.accounts.findIndex(
			(a) => a.serverUrl === account.serverUrl && a.username === account.username
		);
		if (idx >= 0) {
			this.accounts[idx] = account;
		} else {
			this.accounts.push(account);
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
}

export const authState = new AuthState();
