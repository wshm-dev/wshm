<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, fetchIssues, type PullRequest, type Issue } from '$lib/api';
	import * as Tabs from '$lib/components/ui/tabs';
	import * as Dialog from '$lib/components/ui/dialog';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import LabelGraphPanel from '$lib/components/LabelGraphPanel.svelte';

	let prs = $state<PullRequest[]>([]);
	let issues = $state<Issue[]>([]);
	let prLoading = $state(true);
	let issueLoading = $state(true);

	// Aggregate history — daily snapshots. The trend endpoints are a Pro
	// feature; in OSS they 404, so we swallow errors and show "no history".
	type Point = Record<string, number | string>;
	let prTrend = $state<Point[]>([]);
	let issueTrend = $state<Point[]>([]);

	let token = 0;
	async function loadAll() {
		const mine = ++token;
		prLoading = true;
		issueLoading = true;
		// PRs
		try {
			const all: PullRequest[] = [];
			let offset = 0;
			for (;;) {
				const page = await fetchPulls({ limit: 250, offset });
				if (mine !== token) return;
				all.push(...page.items);
				if (all.length >= page.total || page.items.length === 0 || all.length >= 2000) break;
				offset += page.items.length;
			}
			prs = all;
		} catch {
			/* ignore */
		} finally {
			if (mine === token) prLoading = false;
		}
		// Issues
		try {
			const all: Issue[] = [];
			let offset = 0;
			for (;;) {
				const page = await fetchIssues({ limit: 250, offset });
				if (mine !== token) return;
				all.push(...page.items);
				if (all.length >= page.total || page.items.length === 0 || all.length >= 2000) break;
				offset += page.items.length;
			}
			issues = all;
		} catch {
			/* ignore */
		} finally {
			if (mine === token) issueLoading = false;
		}
		// Trends (best-effort)
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

	onMount(() => {
		loadAll();
		const unsub = selectedRepo.subscribe(() => loadAll());
		return unsub;
	});

	let prModal = $state(false);
	let activePr = $state<PullRequest | null>(null);
	let issueModal = $state(false);
	let activeIssue = $state<Issue | null>(null);

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
		Pull requests and issues clustered by label, filterable by domain — plus how the backlog evolved
		over time.
	</p>
</div>

<Tabs.Root value="pr">
	<Tabs.List class="mb-3">
		<Tabs.Trigger value="pr">PR graph</Tabs.Trigger>
		<Tabs.Trigger value="issue">Issue graph</Tabs.Trigger>
		<Tabs.Trigger value="history">History</Tabs.Trigger>
	</Tabs.List>

	<Tabs.Content value="pr">
		<LabelGraphPanel
			items={prs}
			loading={prLoading}
			emptyLabel="pull requests"
			onSelect={(p) => {
				activePr = p as unknown as PullRequest;
				prModal = true;
			}}
		/>
	</Tabs.Content>

	<Tabs.Content value="issue">
		<LabelGraphPanel
			items={issues}
			loading={issueLoading}
			emptyLabel="issues"
			onSelect={(i) => {
				activeIssue = i as unknown as Issue;
				issueModal = true;
			}}
		/>
	</Tabs.Content>

	<Tabs.Content value="history">
		<div class="grid gap-4 md:grid-cols-2">
			{#each [{ title: 'Open pull requests', points: prTrend, key: 'open', color: '#6366f1' }, { title: 'Open issues', points: issueTrend, key: 'total', color: '#10b981' }] as chart}
				<div class="rounded-lg border bg-card p-4">
					<div class="mb-2 flex items-baseline justify-between">
						<span class="text-sm font-medium">{chart.title}</span>
						<span class="mono text-xs text-muted-foreground">
							{chart.points.length ? `now ${maxOf(chart.points.slice(-1), chart.key)} · peak ${maxOf(chart.points, chart.key)}` : ''}
						</span>
					</div>
					{#if chart.points.length}
						<svg viewBox="0 0 300 90" class="w-full" preserveAspectRatio="none" height="90">
							<path d={pathFor(chart.points, chart.key, 300, 90)} fill="none" stroke={chart.color} stroke-width="2" />
						</svg>
						<div class="mt-1 flex justify-between text-[0.65rem] text-muted-foreground">
							<span>{chart.points[0]?.day ?? ''}</span>
							<span>{chart.points[chart.points.length - 1]?.day ?? ''}</span>
						</div>
					{:else}
						<p class="py-6 text-center text-xs text-muted-foreground">
							No history yet — daily snapshots accrue once the daemon has run for a few days
							(Pro insights).
						</p>
					{/if}
				</div>
			{/each}
		</div>
	</Tabs.Content>
</Tabs.Root>

<Dialog.Root bind:open={prModal}>
	<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
		<Dialog.Header>
			<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
				<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number}</span>
				<span class="truncate">{activePr?.title}</span>
			</Dialog.Title>
		</Dialog.Header>
		{#if activePr}
			<PrDetail pr={activePr} />
			<div class="text-right pt-2">
				<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">Open full page →</a>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={issueModal}>
	<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
		<Dialog.Header>
			<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
				<span class="mono text-muted-foreground text-sm font-normal">#{activeIssue?.number}</span>
				<span class="truncate">{activeIssue?.title}</span>
			</Dialog.Title>
		</Dialog.Header>
		{#if activeIssue}
			<IssueDetail issue={activeIssue} />
			<div class="text-right pt-2">
				<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:underline">Open full page →</a>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
