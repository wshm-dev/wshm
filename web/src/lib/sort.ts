export type SortColumn = { key: string; asc: boolean };

export function multiSort<T>(data: T[], columns: SortColumn[]): T[] {
	if (columns.length === 0) return data;
	return [...data].sort((a, b) => {
		for (const col of columns) {
			const av = (a as Record<string, unknown>)[col.key];
			const bv = (b as Record<string, unknown>)[col.key];
			let cmp = 0;
			if (av == null && bv == null) cmp = 0;
			else if (av == null) cmp = -1;
			else if (bv == null) cmp = 1;
			else if (typeof av === 'number' && typeof bv === 'number') cmp = av - bv;
			else cmp = String(av).localeCompare(String(bv));
			if (cmp !== 0) return col.asc ? cmp : -cmp;
		}
		return 0;
	});
}

export function toggleSort(columns: SortColumn[], key: string, shiftKey: boolean): SortColumn[] {
	if (shiftKey) {
		const idx = columns.findIndex((c) => c.key === key);
		if (idx >= 0) {
			const updated = [...columns];
			updated[idx] = { key, asc: !updated[idx].asc };
			return updated;
		}
		return [...columns, { key, asc: true }];
	}
	const existing = columns.find((c) => c.key === key);
	if (existing && columns.length === 1) {
		return [{ key, asc: !existing.asc }];
	}
	return [{ key, asc: true }];
}

export function sortArrow(columns: SortColumn[], key: string): string {
	const col = columns.find((c) => c.key === key);
	if (!col) return '';
	return col.asc ? 'v' : '^';
}

export function sortIndex(columns: SortColumn[], key: string): number {
	if (columns.length <= 1) return -1;
	const idx = columns.findIndex((c) => c.key === key);
	return idx >= 0 ? idx + 1 : -1;
}

export function sortArrowClass(columns: SortColumn[], key: string): string {
	return columns.some((c) => c.key === key) ? 'sort-arrow active' : 'sort-arrow';
}
