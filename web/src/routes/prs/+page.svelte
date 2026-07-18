<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge, Input, Modal } from 'flowbite-svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';

	const PAGE_KEY = 'wshm.pageSize.pulls';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let pulls: PullRequest[] = $state([]);
	let error: string | null = $state(null);
	// True until the first fetch settles - stops the empty-state text from
	// flashing "No results" while the list is still loading.
	let loading = $state(true);
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
		conflicts: p.mergeable === false ? 'yes' : (p.mergeable === true ? 'no' : 'unknown')
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

	let stateOptions = $derived(distinctValues(enriched, 'state'));
	let riskOptions = $derived(distinctValues(enriched, 'risk'));
	let ciOptions = $derived(distinctValues(enriched, 'ci_status'));
	let conflictsOptions = $derived(distinctValues(enriched, 'conflicts'));
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);

	// Race guard against repo-switch overwrites. See issues page for context.
	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchPulls({ limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			pulls = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load pull requests';
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

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
	}

	function ciColor(ci: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (ci === 'success') return 'green';
		if (ci === 'pending') return 'yellow';
		if (ci === 'failure') return 'red';
		return 'gray';
	}

	let modalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);

	function openPr(pr: PullRequest) {
		activePr = pr;
		modalOpen = true;
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
	<div class="w-full overflow-x-auto">
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
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.number} placeholder="#" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.title} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.state} options={stateOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.base_ref} placeholder="main..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.risk} options={riskOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.ci_status} options={ciOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.conflicts} options={conflictsOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.age} placeholder=">N" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as pr}
					<TableBodyRow class="cursor-pointer" onclick={() => openPr(pr)}>
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
							<!-- Only the actionable state gets color: a red badge for real
							     conflicts. "no" is the normal case (plain text) and unknown
							     mergeability is "-" instead of a misleading green "no". -->
							{#if pr.mergeable === false}
								<Badge color="red">yes</Badge>
							{:else if pr.mergeable === true}
								<span class="text-gray-500 text-xs">no</span>
							{:else}
								<span class="text-gray-500">-</span>
							{/if}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{timeAgo(pr.created_at)}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={7} class="text-center text-gray-600 py-8">{loading ? 'Loading…' : 'No pull requests found'}</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
	<Modal
		bind:open={modalOpen}
		size="xl"
		dismissable
		class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
		bodyClass="text-gray-200"
	>
		{#snippet header()}
			<div class="flex w-full items-center gap-3 pr-2">
				<span class="mono text-gray-500 text-sm">#{activePr?.number}</span>
				<span class="text-base font-semibold text-gray-100 truncate">{activePr?.title}</span>
			</div>
		{/snippet}
		{#if activePr}
			<PrDetail pr={activePr} />
			<div class="text-right pt-2">
				<a href="/prs/{activePr.number}" class="text-xs text-blue-400 hover:text-blue-300">
					Open full page →
				</a>
			</div>
		{/if}
	</Modal>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
