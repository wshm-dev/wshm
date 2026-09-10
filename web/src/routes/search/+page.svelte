<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { goto } from '$app/navigation';
	import {
		searchAll,
		fetchIssues,
		fetchPulls,
		type SearchHit,
		type Issue,
		type PullRequest
	} from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Alert from '$lib/components/ui/alert';
	import * as Card from '$lib/components/ui/card';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Table from '$lib/components/ui/table';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';
	import { applyFilters, distinctValues } from '$lib/filter';

	const PAGE_KEY = 'wshm.pageSize.search';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let q: string = $state(($page.url.searchParams.get('q') ?? '').trim());
	let qInput: string = $state(q);
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);
	let total = $state(0);
	let hits: SearchHit[] = $state([]);
	let loading = $state(false);
	let error: string | null = $state(null);
	let filters: Record<string, string> = $state({ kind: '', repo: '', match: '' });

	let enriched = $derived(hits.map(h => ({
		...h,
		kindLabel: kindLabel(h.kind),
		matchText: `${h.title ?? ''} ${(h.snippet ?? '').replace(/<[^>]+>/g, '')}`
	})));
	let filtered = $derived(applyFilters(enriched, {
		kindLabel: filters.kind,
		repo: filters.repo,
		matchText: filters.match
	}));
	let kindOptions = $derived(distinctValues(enriched, 'kindLabel'));
	let repoOptions = $derived(distinctValues(enriched, 'repo'));

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		if (!q) {
			hits = [];
			total = 0;
			return;
		}
		loading = true;
		error = null;
		try {
			const data = await searchAll({ q, limit: pageLimit, offset: pageOffset });
			if (myToken !== loadToken) return;
			hits = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Search failed';
		}
		if (myToken === loadToken) loading = false;
	}

	function onSubmit(event: Event) {
		event.preventDefault();
		q = qInput.trim();
		pageOffset = 0;
		const url = new URL($page.url);
		if (q) url.searchParams.set('q', q);
		else url.searchParams.delete('q');
		goto(url.pathname + url.search, { replaceState: true, keepFocus: true });
		load();
	}

	function onPageChange(next: { limit: number; offset: number }) {
		pageLimit = next.limit;
		pageOffset = next.offset;
		load();
	}

	function kindLabel(k: SearchHit['kind']): string {
		return { issue: 'Issue', pull: 'PR', triage: 'Triage', comment: 'Comment' }[k];
	}
	function kindBadgeClass(k: SearchHit['kind']): string | null {
		return {
			issue: 'bg-primary/15 text-primary',
			pull: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400',
			triage: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
			comment: null
		}[k];
	}

	let modalOpen = $state(false);
	let modalKind: SearchHit['kind'] | null = $state(null);
	let activeIssue: Issue | null = $state(null);
	let activePr: PullRequest | null = $state(null);
	let detailLoading = $state(false);
	let detailError: string | null = $state(null);

	async function openHit(hit: SearchHit) {
		modalKind = hit.kind === 'comment' ? 'issue' : hit.kind === 'triage' ? 'issue' : hit.kind;
		modalOpen = true;
		activeIssue = null;
		activePr = null;
		detailLoading = true;
		detailError = null;
		try {
			if (modalKind === 'pull') {
				const all = await fetchPulls({ limit: 500 });
				activePr = all.items.find((p) => p.repo === hit.repo && p.number === hit.number) ?? null;
				if (!activePr) detailError = `PR #${hit.number} not found`;
			} else {
				const all = await fetchIssues({ limit: 500 });
				activeIssue =
					all.items.find((i) => i.repo === hit.repo && i.number === hit.number) ?? null;
				if (!activeIssue) detailError = `Issue #${hit.number} not found`;
			}
		} catch (e) {
			detailError = e instanceof Error ? e.message : 'Failed to load';
		}
		detailLoading = false;
	}

	onMount(() => {
		load();
	});
</script>

<svelte:head>
	<title>wshm - Search{q ? ` · ${q}` : ''}</title>
</svelte:head>

<div class="mb-4">
	<h2 class="text-xl font-semibold text-foreground mb-1">Search</h2>
	<p class="text-sm text-muted-foreground">
		Full-text search across issues, pull requests, triage results, and comments.
	</p>
</div>

