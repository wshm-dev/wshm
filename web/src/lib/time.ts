/** Human-friendly relative time for RFC3339 timestamps.
 *
 * The daemon emits nanosecond-precision RFC3339 (`2026-05-12T09:53:49.843978501+00:00`),
 * which is unreadable in the UI. Render "31 days ago" instead and keep the
 * exact timestamp available for a `title` tooltip via `exactTime`.
 */
export function timeAgo(rfc3339: string | null | undefined): string {
	if (!rfc3339) return 'never';
	const t = new Date(rfc3339).getTime();
	if (Number.isNaN(t)) return rfc3339;
	const diff = Date.now() - t;
	if (diff < 0) return 'just now';
	const s = Math.floor(diff / 1000);
	if (s < 45) return 'just now';
	const m = Math.floor(s / 60);
	if (m < 60) return `${m} minute${m === 1 ? '' : 's'} ago`;
	const h = Math.floor(m / 60);
	if (h < 24) return `${h} hour${h === 1 ? '' : 's'} ago`;
	const d = Math.floor(h / 24);
	if (d < 30) return `${d} day${d === 1 ? '' : 's'} ago`;
	const mo = Math.floor(d / 30);
	if (mo < 12) return `${mo} month${mo === 1 ? '' : 's'} ago`;
	const y = Math.floor(d / 365);
	return `${y} year${y === 1 ? '' : 's'} ago`;
}

/** Second-precision local rendering of an RFC3339 timestamp (for tooltips). */
export function exactTime(rfc3339: string | null | undefined): string {
	if (!rfc3339) return '';
	const d = new Date(rfc3339);
	if (Number.isNaN(d.getTime())) return rfc3339;
	return d.toLocaleString();
}
