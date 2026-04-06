<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters } from '$lib/filter';
	import { paginate, totalPages, PAGE_SIZE } from '$lib/paginate';
	import { goto } from '$app/navigation';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let pulls: PullRequest[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'risk_level', asc: true }, { key: 'age', asc: false }]);
	let filters: Record<string, string> = $state({
		number: '', title: '', state: '', base_ref: '', risk: '', ci_status: '', conflicts: '', age: ''
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

	let enriched = $derived(pulls.map(p => ({
		...p,
		age: ageDays(p.created_at),
		conflicts: p.mergeable === false ? 'yes' : 'no'
	})));

	let filtered = $derived(applyFilters(enriched, {
		number: filters.number,
		title: filters.title,
		state: filters.state,
		risk: filters.risk,
		ci_status: filters.ci_status,
		conflicts: filters.conflicts,
		age: filters.age
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));
	let page = $state(0);
	let pages = $derived(totalPages(sorted.length));
	let paged = $derived(paginate(sorted, page));

	async function load() {
		page = 0;
		try {
			error = null;
			pulls = await fetchPulls();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load pull requests';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'dark' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'dark';
	}

	function ciColor(ci: string | null): 'green' | 'yellow' | 'red' | 'dark' {
		if (ci === 'success') return 'green';
		if (ci === 'pending') return 'yellow';
		if (ci === 'failure') return 'red';
		return 'dark';
	}
</script>

<svelte:head>
	<title>wshm - Pull Requests</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Pull Requests</h2>
	<p class="text-sm text-gray-500">All tracked pull requests from the repository</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
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
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('base_ref', e)}>
					Base <span class={sortArrowClass(sortColumns, 'base_ref')}>{sortArrow(sortColumns, 'base_ref')}</span>{#if sortIndex(sortColumns, 'base_ref') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'base_ref')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('risk', e)}>
					Risk <span class={sortArrowClass(sortColumns, 'risk')}>{sortArrow(sortColumns, 'risk')}</span>{#if sortIndex(sortColumns, 'risk') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'risk')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('ci_status', e)}>
					CI <span class={sortArrowClass(sortColumns, 'ci_status')}>{sortArrow(sortColumns, 'ci_status')}</span>{#if sortIndex(sortColumns, 'ci_status') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'ci_status')}</span>{/if}
				</TableHeadCell>
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('conflicts', e)}>
					Conflicts <span class={sortArrowClass(sortColumns, 'conflicts')}>{sortArrow(sortColumns, 'conflicts')}</span>{#if sortIndex(sortColumns, 'conflicts') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'conflicts')}</span>{/if}
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
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.base_ref} placeholder="main..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.risk} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.ci_status} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.conflicts} placeholder="filter..." class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><input type="text" bind:value={filters.age} placeholder=">N" class="w-full rounded border border-gray-600 bg-gray-900 px-1 py-0.5 text-xs text-gray-300 focus:border-blue-500 focus:outline-none" /></TableBodyCell>
				</TableBodyRow>
				{#each paged as pr}
					<TableBodyRow class="cursor-pointer" onclick={() => goto(`/prs/${pr.number}`)}>
						<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 truncate">{pr.title}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<Badge color={pr.state === 'open' ? 'green' : 'red'}>{pr.state}</Badge>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-xs mono text-gray-400">{pr.base_ref ?? '-'}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if pr.risk}
								<Badge color={riskColor(pr.risk)}>{pr.risk}</Badge>
							{:else}
								<span class="text-gray-500">-</span>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if pr.ci_status}
								<Badge color={ciColor(pr.ci_status)}>{pr.ci_status}</Badge>
							{:else}
								<span class="text-gray-500">-</span>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#if pr.mergeable === false}
								<Badge color="red">yes</Badge>
							{:else}
								<Badge color="green">no</Badge>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{timeAgo(pr.created_at)}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={7} class="text-center text-gray-600 py-8">No pull requests found</TableBodyCell>
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
