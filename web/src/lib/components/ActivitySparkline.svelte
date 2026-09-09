<script lang="ts">
	import type { DailyCount } from '$lib/api';

	let { data }: { data: DailyCount[] } = $props();

	let max = $derived(Math.max(1, ...data.flatMap((d) => [d.issues, d.prs])));

	function shortDate(iso: string): string {
		const d = new Date(`${iso}T00:00:00`);
		return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
	}
</script>

<div class="flex items-end gap-1 h-16">
	{#each data as day (day.date)}
		<div
			class="flex-1 flex items-end justify-center gap-0.5 h-full"
			title="{shortDate(day.date)}: {day.issues} new issue{day.issues === 1 ? '' : 's'}, {day.prs} new PR{day.prs === 1 ? '' : 's'}"
		>
			<div
				class="flex-1 min-w-[3px] rounded-t-sm bg-primary/60"
				style="height: {Math.max(2, (day.issues / max) * 100)}%"
			></div>
			<div
				class="flex-1 min-w-[3px] rounded-t-sm bg-green-500/60"
				style="height: {Math.max(2, (day.prs / max) * 100)}%"
			></div>
		</div>
	{/each}
</div>
<div class="flex items-center justify-between mt-1.5 text-[0.65rem] text-muted-foreground">
	<span>{data.length > 0 ? shortDate(data[0].date) : ''}</span>
	<div class="flex items-center gap-3">
		<span class="flex items-center gap-1"><span class="inline-block w-2 h-2 rounded-sm bg-primary/60"></span>Issues</span>
		<span class="flex items-center gap-1"><span class="inline-block w-2 h-2 rounded-sm bg-green-500/60"></span>PRs</span>
	</div>
	<span>{data.length > 0 ? shortDate(data[data.length - 1].date) : ''}</span>
</div>
