<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Dialog from '$lib/components/ui/dialog';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import PrLabelGraph from '$lib/components/PrLabelGraph.svelte';

	let pulls = $state<PullRequest[]>([]);
	let error: string | null = $state(null);
	let loading = $state(true);

	// Selected labels the graph is narrowed to (empty = show every label).
	let focus = $state<string[]>([]);
	let legendQuery = $state('');

	// Domain filters — narrow the graph to PRs the AI review tagged with the
	// selected "grand domains" (codex, bun, …). Multi-select; empty = all PRs.
	let domainFilter = $state<string[]>([]);
	let filteredPulls = $derived(
		domainFilter.length === 0
			? pulls
			: pulls.filter((p) => (p.domains ?? []).some((d) => domainFilter.includes(d)))
	);

	/** Stable hue per label so a label keeps its color across renders and both
	 *  themes (mid S/L reads on light and dark). */
	function hueFor(label: string): number {
		let h = 0;
		for (let i = 0; i < label.length; i++) h = (h * 31 + label.charCodeAt(i)) % 360;
		return h;
	}
	function colorFor(label: string): string {
		return `hsl(${hueFor(label)} 65% 55%)`;
	}

	type LabelStat = { name: string; count: number };
	let labelStats: LabelStat[] = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const pr of filteredPulls) for (const l of pr.labels) counts.set(l, (counts.get(l) ?? 0) + 1);
		return [...counts.entries()]
			.map(([name, count]) => ({ name, count }))
			.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
	});
	let unlabeled = $derived(filteredPulls.filter((p) => p.labels.length === 0).length);

	// Distinct domains across ALL loaded PRs (stable list to pick from), with
	// how many PRs carry each. Populated once PRs have been AI-reviewed.
	type DomainStat = { name: string; count: number };
	let domainStats: DomainStat[] = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const pr of pulls) for (const d of pr.domains ?? []) counts.set(d, (counts.get(d) ?? 0) + 1);
		return [...counts.entries()]
			.map(([name, count]) => ({ name, count }))
			.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
	});
	function toggleDomain(name: string) {
		domainFilter = domainFilter.includes(name)
			? domainFilter.filter((d) => d !== name)
			: [...domainFilter, name];
	}
	let visibleLegend = $derived(
		legendQuery.trim()
			? labelStats.filter((l) => l.name.toLowerCase().includes(legendQuery.trim().toLowerCase()))
			: labelStats
	);

	function toggleLabel(name: string) {
		focus = focus.includes(name) ? focus.filter((l) => l !== name) : [...focus, name];
	}
	function clearFocus() {
		focus = [];
	}

	// Fetch every open PR: /pulls clamps limit to 250, so page through until we
	// have `total` (hard-capped so a pathological repo can't spin forever).
	let loadToken = 0;
	async function loadAll() {
		const myToken = ++loadToken;
		loading = true;
		error = null;
		try {
			const all: PullRequest[] = [];
			let offset = 0;
			const limit = 250;
			for (;;) {
				const page = await fetchPulls({ limit, offset });
				if (myToken !== loadToken) return;
				all.push(...page.items);
				if (all.length >= page.total || page.items.length === 0 || all.length >= 2000) break;
				offset += page.items.length;
			}
			pulls = all;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load pull requests';
		} finally {
			if (myToken === loadToken) loading = false;
		}
	}

	onMount(() => {
		loadAll();
		const unsub = selectedRepo.subscribe(() => {
			focus = [];
			domainFilter = [];
			loadAll();
		});
		return unsub;
	});

	let modalOpen = $state(false);
	let activePr = $state<PullRequest | null>(null);
	function openPr(pr: PullRequest) {
		activePr = pr;
		modalOpen = true;
	}
</script>

<svelte:head>
	<title>wshm - PR Label Graph</title>
</svelte:head>

