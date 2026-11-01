<script lang="ts">
	import {
		api,
		type ServerAccount,
		type ExtensionItem,
		type ExtensionKvEntry,
		type ExtensionStorageInfo
	} from '$lib/api';
	import { authState } from '$lib/stores/auth.svelte';
	import { themeStore } from '$lib/stores/theme.svelte';
	import {
		Settings,
		User,
		Server,
		Plus,
		Check,
		Trash2,
		Sun,
		Moon,
		ArrowLeft,
		X,
		Package,
		Upload,
		Database,
		HardDrive,
		RefreshCw,
		FileText,
		Copy,
		ExternalLink,
		ShieldCheck
	} from 'lucide-svelte';

	let activeAnchor = $state<'accounts' | 'server-profile' | 'appearance' | 'extensions'>('accounts');

	// Add Account Modal
	let isAddModalOpen = $state(false);
	let modalServerUrl = $state('http://localhost:8000');
	let modalUsername = $state('root');
	let modalPassword = $state('');
	let isSubmitting = $state(false);

	// Profile form
	let profileDisplayName = $state(authState.profile?.display_name || '');
	let profileAvatarColor = $state(authState.profile?.avatar_color || '#3b82f6');
	let profileAvatarShape = $state(authState.profile?.avatar_shape || 'circle');
	let isSavingProfile = $state(false);
	let profileSavedBadge = $state(false);

	// Extensions State
	let extensionsList = $state<ExtensionItem[]>([]);
	let isLoadingExtensions = $state(false);
	let isUploadingWasm = $state(false);
	let installUrlInput = $state('');
	let isInstallingUrl = $state(false);
	let fileInputRef: HTMLInputElement | null = null;

	// Extension Update Comparison Modal State
	let pendingUpdateInspection = $state<any>(null);
	let pendingUpdateFile = $state<File | null>(null);
	let isComparingUpdate = $state(false);

	// Extension Inspector Drawer State
	let selectedExt = $state<ExtensionItem | null>(null);
	let inspectingKv = $state<ExtensionKvEntry[]>([]);
	let inspectingStorage = $state<ExtensionStorageInfo | null>(null);
	let isLoadingInspector = $state(false);

	function scrollToAnchor(id: 'accounts' | 'server-profile' | 'appearance' | 'extensions', e?: MouseEvent) {
		if (e) e.preventDefault();
		activeAnchor = id;
		const el = document.getElementById(id);
		if (el) {
			el.scrollIntoView({ behavior: 'smooth' });
			if (window.location.hash !== `#${id}`) {
				history.replaceState(null, '', `#${id}`);
			}
		}
	}

	$effect(() => {
		if (typeof window !== 'undefined') {
			const hash = window.location.hash.replace('#', '') as 'accounts' | 'server-profile' | 'appearance' | 'extensions';
			if (hash === 'accounts' || hash === 'server-profile' || hash === 'appearance' || hash === 'extensions') {
				activeAnchor = hash;
				const el = document.getElementById(hash);
				if (el) {
					setTimeout(() => el.scrollIntoView({ behavior: 'smooth' }), 50);
				}
			}
		}
	});

	$effect(() => {
		if (typeof window === 'undefined') return;
		const contentEl = document.getElementById('settings-content');
		if (!contentEl) return;

		const observer = new IntersectionObserver(
			(entries) => {
				for (const entry of entries) {
					if (entry.isIntersecting) {
						activeAnchor = entry.target.id as any;
					}
				}
			},
			{
				root: contentEl,
				threshold: 0.2
			}
		);

		['accounts', 'server-profile', 'appearance', 'extensions'].forEach((id) => {
			const el = document.getElementById(id);
			if (el) observer.observe(el);
		});

		return () => observer.disconnect();
	});

	$effect(() => {
		if (authState.profile) {
			profileDisplayName = authState.profile.display_name || '';
			profileAvatarColor = authState.profile.avatar_color || '#3b82f6';
			profileAvatarShape = authState.profile.avatar_shape || 'circle';
		}
	});

	$effect(() => {
		if (authState.isAuthenticated) {
			loadExtensions();
		}
	});

	async function loadExtensions() {
		isLoadingExtensions = true;
		try {
			extensionsList = await api.extensions.list();
			if (selectedExt) {
				const refreshed = extensionsList.find((e) => e.id === selectedExt?.id);
				if (refreshed) {
					selectedExt = refreshed;
					loadInspectorData(refreshed);
				} else {
					selectedExt = null;
				}
			}
		} catch (e) {
			console.error('Failed to load extensions:', e);
		} finally {
			isLoadingExtensions = false;
		}
	}

	async function handleUploadWasmFile(event: Event) {
		const target = event.target as HTMLInputElement;
		const file = target.files?.[0];
		if (!file) return;

		isUploadingWasm = true;
		try {
			// First, inspect the WASM binary to check if it's an update to an existing extension
			const inspection = await api.extensions.inspect(file);
			if (inspection.existing) {
				pendingUpdateInspection = inspection;
				pendingUpdateFile = file;
				isComparingUpdate = true;
				return;
			}

			// If not existing, install directly
			await api.extensions.upload(file);
			target.value = '';
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to inspect/upload extension: ' + (e?.message || e));
		} finally {
			isUploadingWasm = false;
		}
	}

	async function confirmExtensionUpdate() {
		if (!pendingUpdateFile) return;
		isUploadingWasm = true;
		try {
			await api.extensions.upload(pendingUpdateFile);
			if (fileInputRef) fileInputRef.value = '';
			isComparingUpdate = false;
			pendingUpdateInspection = null;
			pendingUpdateFile = null;
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to apply extension update: ' + (e?.message || e));
		} finally {
			isUploadingWasm = false;
		}
	}

	function cancelExtensionUpdate() {
		isComparingUpdate = false;
		pendingUpdateInspection = null;
		pendingUpdateFile = null;
		if (fileInputRef) fileInputRef.value = '';
	}

	async function handleInstallUrl() {
		const url = installUrlInput.trim();
		if (!url) return;

		isInstallingUrl = true;
		try {
			await api.extensions.installUrl(url);
			installUrlInput = '';
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to install from URL: ' + (e?.message || e));
		} finally {
			isInstallingUrl = false;
		}
	}

	async function toggleExtensionActive(ext: ExtensionItem) {
		try {
			await api.extensions.patch(ext.id, !ext.is_active);
			extensionsList = extensionsList.map((e) =>
				e.id === ext.id ? { ...e, is_active: !ext.is_active } : e
			);
		} catch (e: any) {
			alert('Failed to toggle extension: ' + (e?.message || e));
		}
	}

	async function deleteExtension(id: string) {
		if (!confirm('Are you sure you want to uninstall this extension? All isolated KV and file storage will be permanently wiped.'))
			return;

		try {
			await api.extensions.delete(id);
			if (selectedExt?.id === id) {
				selectedExt = null;
			}
			await loadExtensions();
		} catch (e: any) {
			alert('Failed to delete extension: ' + (e?.message || e));
		}
	}

	async function selectExtensionForInspect(ext: ExtensionItem) {
		selectedExt = ext;
		await loadInspectorData(ext);
	}

	async function loadInspectorData(ext: ExtensionItem) {
		isLoadingInspector = true;
		try {
			const [kv, storage] = await Promise.all([
				api.extensions.getKv(ext.id),
				api.extensions.getStorage(ext.id)
			]);
			inspectingKv = kv;
			inspectingStorage = storage;
		} catch (e) {
			console.error('Failed to load inspector data:', e);
		} finally {
			isLoadingInspector = false;
		}
	}

	async function deleteKvKey(key: string) {
		if (!selectedExt) return;
		if (!confirm(`Delete KV key "${key}"?`)) return;

		try {
			await api.extensions.deleteKvKey(selectedExt.id, key);
			inspectingKv = inspectingKv.filter((k) => k.key !== key);
			await loadInspectorData(selectedExt);
		} catch (e: any) {
			alert('Failed to delete key: ' + (e?.message || e));
		}
	}

	function formatBytes(bytes: number): string {
		if (bytes === 0) return '0 B';
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
	}

	async function handleAddAccount() {
		if (!modalUsername.trim() || !modalServerUrl.trim() || !modalPassword.trim()) return;
		isSubmitting = true;

		try {
			api.setConfig(modalServerUrl.trim(), '');

			const res = await api.auth.login(modalUsername.trim(), modalPassword.trim());

			const newAcc: ServerAccount = {
				id: res.account_id,
				serverUrl: modalServerUrl.trim(),
				username: res.username,
				token: res.token,
				displayName: res.username,
				avatarColor: '#3b82f6'
			};

			await authState.addAndSelectAccount(newAcc);
			isAddModalOpen = false;
			modalPassword = '';
		} catch (e: any) {
			alert('Login failed: ' + (e?.message || e));
		} finally {
			isSubmitting = false;
		}
	}

	async function saveProfile() {
		isSavingProfile = true;
		try {
			const updated = await api.profile.update({
				display_name: profileDisplayName.trim() || undefined,
				avatar_color: profileAvatarColor,
				avatar_shape: profileAvatarShape
			});
			authState.profile = updated;
			profileSavedBadge = true;
			setTimeout(() => (profileSavedBadge = false), 2000);
		} catch (e) {
			alert('Failed to update profile: ' + e);
		} finally {
			isSavingProfile = false;
		}
	}