<form onsubmit={onSubmit} class="mb-4 flex gap-2">
	<Input
		type="search"
		bind:value={qInput}
		placeholder="Search… (e.g. 'hermes', 'oauth flow', 'cve-2025-')"
		class="flex-1"
	/>
	<Button type="submit" disabled={loading}>
		{loading ? 'Searching…' : 'Search'}
	</Button>
</form>

{#if error}
	<Alert.Root variant="destructive" class="mb-3">
		<Alert.Description>{error}</Alert.Description>
	</Alert.Root>
{/if}

{#if !q && !loading}
	<Card.Root>
		<Card.Content>
			<p class="text-muted-foreground text-center py-4 text-sm">
				Type a query above to search across all your repos. Multi-word queries
				narrow (AND); each token does a prefix match (<code>hermes</code> matches
				<code>hermes-agent-cli</code>).
			</p>
		</Card.Content>
	</Card.Root>
{:else if total === 0 && !loading && q}
	<Card.Root>
		<Card.Content>
			<p class="text-muted-foreground text-center py-4 text-sm">
				No matches for <code>{q}</code>.
			</p>
		</Card.Content>
	</Card.Root>
{:else}
	<div class="w-full overflow-x-auto rounded-lg border">
		<Table.Root class="w-full table-fixed">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[80px]">Kind</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[200px]">Repo</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">#</Table.Head>
					<Table.Head class="px-2 py-1.5">Match</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[160px]">Updated</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				<Table.Row>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.kind} options={kindOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.repo} options={repoOptions} /></Table.Cell>
					<Table.Cell class="px-2 py-1"></Table.Cell>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.match} placeholder="filter..." class="h-8 px-2 text-xs" /></Table.Cell>
					<Table.Cell class="px-2 py-1"></Table.Cell>
				</Table.Row>
				{#each filtered as hit}
					<Table.Row class="cursor-pointer" onclick={() => openHit(hit)}>
						<Table.Cell class="px-2 py-1.5">
							{#if kindBadgeClass(hit.kind)}
								<Badge variant="outline" class={kindBadgeClass(hit.kind)}>{kindLabel(hit.kind)}</Badge>
							{:else}
								<Badge variant="secondary">{kindLabel(hit.kind)}</Badge>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-xs text-muted-foreground">{hit.repo}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono">#{hit.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-sm">
							{#if hit.title}
								<div class="font-semibold text-foreground truncate">{hit.title}</div>
							{/if}
							{#if hit.snippet}
								<!-- snippet contains <mark>…</mark> from FTS5 — render as HTML -->
								<div class="text-xs text-muted-foreground truncate">{@html hit.snippet}</div>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-xs text-muted-foreground mono whitespace-nowrap">
							{hit.updated_at?.slice(0, 10) ?? ''}
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={5} class="text-center text-muted-foreground py-8">
							{loading ? 'Searching…' : 'No matches'}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>

	<TablePagination
		{total}
		limit={pageLimit}
		offset={pageOffset}
		storageKey={PAGE_KEY}
		onChange={onPageChange}
		filteredCount={filtered.length}
	/>
{/if}

<Dialog.Root bind:open={modalOpen}>
	<Dialog.Content class="max-h-[85vh] w-[80vw] max-w-[80vw] overflow-y-auto sm:max-w-[80vw]">
		<Dialog.Header>
			<Dialog.Title class="flex w-full items-center gap-3 pr-2">
				<span class="mono text-muted-foreground text-sm">
					{modalKind === 'pull' ? 'PR' : 'Issue'} #{activePr?.number ?? activeIssue?.number ?? ''}
				</span>
				<span class="text-base font-semibold text-foreground truncate">
					{activePr?.title ?? activeIssue?.title ?? (detailLoading ? 'Loading…' : '')}
				</span>
			</Dialog.Title>
		</Dialog.Header>
		{#if detailLoading}
			<p class="text-muted-foreground text-sm">Loading…</p>
		{:else if detailError}
			<p class="text-red-600 dark:text-red-400 text-sm">{detailError}</p>
		{:else if modalKind === 'pull' && activePr}
			<PrDetail pr={activePr} />
			<div class="text-right pt-2">
				<a href="/prs/{activePr.number}" class="text-xs text-primary hover:text-primary/80">
					Open full page →
				</a>
			</div>
		{:else if activeIssue}
			<IssueDetail issue={activeIssue} />
			<div class="text-right pt-2">
				<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:text-primary/80">
					Open full page →
				</a>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
