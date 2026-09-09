<script lang="ts">
	import type { DailyCount } from '$lib/api';

	let { data }: { data: DailyCount[] } = $props();

	// Fixed palette, assigned per-repo in a stable order (first-seen in the
	// data, which the backend emits in a consistent repo iteration order) so
	// a repo keeps the same color across reloads rather than shuffling with
	// whichever repo happens to have the most activity that day.
	const PALETTE = [
		'bg-primary/70',
		'bg-green-500/70',
		'bg-amber-500/70',
		'bg-pink-500/70',
		'bg-cyan-500/70',
		'bg-purple-500/70'
	];

	let repos = $derived([...new Set(data.map((d) => d.repo))]);
	let repoColor = $derived(new Map(repos.map((r, i) => [r, PALETTE[i % PALETTE.length]])));

	let dates = $derived([...new Set(data.map((d) => d.date))].sort());
	let byDate = $derived(
		dates.map((date) => ({
			date,
			entries: data
				.filter((d) => d.date === date)
				.map((d) => ({ ...d, total: d.issues + d.prs }))
				.filter((d) => d.total > 0)
		}))
	);
	let dayTotal = $derived((entries: { total: number }[]) => entries.reduce((sum, e) => sum + e.total, 0));
	let max = $derived(Math.max(1, ...byDate.map((d) => dayTotal(d.entries))));

	function shortDate(iso: string): string {
		const d = new Date(`${iso}T00:00:00`);
		return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
	}
</script>

<div class="flex items-end gap-1 h-16">
	{#each byDate as day (day.date)}
		{@const total = dayTotal(day.entries)}
		<div
			class="flex-1 flex flex-col-reverse h-full min-w-[6px] rounded-t-sm overflow-hidden"
			title="{shortDate(day.date)}: {day.entries.map((e) => `${e.repo} (${e.issues} issues, ${e.prs} PRs)`).join(', ') || 'no activity'}"
		>
			{#each day.entries as e (e.repo)}
				<div
					class="{repoColor.get(e.repo)} w-full"
					style="height: {(e.total / max) * 100}%"
				></div>
			{/each}
			{#if total === 0}
				<div class="w-full bg-muted-foreground/10" style="height: 2px"></div>
			{/if}
		</div>
	{/each}
</div>
<div class="flex items-center justify-between mt-1.5 text-[0.65rem] text-muted-foreground">
	<span>{dates.length > 0 ? shortDate(dates[0]) : ''}</span>
	<div class="flex flex-wrap items-center gap-x-3 gap-y-1 justify-end">
		{#each repos as repo}
			<span class="flex items-center gap-1">
				<span class="inline-block w-2 h-2 rounded-sm {repoColor.get(repo)}"></span>{repo}
			</span>
		{/each}
	</div>
</div>
