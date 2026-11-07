/** Layout helpers of the pipeline run graph (#234). */

export interface GraphStep {
	id: string;
	needs?: string[];
}

/**
 * Columns of a left-to-right flow: a step goes one column after the last
 * step it needs (needs always point to earlier steps), so steps that do not
 * depend on each other share a column and show side by side.
 */
export function layers<T extends GraphStep>(steps: T[]): T[][] {
	const col = new Map<string, number>();
	const out: T[][] = [];
	for (const s of steps) {
		let c = 0;
		for (const n of s.needs ?? []) {
			const d = col.get(n);
			if (d !== undefined) c = Math.max(c, d + 1);
		}
		col.set(s.id, c);
		(out[c] ??= []).push(s);
	}
	return out.filter((l) => l && l.length > 0);
}

export function formatDuration(ms: number | null | undefined): string {
	if (ms === null || ms === undefined) return '—';
	if (ms < 1000) return `${ms} ms`;
	const s = ms / 1000;
	if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
	const m = Math.floor(s / 60);
	return `${m} min ${Math.round(s % 60)} s`;
}

/** Short text of a `when` condition, e.g. `pr · labels: docs · review_prs`. */
export function describeCondition(when: Record<string, unknown> | undefined): string {
	if (!when) return '';
	const parts: string[] = [];
	for (const [k, v] of Object.entries(when)) {
		if (k === 'item') parts.push(String(v));
		else if (k === 'features' && Array.isArray(v)) parts.push(v.join(', '));
		else if (k === 'repo_filters') parts.push('repo filters');
		else if (Array.isArray(v)) parts.push(`${k.replace(/_/g, ' ')}: ${v.join(', ')}`);
		else parts.push(`${k}: ${String(v)}`);
	}
	return parts.join(' · ');
}
