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

/** What the user picked: a fixed mode, or follow the OS. `theme` above
 *  always holds the resolved light/dark value that charts read. */
export type ThemeMode = Theme | 'system';

function readInitialMode(): ThemeMode {
	if (typeof window === 'undefined') return 'dark';
	try {
		const saved = localStorage.getItem('wshm-theme-mode');
		if (saved === 'light' || saved === 'dark' || saved === 'system') return saved;
	} catch {
		// ignore
	}
	return readInitialTheme();
}

export const themeMode = writable<ThemeMode>(readInitialMode());

let systemQuery: MediaQueryList | null = null;
function applySystemTheme() {
	if (systemQuery) theme.set(systemQuery.matches ? 'dark' : 'light');
}

themeMode.subscribe((mode) => {
	if (typeof window === 'undefined') return;
	try {
		localStorage.setItem('wshm-theme-mode', mode);
	} catch {
		// ignore
	}
	systemQuery?.removeEventListener('change', applySystemTheme);
	systemQuery = null;
	if (mode === 'system') {
		systemQuery = window.matchMedia('(prefers-color-scheme: dark)');
		systemQuery.addEventListener('change', applySystemTheme);
		applySystemTheme();
	} else {
		theme.set(mode);
	}
});

/** Color theme (accent and surfaces), independent of light/dark. */
export const PALETTES = [
	'default',
	'violet',
	'emerald',
	'amber',
	'rose',
	'graphite',
	'midnight',
	'contrast'
] as const;
export type Palette = (typeof PALETTES)[number];

function readInitialPalette(): Palette {
	if (typeof window === 'undefined') return 'default';
	try {
		const saved = localStorage.getItem('wshm-palette') as Palette | null;
		if (saved && (PALETTES as readonly string[]).includes(saved)) return saved;
	} catch {
		// ignore
	}
	return 'default';
}

export const palette = writable<Palette>(readInitialPalette());

palette.subscribe((value) => {
	if (typeof document === 'undefined') return;
	try {
		localStorage.setItem('wshm-palette', value);
	} catch {
		// ignore
	}
	const root = document.documentElement;
	if (value === 'default') delete root.dataset.palette;
	else root.dataset.palette = value;
});

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
