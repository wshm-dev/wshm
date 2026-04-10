export const PAGE_SIZE = 50;

export function paginate<T>(data: T[], page: number, pageSize: number = PAGE_SIZE): T[] {
	const start = page * pageSize;
	return data.slice(start, start + pageSize);
}

export function totalPages(total: number, pageSize: number = PAGE_SIZE): number {
	return Math.max(1, Math.ceil(total / pageSize));
}
