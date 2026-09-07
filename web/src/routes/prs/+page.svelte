<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tooltip from '$lib/components/ui/tooltip';
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

	const GREEN_BADGE = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const YELLOW_BADGE = 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	const RED_BADGE = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';

	function riskBadgeClass(risk: string | null): string {
		if (risk === 'low') return GREEN_BADGE;
		if (risk === 'medium') return YELLOW_BADGE;
		if (risk === 'high') return RED_BADGE;
		return '';
	}

	function ciBadgeClass(ci: string | null): string {
		if (ci === 'success') return GREEN_BADGE;
		if (ci === 'pending') return YELLOW_BADGE;
		if (ci === 'failure') return RED_BADGE;
		return '';
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
	<h2 class="text-xl font-semibold text-foreground mb-1">Pull Requests</h2>
	<p class="text-sm text-muted-foreground">All tracked pull requests from the repository</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500/50 bg-card p-5">
		<p class="text-red-600 dark:text-red-400">{error}</p>
	</div>
{:else}
	<div class="rounded-lg border">
		<Table.Root class="w-full table-fixed">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('number', e)}>
						# <span class={sortArrowClass(sortColumns, 'number')}>{sortArrow(sortColumns, 'number')}</span>{#if sortIndex(sortColumns, 'number') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'number')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('title', e)}>
						Title <span class={sortArrowClass(sortColumns, 'title')}>{sortArrow(sortColumns, 'title')}</span>{#if sortIndex(sortColumns, 'title') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'title')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('state', e)}>
						State <span class={sortArrowClass(sortColumns, 'state')}>{sortArrow(sortColumns, 'state')}</span>{#if sortIndex(sortColumns, 'state') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'state')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('base_ref', e)}>
						Base <span class={sortArrowClass(sortColumns, 'base_ref')}>{sortArrow(sortColumns, 'base_ref')}</span>{#if sortIndex(sortColumns, 'base_ref') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'base_ref')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('risk', e)}>
						Risk <span class={sortArrowClass(sortColumns, 'risk')}>{sortArrow(sortColumns, 'risk')}</span>{#if sortIndex(sortColumns, 'risk') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'risk')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('ci_status', e)}>
						CI <span class={sortArrowClass(sortColumns, 'ci_status')}>{sortArrow(sortColumns, 'ci_status')}</span>{#if sortIndex(sortColumns, 'ci_status') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'ci_status')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('conflicts', e)}>
						Conflicts <span class={sortArrowClass(sortColumns, 'conflicts')}>{sortArrow(sortColumns, 'conflicts')}</span>{#if sortIndex(sortColumns, 'conflicts') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'conflicts')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('age', e)}>
						Age <span class={sortArrowClass(sortColumns, 'age')}>{sortArrow(sortColumns, 'age')}</span>{#if sortIndex(sortColumns, 'age') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'age')}</span>{/if}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				<Table.Row>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.number} placeholder="#" class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.title} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.state} options={stateOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.base_ref} placeholder="main..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.risk} options={riskOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.ci_status} options={ciOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.conflicts} options={conflictsOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.age} placeholder=">N" class="h-8 px-2 text-xs" /></Table.Cell>
				</Table.Row>
				{#each sorted as pr}
					<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
						<Table.Cell class="px-2 py-1.5 mono">{pr.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<Tooltip.Provider>
								<Tooltip.Root>
									<Tooltip.Trigger>
										{#snippet child({ props })}
											<span {...props} class="block truncate">{pr.title}</span>
										{/snippet}
									</Tooltip.Trigger>
									<Tooltip.Content class="max-w-sm text-xs leading-snug">{pr.title}</Tooltip.Content>
								</Tooltip.Root>
							</Tooltip.Provider>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<Badge variant="outline" class={pr.state === 'open' ? GREEN_BADGE : RED_BADGE}>{pr.state}</Badge>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-xs mono text-muted-foreground">{pr.base_ref ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.risk}
								<Badge variant="outline" class={riskBadgeClass(pr.risk)}>{pr.risk}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.ci_status}
								<Badge variant="outline" class={ciBadgeClass(pr.ci_status)}>{pr.ci_status}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<!-- Only the actionable state gets color: a red badge for real
							     conflicts. "no" is the normal case (plain text) and unknown
							     mergeability is "-" instead of a misleading green "no". -->
							{#if pr.mergeable === false}
								<Badge variant="outline" class={RED_BADGE}>yes</Badge>
							{:else if pr.mergeable === true}
								<span class="text-muted-foreground text-xs">no</span>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground mono">{timeAgo(pr.created_at)}</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={7} class="text-center text-muted-foreground py-8">{loading ? 'Loading…' : 'No pull requests found'}</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number}</span>
					<span class="truncate">{activePr?.title}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activePr}
				<PrDetail pr={activePr} />
				<div class="text-right pt-2">
					<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">
						Open full page →
					</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