</script>

<div id="settings-page" class="flex h-full flex-col overflow-hidden">
	<!-- Header -->
	<header
		id="settings-header"
		class="flex items-center justify-between border-b border-zinc-200/80 dark:border-zinc-800/80 bg-white/70 dark:bg-zinc-900/40 backdrop-blur-md px-6 py-3.5"
	>
		<div class="flex items-center gap-3">
			<a
				href="/"
				class="text-zinc-400 hover:text-zinc-800 dark:hover:text-white transition-colors"
				aria-label="Back to Agent Playground"
			>
				<ArrowLeft class="h-4 w-4" />
			</a>
			<div class="flex items-center gap-2">
				<Settings class="h-4 w-4 text-cyan-500" />
				<h1 class="text-base font-semibold text-zinc-900 dark:text-zinc-100 tracking-tight">
					Platform Settings
				</h1>
			</div>
		</div>
	</header>

	<!-- Settings Layout: Left Side Navigation + Main Content Area -->
	<div id="settings-layout" class="flex flex-1 overflow-hidden">
		<!-- 🧱 Settings Left Side Navigation (Anchor Links) -->
		<aside
			id="settings-sidebar"
			class="w-64 shrink-0 border-r border-zinc-200/80 dark:border-zinc-800/80 bg-zinc-100/50 dark:bg-zinc-900/40 p-4 flex flex-col justify-between"
		>
			<nav id="settings-nav" class="space-y-1">
				<div class="px-2.5 pb-2 text-[10px] font-semibold uppercase tracking-wider text-zinc-400 dark:text-zinc-500">
					Navigation
				</div>

				<a
					href="#accounts"
					class="flex items-center gap-3 rounded-xl px-3 py-2 text-xs font-medium transition-all {activeAnchor === 'accounts'
						? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
						: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-white'}"
					onclick={(e) => scrollToAnchor('accounts', e)}
				>
					<Server class="h-4 w-4 shrink-0 {activeAnchor === 'accounts' ? 'text-cyan-400 dark:text-cyan-600' : 'text-zinc-400 dark:text-zinc-500'}" />
					<div class="flex flex-col text-left">
						<span>Accounts</span>
						<span class="text-[10px] font-normal opacity-70">Server profiles & tokens</span>
					</div>
				</a>

				<a
					href="#server-profile"
					class="flex items-center gap-3 rounded-xl px-3 py-2 text-xs font-medium transition-all {activeAnchor === 'server-profile'
						? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
						: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-white'}"
					onclick={(e) => scrollToAnchor('server-profile', e)}
				>
					<User class="h-4 w-4 shrink-0 {activeAnchor === 'server-profile' ? 'text-indigo-400 dark:text-indigo-600' : 'text-zinc-400 dark:text-zinc-500'}" />
					<div class="flex flex-col text-left">
						<span>Server Profile</span>
						<span class="text-[10px] font-normal opacity-70">Display name & avatar</span>
					</div>
				</a>

				<a
					href="#appearance"
					class="flex items-center gap-3 rounded-xl px-3 py-2 text-xs font-medium transition-all {activeAnchor === 'appearance'
						? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
						: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-white'}"
					onclick={(e) => scrollToAnchor('appearance', e)}
				>
					<Sun class="h-4 w-4 shrink-0 {activeAnchor === 'appearance' ? 'text-amber-400 dark:text-amber-600' : 'text-zinc-400 dark:text-zinc-500'}" />
					<div class="flex flex-col text-left">
						<span>Appearance</span>
						<span class="text-[10px] font-normal opacity-70">Theme & interface styling</span>
					</div>
				</a>

				<a
					href="#extensions"
					class="flex items-center gap-3 rounded-xl px-3 py-2 text-xs font-medium transition-all {activeAnchor === 'extensions'
						? 'bg-zinc-900 text-white dark:bg-white dark:text-zinc-950 shadow-2xs'
						: 'text-zinc-600 dark:text-zinc-400 hover:bg-zinc-200/60 dark:hover:bg-zinc-800/60 hover:text-zinc-900 dark:hover:text-white'}"
					onclick={(e) => scrollToAnchor('extensions', e)}
				>
					<Package class="h-4 w-4 shrink-0 {activeAnchor === 'extensions' ? 'text-emerald-400 dark:text-emerald-600' : 'text-zinc-400 dark:text-zinc-500'}" />
					<div class="flex flex-col text-left">
						<span>Extensions & Storage</span>
						<span class="text-[10px] font-normal opacity-70">WASM, KV tables & disk</span>
					</div>
				</a>
			</nav>

			<div class="px-2 pt-4 border-t border-zinc-200/70 dark:border-zinc-800/80 text-[11px] text-zinc-400 dark:text-zinc-500">
				NeuxDOT System Configuration
			</div>
		</aside>

		<!-- 🖥️ Settings Content Area with Section Anchors -->
		<main id="settings-content" class="flex-1 overflow-y-auto px-8 py-8 md:px-12">
			<div class="max-w-3xl space-y-12">
				<!-- SECTION 1: ACCOUNTS -->
				<section id="accounts" class="scroll-mt-8 space-y-6">
					<div class="flex items-center justify-between">
						<div>
							<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Connected Server Accounts</h2>
							<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
								Manage multiple server profiles. Switching seamlessly updates the base URL and secure enclave credentials.
							</p>
						</div>
						<button
							type="button"
							class="flex items-center gap-1.5 rounded-xl bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all active:scale-98"
							onclick={() => {
								modalUsername = 'root';
								modalPassword = '';
								isAddModalOpen = true;
							}}
						>
							<Plus class="h-3.5 w-3.5" />
							<span>Add Account</span>
						</button>
					</div>

					<!-- Accounts List -->
					<div class="space-y-2">
						{#if authState.accounts.length === 0}
							<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
								No accounts connected yet. Click <strong>Add Account</strong> to log in with your root credentials from the server CLI.
							</div>
						{:else}
							{#each authState.accounts as acc (acc.id)}
								{@const isActive = authState.activeAccount?.id === acc.id}
								<div
									class="flex items-center justify-between rounded-2xl border p-4 transition-all {isActive
										? 'border-zinc-400 dark:border-zinc-600 bg-white dark:bg-zinc-900 shadow-xs'
										: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50 dark:hover:bg-zinc-800'}"
								>
									<div class="flex items-center gap-3">
										<div class="flex h-10 w-10 items-center justify-center rounded-xl bg-zinc-900 dark:bg-white font-bold text-white dark:text-zinc-950 text-sm shadow-2xs">
											{acc.username.substring(0, 2).toUpperCase()}
										</div>
										<div>
											<div class="flex items-center gap-2">
												<span class="text-sm font-semibold text-zinc-900 dark:text-white">{acc.username}</span>
												{#if isActive}
													<span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
														Active
													</span>
												{/if}
											</div>
											<span class="text-xs font-mono text-zinc-400 dark:text-zinc-500">{acc.serverUrl}</span>
										</div>
									</div>

									<div class="flex items-center gap-2">
										{#if !isActive}
											<button
												type="button"
												class="rounded-xl bg-zinc-100 dark:bg-zinc-800 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors shadow-2xs"
												onclick={() => authState.switchAccount(acc)}
											>
												Switch To
											</button>
										{/if}
										<button
											type="button"
											class="rounded-lg p-1.5 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
											onclick={() => authState.removeAccount(acc.id)}
											title="Remove account"
											aria-label="Remove account"
										>
											<Trash2 class="h-4 w-4" />
										</button>
									</div>
								</div>
							{/each}
						{/if}
					</div>
				</section>

				<hr class="border-zinc-200/80 dark:border-zinc-800/80" />

				<!-- SECTION 2: SERVER PROFILE -->
				<section id="server-profile" class="scroll-mt-8 space-y-6">
					<div>
						<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">Server Profile Settings</h2>
						<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
							Customize your display name and avatar color/shape stored on the backend server.
						</p>
					</div>

					<div class="space-y-4 max-w-md">
						<!-- Display Name -->
						<div>
							<label for="display-name" class="block text-xs font-medium text-zinc-700 dark:text-zinc-300 mb-1">
								Display Name
							</label>
							<input
								id="display-name"
								type="text"
								bind:value={profileDisplayName}
								placeholder="Your Name or Handle"
								class="w-full rounded-xl border border-zinc-200 dark:border-zinc-750 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-medium text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
							/>
						</div>

						<div class="flex items-center gap-3 pt-2">
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
				</section>

				<hr class="border-zinc-200/80 dark:border-zinc-800/80" />

				<!-- SECTION 3: APPEARANCE -->
				<section id="appearance" class="scroll-mt-8 space-y-6">
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

				<hr class="border-zinc-200/80 dark:border-zinc-800/80" />

				<!-- SECTION 4: EXTENSIONS & STORAGE -->
				<section id="extensions" class="scroll-mt-8 space-y-6">
					<div class="flex items-center justify-between">
						<div>
							<h2 class="text-sm font-semibold text-zinc-900 dark:text-white">
								WASM Extensions & Isolated Storage
							</h2>
							<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5">
								Install compiled WASM binaries. Each extension has its own isolated KV store and disk filesystem partitioned by WASM hash.
							</p>
						</div>

						<button
							type="button"
							class="flex items-center gap-1.5 rounded-xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-50 dark:hover:bg-zinc-800 transition-colors shadow-2xs"
							onclick={loadExtensions}
							disabled={isLoadingExtensions}
							title="Refresh extensions list"
						>
							<RefreshCw class="h-3.5 w-3.5 {isLoadingExtensions ? 'animate-spin' : ''}" />
							<span>Refresh</span>
						</button>
					</div>

					<!-- Installation Area (File Upload & URL) -->
					<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/50 p-4 space-y-3 shadow-xs">
						<span class="block text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
							Install New Extension
						</span>

						<div class="grid grid-cols-1 md:grid-cols-2 gap-3 pt-1">
							<!-- Upload File -->
							<div class="flex flex-col justify-between rounded-xl border border-dashed border-zinc-300 dark:border-zinc-750 p-3 bg-zinc-50/50 dark:bg-zinc-950/40">
								<div>
									<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
										<Upload class="h-4 w-4 text-emerald-500" />
										<span>Select .wasm binary</span>
									</div>
									<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-1">
										Upload compiled WebAssembly file directly from disk (e.g. from <code>./examples/chat-extension/target/wasm32-unknown-unknown/release/chat_extension.wasm</code>).
									</p>
								</div>

								<div class="pt-3">
									<input
										type="file"
										accept=".wasm"
										class="hidden"
										bind:this={fileInputRef}
										onchange={handleUploadWasmFile}
									/>
									<button
										type="button"
										class="w-full flex items-center justify-center gap-1.5 rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-40"
										onclick={() => fileInputRef?.click()}
										disabled={isUploadingWasm}
									>
										<Upload class="h-3.5 w-3.5" />
										<span>{isUploadingWasm ? 'Uploading...' : 'Choose .wasm File'}</span>
									</button>
								</div>
							</div>

							<!-- Install via URL -->
							<div class="flex flex-col justify-between rounded-xl border border-zinc-200 dark:border-zinc-800 p-3 bg-zinc-50/50 dark:bg-zinc-950/40">
								<div>
									<div class="flex items-center gap-2 text-xs font-medium text-zinc-900 dark:text-white">
										<ExternalLink class="h-4 w-4 text-cyan-500" />
										<span>Download from URL</span>
									</div>
									<p class="text-[11px] text-zinc-500 dark:text-zinc-400 mt-1">
										Download a remote .wasm binary directly over HTTP/HTTPS.
									</p>
								</div>

								<div class="pt-3 flex gap-2">
									<input
										type="url"
										bind:value={installUrlInput}
										placeholder="https://.../module.wasm"
										class="flex-1 rounded-lg border border-zinc-200 dark:border-zinc-750 bg-white dark:bg-zinc-900 px-2.5 py-1.5 text-xs text-zinc-900 dark:text-white placeholder:text-zinc-400 focus:outline-none"
									/>
									<button
										type="button"
										class="rounded-lg bg-zinc-900 hover:bg-zinc-800 dark:bg-white dark:hover:bg-zinc-100 px-3 py-1.5 text-xs font-medium text-white dark:text-zinc-950 shadow-xs transition-all disabled:opacity-40"
										onclick={handleInstallUrl}
										disabled={!installUrlInput.trim() || isInstallingUrl}
									>
										{isInstallingUrl ? '...' : 'Install'}
									</button>
								</div>
							</div>
						</div>
					</div>

					<!-- Installed Extensions List -->
					<div class="space-y-3">
						<span class="block text-xs font-semibold uppercase tracking-wider text-zinc-700 dark:text-zinc-300">
							Installed Extensions ({extensionsList.length})
						</span>

						{#if extensionsList.length === 0}
							<div class="rounded-2xl border border-dashed border-zinc-200 dark:border-zinc-800 p-8 text-center text-xs text-zinc-400 dark:text-zinc-500">
								No extensions installed yet. Upload your compiled <code>chat_extension.wasm</code> above to test.
							</div>
						{:else}
							<div class="grid grid-cols-1 gap-3">
								{#each extensionsList as ext (ext.id)}
									{@const isSelected = selectedExt?.id === ext.id}
									<div
										class="rounded-2xl border p-4 transition-all {isSelected
											? 'border-emerald-500/60 dark:border-emerald-500/60 bg-emerald-50/20 dark:bg-emerald-950/10 shadow-xs'
											: 'border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900/40 hover:bg-zinc-50 dark:hover:bg-zinc-800/40'}"
									>
										<div class="flex items-start justify-between">
											<div class="flex items-start gap-3">
												<div class="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border border-emerald-500/20 font-bold text-sm">
													<Package class="h-5 w-5" />
												</div>
												<div>
													<div class="flex items-center gap-2">
														<span class="text-sm font-semibold text-zinc-900 dark:text-white">
															{ext.name}
														</span>
														<span class="rounded-full bg-zinc-100 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-mono text-zinc-500">
															v{ext.version}
														</span>
														{#if ext.is_active}
															<span class="rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-medium text-emerald-600 dark:text-emerald-400 border border-emerald-500/20">
																Active
															</span>
														{:else}
															<span class="rounded-full bg-zinc-200/60 dark:bg-zinc-800 px-2 py-0.5 text-[10px] font-medium text-zinc-500">
																Disabled
															</span>
														{/if}
													</div>

													{#if ext.description}
														<p class="text-xs text-zinc-500 dark:text-zinc-400 mt-1">
															{ext.description}
														</p>
													{/if}

													<!-- Metadata Badges -->
													<div class="mt-2 flex flex-wrap items-center gap-2">
														<span class="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-100 dark:bg-zinc-800 text-zinc-600 dark:text-zinc-400 border border-zinc-200 dark:border-zinc-700">
															<HardDrive class="h-3 w-3" />
															{formatBytes(ext.storage_bytes)}
														</span>
														<span
															class="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700"
															title={ext.wasm_hash}
														>
															sha256: {ext.wasm_hash.substring(0, 10)}...
														</span>
														{#if ext.manifest.ui_hooks}
															{#each ext.manifest.ui_hooks as hook}
																<span class="inline-flex items-center rounded px-1.5 py-0.5 text-[10px] font-mono bg-indigo-50 dark:bg-indigo-950/60 text-indigo-700 dark:text-indigo-300 border border-indigo-200 dark:border-indigo-800">
																	Slot: #{hook.target_id}
																</span>
															{/each}
														{/if}
													</div>
												</div>
											</div>

											<div class="flex items-center gap-2">
												<!-- Active Toggle -->
												<button
													type="button"
													aria-label="Toggle extension active state"
													class="relative inline-flex h-5 w-9 shrink-0 cursor-pointer rounded-full border-2 border-transparent transition-colors duration-200 ease-in-out focus:outline-none {ext.is_active
														? 'bg-emerald-500'
														: 'bg-zinc-300 dark:bg-zinc-700'}"
													onclick={() => toggleExtensionActive(ext)}
												>
													<span
														class="pointer-events-none inline-block h-4 w-4 transform rounded-full bg-white shadow ring-0 transition duration-200 ease-in-out {ext.is_active
															? 'translate-x-4'
															: 'translate-x-0'}"
													></span>
												</button>

												<!-- Inspect Storage & KV Button -->
												<button
													type="button"
													class="rounded-xl border border-zinc-200 dark:border-zinc-750 px-3 py-1.5 text-xs font-medium text-zinc-700 dark:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors shadow-2xs flex items-center gap-1.5"
													onclick={() => selectExtensionForInspect(ext)}
												>
													<Database class="h-3.5 w-3.5 text-cyan-500" />
													<span>Inspect Storage</span>
												</button>

												<!-- Delete Button -->
												<button
													type="button"
													class="rounded-lg p-1.5 text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 hover:text-red-500 transition-colors"
													onclick={() => deleteExtension(ext.id)}
													title="Uninstall extension"
													aria-label="Uninstall extension"
												>
													<Trash2 class="h-4 w-4" />
												</button>
											</div>
										</div>
									</div>
								{/each}
							</div>
						{/if}
					</div>

					<!-- Inspector Drawer: Storage Breakdown & KV Table -->
					{#if selectedExt}
						<div class="rounded-2xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-5 space-y-5 shadow-xs">
							<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
								<div class="flex items-center gap-2">
									<Database class="h-4 w-4 text-cyan-500" />
									<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
										Storage Inspector: {selectedExt.name}
									</h3>
								</div>

								<div class="flex items-center gap-3">
									<button
										type="button"
										class="text-xs text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 flex items-center gap-1"
										onclick={() => loadInspectorData(selectedExt!)}
									>
										<RefreshCw class="h-3 w-3 {isLoadingInspector ? 'animate-spin' : ''}" />
										<span>Refresh Data</span>
									</button>
									<button
										type="button"
										class="text-zinc-400 hover:text-zinc-700 dark:hover:text-white"
										onclick={() => (selectedExt = null)}
										aria-label="Close inspector"
									>
										<X class="h-4 w-4" />
									</button>
								</div>
							</div>

							<!-- Storage Resource Stats -->
							<div class="grid grid-cols-3 gap-3 text-xs">
								<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
									<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">Total Storage</span>
									<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
										{formatBytes(inspectingStorage?.total_storage_bytes || selectedExt.storage_bytes)}
									</span>
								</div>
								<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
									<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">KV Entries</span>
									<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
										{inspectingKv.length} keys
									</span>
								</div>
								<div class="rounded-xl border border-zinc-100 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3">
									<span class="text-zinc-400 block text-[10px] uppercase tracking-wider">Disk Files</span>
									<span class="text-sm font-semibold font-mono text-zinc-900 dark:text-white mt-0.5 block">
										{inspectingStorage?.files.length || 0} files
									</span>
								</div>
							</div>

							<!-- Isolated Filesystem Files -->
							{#if inspectingStorage && inspectingStorage.files.length > 0}
								<div class="space-y-2">
									<span class="block text-xs font-semibold text-zinc-800 dark:text-zinc-200">
										Isolated Filesystem Files (<code>data/extensions/{selectedExt.wasm_hash.substring(0, 12)}.../</code>)
									</span>
									<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-950/50 divide-y divide-zinc-100 dark:divide-zinc-800/80">
										{#each inspectingStorage.files as file}
											<div class="flex items-center justify-between p-2.5 text-xs font-mono">
												<div class="flex items-center gap-2 truncate">
													<FileText class="h-3.5 w-3.5 text-zinc-400 shrink-0" />
													<span class="text-zinc-800 dark:text-zinc-200 truncate">{file.name}</span>
												</div>
												<span class="text-zinc-400 text-[11px] shrink-0 ml-2">
													{formatBytes(file.size_bytes)}
												</span>
											</div>
										{/each}
									</div>
								</div>
							{/if}

							<!-- KV Table -->
							<div class="space-y-2">
								<span class="block text-xs font-semibold text-zinc-800 dark:text-zinc-200">
									Host-Managed KV Store Table
								</span>

								{#if inspectingKv.length === 0}
									<div class="rounded-xl border border-dashed border-zinc-200 dark:border-zinc-800 p-6 text-center text-xs text-zinc-400">
										No KV keys stored by this extension yet.
									</div>
								{:else}
									<div class="overflow-hidden rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-white dark:bg-zinc-950 shadow-2xs">
										<table class="w-full text-left text-xs font-mono">
											<thead class="border-b border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-900 text-zinc-500 font-sans text-[11px]">
												<tr>
													<th class="p-2.5 pl-3">Key</th>
													<th class="p-2.5">Value (JSON/String)</th>
													<th class="p-2.5">Updated</th>
													<th class="p-2.5 text-right pr-3">Action</th>
												</tr>
											</thead>
											<tbody class="divide-y divide-zinc-100 dark:divide-zinc-800/80">
												{#each inspectingKv as item}
													<tr class="hover:bg-zinc-50 dark:hover:bg-zinc-900/40 transition-colors">
														<td class="p-2.5 pl-3 font-semibold text-indigo-600 dark:text-indigo-400">
															{item.key}
														</td>
														<td class="p-2.5 max-w-xs truncate text-zinc-700 dark:text-zinc-300">
															{item.value}
														</td>
														<td class="p-2.5 text-[11px] text-zinc-400 font-sans">
															{item.updated_at ? new Date(item.updated_at).toLocaleTimeString() : 'N/A'}
														</td>
														<td class="p-2.5 text-right pr-3 font-sans">
															<button
																type="button"
																class="text-zinc-400 hover:text-red-500 transition-colors"
																onclick={() => deleteKvKey(item.key)}
																title="Delete KV key"
															>
																<Trash2 class="h-3.5 w-3.5 inline" />
															</button>
														</td>
													</tr>
												{/each}
											</tbody>
										</table>
									</div>
								{/if}
							</div>
						</div>
					{/if}
				</section>
			</div>
		</main>
	</div>
</div>

<!-- Modal: Add / Log In to Account -->
{#if isAddModalOpen}
	<div
		id="modal-add-account"
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 dark:bg-black/70 backdrop-blur-sm p-4"
	>
		<div class="w-full max-w-md rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<h3 class="text-sm font-semibold text-zinc-900 dark:text-white">
					Log In to Account
				</h3>
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
						placeholder="e.g. root"
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
						placeholder="Enter root password from server terminal"
						class="w-full rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 px-3 py-2 text-xs font-mono text-zinc-900 dark:text-white focus:border-zinc-400 dark:focus:border-zinc-600 focus:bg-white dark:focus:bg-zinc-950 focus:outline-none"
						required
					/>
				</div>

				<div class="rounded-xl border border-zinc-200/80 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950/60 p-3 text-[11px] text-zinc-600 dark:text-zinc-400">
					🔑 The server CLI displays the <strong>root</strong> user and password once per run. Copy and paste it here.
				</div>

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

{#if isComparingUpdate && pendingUpdateInspection}
	<div class="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-xs p-4">
		<div class="w-full max-w-lg rounded-2xl border border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 p-6 shadow-2xl space-y-4">
			<div class="flex items-center justify-between border-b border-zinc-100 dark:border-zinc-800 pb-3">
				<div class="flex items-center gap-2">
					<div class="flex h-8 w-8 items-center justify-center rounded-xl bg-indigo-500/10 text-indigo-600 dark:text-indigo-400 border border-indigo-500/20">
						<Package class="h-4 w-4" />
					</div>
					<div>
						<h3 class="text-sm font-semibold text-zinc-900 dark:text-zinc-100">Extension Update Comparison</h3>
						<p class="text-[11px] text-zinc-500 dark:text-zinc-400">Review changes before replacing the existing extension binary</p>
					</div>
				</div>
				<button
					type="button"
					class="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200"
					onclick={cancelExtensionUpdate}
				>
					<X class="h-4 w-4" />
				</button>
			</div>

			<!-- Comparison Cards -->
			<div class="grid grid-cols-2 gap-3 text-xs">
				<!-- Old Extension Info -->
				<div class="rounded-xl border border-zinc-200 dark:border-zinc-800 bg-zinc-50 dark:bg-zinc-950 p-3 space-y-2">
					<div class="flex items-center justify-between">
						<span class="font-semibold text-zinc-700 dark:text-zinc-300">Installed Version</span>
						<span class="rounded px-1.5 py-0.5 text-[10px] font-mono bg-zinc-200 dark:bg-zinc-800 text-zinc-700 dark:text-zinc-300">v{pendingUpdateInspection.existing.version}</span>
					</div>
					<div class="space-y-1 text-[11px] text-zinc-500 dark:text-zinc-400">
						<div><span class="text-zinc-400">Name:</span> <strong class="text-zinc-800 dark:text-zinc-200">{pendingUpdateInspection.existing.name}</strong></div>
						<div><span class="text-zinc-400">Hash:</span> <code class="font-mono text-[10px]">{pendingUpdateInspection.existing.wasm_hash.slice(0, 12)}...</code></div>
						<div><span class="text-zinc-400">Storage:</span> <span class="font-mono text-indigo-600 dark:text-indigo-400">{formatBytes(pendingUpdateInspection.existing.storage_bytes)}</span></div>
					</div>
				</div>

				<!-- New Extension Info -->
				<div class="rounded-xl border border-indigo-500/30 bg-indigo-50/50 dark:bg-indigo-950/20 p-3 space-y-2">
					<div class="flex items-center justify-between">
						<span class="font-semibold text-indigo-700 dark:text-indigo-300">Incoming Version</span>
						<span class="rounded px-1.5 py-0.5 text-[10px] font-mono bg-indigo-500/20 text-indigo-700 dark:text-indigo-300">v{pendingUpdateInspection.version}</span>
					</div>
					<div class="space-y-1 text-[11px] text-zinc-500 dark:text-zinc-400">
						<div><span class="text-zinc-400">Name:</span> <strong class="text-zinc-800 dark:text-zinc-200">{pendingUpdateInspection.name}</strong></div>
						<div><span class="text-zinc-400">Hash:</span> <code class="font-mono text-[10px]">{pendingUpdateInspection.wasm_hash.slice(0, 12)}...</code></div>
						<div><span class="text-zinc-400">Status:</span> <span class="text-emerald-600 dark:text-emerald-400 font-medium">Ready to apply</span></div>
					</div>
				</div>
			</div>

			<!-- Zero-Trust Migration Notice -->
			<div class="rounded-xl border border-emerald-500/20 bg-emerald-500/5 p-3 text-[11px] text-emerald-800 dark:text-emerald-300 flex items-start gap-2">
				<ShieldCheck class="h-4 w-4 shrink-0 mt-0.5" />
				<div>
					<strong>Automatic Data Migration:</strong> All existing KV tables and persistent storage files will be seamlessly moved to the new hash directory. The old hash directory will be purged to keep storage clean.
				</div>
			</div>

			<!-- Action buttons -->
			<div class="flex items-center justify-end gap-2 pt-2 border-t border-zinc-100 dark:border-zinc-800">
				<button
					type="button"
					class="rounded-xl px-4 py-2 text-xs font-medium text-zinc-600 dark:text-zinc-400 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
					onclick={cancelExtensionUpdate}
				>
					Cancel
				</button>
				<button
					type="button"
					class="rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white px-4 py-2 text-xs font-medium transition-all shadow-xs disabled:opacity-40"
					disabled={isUploadingWasm}
					onclick={confirmExtensionUpdate}
				>
					{isUploadingWasm ? 'Updating & Migrating...' : 'Confirm Update & Migrate Data'}
				</button>
			</div>
		</div>
	</div>
{/if}
