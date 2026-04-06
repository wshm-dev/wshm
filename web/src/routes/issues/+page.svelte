<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchIssues, type Issue } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters } from '$lib/filter';
	import { Card, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let issues: Issue[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'number', asc: false }]);
	let filters: Record<string, string> = $state({
		number: '', title: '', state: '', labels: '', priority: '', category: '', age: ''
	});

	function timeAgo(dateStr: string): string {
		const diff = Date.now() - new Date(dateStr).getTime();
		const days = Math.floor(diff / 86400000);
		if (days === 0) return 'today';
		if (days === 1) return '1d';
		return `${days}d`;
	}

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	let enriched = $derived(issues.map(i => ({
		...i,
		age: ageDays(i.created_at),
		labels_str: i.labels.join(', ')
	})));

	let filtered = $derived(applyFilters(enriched, {
		number: filters.number,
		title: filters.title,
		state: filters.state,
		labels_str: filters.labels,
		priority: filters.priority,
		category: filters.category,
		age: filters.age
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));

	async function load() {
		try {
			error = null;
			issues = await fetchIssues();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load issues';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});
</script>

<svelte:head>
	<title>wshm - Issues</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Issues</h2>
	<p class="text-sm text-gray-500">All tracked issues from the repository</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800">
		<p class="text-red-400">{error}</p>
	</Card>
{:else}
	<div class="overflow-x-auto">
		<Table striped hoverable class="w-full">
			<TableHead class="text-xs uppercase text-gray-400">
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('number', e)}>
					# <span class={sortArrowClass(sortColumns, 'number')}>{sortArrow(sortColumns, 'number')}</span>{#if sortIndex(sortColumns, 'number') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'number')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('title', e)}>
					Title <span class={sortArrowClass(sortColumns, 'title')}>{sortArrow(sortColumns, 'title')}</span>{#if sortIndex(sortColumns, 'title') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'title')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('state', e)}>
					State <span class={sortArrowClass(sortColumns, 'state')}>{sortArrow(sortColumns, 'state')}</span>{#if sortIndex(sortColumns, 'state') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'state')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="px-2 py-1.5 w-[140px]">Labels</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('priority', e)}>
					Priority <span class={sortArrowClass(sortColumns, 'priority')}>{sortArrow(sortColumns, 'priority')}</span>{#if sortIndex(sortColumns, 'priority') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'priority')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('category', e)}>
					Category <span class={sortArrowClass(sortColumns, 'category')}>{sortArrow(sortColumns, 'category')}</span>{#if sortIndex(sortColumns, 'category') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'category')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('age', e)}>
					Age <span class={sortArrowClass(sortColumns, 'age')}>{sortArrow(sortColumns, 'age')}</span>{#if sortIndex(sortColumns, 'age') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'age')}</span>{/if}
				</TableHeadCell>
			</TableHead>
			<TableBody>
				<TableBodyRow class="border-b border-gray-700">
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.number} placeholder="#" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.title} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.state} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.labels} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.priority} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.category} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.age} placeholder=">N" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as issue}
					<TableBodyRow>
						<TableBodyCell class="px-2 py-1.5 mono">{issue.number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 truncate">{issue.title}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<Badge color={issue.state === 'open' ? 'green' : 'red'}>{issue.state}</Badge>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#each issue.labels as label}
								<Badge color="blue" class="mr-1">{label}</Badge>
							{/each}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">{issue.priority ?? '-'}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">{issue.category ?? '-'}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{timeAgo(issue.created_at)}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={7} class="text-center text-gray-600 py-8">No issues found</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
{/if}
