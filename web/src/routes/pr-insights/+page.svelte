<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { selectedRepo, itemRef } from '$lib/stores';
	import {
		fetchPrInsights,
		fetchPrTrend,
		type PrInsightItem,
		type PrInsights,
		type PrTrendPoint
	} from '$lib/api';
	import { t } from '$lib/i18n';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Table from '$lib/components/ui/table';
	import * as Tabs from '$lib/components/ui/tabs';
	import DoughnutChart, { type Segment } from '$lib/components/DoughnutChart.svelte';
	import LineChart from '$lib/components/LineChart.svelte';

	let translate = $state<(k: string) => string>((k) => k);
	t.subscribe((fn) => (translate = fn));

	let data = $state<PrInsights | null>(null);
	let trend = $state<PrTrendPoint[]>([]);
	let error: string | null = $state(null);
	let loading = $state(true);

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		loading = true;
		error = null;
		try {
			const res = await fetchPrInsights();
			if (myToken !== loadToken) return;
			data = res;
		} catch (e) {
			if (myToken !== loadToken) return;
			const msg = e instanceof Error ? e.message : 'Failed to load';
			error = msg;
		}
		// Trend history is best-effort — a failure here must not blank the page.
		try {
			const tr = await fetchPrTrend();
			if (myToken === loadToken) trend = tr.points;
		} catch {
			/* ignore */
		}
		loading = false;
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => load());
		return unsub;
	});

	let items = $derived(data?.items ?? []);
	let defaults = $derived(new Set(data?.default_branches ?? []));

	let conflictItems = $derived(items.filter((p) => p.status === 'conflict'));
	let mergeableItems = $derived(items.filter((p) => p.status === 'mergeable'));
	let unknownCount = $derived(items.filter((p) => p.status === 'unknown').length);

	let toDefaultCount = $derived(
		items.filter((p) => p.base_ref != null && defaults.has(p.base_ref)).length
	);
	let toOtherCount = $derived(items.length - toDefaultCount);

	// Target-branch breakdown: count per base_ref, default branches first.
	type BranchGroup = { branch: string; count: number; isDefault: boolean };
	let branchGroups = $derived.by<BranchGroup[]>(() => {
		const counts = new Map<string, number>();
		for (const p of items) {
			const b = p.base_ref ?? '(unknown)';
			counts.set(b, (counts.get(b) ?? 0) + 1);
		}
		const groups = [...counts.entries()].map(([branch, count]) => ({
			branch,
			count,
			isDefault: defaults.has(branch)
		}));
		groups.sort((a, b) => {
			if (a.isDefault !== b.isDefault) return a.isDefault ? -1 : 1;
			return b.count - a.count;
		});
		return groups;
	});

	const BRANCH_PALETTE = [
		'#3b82f6', '#8b5cf6', '#ec4899', '#f59e0b', '#14b8a6',
		'#06b6d4', '#a3e635', '#f97316', '#64748b'
	];

	// Each segment carries a `meta` query string — clicking a slice navigates
	// to /pr-insights/list with that filter applied.
	let mergeabilitySegments = $derived<Segment[]>(
		[
			{
				label: translate('prInsights.legend.mergeable'),
				value: mergeableItems.length,
				color: '#22c55e',
				meta: 'status=mergeable'
			},
			{
				label: translate('prInsights.legend.conflict'),
				value: conflictItems.length,
				color: '#ef4444',
				meta: 'status=conflict'
			},
			{
				label: translate('prInsights.legend.unknown'),
				value: unknownCount,
				color: '#6b7280',
				meta: 'status=unknown'
			}
		].filter((s) => s.value > 0)
	);

	let branchSegments = $derived<Segment[]>(
		branchGroups.map((g, i) => ({
			label: g.branch,
			value: g.count,
			color: BRANCH_PALETTE[i % BRANCH_PALETTE.length],
			meta: `branch=${encodeURIComponent(g.branch)}`
		}))
	);

	// Staleness: bucket PRs by days since last activity (`updated_at`).
	// Colour ramps green (fresh) -> red (stale).
	function daysSince(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}
	const STALENESS_BUCKETS = [
		{ key: 'lt1w', color: '#22c55e' },
		{ key: '1to4w', color: '#84cc16' },
		{ key: '1to3m', color: '#f59e0b' },
		{ key: 'gt3m', color: '#ef4444' }
	];
	function stalenessBucket(days: number): number {
		if (days < 7) return 0;
		if (days < 30) return 1;
		if (days < 90) return 2;
		return 3;
	}
	let stalenessSegments = $derived.by<Segment[]>(() => {
		const counts = [0, 0, 0, 0];
		for (const p of items) counts[stalenessBucket(daysSince(p.updated_at))]++;
		return STALENESS_BUCKETS.map((b, i) => ({
			label: translate('prInsights.age.' + b.key),
			value: counts[i],
			color: b.color,
			meta: `age=${b.key}`
		})).filter((s) => s.value > 0);
	});

	function openFilteredList(seg: Segment) {
		const qs = typeof seg.meta === 'string' ? seg.meta : '';
		goto(`/pr-insights/list?${qs}`);
	}

	// Trend line series — day labels trimmed to MM-DD.
	let trendLabels = $derived(trend.map((p) => p.day.slice(5)));
	let trendLines = $derived([
		{ label: translate('prInsights.trend.open'), data: trend.map((p) => p.open), color: '#3b82f6' },
		{ label: translate('prInsights.trend.conflict'), data: trend.map((p) => p.conflict), color: '#ef4444' },
		{ label: translate('prInsights.trend.mergeable'), data: trend.map((p) => p.mergeable), color: '#22c55e' }
	]);

	// "By branch" tab filter (empty = all branches).
	let branchFilter = $state<string>('');
	let byBranchItems = $derived(
		branchFilter ? items.filter((p) => (p.base_ref ?? '(unknown)') === branchFilter) : items
	);

	// Status-colour badge classes (applied with variant="outline").
	const BADGE_CLASSES: Record<'green' | 'red' | 'yellow' | 'blue' | 'gray', string> = {
		green: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400',
		red: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400',
		yellow: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
		blue: 'border-transparent bg-primary/15 text-primary',
		gray: 'border-transparent bg-secondary text-secondary-foreground'
	};

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
	}

	function ciColor(ci: string | null): 'green' | 'red' | 'yellow' | 'gray' {
		if (ci === 'success') return 'green';
		if (ci === 'failure') return 'red';
		if (ci === 'pending') return 'yellow';
		return 'gray';
	}

	function statusColor(status: string): 'green' | 'red' | 'gray' {
		if (status === 'mergeable') return 'green';
		if (status === 'conflict') return 'red';
		return 'gray';
	}

	// PR detail modal.
	let modalOpen = $state(false);
	let activePr = $state<PrInsightItem | null>(null);
	function openPr(pr: PrInsightItem) {
		activePr = pr;
		modalOpen = true;
	}

	// Active tab ("overview" matches the flowbite TabItem that had `open`).
	let activeTab = $state('overview');
