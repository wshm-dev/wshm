<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchActivity, fetchIssues, fetchPulls, type ActivityEntry, type Issue, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge, Input, Modal } from 'flowbite-svelte';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';

	const PAGE_KEY = 'wshm.pageSize.activity';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let activities: ActivityEntry[] = $state([]);
	let error: string | null = $state(null);
	// True until the first fetch settles - stops the empty-state text from
	// flashing "No results" while the list is still loading.
	let loading = $state(true);
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

	let actionOptions = $derived(distinctValues(activities as unknown as Array<{ action?: string }>, 'action'));
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);

	// Race guard against repo-switch overwrites. See issues page for context.
	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchActivity({ limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			activities = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load activity';
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

	function actionColor(action: string): 'blue' | 'green' | 'yellow' | 'gray' {
		if (action === 'triage') return 'blue';
		if (action === 'merge') return 'green';
		if (action === 'analyze') return 'yellow';
		return 'gray';
	}

	let issueModalOpen = $state(false);
	let activeIssue: Issue | null = $state(null);
	let prModalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	let detailLoading = $state(false);
	let detailError: string | null = $state(null);

	async function openTarget(targetType: string, num: number) {
		detailError = null;
		detailLoading = true;
		const isPr = targetType === 'pr' || targetType === 'pull' || targetType === 'pull_request';
		if (isPr) {
			activePr = null;
			prModalOpen = true;
			try {
				const all = await fetchPulls({ limit: 500 });
				activePr = all.items.find((p) => p.number === num) ?? null;
				if (!activePr) detailError = `PR #${num} not found`;
			} catch (e) {
				detailError = e instanceof Error ? e.message : 'Failed to load';
			}
		} else {
			activeIssue = null;
			issueModalOpen = true;
			try {
				const all = await fetchIssues({ limit: 500 });
				activeIssue = all.items.find((i) => i.number === num) ?? null;
				if (!activeIssue) detailError = `Issue #${num} not found`;
			} catch (e) {
				detailError = e instanceof Error ? e.message : 'Failed to load';
			}
		}
		detailLoading = false;
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
	<div class="w-full overflow-x-auto">
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
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.created_at} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><FilterSelect bind:value={filters.action} options={actionOptions} /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.target} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
					<TableBodyCell class="px-2 py-1"><Input type="text" bind:value={filters.summary} placeholder="filter..." size="sm" class="!py-0.5 !px-1 text-xs" /></TableBodyCell>
				</TableBodyRow>
				{#each sorted as entry}
					<TableBodyRow class="cursor-pointer" onclick={() => openTarget(entry.target_type, entry.target_number)}>
						<TableBodyCell class="px-2 py-1.5 text-gray-500 whitespace-nowrap text-sm">{formatTime(entry.created_at)}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">
							<Badge color={actionColor(entry.action)}>{entry.action}</Badge>
						</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5 whitespace-nowrap mono">{entry.target_type} #{entry.target_number}</TableBodyCell>
						<TableBodyCell class="px-2 py-1.5">{entry.summary}</TableBodyCell>
					</TableBodyRow>
				{:else}
					<TableBodyRow>
						<TableBodyCell colspan={4} class="text-center text-gray-600 py-8">
							{#if loading}
								Loading…
							{:else}
								No activity recorded yet.
								<span class="block text-xs text-gray-500 mt-1">
									Activity appears here once wshm applies triage labels or PR analyses —
									sync your repos and enable features in <a href="/settings" class="text-blue-400 hover:underline">Settings → Repos</a>.
								</span>
							{/if}
						</TableBodyCell>
					</TableBodyRow>
				{/each}
			</TableBody>
		</Table>
	</div>
	<Modal
		bind:open={issueModalOpen}
		size="xl"
		dismissable
		class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
		bodyClass="text-gray-200"
	>
		{#snippet header()}
			<div class="flex w-full items-center gap-3 pr-2">
				<span class="mono text-gray-500 text-sm">#{activeIssue?.number ?? ''}</span>
				<span class="text-base font-semibold text-gray-100 truncate">
					{activeIssue?.title ?? (detailLoading ? 'Loading…' : '')}
				</span>
			</div>
		{/snippet}
		{#if detailLoading}
			<p class="text-gray-500 text-sm">Loading…</p>
		{:else if detailError}
			<p class="text-red-400 text-sm">{detailError}</p>
		{:else if activeIssue}
			<IssueDetail issue={activeIssue} />
			<div class="text-right pt-2">
				<a href="/issues/{activeIssue.number}" class="text-xs text-blue-400 hover:text-blue-300">
					Open full page →
				</a>
			</div>
		{/if}
	</Modal>

	<Modal
		bind:open={prModalOpen}
		size="xl"
		dismissable
		class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
		bodyClass="text-gray-200"
	>
		{#snippet header()}
			<div class="flex w-full items-center gap-3 pr-2">
				<span class="mono text-gray-500 text-sm">#{activePr?.number ?? ''}</span>
				<span class="text-base font-semibold text-gray-100 truncate">
					{activePr?.title ?? (detailLoading ? 'Loading…' : '')}
				</span>
			</div>
		{/snippet}
		{#if detailLoading}
			<p class="text-gray-500 text-sm">Loading…</p>
		{:else if detailError}
			<p class="text-red-400 text-sm">{detailError}</p>
		{:else if activePr}
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
