<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchQueue, fetchPulls, type QueueEntry, type PullRequest } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';

	const PAGE_KEY = 'wshm.pageSize.queue';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let entries: QueueEntry[] = $state([]);
	let error: string | null = $state(null);
	// True until the first fetch settles - stops the empty-state text from
	// flashing "No results" while the list is still loading.
	let loading = $state(true);
	let sortColumns: SortColumn[] = $state([{ key: 'score', asc: false }]);
	let filters: Record<string, string> = $state({
		number: '', title: '', score: '', ci: '', conflicts: '', risk: ''
	});

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	let enriched = $derived(entries.map(e => ({
		...e,
		ci: e.ci_status === 'success' ? 'passing' : (e.ci_status ? 'failing' : 'unknown'),
		conflicts: e.mergeable === false ? 'yes' : (e.mergeable === true ? 'no' : 'unknown'),
		risk: e.risk_level ?? ''
	})));

	let filtered = $derived(applyFilters(enriched, {
		number: filters.number,
		title: filters.title,
		score: filters.score,
		ci: filters.ci,
		conflicts: filters.conflicts,
		risk: filters.risk
	}));

	let sorted = $derived(multiSort(filtered, sortColumns));

	let ciOptions = $derived(distinctValues(enriched, 'ci'));
	let conflictsOptions = $derived(distinctValues(enriched, 'conflicts'));
	let riskOptions = $derived(distinctValues(enriched, 'risk'));
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);

	// Race guard against repo-switch overwrites. See issues page for context.
	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchQueue({ limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			entries = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load merge queue';
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

	function scoreColor(score: number): string {
		if (score >= 15) return 'text-green-600 dark:text-green-400';
		if (score >= 5) return 'text-yellow-600 dark:text-yellow-400';
		return 'text-red-600 dark:text-red-400';
	}

	const GREEN_BADGE = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const YELLOW_BADGE = 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	const RED_BADGE = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';

	function riskBadgeClass(risk: string | null): string {
		if (risk === 'low') return GREEN_BADGE;
		if (risk === 'medium') return YELLOW_BADGE;
		if (risk === 'high') return RED_BADGE;
		return '';
	}

	let modalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	let prLoading = $state(false);
	let prError: string | null = $state(null);

	async function openPr(num: number) {
		modalOpen = true;
		activePr = null;
		prError = null;
		prLoading = true;
		try {
			const all = await fetchPulls({ limit: 500 });
			activePr = all.items.find((p) => p.number === num) ?? null;
			if (!activePr) prError = `PR #${num} not found`;
		} catch (e) {
			prError = e instanceof Error ? e.message : 'Failed to load';
		}
		prLoading = false;
	}
</script>

<svelte:head>
	<title>wshm - Merge Queue</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">Merge Queue</h2>
	<p class="text-sm text-muted-foreground">Pull requests ranked by merge readiness score</p>
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
					<Table.Head class="px-2 py-1.5 w-[50px]">Rank</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('number', e)}>
						PR <span class={sortArrowClass(sortColumns, 'number')}>{sortArrow(sortColumns, 'number')}</span>{#if sortIndex(sortColumns, 'number') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'number')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('title', e)}>
						Title <span class={sortArrowClass(sortColumns, 'title')}>{sortArrow(sortColumns, 'title')}</span>{#if sortIndex(sortColumns, 'title') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'title')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[60px]" onclick={(e: MouseEvent) => handleSort('score', e)}>
						Score <span class={sortArrowClass(sortColumns, 'score')}>{sortArrow(sortColumns, 'score')}</span>{#if sortIndex(sortColumns, 'score') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'score')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('ci', e)}>
						CI <span class={sortArrowClass(sortColumns, 'ci')}>{sortArrow(sortColumns, 'ci')}</span>{#if sortIndex(sortColumns, 'ci') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'ci')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('conflicts', e)}>
						Conflicts <span class={sortArrowClass(sortColumns, 'conflicts')}>{sortArrow(sortColumns, 'conflicts')}</span>{#if sortIndex(sortColumns, 'conflicts') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'conflicts')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('risk', e)}>
						Risk <span class={sortArrowClass(sortColumns, 'risk')}>{sortArrow(sortColumns, 'risk')}</span>{#if sortIndex(sortColumns, 'risk') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'risk')}</span>{/if}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				<Table.Row>
					<Table.Cell class="px-2 py-1"></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.number} placeholder="#" class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.title} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.score} placeholder=">15" class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.ci} options={ciOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.conflicts} options={conflictsOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.risk} options={riskOptions} /></Table.Cell>
				</Table.Row>
				{#each sorted as entry, i}
					<Table.Row class="cursor-pointer" onclick={() => openPr(entry.number)}>
						<Table.Cell class="px-2 py-1.5 mono text-muted-foreground font-bold text-sm">{i + 1}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono">#{entry.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 truncate">{entry.title}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<span class="mono font-bold {scoreColor(entry.score)}">{entry.score}</span>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if entry.ci_status === 'success'}
								<Badge variant="outline" class={GREEN_BADGE}>passing</Badge>
							{:else if entry.ci_status}
								<Badge variant="outline" class={RED_BADGE}>failing</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if entry.mergeable === false}
								<Badge variant="outline" class={RED_BADGE}>yes</Badge>
							{:else if entry.mergeable === true}
								<Badge variant="outline" class={GREEN_BADGE}>no</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if entry.risk_level}
								<Badge variant="outline" class={riskBadgeClass(entry.risk_level)}>{entry.risk_level}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={7} class="text-center text-muted-foreground py-8">{loading ? 'Loading…' : 'No pull requests in queue'}</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number ?? ''}</span>
					<span class="truncate">
						{activePr?.title ?? (prLoading ? 'Loading…' : '')}
					</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if prLoading}
				<p class="text-muted-foreground text-sm">Loading…</p>
			{:else if prError}
				<p class="text-red-600 dark:text-red-400 text-sm">{prError}</p>
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
