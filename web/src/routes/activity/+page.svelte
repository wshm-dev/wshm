<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchActivity, fetchIssues, fetchPulls, type ActivityEntry, type Issue, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
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

	function actionBadgeClass(action: string): string {
		if (action === 'triage') return 'bg-primary/15 text-primary';
		if (action === 'merge') return 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
		if (action === 'analyze') return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		return '';
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
	<h2 class="text-xl font-semibold text-foreground mb-1">Activity Log</h2>
	<p class="text-sm text-muted-foreground">Recent triage and analysis actions</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500/50 bg-card p-5">
		<p class="text-red-600 dark:text-red-400">{error}</p>
	</div>
{:else}
	<div class="rounded-lg border">
		<Table.Root class="w-full">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[180px]" onclick={(e: MouseEvent) => handleSort('created_at', e)}>
						Time <span class={sortArrowClass(sortColumns, 'created_at')}>{sortArrow(sortColumns, 'created_at')}</span>{#if sortIndex(sortColumns, 'created_at') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'created_at')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('action', e)}>
						Action <span class={sortArrowClass(sortColumns, 'action')}>{sortArrow(sortColumns, 'action')}</span>{#if sortIndex(sortColumns, 'action') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'action')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[120px]" onclick={(e: MouseEvent) => handleSort('target', e)}>
						Target <span class={sortArrowClass(sortColumns, 'target')}>{sortArrow(sortColumns, 'target')}</span>{#if sortIndex(sortColumns, 'target') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'target')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('summary', e)}>
						Summary <span class={sortArrowClass(sortColumns, 'summary')}>{sortArrow(sortColumns, 'summary')}</span>{#if sortIndex(sortColumns, 'summary') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'summary')}</span>{/if}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				<Table.Row>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.created_at} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.action} options={actionOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.target} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.summary} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
				</Table.Row>
				{#each sorted as entry}
					<Table.Row class="cursor-pointer" onclick={() => openTarget(entry.target_type, entry.target_number)}>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground whitespace-nowrap text-sm">{formatTime(entry.created_at)}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<Badge variant="outline" class={actionBadgeClass(entry.action)}>{entry.action}</Badge>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 whitespace-nowrap mono">{entry.target_type} #{entry.target_number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">{entry.summary}</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={4} class="text-center text-muted-foreground py-8">
							{#if loading}
								Loading…
							{:else}
								No activity recorded yet.
								<span class="block text-xs text-muted-foreground mt-1">
									Activity appears here once wshm applies triage labels or PR analyses —
									sync your repos and enable features in <a href="/settings" class="text-primary hover:underline">Settings → Repos</a>.
								</span>
							{/if}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
	<Dialog.Root bind:open={issueModalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activeIssue?.number ?? ''}</span>
					<span class="truncate">
						{activeIssue?.title ?? (detailLoading ? 'Loading…' : '')}
					</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if detailLoading}
				<p class="text-muted-foreground text-sm">Loading…</p>
			{:else if detailError}
				<p class="text-red-600 dark:text-red-400 text-sm">{detailError}</p>
			{:else if activeIssue}
				<IssueDetail issue={activeIssue} />
				<div class="text-right pt-2">
					<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:underline">
						Open full page →
					</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<Dialog.Root bind:open={prModalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number ?? ''}</span>
					<span class="truncate">
						{activePr?.title ?? (detailLoading ? 'Loading…' : '')}
					</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if detailLoading}
				<p class="text-muted-foreground text-sm">Loading…</p>
			{:else if detailError}
				<p class="text-red-600 dark:text-red-400 text-sm">{detailError}</p>
			{:else if activePr}
				<PrDetail pr={activePr} />
				<div class="text-right pt-2">
					<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">
						Open full page →
					</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} filteredCount={sorted.length} />
{/if}