</script>

<svelte:head>
	<title>wshm - PR Insights</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">{translate('prInsights.title')}</h2>
	<p class="text-sm text-muted-foreground">{translate('prInsights.subtitle')}</p>
</div>

{#snippet statCard(label: string, value: number | string, accent: string)}
	<Card.Root class="text-center py-4">
		<Card.Content class="px-4">
			<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">{label}</div>
			<div class="text-3xl font-bold mono {accent}">{value}</div>
		</Card.Content>
	</Card.Root>
{/snippet}

{#snippet prTable(rows: PrInsightItem[])}
	<div class="w-full overflow-x-auto rounded-lg border">
		<Table.Root class="w-full">
			<Table.Header class="text-xs uppercase">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[70px]">{translate('prInsights.table.pr')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.repo')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.title')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.author')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.target')}</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">{translate('prInsights.table.ci')}</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">{translate('prInsights.table.risk')}</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each rows as pr (pr.repo + '#' + pr.number)}
					<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
						<Table.Cell class="px-2 py-1.5 mono text-primary">{$itemRef(pr.number)}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-xs text-muted-foreground">{pr.repo}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 truncate max-w-[420px]">{pr.title}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground">{pr.author ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.base_ref}
								<Badge variant="outline" class={BADGE_CLASSES[defaults.has(pr.base_ref) ? 'blue' : 'gray']}>{pr.base_ref}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.ci_status}
								<Badge variant="outline" class={BADGE_CLASSES[ciColor(pr.ci_status)]}>{pr.ci_status}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.risk_level}
								<Badge variant="outline" class={BADGE_CLASSES[riskColor(pr.risk_level)]}>{pr.risk_level}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={7} class="text-center text-muted-foreground py-8">
							{translate('prInsights.empty')}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
{/snippet}

