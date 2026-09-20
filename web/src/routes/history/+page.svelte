<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchHistory, fetchIssues, fetchPulls, type HistoryEvent, type Issue, type PullRequest } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TablePagination from '$lib/components/TablePagination.svelte';
	import FilterSelect from '$lib/components/FilterSelect.svelte';
	import QuickFilters from '$lib/components/QuickFilters.svelte';
	import type { QuickChip } from '$lib/filter';

	const PAGE_KEY = 'wshm.pageSize.history';
	function readStoredLimit(): number {
		try {
			const raw = localStorage.getItem(PAGE_KEY);
			const n = raw ? Number(raw) : NaN;
			return Number.isFinite(n) && n > 0 ? n : 50;
		} catch {
			return 50;
		}
	}

	let events: HistoryEvent[] = $state([]);
	let error: string | null = $state(null);
	let loading = $state(true);
	let total = $state(0);
	let pageLimit = $state(readStoredLimit());
	let pageOffset = $state(0);

	// Server-side filters: the feed is paginated on the daemon, so a filter
	// must round-trip instead of narrowing the loaded page like list pages do.
	// Values use the exact-match form (`=x`) so QuickFilters / FilterSelect
	// share one vocabulary; `toParam` strips the marker before the request.
	let filters: Record<string, string> = $state({ kind: '', source: '', field: '', number: '' });
	const toParam = (v: string) => (v.startsWith('=') ? v.slice(1) : v).trim();

	const KIND_OPTIONS = ['issue', 'pr'];
	const SOURCE_OPTIONS = ['sync', 'webhook', 'wshm'];
	const FIELD_OPTIONS = ['labels', 'state', 'title', 'body', 'head_sha', 'base_ref', 'ci_status'];

	const chips: QuickChip[] = [
		{ label: 'wshm actions', patch: { source: '=wshm' } },
		{ label: 'Label changes', patch: { field: '=labels' } },
		{ label: 'State changes', patch: { field: '=state' } },
		{ label: 'Pushes', patch: { kind: '=pr', field: '=head_sha' } },
		{ label: 'CI transitions', patch: { field: '=ci_status' } }
	];

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const num = Number(toParam(filters.number));
			const data = await fetchHistory({
				limit: pageLimit,
				offset: pageOffset,
				kind: toParam(filters.kind) || undefined,
				source: toParam(filters.source) || undefined,
				field: toParam(filters.field) || undefined,
				number: Number.isFinite(num) && num > 0 ? num : undefined
			});
			if (myToken !== loadToken) return;
			events = data.items;
			total = data.total;
			pageLimit = data.limit;
			pageOffset = data.offset;
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load history';
		} finally {
			if (myToken === loadToken) loading = false;
		}
	}

	function onPageChange(next: { limit: number; offset: number }) {
		pageLimit = next.limit;
		pageOffset = next.offset;
		load();
	}

	// Any filter edit restarts from the first page.
	let filterKey = $derived(JSON.stringify(filters));
	let lastFilterKey: string | null = null;
	$effect(() => {
		const key = filterKey;
		if (lastFilterKey !== null && key !== lastFilterKey) {
			pageOffset = 0;
			load();
		}
		lastFilterKey = key;
	});

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => {
			pageOffset = 0;
			load();
		});
		return unsub;
	});

	function formatTime(iso: string): string {
		return new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
	}
	function dayOf(iso: string): string {
		return new Date(iso).toLocaleDateString([], { weekday: 'short', year: 'numeric', month: 'short', day: 'numeric' });
	}
	function sourceBadgeClass(source: string): string {
		if (source === 'wshm') return 'bg-primary/15 text-primary border-primary/30';
		if (source === 'webhook') return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		return '';
	}
	function parseLabels(v: string | null): string[] {
		if (!v) return [];
		try {
			const arr = JSON.parse(v);
			return Array.isArray(arr) ? arr.map(String) : [];
		} catch {
			return [];
		}
	}
	function labelDiff(ev: HistoryEvent): { added: string[]; removed: string[] } {
		const before = parseLabels(ev.old_value);
		const after = parseLabels(ev.new_value);
		return {
			added: after.filter((l) => !before.includes(l)),
			removed: before.filter((l) => !after.includes(l))
		};
	}
	function short(v: string | null, n = 60): string {
		if (v === null || v === undefined) return '∅';
		if (v === '') return '""';
		return v.length > n ? v.slice(0, n - 1) + '…' : v;
	}
	function bodySummary(ev: HistoryEvent): string {
		const a = ev.old_value?.length ?? 0;
		const b = ev.new_value?.length ?? 0;
		const delta = b - a;
		return `body edited (${delta >= 0 ? '+' : ''}${delta} chars, ${a} → ${b})`;
	}

	// Rows with a day separator when the calendar day changes (newest first).
	let rows = $derived(
		events.map((ev, i) => ({
			ev,
			day: dayOf(ev.observed_at),
			newDay: i === 0 || dayOf(events[i - 1].observed_at) !== dayOf(ev.observed_at)
		}))
	);

	let issueModalOpen = $state(false);
	let activeIssue: Issue | null = $state(null);
	let prModalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	let detailLoading = $state(false);
	let detailError: string | null = $state(null);

	async function openTarget(kind: string, num: number) {
		detailError = null;
		detailLoading = true;
		if (kind === 'pr') {
			activePr = null;
			prModalOpen = true;
			try {
				const all = await fetchPulls({ limit: 500 });
				activePr = all.items.find((p) => p.number === num) ?? null;
				if (!activePr) detailError = `PR #${num} is not in the open-PR cache (closed or merged?)`;
			} catch (e) {
				detailError = e instanceof Error ? e.message : 'Failed to load';
			}
		} else {
			activeIssue = null;
			issueModalOpen = true;
			try {
				const all = await fetchIssues({ limit: 500 });
				activeIssue = all.items.find((i) => i.number === num) ?? null;
				if (!activeIssue) detailError = `Issue #${num} is not in the open-issue cache (closed?)`;
			} catch (e) {
				detailError = e instanceof Error ? e.message : 'Failed to load';
			}
		}
		detailLoading = false;
	}
