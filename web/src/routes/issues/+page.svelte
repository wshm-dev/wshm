<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchIssues, type Issue } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tooltip from '$lib/components/ui/tooltip';
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
	<h2 class="text-xl font-semibold text-foreground mb-1">Issues</h2>
	<p class="text-sm text-muted-foreground">All tracked issues from the repository</p>
</div>

{#if error}
	<Card.Root class="border-red-500/50">
		<Card.Content>
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</Card.Content>
	</Card.Root>
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
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('pr_status', e)}>
						PR <span class={sortArrowClass(sortColumns, 'pr_status')}>{sortArrow(sortColumns, 'pr_status')}</span>{#if sortIndex(sortColumns, 'pr_status') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'pr_status')}</span>{/if}
					</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[140px]">Labels</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[80px]" onclick={(e: MouseEvent) => handleSort('priority', e)}>
						Priority <span class={sortArrowClass(sortColumns, 'priority')}>{sortArrow(sortColumns, 'priority')}</span>{#if sortIndex(sortColumns, 'priority') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'priority')}</span>{/if}
					</Table.Head>
					<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('category', e)}>
						Category <span class={sortArrowClass(sortColumns, 'category')}>{sortArrow(sortColumns, 'category')}</span>{#if sortIndex(sortColumns, 'category') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'category')}</span>{/if}
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
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.pr_status} options={prStatusOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.labels} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.priority} options={priorityOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.category} options={categoryOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.age} placeholder=">N" class="h-8 px-2 text-xs" /></Table.Cell>
				</Table.Row>
				{#each sorted as issue}
					<Table.Row
						class="cursor-pointer"
						style="border-left: 3px solid {prStatusBorder(colors, issue.pr_status ?? 'no_pr')}; background-color: {prStatusBorder(colors, issue.pr_status ?? 'no_pr')}18;"
						onclick={() => openIssue(issue)}
					>
						<Table.Cell class="px-2 py-1.5 mono text-foreground">{issue.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-foreground">
							<Tooltip.Provider>
								<Tooltip.Root>
									<Tooltip.Trigger>
										{#snippet child({ props })}
											<span {...props} class="block truncate">{issue.title}</span>
										{/snippet}
									</Tooltip.Trigger>
									<Tooltip.Content class="max-w-sm text-xs leading-snug">{issue.title}</Tooltip.Content>
								</Tooltip.Root>
							</Tooltip.Provider>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-foreground text-xs">
							{issue.pr_status === 'pr_ready' ? 'PR ready' : issue.pr_status === 'has_pr' ? 'PR open' : 'No PR'}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 overflow-hidden whitespace-nowrap">
							{#each issue.labels.slice(0, 2) as label}
								<Badge variant="outline" class="bg-primary/15 text-primary mr-1">{label}</Badge>
							{/each}
							{#if issue.labels.length > 2}
								<Badge variant="outline" class="text-muted-foreground">+{issue.labels.length - 2}</Badge>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-foreground">{issue.priority ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-foreground">{issue.category ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground mono">{timeAgo(issue.created_at)}</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={8} class="text-center text-muted-foreground py-8">{loading ? 'Loading…' : 'No issues found'}</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>
	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activeIssue?.number}</span>
					<span class="truncate">{activeIssue?.title}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activeIssue}
				<IssueDetail issue={activeIssue} />
				<div class="text-right pt-2">
					<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:underline">
						Open full page →
					</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
