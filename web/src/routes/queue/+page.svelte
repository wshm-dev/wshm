<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchQueue, type QueueEntry } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters } from '$lib/filter';
	import { paginate, totalPages, PAGE_SIZE } from '$lib/paginate';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let entries: QueueEntry[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'score', asc: false }]);
	let filters: Record<string, string> = $state({
		pr_number: '', title: '', score: '', ci: '', approvals: '', conflicts: '', risk: ''
	});

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	let enriched = $derived(entries.map(e => ({
		...e,
		ci: e.ci_passing ? 'passing' : 'failing',
		conflicts: e.has_conflicts ? 'yes' : 'no'
	})));

	let filtered = $derived(applyFilters(enriched, {
		pr_number: filters.pr_number,
		title: filters.title,
		score: filters.score,
		ci: filters.ci,
		approvals: filters.approvals,
		conflicts: filters.conflicts,
		risk: filters.risk
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));
	let page = $state(0);
	let pages = $derived(totalPages(sorted.length));
	let paged = $derived(paginate(sorted, page));

	async function load() {
		page = 0;
		try {
			error = null;
			entries = await fetchQueue();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load merge queue';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function scoreColor(score: number): string {
		if (score >= 15) return 'text-green-400';
		if (score >= 5) return 'text-yellow-400';
		return 'text-red-400';
	}

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
	}
</script>

<svelte:head>
	<title>wshm - Merge Queue</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Merge Queue</h2>
	<p class="text-sm text-gray-500">Pull requests ranked by merge readiness score</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
{:else}
	<div class="w-full overflow-x-auto">
		<Table striped hoverable class="w-full">
			<TableHead class="text-xs uppercase text-gray-400">
				<TableHeadCell class="px-2 py-1.5 w-[50px]">Rank</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('pr_number', e)}>
					PR <span class={sortArrowClass(sortColumns, 'pr_number')}>{sortArrow(sortColumns, 'pr_number')}</span>{#if sortIndex(sortColumns, 'pr_number') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'pr_number')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('title', e)}>
					Title <span class={sortArrowClass(sortColumns, 'title')}>{sortArrow(sortColumns, 'title')}</span>{#if sortIndex(sortColumns, 'title') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'title')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('score', e)}>
					Score <span class={sortArrowClass(sortColumns, 'score')}>{sortArrow(sortColumns, 'score')}</span>{#if sortIndex(sortColumns, 'score') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'score')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('ci', e)}>
					CI <span class={sortArrowClass(sortColumns, 'ci')}>{sortArrow(sortColumns, 'ci')}</span>{#if sortIndex(sortColumns, 'ci') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'ci')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('approvals', e)}>
					Apprvls <span class={sortArrowClass(sortColumns, 'approvals')}>{sortArrow(sortColumns, 'approvals')}</span>{#if sortIndex(sortColumns, 'approvals') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'approvals')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('conflicts', e)}>
					Conflicts <span class={sortArrowClass(sortColumns, 'conflicts')}>{sortArrow(sortColumns, 'conflicts')}</span>{#if sortIndex(sortColumns, 'conflicts') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'conflicts')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('risk', e)}>
					Risk <span class={sortArrowClass(sortColumns, 'risk')}>{sortArrow(sortColumns, 'risk')}</span>{#if sortIndex(sortColumns, 'risk') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'risk')}</span>{/if}
				</TableHeadCell>
			</TableHead>
			<TableBody>
				<TableBodyRow class="border-b border-gray-700">
					<TableBodyCell class="px-2 py-1"></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.pr_number} placeholder="#" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.title} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.score} placeholder=">15" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.ci} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.approvals} placeholder=">0" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.conflicts} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.risk} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
				</TableBodyRow>
				{#each paged as entry, i}
					<TableBodyRow>
						<TableBodyCell class="px-2 py-1.5 mono text-gray-500 font-bold text-sm">{i + 1}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 mono">#{entry.pr_number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 truncate">{entry.title}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<span class="mono font-bold {scoreColor(entry.score)}">{entry.score}</span>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if entry.ci_passing}
								<Badge color="green">passing</Badge>
							{:else}
								<Badge color="red">failing</Badge>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 mono">{entry.approvals}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if entry.has_conflicts}
								<Badge color="red">yes</Badge>
							{:else}
								<Badge color="green">no</Badge>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if entry.risk}
								<Badge color={riskColor(entry.risk)}>{entry.risk}</Badge>
							{:else}
								<span class="text-gray-500">-</span>
							{/if}
						</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={8} class="text-center text-gray-600 py-8">No pull requests in queue</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
	{#if pages > 1}
		<div class="flex items-center justify-between mt-2 text-sm text-gray-400">
			<span>{sorted.length} results (page {page + 1}/{pages})</span>
			<div class="flex gap-1">
				<button onclick={() => page = 0} disabled={page === 0} class="px-2 py-0.5 rounded border border-gray-600 hover:border-blue-500 disabled:opacity-30 disabled:cursor-default text-xs">|&lt;</button>
				<button onclick={() => page = Math.max(0, page - 1)} disabled={page === 0} class="px-2 py-0.5 rounded border border-gray-600 hover:border-blue-500 disabled:opacity-30 disabled:cursor-default text-xs">&lt;</button>
				<button onclick={() => page = Math.min(pages - 1, page + 1)} disabled={page >= pages - 1} class="px-2 py-0.5 rounded border border-gray-600 hover:border-blue-500 disabled:opacity-30 disabled:cursor-default text-xs">&gt;</button>
				<button onclick={() => page = pages - 1} disabled={page >= pages - 1} class="px-2 py-0.5 rounded border border-gray-600 hover:border-blue-500 disabled:opacity-30 disabled:cursor-default text-xs">&gt;|</button>
			</div>
		</div>
	{/if}
{/if}
