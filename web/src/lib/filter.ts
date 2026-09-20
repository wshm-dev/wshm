export function matchesFilter(value: unknown, filter: string): boolean {
	if (!filter || filter.trim() === '') return true;
	const f = filter.trim();

	// Presence filters: `!` = empty/null, `*` = non-empty. Lets quick-filter
	// chips express "untriaged" / "unanalyzed" (a null field), which a
	// substring match can't.
	const empty = value === null || value === undefined || String(value).trim() === '';
	if (f === '!') return empty;
	if (f === '*') return !empty;

	// Exact (case-insensitive) match: `=no` must not also match "unknown"
	// the way the default substring match does. Numeric `=N` is handled by
	// the operator branch below.
	if (f.startsWith('=') && !/^=\s*-?\d/.test(f)) {
		return String(value ?? '').toLowerCase() === f.slice(1).trim().toLowerCase();
	}

	// Numeric operator filters: >N, <N, =N, >=N, <=N
	const numMatch = f.match(/^([><=!]{1,2})\s*(-?\d+\.?\d*)$/);
	if (numMatch) {
		const op = numMatch[1];
		const threshold = parseFloat(numMatch[2]);
		const num = typeof value === 'number' ? value : parseFloat(String(value ?? ''));
		if (isNaN(num)) return false;
		switch (op) {
			case '>': return num > threshold;
			case '<': return num < threshold;
			case '=': return num === threshold;
			case '>=': return num >= threshold;
			case '<=': return num <= threshold;
			case '!=': return num !== threshold;
			default: return false;
		}
	}

	// Default: case-insensitive substring match
	const strVal = value == null ? '' : String(value);
	return strVal.toLowerCase().includes(f.toLowerCase());
}

/**
 * Compute the sorted distinct values of a field across `items`. Used to
 * populate `<FilterSelect>` dropdowns from real data — never hardcode
 * enum lists in components, since priority / category / risk are
 * produced by the AI provider and may evolve over time.
 */
export function distinctValues<T>(items: T[], key: keyof T): string[] {
	const set = new Set<string>();
	for (const item of items) {
		const v = item[key] as unknown;
		if (v === null || v === undefined) continue;
		const s = String(v).trim();
		if (s !== '') set.add(s);
	}
	return [...set].sort();
}

/** A one-click preset for the filter row: `patch` is merged into the page's
 *  `filters` state (same keys, same syntax as typing into the filter inputs,
 *  so `=yes`, `!`, `>30` all work). Rendered by `QuickFilters.svelte`. */
export interface QuickChip {
	label: string;
	patch: Record<string, string>;
	/** Rows on the current page this chip would keep — shown next to the label. */
	count?: number;
}

export function applyFilters<T>(data: T[], filters: Record<string, string>): T[] {
	const activeFilters = Object.entries(filters).filter(([, v]) => v.trim() !== '');
	if (activeFilters.length === 0) return data;
	return data.filter((item) =>
		activeFilters.every(([key, filter]) =>
			matchesFilter((item as Record<string, unknown>)[key], filter)
		)
	);
}