</script>

<svelte:head>
	<title>wshm - History</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">Change History</h2>
	<p class="text-sm text-muted-foreground">
		Every observed change on issues and pull requests — who changed what, and whether it was wshm or a human.
	</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500/50 bg-card p-5">
		<p class="text-red-600 dark:text-red-400">{error}</p>
	</div>
{:else}
	<QuickFilters {chips} bind:filters />
	<div class="rounded-lg border">
		<Table.Root class="w-full table-fixed">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[100px]">Time</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[110px]">Target</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[100px]">Field</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[100px]">Source</Table.Head>
					<Table.Head class="px-2 py-1.5">Change</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				<Table.Row>
					<Table.Cell class="px-2 py-1"><Input type="text" bind:value={filters.number} placeholder="#" class="h-8 px-2 text-xs" aria-label="Filter by number" /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.kind} options={KIND_OPTIONS} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.field} options={FIELD_OPTIONS} /></Table.Cell>
					<Table.Cell class="px-2 py-1"><FilterSelect bind:value={filters.source} options={SOURCE_OPTIONS} /></Table.Cell>
					<Table.Cell class="px-2 py-1 text-xs text-muted-foreground">
						{#if total > events.length}showing {events.length} of {total}{/if}
					</Table.Cell>
				</Table.Row>
				{#each rows as { ev, day, newDay } (ev.repo + ':' + ev.id)}
					{#if newDay}
						<Table.Row class="bg-muted/40 hover:bg-muted/40">
							<Table.Cell colspan={5} class="px-2 py-1 text-xs font-medium text-muted-foreground">{day}</Table.Cell>
						</Table.Row>
					{/if}
					<Table.Row class="cursor-pointer" onclick={() => openTarget(ev.kind, ev.number)}>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground whitespace-nowrap text-sm">{formatTime(ev.observed_at)}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 whitespace-nowrap mono text-sm" title={ev.repo}>{ev.kind === 'pr' ? 'PR' : 'issue'} #{ev.number}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-sm">{ev.field}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							<Badge variant="outline" class={sourceBadgeClass(ev.source)}>{ev.source}</Badge>
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-sm overflow-hidden">
							{#if ev.field === 'labels'}
								{@const d = labelDiff(ev)}
								<span class="flex flex-wrap gap-1">
									{#each d.added as l (l)}<Badge variant="outline" class="border-green-500/30 bg-green-500/15 text-green-700 dark:text-green-400">+ {l}</Badge>{/each}
									{#each d.removed as l (l)}<Badge variant="outline" class="border-red-500/30 bg-red-500/15 text-red-700 dark:text-red-400 line-through">− {l}</Badge>{/each}
									{#if d.added.length === 0 && d.removed.length === 0}<span class="text-muted-foreground">labels reordered</span>{/if}
								</span>
							{:else if ev.field === 'body'}
								<span class="text-muted-foreground" title={short(ev.new_value, 400)}>{bodySummary(ev)}</span>
							{:else if ev.field === 'head_sha'}
								<span class="mono" title="{ev.old_value ?? '∅'} → {ev.new_value ?? '∅'}">{short(ev.old_value, 8)} → {short(ev.new_value, 8)}</span>
							{:else}
								<span class="block truncate" title="{ev.old_value ?? '∅'} → {ev.new_value ?? '∅'}">
									<span class="text-muted-foreground line-through">{short(ev.old_value)}</span>
									<span class="text-muted-foreground"> → </span>
									<span>{short(ev.new_value)}</span>
								</span>
							{/if}
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={5} class="text-center text-muted-foreground py-8">
							{#if loading}
								Loading…
							{:else}
								No changes recorded yet.
								<span class="block text-xs text-muted-foreground mt-1">
									History fills in as syncs, webhooks and wshm actions modify issues and PRs. Only changes observed after this
									version was installed are captured; nothing is backfilled from the forge.
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
					<span class="truncate">{activeIssue?.title ?? (detailLoading ? 'Loading…' : '')}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if detailLoading}
				<p class="text-muted-foreground text-sm">Loading…</p>
			{:else if detailError}
				<p class="text-red-600 dark:text-red-400 text-sm">{detailError}</p>
			{:else if activeIssue}
				<IssueDetail issue={activeIssue} />
				<div class="text-right pt-2">
					<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:underline">Open full page →</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<Dialog.Root bind:open={prModalOpen}>
		<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
					<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number ?? ''}</span>
					<span class="truncate">{activePr?.title ?? (detailLoading ? 'Loading…' : '')}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if detailLoading}
				<p class="text-muted-foreground text-sm">Loading…</p>
			{:else if detailError}
				<p class="text-red-600 dark:text-red-400 text-sm">{detailError}</p>
			{:else if activePr}
				<PrDetail pr={activePr} />
				<div class="text-right pt-2">
					<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">Open full page →</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>

	<TablePagination {total} limit={pageLimit} offset={pageOffset} storageKey={PAGE_KEY} onChange={onPageChange} />
{/if}
