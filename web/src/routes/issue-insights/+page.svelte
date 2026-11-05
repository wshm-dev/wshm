<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { selectedRepo } from '$lib/stores';
	import {
		fetchIssueInsights,
		fetchIssueTrend,
		type IssueInsightItem,
		type IssueInsights,
		type IssueTrendPoint
	} from '$lib/api';
	import { t } from '$lib/i18n';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Tabs from '$lib/components/ui/tabs';
	import * as Dialog from '$lib/components/ui/dialog';
	import DoughnutChart, { type Segment } from '$lib/components/DoughnutChart.svelte';
	import LineChart from '$lib/components/LineChart.svelte';
	import BarChart, { type Bar } from '$lib/components/BarChart.svelte';

	let translate = $state<(k: string) => string>((k) => k);
	t.subscribe((fn) => (translate = fn));

	let data = $state<IssueInsights | null>(null);
	let trend = $state<IssueTrendPoint[]>([]);
	let error: string | null = $state(null);
	let proLocked = $state(false);
	let loading = $state(true);

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		loading = true;
		error = null;
		proLocked = false;
		try {
			const res = await fetchIssueInsights();
			if (myToken !== loadToken) return;
			data = res;
		} catch (e) {
			if (myToken !== loadToken) return;
			const msg = e instanceof Error ? e.message : 'Failed to load';
			// A 403 from the Pro gate surfaces as "API error: 403 ...".
			if (msg.includes('403')) proLocked = true;
			else error = msg;
		}
		// Trend history is best-effort — a failure here must not blank the page.
		try {
			const tr = await fetchIssueTrend();
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
	let triagedItems = $derived(items.filter((i) => i.triaged));
	let untriagedItems = $derived(items.filter((i) => !i.triaged));

	let highCount = $derived(items.filter((i) => i.priority === 'high').length);
	let mediumCount = $derived(items.filter((i) => i.priority === 'medium').length);
	let lowCount = $derived(items.filter((i) => i.priority === 'low').length);

	const CATEGORY_PALETTE = [
		'#3b82f6', '#8b5cf6', '#ec4899', '#f59e0b', '#14b8a6',
		'#06b6d4', '#a3e635', '#f97316', '#64748b'
	];

	// Each segment carries a `meta` query string — clicking a slice navigates
	// to /issue-insights/list with that filter applied.
	let triageStatusSegments = $derived<Segment[]>(
		[
			{
				label: translate('issueInsights.legend.triaged'),
				value: triagedItems.length,
				color: '#22c55e',
				meta: 'triage=triaged'
			},
			{
				label: translate('issueInsights.legend.untriaged'),
				value: untriagedItems.length,
				color: '#6b7280',
				meta: 'triage=untriaged'
			}
		].filter((s) => s.value > 0)
	);

	// Category breakdown — over triaged issues only (category is a triage output).
	let categorySegments = $derived.by<Segment[]>(() => {
		const counts = new Map<string, number>();
		for (const i of triagedItems) {
			const c = i.category ?? '(uncategorized)';
			counts.set(c, (counts.get(c) ?? 0) + 1);
		}
		return [...counts.entries()]
			.sort((a, b) => b[1] - a[1])
			.map(([label, value], idx) => ({
				label,
				value,
				color: CATEGORY_PALETTE[idx % CATEGORY_PALETTE.length],
				meta: `triage=triaged&category=${encodeURIComponent(label)}`
			}));
	});

	// Priority breakdown — over triaged issues; `none` = triaged without a priority.
	let prioritySegments = $derived.by<Segment[]>(() => {
		const none = triagedItems.filter((i) => i.priority == null).length;
		return [
			{
				label: translate('issueInsights.priority.high'),
				value: highCount,
				color: '#ef4444',
				meta: 'triage=triaged&priority=high'
			},
			{
				label: translate('issueInsights.priority.medium'),
				value: mediumCount,
				color: '#f59e0b',
				meta: 'triage=triaged&priority=medium'
			},
			{
				label: translate('issueInsights.priority.low'),
				value: lowCount,
				color: '#22c55e',
				meta: 'triage=triaged&priority=low'
			},
			{
				label: translate('issueInsights.priority.none'),
				value: none,
				color: '#6b7280',
				meta: 'triage=triaged&priority=none'
			}
		].filter((s) => s.value > 0);
	});

	function openFilteredList(seg: Segment) {
		const qs = typeof seg.meta === 'string' ? seg.meta : '';
		goto(`/issue-insights/list?${qs}`);
	}

	// Label histogram — counts every label across all open issues, top 12.
	let labelBars = $derived.by<Bar[]>(() => {
		const counts = new Map<string, number>();
		for (const i of items) {
			for (const label of i.labels) {
				counts.set(label, (counts.get(label) ?? 0) + 1);
			}
		}
		return [...counts.entries()]
			.sort((a, b) => b[1] - a[1])
			.slice(0, 12)
			.map(([label, value], idx) => ({
				label,
				value,
				color: CATEGORY_PALETTE[idx % CATEGORY_PALETTE.length]
			}));
	});

	// Untriaged backlog — oldest first, so the most-overdue issues lead.
	let backlogItems = $derived(
		[...untriagedItems].sort((a, b) => a.created_at.localeCompare(b.created_at))
	);

	// Trend line series — day labels trimmed to MM-DD.
	let trendLabels = $derived(trend.map((p) => p.day.slice(5)));
	let trendLines = $derived([
		{ label: translate('issueInsights.trend.total'), data: trend.map((p) => p.total), color: '#3b82f6' },
		{ label: translate('issueInsights.trend.triaged'), data: trend.map((p) => p.triaged), color: '#22c55e' },
		{ label: translate('issueInsights.trend.untriaged'), data: trend.map((p) => p.untriaged), color: '#6b7280' }
	]);

	function daysSince(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	const GREEN_BADGE = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const YELLOW_BADGE = 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	const RED_BADGE = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';

	function priorityBadgeClass(priority: string | null): string {
		if (priority === 'low') return GREEN_BADGE;
		if (priority === 'medium') return YELLOW_BADGE;
		if (priority === 'high') return RED_BADGE;
		return '';
	}

	let activeTab = $state('overview');

	// Issue detail modal.
	let modalOpen = $state(false);
	let activeIssue = $state<IssueInsightItem | null>(null);
	function openIssue(issue: IssueInsightItem) {
		activeIssue = issue;
		modalOpen = true;
	}
</script>

<svelte:head>
	<title>wshm - Issue Insights</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">{translate('issueInsights.title')}</h2>
	<p class="text-sm text-muted-foreground">{translate('issueInsights.subtitle')}</p>
</div>

{#snippet statCard(label: string, value: number | string, accent: string)}
	<Card.Root class="gap-0 py-4 text-center">
		<Card.Content class="px-4">
			<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">{label}</div>
			<div class="text-3xl font-bold mono {accent}">{value}</div>
		</Card.Content>
	</Card.Root>
{/snippet}

{#snippet issueTable(rows: IssueInsightItem[], showAge: boolean)}
	<div class="w-full overflow-x-auto rounded-lg border">
		<Table.Root class="w-full">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[70px]">{translate('issueInsights.table.issue')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.repo')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.title')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.author')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.category')}</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">{translate('issueInsights.table.priority')}</Table.Head>
					{#if showAge}
						<Table.Head class="px-2 py-1.5 w-[70px]">{translate('issueInsights.table.age')}</Table.Head>
					{/if}
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each rows as issue (issue.repo + '#' + issue.number)}
					<Table.Row class="cursor-pointer" onclick={() => openIssue(issue)}>
						<Table.Cell class="px-2 py-1.5 mono text-primary">#{issue.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-xs text-muted-foreground">{issue.repo}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 truncate max-w-[380px]">{issue.title}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground">{issue.author ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if issue.category}
								<Badge variant="secondary">{issue.category}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if issue.priority}
								<Badge variant="outline" class={priorityBadgeClass(issue.priority)}>{issue.priority}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						{#if showAge}
							<Table.Cell class="px-2 py-1.5 mono text-muted-foreground">{daysSince(issue.created_at)}d</Table.Cell>
						{/if}
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={showAge ? 7 : 6} class="text-center text-muted-foreground py-8">
							{translate('issueInsights.empty')}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
{/snippet}

{#snippet modalField(label: string, value: string)}
	<Card.Root class="gap-1 rounded-lg p-3">
		<div class="text-[0.625rem] uppercase text-muted-foreground">{label}</div>
		<span class="text-foreground text-sm">{value}</span>
	</Card.Root>
{/snippet}

{#if loading}
	<p class="text-sm text-muted-foreground">{translate('issueInsights.loading')}</p>
{:else if proLocked}
	<Card.Root class="border-primary/40 max-w-xl">
		<Card.Header>
			<Card.Title>{translate('issueInsights.proOnly.title')}</Card.Title>
		</Card.Header>
		<Card.Content>
			<p class="text-sm text-muted-foreground">{translate('issueInsights.proOnly.body')}</p>
		</Card.Content>
	</Card.Root>
{:else if error}
	<Card.Root class="border-destructive/50">
		<Card.Content>
			<p class="text-destructive">{error}</p>
		</Card.Content>
	</Card.Root>
{:else}
	<Tabs.Root bind:value={activeTab}>
		<Tabs.List>
			<Tabs.Trigger value="overview">{translate('issueInsights.tab.overview')}</Tabs.Trigger>
			<Tabs.Trigger value="backlog">{translate('issueInsights.tab.backlog')} ({backlogItems.length})</Tabs.Trigger>
			<Tabs.Trigger value="labels">{translate('issueInsights.tab.labels')}</Tabs.Trigger>
			<Tabs.Trigger value="trend">{translate('issueInsights.tab.trend')}</Tabs.Trigger>
		</Tabs.List>

		<Tabs.Content value="overview" class="mt-4">
			<div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-4 mb-6">
				{@render statCard(translate('issueInsights.stat.total'), items.length, 'text-foreground')}
				{@render statCard(translate('issueInsights.stat.triaged'), triagedItems.length, 'text-green-600 dark:text-green-400')}
				{@render statCard(translate('issueInsights.stat.untriaged'), untriagedItems.length, 'text-muted-foreground')}
				{@render statCard(translate('issueInsights.stat.high'), highCount, 'text-red-600 dark:text-red-400')}
				{@render statCard(translate('issueInsights.stat.medium'), mediumCount, 'text-amber-600 dark:text-amber-400')}
				{@render statCard(translate('issueInsights.stat.low'), lowCount, 'text-green-600 dark:text-green-400')}
			</div>
			<div class="grid grid-cols-1 lg:grid-cols-3 gap-4">
				<Card.Root>
					<Card.Content>
						<h3 class="text-sm font-semibold text-foreground mb-3">
							{translate('issueInsights.chart.triageStatus')}
						</h3>
						{#if items.length > 0}
							<DoughnutChart
								segments={triageStatusSegments}
								centerLabel={String(items.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('issueInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
				<Card.Root>
					<Card.Content>
						<h3 class="text-sm font-semibold text-foreground mb-3">
							{translate('issueInsights.chart.category')}
						</h3>
						{#if categorySegments.length > 0}
							<DoughnutChart
								segments={categorySegments}
								centerLabel={String(triagedItems.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('issueInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
				<Card.Root>
					<Card.Content>
						<h3 class="text-sm font-semibold text-foreground mb-3">
							{translate('issueInsights.chart.priority')}
						</h3>
						{#if prioritySegments.length > 0}
							<DoughnutChart
								segments={prioritySegments}
								centerLabel={String(triagedItems.length)}
								onSegmentClick={openFilteredList}
							/>
						{:else}
							<p class="text-sm text-muted-foreground py-8 text-center">{translate('issueInsights.empty')}</p>
						{/if}
					</Card.Content>
				</Card.Root>
			</div>
		</Tabs.Content>

		<Tabs.Content value="backlog" class="mt-4">
			<p class="text-sm text-muted-foreground mb-3">{translate('issueInsights.backlog.subtitle')}</p>
			{@render issueTable(backlogItems, true)}
		</Tabs.Content>

		<Tabs.Content value="labels" class="mt-4">
			<p class="text-sm text-muted-foreground mb-3">{translate('issueInsights.labels.subtitle')}</p>
			<Card.Root>
				<Card.Content>
					<h3 class="text-sm font-semibold text-foreground mb-3">
						{translate('issueInsights.labels.title')}
					</h3>
					{#if labelBars.length > 0}
						<BarChart bars={labelBars} />
					{:else}
						<p class="text-sm text-muted-foreground py-8 text-center">
							{translate('issueInsights.labels.empty')}
						</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</Tabs.Content>

		<Tabs.Content value="trend" class="mt-4">
			<p class="text-sm text-muted-foreground mb-3">{translate('issueInsights.trend.subtitle')}</p>
			<Card.Root>
				<Card.Content>
					<h3 class="text-sm font-semibold text-foreground mb-3">
						{translate('issueInsights.trend.title')}
					</h3>
					{#if trend.length > 0}
						<LineChart labels={trendLabels} lines={trendLines} />
					{:else}
						<p class="text-sm text-muted-foreground py-8 text-center">
							{translate('issueInsights.trend.empty')}
						</p>
					{/if}
				</Card.Content>
			</Card.Root>
		</Tabs.Content>
	</Tabs.Root>

	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-2xl max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex items-center gap-3 pr-6">
					<span class="mono text-muted-foreground text-sm">#{activeIssue?.number ?? ''}</span>
					<span class="text-base font-semibold text-foreground truncate">{activeIssue?.title ?? ''}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activeIssue}
				<div class="mb-4 flex flex-wrap items-center gap-2">
					<Badge
						variant={activeIssue.triaged ? 'outline' : 'secondary'}
						class={activeIssue.triaged ? GREEN_BADGE : ''}
					>
						{translate(activeIssue.triaged ? 'issueInsights.legend.triaged' : 'issueInsights.legend.untriaged')}
					</Badge>
					{#if activeIssue.category}
						<Badge variant="secondary">{activeIssue.category}</Badge>
					{/if}
					{#if activeIssue.priority}
						<Badge variant="outline" class={priorityBadgeClass(activeIssue.priority)}>{activeIssue.priority}</Badge>
					{/if}
				</div>

				<div class="grid grid-cols-2 md:grid-cols-3 gap-3 mb-4">
					{@render modalField(translate('issueInsights.table.repo'), activeIssue.repo)}
					{@render modalField(translate('issueInsights.table.author'), activeIssue.author ?? '-')}
					{@render modalField(
						translate('issueInsights.modal.confidence'),
						activeIssue.confidence != null ? `${Math.round(activeIssue.confidence * 100)}%` : '-'
					)}
					{@render modalField(
						translate('issueInsights.modal.lastActivity'),
						`${daysSince(activeIssue.updated_at)}d · ${activeIssue.updated_at.slice(0, 10)}`
					)}
					{@render modalField(
						translate('issueInsights.modal.created'),
						activeIssue.created_at.slice(0, 10)
					)}
					{@render modalField(
						translate('issueInsights.modal.labels'),
						activeIssue.labels.length > 0 ? activeIssue.labels.join(', ') : '-'
					)}
				</div>

				<Card.Root class="gap-1 rounded-lg p-3 mb-4">
					<div class="text-[0.625rem] uppercase text-muted-foreground">
						{translate('issueInsights.modal.summary')}
					</div>
					<p class="text-sm text-foreground/90 whitespace-pre-wrap break-words">
						{activeIssue.summary ?? translate('issueInsights.modal.noSummary')}
					</p>
				</Card.Root>

				{#if activeIssue.url}
					<a
						href={activeIssue.url}
						target="_blank"
						rel="noopener noreferrer"
						class="text-sm text-primary hover:text-primary/80 underline"
					>
						{translate('issueInsights.modal.openOnForge')} →
					</a>
				{/if}
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
