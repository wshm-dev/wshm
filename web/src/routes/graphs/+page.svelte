<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo, collapseSidebarSignal } from '$lib/stores';
	import * as Tabs from '$lib/components/ui/tabs';
	import GroupGraphTab from '$lib/components/GroupGraphTab.svelte';

	// Aggregate history — daily snapshots. The trend endpoints are a Pro feature;
	// in OSS they 404, so we swallow errors and show "no history".
	type Point = Record<string, number | string>;
	let prTrend = $state<Point[]>([]);
	let issueTrend = $state<Point[]>([]);

	async function loadTrends() {
		try {
			const r = await fetch('/api/v1/pr-insights/trend');
			if (r.ok) prTrend = (await r.json()).points ?? [];
		} catch {
			/* ignore */
		}
		try {
			const r = await fetch('/api/v1/issue-insights/trend');
			if (r.ok) issueTrend = (await r.json()).points ?? [];
		} catch {
			/* ignore */
		}
	}

	// Current repo slug (null = all repos); passed to each graph tab.
	let repo = $state<string | null>(null);
	onMount(() => {
		const unsub = selectedRepo.subscribe((slug) => {
			repo = slug;
			loadTrends();
		});
		return unsub;
	});

	const collapseNav = () => collapseSidebarSignal.update((n) => n + 1);

	/** Build an SVG polyline path for `points[valueKey]` over their order. */
	function pathFor(points: Point[], key: string, w: number, h: number): string {
		const vals = points.map((p) => Number(p[key] ?? 0));
		if (vals.length === 0) return '';
		const max = Math.max(1, ...vals);
		const stepX = vals.length > 1 ? w / (vals.length - 1) : 0;
		return vals
			.map((v, i) => `${i === 0 ? 'M' : 'L'}${(i * stepX).toFixed(1)},${(h - (v / max) * h).toFixed(1)}`)
			.join(' ');
	}
	function maxOf(points: Point[], key: string): number {
		return Math.max(0, ...points.map((p) => Number(p[key] ?? 0)));
	}
</script>

<svelte:head>
	<title>wshm - Graphs</title>
</svelte:head>

<div class="mb-4">
	<h2 class="text-xl font-semibold text-foreground mb-1">Graphs</h2>
	<p class="text-sm text-muted-foreground">
		Pull requests and issues clustered by subject across the whole DB — grands groupes and their
		sous-groupes — plus how the backlog evolved over time.
	</p>
</div>

<Tabs.Root value="pr">
	<Tabs.List class="mb-3">
		<Tabs.Trigger value="pr">PR graph</Tabs.Trigger>
		<Tabs.Trigger value="issue">Issue graph</Tabs.Trigger>
		<Tabs.Trigger value="history">History</Tabs.Trigger>
	</Tabs.List>

	<Tabs.Content value="pr">
		<GroupGraphTab kind="pr" {repo} onInteract={collapseNav} />
	</Tabs.Content>

	<Tabs.Content value="issue">
		<GroupGraphTab kind="issue" {repo} onInteract={collapseNav} />
	</Tabs.Content>

	<Tabs.Content value="history">
		<div class="grid gap-4 md:grid-cols-2">
			{#each [{ title: 'Open pull requests', points: prTrend, key: 'open', color: '#6366f1' }, { title: 'Open issues', points: issueTrend, key: 'total', color: '#10b981' }] as chart}
				<div class="rounded-lg border bg-card p-4">
					<div class="mb-2 flex items-baseline justify-between">
						<span class="text-sm font-medium">{chart.title}</span>
						<span class="mono text-xs text-muted-foreground">
							{chart.points.length
								? `now ${maxOf(chart.points.slice(-1), chart.key)} · peak ${maxOf(chart.points, chart.key)}`
								: ''}
						</span>
					</div>
					{#if chart.points.length}
						<svg viewBox="0 0 300 90" class="w-full" preserveAspectRatio="none" height="90">
							<path
								d={pathFor(chart.points, chart.key, 300, 90)}
								fill="none"
								stroke={chart.color}
								stroke-width="2"
							/>
						</svg>
						<div class="mt-1 flex justify-between text-[0.65rem] text-muted-foreground">
							<span>{chart.points[0]?.day ?? ''}</span>
							<span>{chart.points[chart.points.length - 1]?.day ?? ''}</span>
						</div>
					{:else}
						<p class="py-6 text-center text-xs text-muted-foreground">
							No history yet — daily snapshots accrue once the daemon has run for a few days (Pro
							insights).
						</p>
					{/if}
				</div>
			{/each}
		</div>
	</Tabs.Content>
</Tabs.Root>
