<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { selectedRepo } from '$lib/stores';
	import {
		fetchIssueInsights,
		type IssueInsightItem,
		type IssueInsights
	} from '$lib/api';
	import { t } from '$lib/i18n';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import XIcon from '@lucide/svelte/icons/x';

	let translate = $state<(k: string) => string>((k) => k);
	t.subscribe((fn) => (translate = fn));

	let data = $state<IssueInsights | null>(null);
	let error: string | null = $state(null);
	let proLocked = $state(false);
	let loading = $state(true);

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		loading = true;
		error = null;
		proLocked = false;
		try {
			const res = await fetchIssueInsights();
			if (myToken !== loadToken) return;
			data = res;
		} catch (e) {
			if (myToken !== loadToken) return;
			const msg = e instanceof Error ? e.message : 'Failed to load';
			if (msg.includes('403')) proLocked = true;
			else error = msg;
		}
		loading = false;
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => load());
		return unsub;
	});

	// URL-driven filters. Each key maps to one predicate; absent = no constraint.
	// Re-derived from the page store so back/forward buttons stay correct.
	let params = $derived($page.url.searchParams);
	let priorityFilter = $derived(params.get('priority'));
	let categoryFilter = $derived(params.get('category'));
	let triageFilter = $derived(params.get('triage')); // 'triaged' | 'untriaged'

	let items = $derived(data?.items ?? []);

	let filteredItems = $derived.by<IssueInsightItem[]>(() => {
		let out = items;
		if (triageFilter === 'triaged') out = out.filter((i) => i.triaged);
		else if (triageFilter === 'untriaged') out = out.filter((i) => !i.triaged);
		if (priorityFilter) {
			if (priorityFilter === 'none') out = out.filter((i) => i.triaged && i.priority == null);
			else out = out.filter((i) => i.priority === priorityFilter);
		}
		if (categoryFilter) {
			if (categoryFilter === '(uncategorized)') out = out.filter((i) => i.category == null);
			else out = out.filter((i) => i.category === categoryFilter);
		}
		return out;
	});

	type ActiveChip = { key: 'triage' | 'priority' | 'category'; label: string; value: string };
	let activeChips = $derived.by<ActiveChip[]>(() => {
		const chips: ActiveChip[] = [];
		if (triageFilter) {
			chips.push({
				key: 'triage',
				label: translate('issueInsights.list.filter.triage'),
				value: triageFilter
			});
		}
		if (priorityFilter) {
			chips.push({
				key: 'priority',
				label: translate('issueInsights.list.filter.priority'),
				value: priorityFilter
			});
		}
		if (categoryFilter) {
			chips.push({
				key: 'category',
				label: translate('issueInsights.list.filter.category'),
				value: categoryFilter
			});
		}
		return chips;
	});

	function removeFilter(key: ActiveChip['key']) {
		const next = new URLSearchParams(params);
		next.delete(key);
		goto(`?${next.toString()}`, { keepFocus: true, noScroll: true, replaceState: false });
	}

	function clearAll() {
		goto('?', { keepFocus: true, noScroll: true, replaceState: false });
	}

	function daysSince(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	const GREEN_BADGE = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const YELLOW_BADGE = 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	const RED_BADGE = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';

	function priorityBadgeClass(priority: string | null): string {
		if (priority === 'low') return GREEN_BADGE;
		if (priority === 'medium') return YELLOW_BADGE;
		if (priority === 'high') return RED_BADGE;
		return '';
	}

	// Issue detail modal (same UX as the parent overview page).
	let modalOpen = $state(false);
	let activeIssue = $state<IssueInsightItem | null>(null);
	function openIssue(issue: IssueInsightItem) {
		activeIssue = issue;
		modalOpen = true;
	}
</script>

<svelte:head>
	<title>wshm - Issues List</title>
</svelte:head>

<div class="mb-6 flex flex-wrap items-end justify-between gap-3">
	<div>
		<h2 class="text-xl font-semibold text-foreground mb-1">
			{translate('issueInsights.list.title')}
		</h2>
		<p class="text-sm text-muted-foreground">
			{translate('issueInsights.list.subtitle')}
		</p>
	</div>
	<Button
		size="xs"
		variant="outline"
		onclick={() => goto('/issue-insights')}
	>
		← {translate('issueInsights.list.backToOverview')}
	</Button>
</div>

{#snippet modalField(label: string, value: string)}
	<Card.Root class="gap-1 rounded-lg p-3">
		<div class="text-[0.625rem] uppercase text-muted-foreground">{label}</div>
		<span class="text-foreground text-sm">{value}</span>
	</Card.Root>
{/snippet}

{#if loading}
	<p class="text-sm text-muted-foreground">{translate('issueInsights.loading')}</p>
{:else if proLocked}
	<Card.Root class="border-primary/40 max-w-xl">
		<Card.Header>
			<Card.Title>{translate('issueInsights.proOnly.title')}</Card.Title>
		</Card.Header>
		<Card.Content>
			<p class="text-sm text-muted-foreground">{translate('issueInsights.proOnly.body')}</p>
		</Card.Content>
	</Card.Root>
{:else if error}
	<Card.Root class="border-destructive/50">
		<Card.Content>
			<p class="text-destructive">{error}</p>
		</Card.Content>
	</Card.Root>
{:else}
	<Card.Root class="gap-0 py-3 mb-4">
		<Card.Content class="px-4">
			<div class="flex flex-wrap items-center gap-2">
				<span class="text-xs uppercase tracking-wider text-muted-foreground mr-1">
					{translate('issueInsights.list.activeFilters')}
				</span>
				{#if activeChips.length === 0}
					<span class="text-sm text-muted-foreground">
						{translate('issueInsights.list.noFilters')}
					</span>
				{:else}
					{#each activeChips as chip (chip.key)}
						<Badge variant="outline" class="h-6 bg-primary/15 text-primary flex items-center gap-1">
							<span class="text-[0.65rem] uppercase text-primary/70">{chip.label}</span>
							<span class="mono">{chip.value}</span>
							<Button
								variant="ghost"
								size="icon-xs"
								class="ms-1 -me-1 size-4 hover:bg-primary/20 hover:text-primary"
								onclick={() => removeFilter(chip.key)}
							>
								<XIcon />
							</Button>
						</Badge>
					{/each}
					<Button size="xs" variant="outline" class="ms-2" onclick={clearAll}>
						{translate('issueInsights.list.clearAll')}
					</Button>
				{/if}
				<span class="ms-auto text-sm text-muted-foreground mono">
					{filteredItems.length} / {items.length}
				</span>
			</div>
		</Card.Content>
	</Card.Root>

	<div class="w-full overflow-x-auto rounded-lg border">
		<Table.Root class="w-full">
			<Table.Header class="text-xs uppercase text-muted-foreground">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[70px]">
						{translate('issueInsights.table.issue')}
					</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.repo')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.title')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('issueInsights.table.author')}</Table.Head>
					<Table.Head class="px-2 py-1.5">
						{translate('issueInsights.table.category')}
					</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">
						{translate('issueInsights.table.priority')}
					</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[70px]">
						{translate('issueInsights.table.age')}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each filteredItems as issue (issue.repo + '#' + issue.number)}
					<Table.Row class="cursor-pointer" onclick={() => openIssue(issue)}>
						<Table.Cell class="px-2 py-1.5 mono text-primary">
							#{issue.number}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-xs text-muted-foreground">
							{issue.repo}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 truncate max-w-[380px]">
							{issue.title}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground">
							{issue.author ?? '-'}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if issue.category}
								<Badge variant="secondary">{issue.category}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if issue.priority}
								<Badge variant="outline" class={priorityBadgeClass(issue.priority)}>{issue.priority}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-muted-foreground">
							{daysSince(issue.created_at)}d
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={7} class="text-center text-muted-foreground py-8">
							{translate('issueInsights.list.empty')}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>

	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-2xl max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex items-center gap-3 pr-6">
					<span class="mono text-muted-foreground text-sm">#{activeIssue?.number ?? ''}</span>
					<span class="text-base font-semibold text-foreground truncate">
						{activeIssue?.title ?? ''}
					</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activeIssue}
				<div class="mb-4 flex flex-wrap items-center gap-2">
					<Badge
						variant={activeIssue.triaged ? 'outline' : 'secondary'}
						class={activeIssue.triaged ? GREEN_BADGE : ''}
					>
						{translate(
							activeIssue.triaged
								? 'issueInsights.legend.triaged'
								: 'issueInsights.legend.untriaged'
						)}
					</Badge>
					{#if activeIssue.category}
						<Badge variant="secondary">{activeIssue.category}</Badge>
					{/if}
					{#if activeIssue.priority}
						<Badge variant="outline" class={priorityBadgeClass(activeIssue.priority)}>{activeIssue.priority}</Badge>
					{/if}
				</div>

				<div class="grid grid-cols-2 md:grid-cols-3 gap-3 mb-4">
					{@render modalField(translate('issueInsights.table.repo'), activeIssue.repo)}
					{@render modalField(translate('issueInsights.table.author'), activeIssue.author ?? '-')}
					{@render modalField(
						translate('issueInsights.modal.confidence'),
						activeIssue.confidence != null
							? `${Math.round(activeIssue.confidence * 100)}%`
							: '-'
					)}
					{@render modalField(
						translate('issueInsights.modal.lastActivity'),
						`${daysSince(activeIssue.updated_at)}d · ${activeIssue.updated_at.slice(0, 10)}`
					)}
					{@render modalField(
						translate('issueInsights.modal.created'),
						activeIssue.created_at.slice(0, 10)
					)}
					{@render modalField(
						translate('issueInsights.modal.labels'),
						activeIssue.labels.length > 0 ? activeIssue.labels.join(', ') : '-'
					)}
				</div>

				<Card.Root class="gap-1 rounded-lg p-3 mb-4">
					<div class="text-[0.625rem] uppercase text-muted-foreground">
						{translate('issueInsights.modal.summary')}
					</div>
					<p class="text-sm text-foreground/90 whitespace-pre-wrap break-words">
						{activeIssue.summary ?? translate('issueInsights.modal.noSummary')}
					</p>
				</Card.Root>

				{#if activeIssue.url}
					<a
						href={activeIssue.url}
						target="_blank"
						rel="noopener noreferrer"
						class="text-sm text-primary hover:text-primary/80 underline"
					>
						{translate('issueInsights.modal.openOnForge')} →
					</a>
				{/if}
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
