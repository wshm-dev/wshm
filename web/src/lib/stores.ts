import { writable } from 'svelte/store';

/** Currently selected repo slug (e.g. "owner/name"), or null for "All repos". */
export const selectedRepo = writable<string | null>(null);
