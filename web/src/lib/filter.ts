export function matchesFilter(value: unknown, filter: string): boolean {
	if (!filter || filter.trim() === '') return true;
	const f = filter.trim();

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

export function applyFilters<T>(data: T[], filters: Record<string, string>): T[] {
	const activeFilters = Object.entries(filters).filter(([, v]) => v.trim() !== '');
	if (activeFilters.length === 0) return data;
	return data.filter((item) =>
		activeFilters.every(([key, filter]) =>
			matchesFilter((item as Record<string, unknown>)[key], filter)
		)
	);
}
