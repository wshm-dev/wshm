<script lang="ts">
	/**
	 * One tab of the Graphs page: the group → subgroup network graph for either
	 * pull requests or issues, driven by the server-side hierarchy over the whole
	 * DB. Owns its own fetch, "grands groupes" slider, selection, and the
	 * side-panel that lists the selected group/subgroup's items.
	 */
	import PrGroupGraph from '$lib/components/PrGroupGraph.svelte';
	import {
		fetchPrGroups,
		fetchIssueGroups,
		type PrGroup,
		type PrSubGroup,
		type PrGroupPr
	} from '$lib/api';

	let {
		kind,
		repo = null,
		onInteract
	}: {
		kind: 'pr' | 'issue';
		repo?: string | null;
		onInteract?: () => void;
	} = $props();

	let noun = $derived(kind === 'pr' ? 'PR' : 'issue');
	let linkBase = $derived(kind === 'pr' ? '/prs/' : '/issues/');
	let fetcher = $derived(kind === 'pr' ? fetchPrGroups : fetchIssueGroups);

	let groups = $state<PrGroup[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let count = $state(12);
	let selected = $state<{ id: string; label: string; count: number; prs: PrGroupPr[] } | null>(
		null
	);

	let token = 0;
	async function load() {
		const mine = ++token;
		loading = true;
		error = null;
		try {
			const r = await fetcher({ repo: repo ?? undefined, groups: count });
			if (mine !== token) return;
			groups = r.groups;
			count = r.groups_limit;
			selected = null;
		} catch (e) {
			if (mine === token) error = e instanceof Error ? e.message : 'failed to load';
		} finally {
			if (mine === token) loading = false;
		}
	}

	// Reload when the repo changes.
	let lastRepo: string | null | undefined;
	$effect(() => {
		if (repo !== lastRepo) {
			lastRepo = repo;
			load();
		}
	});

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
</script>

<div class="mb-3 flex flex-wrap items-center gap-3">
	<span class="text-xs font-medium text-muted-foreground">Grands groupes</span>
	<input
		type="range"
		min="5"
		max="30"
		step="1"
		bind:value={count}
		onchange={load}
		class="w-44 accent-primary"
	/>
	<span class="tabular-nums text-xs">{count}</span>
	{#if loading}
		<span class="text-xs text-muted-foreground">chargement…</span>
	{/if}
	<span class="text-xs text-muted-foreground">
		· molette = zoom · glisser = déplacer
	</span>
</div>

{#if error}
	<p class="text-sm text-destructive">{error}</p>
{:else if !loading && groups.length === 0}
	<p class="py-10 text-center text-sm text-muted-foreground">
		Aucun groupe — pas encore de {noun}s synchronisées.
	</p>
{:else}
	<!-- The side panel only appears once something is selected, so the graph
	     gets the full width by default. Selecting a node splits to two columns. -->
	<div class="grid gap-4 {selected ? 'lg:grid-cols-[1fr_300px]' : ''}">
		<PrGroupGraph
			{groups}
			{noun}
			selectedId={selected?.id ?? null}
			onSelect={selectNode}
			{onInteract}
		/>
		{#if selected}
			<div class="rounded-lg border bg-card p-3">
				<div class="mb-2 flex items-start justify-between gap-2">
					<div>
						<div class="text-sm font-semibold">{selected.label}</div>
						<div class="text-xs text-muted-foreground">
							{selected.count}
							{noun}{selected.count > 1 ? 's' : ''}
						</div>
					</div>
					<button
						type="button"
						class="shrink-0 text-muted-foreground hover:text-foreground"
						aria-label="fermer"
						onclick={() => (selected = null)}
					>
						✕
					</button>
				</div>
				<div class="max-h-[70vh] space-y-0.5 overflow-y-auto">
					{#each selected.prs as pr}
						<a href="{linkBase}{pr.number}" class="block rounded px-2 py-1 text-xs hover:bg-muted">
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
			</div>
		{/if}
	</div>
{/if}
