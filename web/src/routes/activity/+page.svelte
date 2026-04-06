<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchActivity, type ActivityEntry } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters } from '$lib/filter';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let activities: ActivityEntry[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'created_at', asc: false }]);
	let filters: Record<string, string> = $state({
		created_at: '', action: '', target: '', summary: ''
	});

	function formatTime(dateStr: string): string {
		return new Date(dateStr).toLocaleString();
	}

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	let enriched = $derived(activities.map(a => ({
		...a,
		target: `${a.target_type} #${a.target_number}`
	})));

	let filtered = $derived(applyFilters(enriched, {
		created_at: filters.created_at,
		action: filters.action,
		target: filters.target,
		summary: filters.summary
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));

	async function load() {
		try {
			error = null;
			activities = await fetchActivity();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load activity';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function actionColor(action: string): 'blue' | 'green' | 'yellow' | 'dark' {
		if (action === 'triage') return 'blue';
		if (action === 'merge') return 'green';
		if (action === 'analyze') return 'yellow';
		return 'dark';
	}
</script>

<svelte:head>
	<title>wshm - Activity</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Activity Log</h2>
	<p class="text-sm text-gray-500">Recent triage and analysis actions</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
{:else}
	<div class="overflow-x-auto">
		<Table striped hoverable class="w-full">
			<TableHead class="text-xs uppercase text-gray-400">
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[180px]" onclick={(e: MouseEvent) => handleSort('created_at', e)}>
					Time <span class={sortArrowClass(sortColumns, 'created_at')}>{sortArrow(sortColumns, 'created_at')}</span>{#if sortIndex(sortColumns, 'created_at') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'created_at')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('action', e)}>
					Action <span class={sortArrowClass(sortColumns, 'action')}>{sortArrow(sortColumns, 'action')}</span>{#if sortIndex(sortColumns, 'action') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'action')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[120px]" onclick={(e: MouseEvent) => handleSort('target', e)}>
					Target <span class={sortArrowClass(sortColumns, 'target')}>{sortArrow(sortColumns, 'target')}</span>{#if sortIndex(sortColumns, 'target') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'target')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('summary', e)}>
					Summary <span class={sortArrowClass(sortColumns, 'summary')}>{sortArrow(sortColumns, 'summary')}</span>{#if sortIndex(sortColumns, 'summary') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'summary')}</span>{/if}
				</TableHeadCell>
			</TableHead>
			<TableBody>
				<TableBodyRow class="border-b border-gray-700">
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.created_at} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.action} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.target} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.summary} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as entry}
					<TableBodyRow>
						<TableBodyCell class="px-2 py-1.5 text-gray-500 whitespace-nowrap text-sm">{formatTime(entry.created_at)}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<Badge color={actionColor(entry.action)}>{entry.action}</Badge>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 whitespace-nowrap mono">{entry.target_type} #{entry.target_number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">{entry.summary}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={4} class="text-center text-gray-600 py-8">No activity recorded yet</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
{/if}
