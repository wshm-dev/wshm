import { writable } from 'svelte/store';

/** Currently selected repo slug (e.g. "owner/name"), or null for "All repos". */
export const selectedRepo = writable<string | null>(null);

/**
 * Bumped by content pages when the user starts an immersive interaction (e.g.
 * manipulating the graph) to request the nav sidebar auto-collapse and free up
 * horizontal space. The layout watches this counter and folds the sidebar.
 */
export const collapseSidebarSignal = writable<number>(0);

export type Theme = 'dark' | 'light';

function readInitialTheme(): Theme {
	if (typeof window === 'undefined') return 'dark';
	try {
		const saved = localStorage.getItem('wshm-theme');
		if (saved === 'light' || saved === 'dark') return saved;
	} catch {
		// ignore
	}
	return 'dark';
}

export const theme = writable<Theme>(readInitialTheme());

theme.subscribe((value) => {
	if (typeof document === 'undefined') return;
	try {
		localStorage.setItem('wshm-theme', value);
	} catch {
		// ignore
	}
	const root = document.documentElement;
	if (value === 'dark') {
		root.classList.add('dark');
		root.classList.remove('light');
	} else {
		root.classList.add('light');
		root.classList.remove('dark');
	}
});
