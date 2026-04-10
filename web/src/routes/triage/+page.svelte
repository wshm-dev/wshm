<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchTriage, type TriageResult } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters } from '$lib/filter';
	import { paginate, totalPages, PAGE_SIZE } from '$lib/paginate';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let results: TriageResult[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'issue_number', asc: true }]);
	let filters: Record<string, string> = $state({
		issue_number: '', category: '', confidence: '', priority: '', acted_at: ''
	});

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	let enriched = $derived(results.map(r => ({
		...r,
		confidence_pct: Math.round(r.confidence * 100)
	})));

	let filtered = $derived(applyFilters(enriched, {
		issue_number: filters.issue_number,
		category: filters.category,
		confidence_pct: filters.confidence,
		priority: filters.priority,
		acted_at: filters.acted_at
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));
	let page = $state(0);
	let pages = $derived(totalPages(sorted.length));
	let paged = $derived(paginate(sorted, page));

	async function load() {
		page = 0;
		try {
			error = null;
			results = await fetchTriage();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load triage results';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function categoryColor(cat: string): 'red' | 'blue' | 'yellow' | 'gray' {
		if (cat === 'bug') return 'red';
		if (cat === 'feature') return 'blue';
		if (cat === 'needs-info') return 'yellow';
		return 'gray';
	}

	function confidenceColor(conf: number): string {
		if (conf >= 0.85) return 'text-green-400';
		if (conf >= 0.6) return 'text-yellow-400';
		return 'text-red-400';
	}
</script>

<svelte:head>
	<title>wshm - Triage</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Triage Results</h2>
	<p class="text-sm text-gray-500">AI classification results for issues</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
{:else}
	<div class="overflow-x-auto">
		<Table striped hoverable class="w-full">
			<TableHead class="text-xs uppercase text-gray-400">
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('issue_number', e)}>
					Issue <span class={sortArrowClass(sortColumns, 'issue_number')}>{sortArrow(sortColumns, 'issue_number')}</span>{#if sortIndex(sortColumns, 'issue_number') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'issue_number')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[100px]" onclick={(e: MouseEvent) => handleSort('category', e)}>
					Category <span class={sortArrowClass(sortColumns, 'category')}>{sortArrow(sortColumns, 'category')}</span>{#if sortIndex(sortColumns, 'category') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'category')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[100px]" onclick={(e: MouseEvent) => handleSort('confidence_pct', e)}>
					Confidence <span class={sortArrowClass(sortColumns, 'confidence_pct')}>{sortArrow(sortColumns, 'confidence_pct')}</span>{#if sortIndex(sortColumns, 'confidence_pct') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'confidence_pct')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('priority', e)}>
					Priority <span class={sortArrowClass(sortColumns, 'priority')}>{sortArrow(sortColumns, 'priority')}</span>{#if sortIndex(sortColumns, 'priority') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'priority')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('acted_at', e)}>
					Acted At <span class={sortArrowClass(sortColumns, 'acted_at')}>{sortArrow(sortColumns, 'acted_at')}</span>{#if sortIndex(sortColumns, 'acted_at') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'acted_at')}</span>{/if}
				</TableHeadCell>
			</TableHead>
			<TableBody>
				<TableBodyRow class="border-b border-gray-700">
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.issue_number} placeholder="#" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.category} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.confidence} placeholder=">85" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.priority} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.acted_at} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
				</TableBodyRow>
				{#each paged as result}
					<TableBodyRow>
						<TableBodyCell class="px-2 py-1.5 mono"><a href="/issues">#{result.issue_number}</a></TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<Badge color={categoryColor(result.category)}>{result.category}</Badge>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<span class="mono font-semibold {confidenceColor(result.confidence)}">{(result.confidence * 100).toFixed(0)}%</span>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">{result.priority}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-500">{result.acted_at ?? 'Not acted'}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={5} class="text-center text-gray-600 py-8">No triage results yet</TableBodyCell>
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