{#snippet modalField(label: string, value: string)}
	<Card.Root class="p-3">
		<Card.Content class="p-0">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">{label}</div>
			<span class="text-foreground text-sm">{value}</span>
		</Card.Content>
	</Card.Root>
{/snippet}

{#if loading}
	<p class="text-sm text-muted-foreground">{translate('prInsights.loading')}</p>
{:else if error}
	<Card.Root class="border-destructive">
		<Card.Content>
			<p class="text-destructive">{error}</p>
		</Card.Content>
	</Card.Root>
{:else}
	<Tabs.Root bind:value={activeTab}>
		<Tabs.List>
			<Tabs.Trigger value="overview">{translate('prInsights.tab.overview')}</Tabs.Trigger>
			<Tabs.Trigger value="conflicts">{translate('prInsights.tab.conflicts')} ({conflictItems.length})</Tabs.Trigger>
			<Tabs.Trigger value="mergeable">{translate('prInsights.tab.mergeable')} ({mergeableItems.length})</Tabs.Trigger>
			<Tabs.Trigger value="byBranch">{translate('prInsights.tab.byBranch')}</Tabs.Trigger>
			<Tabs.Trigger value="trend">{translate('prInsights.tab.trend')}</Tabs.Trigger>
		</Tabs.List>

		<Tabs.Content value="overview" class="mt-4">
			<div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-4 mb-6">
				{@render statCard(translate('prInsights.stat.total'), items.length, 'text-foreground')}
				{@render statCard(translate('prInsights.stat.mergeable'), mergeableItems.length, 'text-green-600 dark:text-green-400')}
				{@render statCard(translate('prInsights.stat.conflict'), conflictItems.length, 'text-red-600 dark:text-red-400')}
				{@render statCard(translate('prInsights.stat.unknown'), unknownCount, 'text-muted-foreground')}
				{@render statCard(translate('prInsights.stat.toDefault'), toDefaultCount, 'text-primary')}
				{@render statCard(translate('prInsights.stat.toOther'), toOtherCount, 'text-purple-600 dark:text-purple-400')}
			</div>
			<div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
				<Card.Root>
					<Card.Header>
						<Card.Title class="text-sm">{translate('prInsights.chart.mergeability')}</Card.Title>
					</Card.Header>
					<Card.Content>
						{#if items.length > 0}
							<DoughnutChart
								segments={mergeabilitySegments}
								centerLabel={String(items.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('prInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
				<Card.Root>
					<Card.Header>
						<Card.Title class="text-sm">{translate('prInsights.chart.targetBranch')}</Card.Title>
					</Card.Header>
					<Card.Content>
						{#if items.length > 0}
							<DoughnutChart
								segments={branchSegments}
								centerLabel={String(items.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('prInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
				<Card.Root>
					<Card.Header>
						<Card.Title class="text-sm">{translate('prInsights.chart.staleness')}</Card.Title>
					</Card.Header>
					<Card.Content>
						{#if items.length > 0}
							<DoughnutChart
								segments={stalenessSegments}
								centerLabel={String(items.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('prInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
			</div>
		</Tabs.Content>

		<Tabs.Content value="conflicts" class="mt-4">
			{@render prTable(conflictItems)}
		</Tabs.Content>

		<Tabs.Content value="mergeable" class="mt-4">
			{@render prTable(mergeableItems)}
		</Tabs.Content>

		<Tabs.Content value="byBranch" class="mt-4">
			<div class="flex flex-wrap gap-2 mb-4">
				<Button
					size="sm"
					variant={branchFilter === '' ? 'default' : 'outline'}
					onclick={() => (branchFilter = '')}
				>
					{translate('prInsights.allBranches')} ({items.length})
				</Button>
				{#each branchGroups as g (g.branch)}
					<Button
						size="sm"
						variant={branchFilter === g.branch ? 'default' : 'outline'}
						onclick={() => (branchFilter = g.branch)}
					>
						{g.branch} ({g.count}){#if g.isDefault}<span class="ml-1 text-[0.6rem] uppercase opacity-70">{translate('prInsights.defaultBadge')}</span>{/if}
					</Button>
				{/each}
			</div>
			{@render prTable(byBranchItems)}
		</Tabs.Content>

		<Tabs.Content value="trend" class="mt-4">
			<p class="text-sm text-muted-foreground mb-3">{translate('prInsights.trend.subtitle')}</p>
			<Card.Root>
				<Card.Header>
					<Card.Title class="text-sm">{translate('prInsights.trend.title')}</Card.Title>
				</Card.Header>
				<Card.Content>
					{#if trend.length > 0}
						<LineChart labels={trendLabels} lines={trendLines} />
					{:else}
						<p class="text-sm text-muted-foreground py-8 text-center">
							{translate('prInsights.trend.empty')}
						</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</Tabs.Content>
	</Tabs.Root>

	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-2xl max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-6 text-left">
					<span class="mono text-muted-foreground text-sm">#{activePr?.number ?? ''}</span>
					<span class="text-base font-semibold text-foreground truncate">{activePr?.title ?? ''}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activePr}
				<div class="mb-4 flex flex-wrap items-center gap-2">
					<Badge variant="outline" class={BADGE_CLASSES[statusColor(activePr.status)]}>
						{translate('prInsights.legend.' + activePr.status)}
					</Badge>
					{#if activePr.ci_status}
						<Badge variant="outline" class={BADGE_CLASSES[ciColor(activePr.ci_status)]}>{activePr.ci_status}</Badge>
					{/if}
					{#if activePr.risk_level}
						<Badge variant="outline" class={BADGE_CLASSES[riskColor(activePr.risk_level)]}>{activePr.risk_level}</Badge>
					{/if}
				</div>

				<div class="grid grid-cols-2 md:grid-cols-3 gap-3 mb-4">
					{@render modalField(translate('prInsights.table.repo'), activePr.repo)}
					{@render modalField(translate('prInsights.table.author'), activePr.author ?? '-')}
					{@render modalField(
						translate('prInsights.modal.branches'),
						`${activePr.head_ref ?? '-'} → ${activePr.base_ref ?? '-'}`
					)}
					{@render modalField(
						translate('prInsights.modal.lastActivity'),
						`${daysSince(activePr.updated_at)}d · ${activePr.updated_at.slice(0, 10)}`
					)}
					{@render modalField(
						translate('prInsights.modal.created'),
						activePr.created_at.slice(0, 10)
					)}
				</div>

				<Card.Root class="p-3 mb-4">
					<Card.Content class="p-0">
						<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">
							{translate('prInsights.modal.summary')}
						</div>
						<p class="text-sm text-foreground/90 whitespace-pre-wrap break-words">
							{activePr.summary ?? translate('prInsights.modal.noSummary')}
						</p>
					</Card.Content>
				</Card.Root>

				{#if activePr.url}
					<a
						href={activePr.url}
						target="_blank"
						rel="noopener noreferrer"
						class="text-sm text-primary hover:text-primary/80 underline"
					>
						{translate('prInsights.modal.openOnForge')} →
					</a>
				{/if}
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
