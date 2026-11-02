import { redirect } from '@sveltejs/kit';

// /settings is only a hub — send visitors to the default section.
export function load() {
	redirect(307, '/settings/accounts');
}