<div class="mb-4">
	<h2 class="text-xl font-semibold text-foreground mb-1">PR Label Graph</h2>
	<p class="text-sm text-muted-foreground">
		Open pull requests clustered by label. Node size = number of PRs. Filter by one or more
		<strong>domains</strong> (assigned by the AI review), click a label to focus, a PR to open it.
		Scroll to zoom, drag to pan.
	</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500/50 bg-card p-5">
		<p class="text-red-600 dark:text-red-400">{error}</p>
	</div>
{:else}
	<div class="grid grid-cols-[260px_1fr] gap-4">
		<!-- Legend / filter -->
		<div class="rounded-lg border bg-card p-3 h-[calc(100vh-220px)] min-h-[480px] flex flex-col">
			{#if domainStats.length}
				<div class="mb-3 border-b pb-3">
					<div class="flex items-center justify-between mb-1.5">
						<span class="text-xs uppercase text-muted-foreground">Domains</span>
						{#if domainFilter.length}
							<button class="text-xs text-primary hover:underline" onclick={() => (domainFilter = [])}>
								clear
							</button>
						{/if}
					</div>
					<div class="flex flex-wrap gap-1">
						{#each domainStats as d}
							<button
								class="rounded-full border px-2 py-0.5 text-[0.7rem] transition-colors {domainFilter.includes(
									d.name
								)
									? 'bg-primary text-primary-foreground border-primary'
									: 'hover:bg-accent'}"
								onclick={() => toggleDomain(d.name)}
							>
								{d.name} <span class="opacity-70">{d.count}</span>
							</button>
						{/each}
					</div>
				</div>
			{/if}
			<div class="flex items-center justify-between mb-2">
				<span class="text-xs uppercase text-muted-foreground">Labels ({labelStats.length})</span>
				{#if focus.length}
					<button class="text-xs text-primary hover:underline" onclick={clearFocus}>clear</button>
				{/if}
			</div>
			<Input
				type="text"
				bind:value={legendQuery}
				placeholder="filter labels…"
				class="h-8 px-2 text-xs mb-2"
			/>
			<div class="flex-1 overflow-y-auto -mx-1 px-1">
				{#each visibleLegend as l}
					<button
						class="flex w-full items-center gap-2 rounded px-1.5 py-1 text-left text-xs hover:bg-accent {focus.includes(
							l.name
						)
							? 'bg-accent'
							: ''}"
						onclick={() => toggleLabel(l.name)}
					>
						<span
							class="inline-block h-3 w-3 shrink-0 rounded-full"
							style="background: {colorFor(l.name)}"
						></span>
						<span class="truncate flex-1">{l.name}</span>
						<span class="mono text-muted-foreground">{l.count}</span>
					</button>
				{:else}
					<p class="text-xs text-muted-foreground px-1.5 py-1">
						{loading ? 'Loading…' : 'No labels'}
					</p>
				{/each}
			</div>
			<div class="mt-2 border-t pt-2 text-[0.7rem] text-muted-foreground">
				{filteredPulls.length}{#if domainFilter.length}/{pulls.length}{/if} open PR{filteredPulls.length === 1 ? '' : 's'}
				{#if unlabeled}· {unlabeled} unlabeled (hidden){/if}
			</div>
		</div>

		<!-- Graph canvas -->
		<div class="rounded-lg border bg-card h-[calc(100vh-220px)] min-h-[480px] overflow-hidden">
			{#if loading}
				<div class="flex h-full items-center justify-center text-sm text-muted-foreground">
					Loading pull requests…
				</div>
			{:else if labelStats.length === 0}
				<div class="flex h-full items-center justify-center text-sm text-muted-foreground">
					No labelled pull requests to graph
				</div>
			{:else}
				<PrLabelGraph pulls={filteredPulls} {focus} {colorFor} onSelectPr={openPr} onToggleLabel={toggleLabel} />
			{/if}
		</div>
	</div>

	{#if focus.length}
		<div class="mt-3 flex flex-wrap items-center gap-1.5 text-xs">
			<span class="text-muted-foreground">Focused:</span>
			{#each focus as f}
				<Badge
					variant="outline"
					class="cursor-pointer"
					style="border-color: {colorFor(f)}"
					onclick={() => toggleLabel(f)}
				>
					{f} ✕
				</Badge>
			{/each}
		</div>
	{/if}

	<Dialog.Root bind:open={modalOpen}>
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
					<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">
						Open full page →
					</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
