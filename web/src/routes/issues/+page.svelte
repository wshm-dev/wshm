<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchIssues, type Issue } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Card, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge, Input, Modal } from 'flowbite-svelte';
	import { colorConfig, prStatusBorder, priorityColor, categoryColor, type ColorConfig } from '$lib/colors';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';

	const PAGE_KEY = 'wshm.pageSize.issues';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let colors: ColorConfig = $state(colorConfig.defaults);
	colorConfig.subscribe(c => colors = c);

	let issues: Issue[] = $state([]);
	let error: string | null = $state(null);
	// True until the first fetch settles - stops the empty-state text from
	// flashing "No results" while the list is still loading.
	let loading = $state(true);
	let sortColumns: SortColumn[] = $state([{ key: 'priority', asc: true }, { key: 'age', asc: false }]);
	let filters: Record<string, string> = $state({
		number: '', title: '', pr_status: '', labels: '', priority: '', category: '', age: ''
	});
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);

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
		pr_status: filters.pr_status,
		labels_str: filters.labels,
		priority: filters.priority,
		category: filters.category,
		age: filters.age
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));

	let prStatusOptions = $derived(distinctValues(issues, 'pr_status'));
	let priorityOptions = $derived(distinctValues(issues, 'priority'));
	let categoryOptions = $derived(distinctValues(issues, 'category'));

	// Race guard: a fetch in flight from repo A must not overwrite the
	// list when the user has already switched to repo B. Each load()
	// claims a monotonic token; results are dropped if a newer load()
	// has started since.
	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchIssues({ limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			issues = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load issues';
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

	let modalOpen = $state(false);
	let activeIssue: Issue | null = $state(null);

	function openIssue(issue: Issue) {
		activeIssue = issue;
		modalOpen = true;
	}
</script>

<svelte:head>
	<title>wshm - Issues</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Issues</h2>
	<p class="text-sm text-gray-500">All tracked issues from the repository</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
	</Card>
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
				<TableHeadCell class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('pr_status', e)}>
					PR <span class={sortArrowClass(sortColumns, 'pr_status')}>{sortArrow(sortColumns, 'pr_status')}</span>{#if sortIndex(sortColumns, 'pr_status') > 0}<span class="text-[0.625rem] text-blue-400 ml-0.5">{sortIndex(sortColumns, 'pr_status')}</span>{/if}
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
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.number} placeholder="#" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.title} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.pr_status} options={prStatusOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.labels} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.priority} options={priorityOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.category} options={categoryOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.age} placeholder=">N" size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as issue}
					<TableBodyRow
						class="cursor-pointer"
						style="border-left: 3px solid {prStatusBorder(colors, issue.pr_status ?? 'no_pr')}; background-color: {prStatusBorder(colors, issue.pr_status ?? 'no_pr')}18;"
						onclick={() => openIssue(issue)}
					>
						<TableBodyCell class="px-2 py-1.5 mono text-gray-200">{issue.number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 truncate text-gray-200">{issue.title}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-200 text-xs">
							{issue.pr_status === 'pr_ready' ? 'PR ready' : issue.pr_status === 'has_pr' ? 'PR open' : 'No PR'}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							{#each issue.labels as label}
								<Badge color="blue" class="mr-1">{label}</Badge>
							{/each}
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-200">{issue.priority ?? '-'}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-200">{issue.category ?? '-'}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 text-gray-400 mono">{timeAgo(issue.created_at)}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={8} class="text-center text-gray-600 py-8">{loading ? 'Loading…' : 'No issues found'}</TableBodyCell>
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
				<span class="mono text-gray-500 text-sm">#{activeIssue?.number}</span>
				<span class="text-base font-semibold text-gray-100 truncate">{activeIssue?.title}</span>
			</div>
		{/snippet}
		{#if activeIssue}
			<IssueDetail issue={activeIssue} />
			<div class="text-right pt-2">
				<a href="/issues/{activeIssue.number}" class="text-xs text-blue-400 hover:text-blue-300">
					Open full page →
				</a>
			</div>
		{/if}
	</Modal>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
