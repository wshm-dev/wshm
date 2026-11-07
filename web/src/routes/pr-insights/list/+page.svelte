<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import { selectedRepo, itemRef } from '$lib/stores';
	import {
		fetchPrInsights,
		type PrInsightItem,
		type PrInsights
	} from '$lib/api';
	import { t } from '$lib/i18n';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Table from '$lib/components/ui/table';
	import XIcon from '@lucide/svelte/icons/x';

	let translate = $state<(k: string) => string>((k) => k);
	t.subscribe((fn) => (translate = fn));

	let data = $state<PrInsights | null>(null);
	let error: string | null = $state(null);
	let loading = $state(true);

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		loading = true;
		error = null;
		try {
			const res = await fetchPrInsights();
			if (myToken !== loadToken) return;
			data = res;
		} catch (e) {
			if (myToken !== loadToken) return;
			const msg = e instanceof Error ? e.message : 'Failed to load';
			error = msg;
		}
		loading = false;
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => load());
		return unsub;
	});

	// URL-driven filters. Each key maps to one predicate; absent = no constraint.
	let params = $derived($page.url.searchParams);
	let statusFilter = $derived(params.get('status')); // 'mergeable' | 'conflict' | 'unknown'
	let branchFilter = $derived(params.get('branch'));
	let ageFilter = $derived(params.get('age')); // 'lt1w' | '1to4w' | '1to3m' | 'gt3m'
	let targetFilter = $derived(params.get('target')); // 'default' | 'other'

	let items = $derived(data?.items ?? []);
	let defaults = $derived(new Set(data?.default_branches ?? []));

	function daysSince(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	function ageBucket(days: number): 'lt1w' | '1to4w' | '1to3m' | 'gt3m' {
		if (days < 7) return 'lt1w';
		if (days < 30) return '1to4w';
		if (days < 90) return '1to3m';
		return 'gt3m';
	}

	let filteredItems = $derived.by<PrInsightItem[]>(() => {
		let out = items;
		if (statusFilter) out = out.filter((p) => p.status === statusFilter);
		if (branchFilter) {
			if (branchFilter === '(unknown)') out = out.filter((p) => p.base_ref == null);
			else out = out.filter((p) => p.base_ref === branchFilter);
		}
		if (ageFilter) out = out.filter((p) => ageBucket(daysSince(p.updated_at)) === ageFilter);
		if (targetFilter === 'default')
			out = out.filter((p) => p.base_ref != null && defaults.has(p.base_ref));
		else if (targetFilter === 'other')
			out = out.filter((p) => p.base_ref == null || !defaults.has(p.base_ref));
		return out;
	});

	type ActiveChip = {
		key: 'status' | 'branch' | 'age' | 'target';
		label: string;
		value: string;
	};
	let activeChips = $derived.by<ActiveChip[]>(() => {
		const chips: ActiveChip[] = [];
		if (statusFilter) {
			chips.push({
				key: 'status',
				label: translate('prInsights.list.filter.status'),
				value: statusFilter
			});
		}
		if (branchFilter) {
			chips.push({
				key: 'branch',
				label: translate('prInsights.list.filter.branch'),
				value: branchFilter
			});
		}
		if (ageFilter) {
			chips.push({
				key: 'age',
				label: translate('prInsights.list.filter.age'),
				value: translate('prInsights.age.' + ageFilter)
			});
		}
		if (targetFilter) {
			chips.push({
				key: 'target',
				label: translate('prInsights.list.filter.target'),
				value: targetFilter
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

	// Status-colour badge classes (applied with variant="outline").
	const BADGE_CLASSES: Record<'green' | 'red' | 'yellow' | 'blue' | 'gray', string> = {
		green: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400',
		red: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400',
		yellow: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
		blue: 'border-transparent bg-primary/15 text-primary',
		gray: 'border-transparent bg-secondary text-secondary-foreground'
	};

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
	}

	function ciColor(ci: string | null): 'green' | 'red' | 'yellow' | 'gray' {
		if (ci === 'success') return 'green';
		if (ci === 'failure') return 'red';
		if (ci === 'pending') return 'yellow';
		return 'gray';
	}

	function statusColor(status: string): 'green' | 'red' | 'gray' {
		if (status === 'mergeable') return 'green';
		if (status === 'conflict') return 'red';
		return 'gray';
	}

	// PR detail modal.
	let modalOpen = $state(false);
	let activePr = $state<PrInsightItem | null>(null);
	function openPr(pr: PrInsightItem) {
		activePr = pr;
		modalOpen = true;
	}
</script>

<svelte:head>
	<title>wshm - PRs List</title>
</svelte:head>

<div class="mb-6 flex flex-wrap items-end justify-between gap-3">
	<div>
		<h2 class="text-xl font-semibold text-foreground mb-1">
			{translate('prInsights.list.title')}
		</h2>
		<p class="text-sm text-muted-foreground">{translate('prInsights.list.subtitle')}</p>
	</div>
	<Button size="sm" variant="outline" onclick={() => goto('/pr-insights')}>
		← {translate('prInsights.list.backToOverview')}
	</Button>
</div>

{#snippet modalField(label: string, value: string)}
	<Card.Root class="p-3">
		<Card.Content class="p-0">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">{label}</div>
			<span class="text-foreground text-sm">{value}</span>
		</Card.Content>
	</Card.Root>
{/snippet}

{#if loading}
	<p class="text-sm text-muted-foreground">{translate('prInsights.loading')}</p>
{:else if error}
	<Card.Root class="border-destructive">
		<Card.Content>
			<p class="text-destructive">{error}</p>
		</Card.Content>
	</Card.Root>
{:else}
	<Card.Root class="mb-4 py-4">
		<Card.Content class="px-4">
			<div class="flex flex-wrap items-center gap-2">
				<span class="text-xs uppercase tracking-wider text-muted-foreground mr-1">
					{translate('prInsights.list.activeFilters')}
				</span>
				{#if activeChips.length === 0}
					<span class="text-sm text-muted-foreground">
						{translate('prInsights.list.noFilters')}
					</span>
				{:else}
					{#each activeChips as chip (chip.key)}
						<Badge variant="outline" class="{BADGE_CLASSES.blue} flex items-center gap-1">
							<span class="text-[0.65rem] uppercase text-primary/70">{chip.label}</span>
							<span class="mono">{chip.value}</span>
							<Button
								variant="ghost"
								size="icon-xs"
								class="ms-1 -me-1 size-4 text-primary/70 hover:text-primary"
								onclick={() => removeFilter(chip.key)}
							>
								<XIcon />
							</Button>
						</Badge>
					{/each}
					<Button size="sm" variant="outline" class="ms-2" onclick={clearAll}>
						{translate('prInsights.list.clearAll')}
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
			<Table.Header class="text-xs uppercase">
				<Table.Row>
					<Table.Head class="px-2 py-1.5 w-[70px]">
						{translate('prInsights.table.pr')}
					</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.repo')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.title')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.author')}</Table.Head>
					<Table.Head class="px-2 py-1.5">{translate('prInsights.table.target')}</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">
						{translate('prInsights.table.ci')}
					</Table.Head>
					<Table.Head class="px-2 py-1.5 w-[80px]">
						{translate('prInsights.table.risk')}
					</Table.Head>
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each filteredItems as pr (pr.repo + '#' + pr.number)}
					<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
						<Table.Cell class="px-2 py-1.5 mono text-primary">{$itemRef(pr.number)}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 mono text-xs text-muted-foreground">
							{pr.repo}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5 truncate max-w-[420px]">{pr.title}</Table.Cell>
						<Table.Cell class="px-2 py-1.5 text-muted-foreground">{pr.author ?? '-'}</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.base_ref}
								<Badge variant="outline" class={BADGE_CLASSES[defaults.has(pr.base_ref) ? 'blue' : 'gray']}>{pr.base_ref}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.ci_status}
								<Badge variant="outline" class={BADGE_CLASSES[ciColor(pr.ci_status)]}>{pr.ci_status}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
						<Table.Cell class="px-2 py-1.5">
							{#if pr.risk_level}
								<Badge variant="outline" class={BADGE_CLASSES[riskColor(pr.risk_level)]}>{pr.risk_level}</Badge>
							{:else}
								<span class="text-muted-foreground">-</span>
							{/if}
						</Table.Cell>
					</Table.Row>
				{:else}
					<Table.Row>
						<Table.Cell colspan={7} class="text-center text-muted-foreground py-8">
							{translate('prInsights.list.empty')}
						</Table.Cell>
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
	</div>

	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="sm:max-w-2xl max-h-[85vh] overflow-y-auto">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-6 text-left">
					<span class="mono text-muted-foreground text-sm">#{activePr?.number ?? ''}</span>
					<span class="text-base font-semibold text-foreground truncate">
						{activePr?.title ?? ''}
					</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activePr}
				<div class="mb-4 flex flex-wrap items-center gap-2">
					<Badge variant="outline" class={BADGE_CLASSES[statusColor(activePr.status)]}>
						{translate('prInsights.legend.' + activePr.status)}
					</Badge>
					{#if activePr.ci_status}
						<Badge variant="outline" class={BADGE_CLASSES[ciColor(activePr.ci_status)]}>{activePr.ci_status}</Badge>
					{/if}
					{#if activePr.risk_level}
						<Badge variant="outline" class={BADGE_CLASSES[riskColor(activePr.risk_level)]}>{activePr.risk_level}</Badge>
					{/if}
				</div>

				<div class="grid grid-cols-2 md:grid-cols-3 gap-3 mb-4">
					{@render modalField(translate('prInsights.table.repo'), activePr.repo)}
					{@render modalField(translate('prInsights.table.author'), activePr.author ?? '-')}
					{@render modalField(
						translate('prInsights.modal.branches'),
						`${activePr.head_ref ?? '-'} → ${activePr.base_ref ?? '-'}`
					)}
					{@render modalField(
						translate('prInsights.modal.lastActivity'),
						`${daysSince(activePr.updated_at)}d · ${activePr.updated_at.slice(0, 10)}`
					)}
					{@render modalField(
						translate('prInsights.modal.created'),
						activePr.created_at.slice(0, 10)
					)}
				</div>

				<Card.Root class="p-3 mb-4">
					<Card.Content class="p-0">
						<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">
							{translate('prInsights.modal.summary')}
						</div>
						<p class="text-sm text-foreground/90 whitespace-pre-wrap break-words">
							{activePr.summary ?? translate('prInsights.modal.noSummary')}
						</p>
					</Card.Content>
				</Card.Root>

				{#if activePr.url}
					<a
						href={activePr.url}
						target="_blank"
						rel="noopener noreferrer"
						class="text-sm text-primary hover:text-primary/80 underline"
					>
						{translate('prInsights.modal.openOnForge')} →
					</a>
				{/if}
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
