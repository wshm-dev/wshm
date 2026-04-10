import { writable } from 'svelte/store';

export interface ColorConfig {
	[key: string]: string;

	// Issue PR status
	noPr: string;
	hasPr: string;
	prReady: string;

	// Priority
	critical: string;
	high: string;
	medium: string;
	low: string;

	// Risk level (PRs)
	riskHigh: string;
	riskMedium: string;
	riskLow: string;

	// Category
	bug: string;
	feature: string;
	docs: string;
}

const STORAGE_KEY = 'wshm-color-config';

const defaults: ColorConfig = {
	noPr: '#6b2126',
	hasPr: '#1e3a5f',
	prReady: '#1a4731',

	critical: '#f85149',
	high: '#d29922',
	medium: '#58a6ff',
	low: '#8b949e',

	riskHigh: '#f85149',
	riskMedium: '#d29922',
	riskLow: '#3fb950',

	bug: '#f85149',
	feature: '#a371f7',
	docs: '#58a6ff',
};

function loadColors(): ColorConfig {
	try {
		const saved = localStorage.getItem(STORAGE_KEY);
		if (saved) {
			return { ...defaults, ...JSON.parse(saved) };
		}
	} catch {
		// ignore
	}
	return { ...defaults };
}

function createColorStore() {
	const store = writable<ColorConfig>(defaults);

	// Load from localStorage on init (deferred to avoid SSR issues)
	if (typeof window !== 'undefined') {
		store.set(loadColors());
	}

	return {
		...store,
		save(config: ColorConfig) {
			store.set(config);
			try {
				localStorage.setItem(STORAGE_KEY, JSON.stringify(config));
			} catch {
				// ignore
			}
		},
		reset() {
			store.set({ ...defaults });
			try {
				localStorage.removeItem(STORAGE_KEY);
			} catch {
				// ignore
			}
		},
		defaults,
	};
}

export const colorConfig = createColorStore();

// Helper: get row border color for an issue based on pr_status
export function prStatusBorder(config: ColorConfig, status: string): string {
	if (status === 'pr_ready') return config.prReady;
	if (status === 'has_pr') return config.hasPr;
	return config.noPr;
}

// Helper: get priority color
export function priorityColor(config: ColorConfig, priority: string | null): string {
	switch (priority?.toLowerCase()) {
		case 'critical': case 'p1-critical': return config.critical;
		case 'high': case 'p2-important': return config.high;
		case 'medium': case 'p3-nice-to-have': return config.medium;
		case 'low': return config.low;
		default: return config.low;
	}
}

// Helper: get risk color
export function riskColor(config: ColorConfig, risk: string | null): string {
	switch (risk?.toLowerCase()) {
		case 'high': return config.riskHigh;
		case 'medium': return config.riskMedium;
		case 'low': return config.riskLow;
		default: return '#8b949e';
	}
}

// Helper: get category color
export function categoryColor(config: ColorConfig, category: string | null): string {
	switch (category?.toLowerCase()) {
		case 'bug': return config.bug;
		case 'feature': case 'enhancement': return config.feature;
		case 'docs': case 'documentation': return config.docs;
		default: return '#8b949e';
	}
}
