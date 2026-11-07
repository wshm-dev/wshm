<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo, itemRef } from '$lib/stores';
	import { fetchReviews, type ReviewEntry } from '$lib/api';
	import { multiSort, toggleSort as toggle, sortArrow, sortIndex, sortArrowClass, type SortColumn } from '$lib/sort';
	import { applyFilters, distinctValues } from '$lib/filter';
	import { paginate, totalPages } from '$lib/paginate';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import FilterSelect from '$lib/components/FilterSelect.svelte';
	import QuickFilters from '$lib/components/QuickFilters.svelte';
	import type { QuickChip } from '$lib/filter';

	let reviews: ReviewEntry[] = $state([]);
	let error: string | null = $state(null);
	let sortColumns: SortColumn[] = $state([{ key: 'reviewed_at', asc: false }]);
	let filters: Record<string, string> = $state({
		pr_number: '', title: '', findings: '', posted: '', ci_status: ''
	});
	let expanded: Set<string> = $state(new Set());

	function handleSort(key: string, event: MouseEvent) {
		sortColumns = toggle(sortColumns, key, event.shiftKey);
	}

	function rowKey(r: ReviewEntry): string {
		return `${r.repo}#${r.pr_number}`;
	}

	function toggleExpand(r: ReviewEntry) {
		const key = rowKey(r);
		const next = new Set(expanded);
		if (next.has(key)) {
			next.delete(key);
		} else {
			next.add(key);
		}
		expanded = next;
	}

	// Derived facets for filtering: worst finding severity and posting state.
	let enriched = $derived(reviews.map((r) => ({
		...r,
		findings: r.stats.errors > 0 ? 'errors' : r.stats.warnings > 0 ? 'warnings' : r.stats.infos > 0 ? 'infos' : 'clean',
		posted: r.posted_to_github ? 'posted' : 'web only'
	})));

	let filtered = $derived(applyFilters(enriched, {
		pr_number: filters.pr_number,
		title: filters.title,
		findings: filters.findings,
		posted: filters.posted,
		ci_status: filters.ci_status
	}));

	let findingsOptions = $derived(distinctValues(enriched, 'findings'));
	let postedOptions = $derived(distinctValues(enriched, 'posted'));
	let ciOptions = $derived(distinctValues(enriched, 'ci_status'));

	let sorted = $derived(multiSort(filtered, sortColumns));

	const chip = (label: string, patch: Record<string, string>): QuickChip =>
		({ label, patch, count: applyFilters(enriched, patch).length });
	let chips: QuickChip[] = $derived([
		chip('Errors', { findings: '=errors' }),
		chip('Warnings', { findings: '=warnings' }),
		chip('Clean', { findings: '=clean' }),
		chip('Web only', { posted: '=web only' }),
		chip('Posted to GitHub', { posted: '=posted' })
	]);
	let page = $state(0);
	let pages = $derived(totalPages(sorted.length));
	let paged = $derived(paginate(sorted, page));

	async function load() {
		page = 0;
		try {
			error = null;
			reviews = await fetchReviews();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load reviews';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function severityClass(severity: string): string {
		if (severity === 'error') return 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
		if (severity === 'warning') return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		return 'border-blue-500/30 bg-blue-500/15 text-blue-600 dark:text-blue-400';
	}

	function ciBadgeClass(status: string | null): string {
		if (status === 'success') return 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
		if (status === 'failure') return 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
		return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	}
</script>

<svelte:head>
	<title>wshm - Code Review</title>
</svelte:head>

	<div class="mb-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Code Reviews</h2>
		<p class="text-sm text-muted-foreground">AI-powered inline code review results for pull requests</p>
	</div>

	{#if error}
		<div class="rounded-lg border border-red-500 bg-card p-5">
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</div>
	{:else if reviews.length === 0}
		<div class="rounded-lg border bg-card p-10 text-center">
			<p class="text-muted-foreground">No code reviews yet. Enable "Review PRs" for a repo in Settings, or run <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm review --pr N --apply</code> to generate one.</p>
		</div>
	{:else}
		<QuickFilters {chips} bind:filters />
		<div class="rounded-lg border">
			<Table.Root class="w-full table-fixed">
				<Table.Header class="text-xs uppercase text-muted-foreground">
					<Table.Row>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[70px]" onclick={(e: MouseEvent) => handleSort('pr_number', e)}>
							PR <span class={sortArrowClass(sortColumns, 'pr_number')}>{sortArrow(sortColumns, 'pr_number')}</span>{#if sortIndex(sortColumns, 'pr_number') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'pr_number')}</span>{/if}
						</Table.Head>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5" onclick={(e: MouseEvent) => handleSort('title', e)}>
							Title <span class={sortArrowClass(sortColumns, 'title')}>{sortArrow(sortColumns, 'title')}</span>{#if sortIndex(sortColumns, 'title') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'title')}</span>{/if}
						</Table.Head>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[150px]" onclick={(e: MouseEvent) => handleSort('findings', e)}>
							Findings <span class={sortArrowClass(sortColumns, 'findings')}>{sortArrow(sortColumns, 'findings')}</span>{#if sortIndex(sortColumns, 'findings') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'findings')}</span>{/if}
						</Table.Head>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('posted', e)}>
							Posted <span class={sortArrowClass(sortColumns, 'posted')}>{sortArrow(sortColumns, 'posted')}</span>{#if sortIndex(sortColumns, 'posted') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'posted')}</span>{/if}
						</Table.Head>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[90px]" onclick={(e: MouseEvent) => handleSort('ci_status', e)}>
							CI <span class={sortArrowClass(sortColumns, 'ci_status')}>{sortArrow(sortColumns, 'ci_status')}</span>{#if sortIndex(sortColumns, 'ci_status') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'ci_status')}</span>{/if}
						</Table.Head>
						<Table.Head class="px-2 py-1.5 w-[130px]">Branch</Table.Head>
						<Table.Head class="cursor-pointer select-none px-2 py-1.5 w-[100px]" onclick={(e: MouseEvent) => handleSort('reviewed_at', e)}>
							Reviewed <span class={sortArrowClass(sortColumns, 'reviewed_at')}>{sortArrow(sortColumns, 'reviewed_at')}</span>{#if sortIndex(sortColumns, 'reviewed_at') > 0}<span class="text-[0.625rem] text-primary ml-0.5">{sortIndex(sortColumns, 'reviewed_at')}</span>{/if}
						</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					<Table.Row>
						<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.pr_number} placeholder="#" class="h-8 px-2 text-xs" /></Table.Cell>
						<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.title} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
						<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.findings} options={findingsOptions} /></Table.Cell>
						<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.posted} options={postedOptions} /></Table.Cell>
						<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.ci_status} options={ciOptions} /></Table.Cell>
						<Table.Cell class="px-2 py-1"></Table.Cell>
						<Table.Cell class="px-2 py-1"></Table.Cell>
					</Table.Row>
					{#each paged as r}
						<Table.Row class="cursor-pointer" onclick={() => toggleExpand(r)}>
							<Table.Cell class="px-2 py-1.5 font-mono text-primary">{$itemRef(r.pr_number)}</Table.Cell>
							<Table.Cell class="px-2 py-1.5 overflow-hidden">
								<div class="truncate">{r.title}</div>
								{#if r.summary}
									<div class="text-xs text-muted-foreground mt-1 truncate">{r.summary}</div>
								{/if}
							</Table.Cell>
							<Table.Cell>
								<div class="flex gap-1">
									{#if r.stats.errors > 0}
										<Badge variant="outline" class={severityClass('error')}>{r.stats.errors} err</Badge>
									{/if}
									{#if r.stats.warnings > 0}
										<Badge variant="outline" class={severityClass('warning')}>{r.stats.warnings} warn</Badge>
									{/if}
									{#if r.stats.infos > 0}
										<Badge variant="outline" class={severityClass('info')}>{r.stats.infos} info</Badge>
									{/if}
									{#if r.stats.errors === 0 && r.stats.warnings === 0 && r.stats.infos === 0}
										<span class="text-xs text-muted-foreground">clean</span>
									{/if}
								</div>
							</Table.Cell>
							<Table.Cell>
								{#if r.posted_to_github}
									<Badge variant="secondary">posted</Badge>
								{:else}
									<span class="text-xs text-muted-foreground">web only</span>
								{/if}
							</Table.Cell>
							<Table.Cell>
								{#if r.ci_status}
									<Badge variant="outline" class={ciBadgeClass(r.ci_status)}>{r.ci_status}</Badge>
								{:else}
									<span class="text-xs text-muted-foreground">—</span>
								{/if}
							</Table.Cell>
							<Table.Cell class="text-xs text-muted-foreground font-mono">{r.head_ref ?? ''}</Table.Cell>
							<Table.Cell class="text-xs text-muted-foreground">{r.reviewed_at?.slice(0, 10) ?? ''}</Table.Cell>
						</Table.Row>
						{#if expanded.has(rowKey(r))}
							<Table.Row>
								<Table.Cell colspan={7} class="bg-muted/20">
									{#if r.comments.length === 0}
										<p class="text-xs text-muted-foreground py-2">No inline findings.</p>
									{:else}
										<div class="flex flex-col gap-2 py-2">
											{#each r.comments as c}
												<div class="rounded border bg-card p-2 text-xs">
													<div class="flex items-center gap-2 mb-1">
														<Badge variant="outline" class={severityClass(c.severity)}>{c.severity}</Badge>
														<Badge variant="secondary">{c.category}</Badge>
														<span class="font-mono text-muted-foreground">{c.path}:{c.line}</span>
													</div>
													<p class="text-foreground">{c.body}</p>
													{#if c.suggestion}
														<pre class="mt-1 bg-muted/40 rounded p-2 overflow-x-auto">{c.suggestion}</pre>
													{/if}
												</div>
											{/each}
										</div>
									{/if}
								</Table.Cell>
							</Table.Row>
						{/if}
					{/each}
				</Table.Body>
			</Table.Root>
		</div>

		{#if pages > 1}
			<div class="flex justify-center gap-2 mt-4">
				<button disabled={page === 0} onclick={() => page--} class="px-3 py-1 text-xs rounded bg-muted disabled:opacity-40">&laquo;</button>
				<span class="text-xs text-muted-foreground py-1">{page + 1} / {pages}</span>
				<button disabled={page >= pages - 1} onclick={() => page++} class="px-3 py-1 text-xs rounded bg-muted disabled:opacity-40">&raquo;</button>
			</div>
		{/if}
	{/if}
