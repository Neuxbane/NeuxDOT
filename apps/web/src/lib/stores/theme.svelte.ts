class ThemeStore {
	isDark = $state<boolean>(
		typeof document !== 'undefined'
			? document.documentElement.classList.contains('dark')
			: true
	);

	constructor() {
		if (typeof window !== 'undefined') {
			const saved = localStorage.getItem('neuxdot_theme');
			if (saved !== null) {
				this.isDark = saved === 'dark';
			} else {
				this.isDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
			}
			this.apply();

			// Listen for OS theme preference changes if user hasn't explicitly set a preference
			try {
				window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
					if (localStorage.getItem('neuxdot_theme') === null) {
						this.isDark = e.matches;
						this.apply();
					}
				});
			} catch (_) {}
		}
	}

	toggle() {
		this.isDark = !this.isDark;
		this.apply();
	}

	set(dark: boolean) {
		this.isDark = dark;
		this.apply();
	}

	private apply() {
		if (typeof document === 'undefined') return;
		if (this.isDark) {
			document.documentElement.classList.add('dark');
			localStorage.setItem('neuxdot_theme', 'dark');
		} else {
			document.documentElement.classList.remove('dark');
			localStorage.setItem('neuxdot_theme', 'light');
		}
	}
}

export const themeStore = new ThemeStore();
