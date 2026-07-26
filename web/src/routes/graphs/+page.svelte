<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo, collapseSidebarSignal } from '$lib/stores';
	import { fetchIssues, type Issue } from '$lib/api';
	import * as Tabs from '$lib/components/ui/tabs';
	import * as Dialog from '$lib/components/ui/dialog';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import LabelGraphPanel from '$lib/components/LabelGraphPanel.svelte';
	import PrGroupGraph from '$lib/components/PrGroupGraph.svelte';
	import {
		fetchPrGroups,
		type PrGroup,
		type PrSubGroup,
		type PrGroupPr
	} from '$lib/api';

	let issues = $state<Issue[]>([]);
	let issueLoading = $state(true);

	// Server-side PR subject hierarchy (grand groupes → sous-groupes) over the
	// WHOLE DB — powers the PR network graph. Independent of the client PR sample.
	let prGroups = $state<PrGroup[]>([]);
	let groupsLoading = $state(true);
	let groupsCount = $state(12);
	let groupsError = $state<string | null>(null);
	// Unified selection: a grand groupe (sub null) or a sous-groupe.
	let selected = $state<{ id: string; label: string; count: number; prs: PrGroupPr[] } | null>(
		null
	);
	function selectNode(group: PrGroup, sub: PrSubGroup | null) {
		selected = sub
			? {
					id: `g:${group.name}/s:${sub.name}`,
					label: `${group.name} → ${sub.name}`,
					count: sub.count,
					prs: sub.prs
				}
			: { id: `g:${group.name}`, label: group.name, count: group.count, prs: group.prs };
	}

	let groupToken = 0;
	async function loadGroups(slug: string | null) {
		const mine = ++groupToken;
		groupsLoading = true;
		groupsError = null;
		try {
			// No repo selected → the endpoint aggregates across all repos, so the
			// graph still works "de base" (same convention as the other lists).
			const r = await fetchPrGroups({ repo: slug ?? undefined, groups: groupsCount });
			if (mine !== groupToken) return;
			prGroups = r.groups;
			groupsCount = r.groups_limit;
			selected = null;
		} catch (e) {
			if (mine === groupToken) groupsError = e instanceof Error ? e.message : 'failed to load groups';
		} finally {
			if (mine === groupToken) groupsLoading = false;
		}
	}

	// Aggregate history — daily snapshots. The trend endpoints are a Pro
	// feature; in OSS they 404, so we swallow errors and show "no history".
	type Point = Record<string, number | string>;
	let prTrend = $state<Point[]>([]);
	let issueTrend = $state<Point[]>([]);

	let token = 0;
	async function loadAll() {
		const mine = ++token;
		issueLoading = true;
		// Issues (the issue-graph tab still groups a client sample by label)
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
		const unsub = selectedRepo.subscribe((slug) => {
			loadAll();
			loadGroups(slug);
		});
		return unsub;
	});

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
		<div class="mb-3 flex flex-wrap items-center gap-3">
			<span class="text-xs font-medium text-muted-foreground">Grands groupes</span>
			<input
				type="range"
				min="5"
				max="30"
				step="1"
				bind:value={groupsCount}
				onchange={() => loadGroups($selectedRepo)}
				class="w-44 accent-primary"
			/>
			<span class="tabular-nums text-xs">{groupsCount}</span>
			{#if groupsLoading}
				<span class="text-xs text-muted-foreground">chargement…</span>
			{/if}
			<span class="text-xs text-muted-foreground"
				>· sujets des PRs sur toute la base — clique un sous-groupe pour ses PRs</span
			>
		</div>
		{#if groupsError}
			<p class="text-sm text-destructive">{groupsError}</p>
		{:else if !groupsLoading && prGroups.length === 0}
			<p class="py-10 text-center text-sm text-muted-foreground">
				Aucun groupe — pas encore de pull requests synchronisées pour ce dépôt.
			</p>
		{:else}
			<div class="grid gap-4 lg:grid-cols-[1fr_300px]">
				<PrGroupGraph
					groups={prGroups}
					selectedId={selected?.id ?? null}
					onSelect={selectNode}
					onInteract={() => collapseSidebarSignal.update((n) => n + 1)}
				/>
				<div class="rounded-lg border bg-card p-3">
					{#if selected}
						<div class="mb-2">
							<div class="text-sm font-semibold">{selected.label}</div>
							<div class="text-xs text-muted-foreground">
								{selected.count} PR{selected.count > 1 ? 's' : ''}
							</div>
						</div>
						<div class="max-h-[600px] space-y-0.5 overflow-y-auto">
							{#each selected.prs as pr}
								<a href="/prs/{pr.number}" class="block rounded px-2 py-1 text-xs hover:bg-muted">
									<span class="mono text-muted-foreground">#{pr.number}</span>
									{pr.title}
								</a>
							{/each}
							{#if selected.count > selected.prs.length}
								<p class="px-2 pt-1 text-[0.7rem] text-muted-foreground">
									+ {selected.count - selected.prs.length} de plus…
								</p>
							{/if}
						</div>
					{:else}
						<p class="text-xs text-muted-foreground">
							Clique un <strong>groupe</strong> ou un <strong>sous-groupe</strong> dans le graphe pour
							lister ses pull requests ici. Molette = zoom, glisser le fond = déplacer.
						</p>
					{/if}
				</div>
			</div>
		{/if}
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
