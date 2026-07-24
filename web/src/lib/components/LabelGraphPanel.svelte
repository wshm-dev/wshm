<script lang="ts">
	/**
	 * Reusable panel: a domain multi-select + label legend on the left and the
	 * bipartite label→item force graph on the right. Works for PRs and issues —
	 * both expose {number, title, labels, domains?, reactions_plus1?, repo}.
	 */
	import { Input } from '$lib/components/ui/input';
	import { Badge } from '$lib/components/ui/badge';
	import PrLabelGraph from '$lib/components/PrLabelGraph.svelte';
	import type { PullRequest } from '$lib/api';

	type Item = {
		number: number;
		title: string;
		labels: string[];
		domains?: string[] | null;
		reactions_plus1?: number | null;
		repo: string;
	};

	type Props = {
		items: Item[];
		loading?: boolean;
		emptyLabel?: string;
		onSelect?: (item: Item) => void;
	};
	let { items, loading = false, emptyLabel = 'items', onSelect }: Props = $props();

	function hueFor(label: string): number {
		let h = 0;
		for (let i = 0; i < label.length; i++) h = (h * 31 + label.charCodeAt(i)) % 360;
		return h;
	}
	function colorFor(label: string): string {
		return `hsl(${hueFor(label)} 65% 55%)`;
	}

	let focus = $state<string[]>([]);
	let legendQuery = $state('');
	let domainFilter = $state<string[]>([]);

	let filtered = $derived(
		domainFilter.length === 0
			? items
			: items.filter((p) => (p.domains ?? []).some((d) => domainFilter.includes(d)))
	);

	type Stat = { name: string; count: number };
	let labelStats: Stat[] = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const p of filtered) for (const l of p.labels) counts.set(l, (counts.get(l) ?? 0) + 1);
		return [...counts.entries()]
			.map(([name, count]) => ({ name, count }))
			.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
	});
	let unlabeled = $derived(filtered.filter((p) => p.labels.length === 0).length);
	let domainStats: Stat[] = $derived.by(() => {
		const counts = new Map<string, number>();
		for (const p of items) for (const d of p.domains ?? []) counts.set(d, (counts.get(d) ?? 0) + 1);
		return [...counts.entries()]
			.map(([name, count]) => ({ name, count }))
			.sort((a, b) => b.count - a.count || a.name.localeCompare(b.name));
	});
	let visibleLegend = $derived(
		legendQuery.trim()
			? labelStats.filter((l) => l.name.toLowerCase().includes(legendQuery.trim().toLowerCase()))
			: labelStats
	);

	function toggleLabel(name: string) {
		focus = focus.includes(name) ? focus.filter((l) => l !== name) : [...focus, name];
	}
	function toggleDomain(name: string) {
		domainFilter = domainFilter.includes(name)
			? domainFilter.filter((d) => d !== name)
			: [...domainFilter, name];
	}
</script>

<div class="grid grid-cols-[260px_1fr] gap-4">
	<div class="rounded-lg border bg-card p-3 h-[calc(100vh-260px)] min-h-[440px] flex flex-col">
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
				<button class="text-xs text-primary hover:underline" onclick={() => (focus = [])}>clear</button>
			{/if}
		</div>
		<Input type="text" bind:value={legendQuery} placeholder="filter labels…" class="h-8 px-2 text-xs mb-2" />
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
					<span class="inline-block h-3 w-3 shrink-0 rounded-full" style="background: {colorFor(l.name)}"></span>
					<span class="truncate flex-1">{l.name}</span>
					<span class="mono text-muted-foreground">{l.count}</span>
				</button>
			{:else}
				<p class="text-xs text-muted-foreground px-1.5 py-1">{loading ? 'Loading…' : 'No labels'}</p>
			{/each}
		</div>
		<div class="mt-2 border-t pt-2 text-[0.7rem] text-muted-foreground">
			{filtered.length}{#if domainFilter.length}/{items.length}{/if}
			{emptyLabel}
			{#if unlabeled}· {unlabeled} unlabeled (hidden){/if}
		</div>
	</div>

	<div class="rounded-lg border bg-card h-[calc(100vh-260px)] min-h-[440px] overflow-hidden">
		{#if loading}
			<div class="flex h-full items-center justify-center text-sm text-muted-foreground">Loading…</div>
		{:else if labelStats.length === 0}
			<div class="flex h-full items-center justify-center text-sm text-muted-foreground">
				Nothing labelled to graph
			</div>
		{:else}
			<PrLabelGraph
				pulls={filtered as unknown as PullRequest[]}
				{focus}
				{colorFor}
				onSelectPr={(p) => onSelect?.(p as unknown as Item)}
				onToggleLabel={toggleLabel}
			/>
		{/if}
	</div>
</div>

{#if focus.length}
	<div class="mt-3 flex flex-wrap items-center gap-1.5 text-xs">
		<span class="text-muted-foreground">Focused:</span>
		{#each focus as f}
			<Badge variant="outline" class="cursor-pointer" style="border-color: {colorFor(f)}" onclick={() => toggleLabel(f)}>
				{f} ✕
			</Badge>
		{/each}
	</div>
{/if}
