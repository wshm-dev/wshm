<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchTriage, type TriageResult } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge, Input } from 'flowbite-svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';

	const PAGE_KEY = 'wshm.pageSize.triage';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let results: TriageResult[] = $state([]);
	let error: string | null = $state(null);
	// True until the first fetch settles - stops the empty-state text from
	// flashing "No results" while the list is still loading.
	let loading = $state(true);
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

	let categoryOptions = $derived(distinctValues(results, 'category'));
	let priorityOptions = $derived(distinctValues(results, 'priority'));
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);

	// Race guard against repo-switch overwrites. See issues page for context.
	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchTriage({ limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			results = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load triage results';
		} finally {
			if (myToken === loadToken) loading = false;
		}
	}

	function onPageChange(next: { limit: number; offset: number }) {
		pageLimit = next.limit;
		pageOffset = next.offset;
		load();
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { pageOffset = 0; load(); });
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
	<div class="w-full overflow-x-auto">
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
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.issue_number} placeholder="#" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.category} options={categoryOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.confidence} placeholder=">85" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.priority} options={priorityOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.acted_at} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as result}
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
						<TableBodyCell colspan={5} class="text-center text-gray-600 py-8">
							{#if loading}
								Loading…
							{:else}
								No triage results yet.
								<span class="block text-xs text-gray-500 mt-1">
									Run <code class="bg-gray-900 px-1.5 py-0.5 rounded">wshm triage</code> from the CLI,
									or enable the triage feature in <a href="/settings" class="text-blue-400 hover:underline">Settings → Repos</a>.
								</span>
							{/if}
						</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
