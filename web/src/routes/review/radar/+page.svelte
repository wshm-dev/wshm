<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { timeAgo, exactTime } from '$lib/time';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Table from '$lib/components/ui/table';
	import PrDetail from '$lib/components/PrDetail.svelte';

	let pulls: PullRequest[] = $state([]);
	let error: string | null = $state(null);
	let loading = $state(true);

	let loadToken = 0;
	async function load() {
		const myToken = ++loadToken;
		try {
			error = null;
			const data = await fetchPulls({ limit: 500 });
			if (myToken !== loadToken) return;
			pulls = data.items.filter((p) => p.state === 'open');
		} catch (e) {
			if (myToken !== loadToken) return;
			error = e instanceof Error ? e.message : 'Failed to load pull requests';
		} finally {
			if (myToken === loadToken) loading = false;
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	// ── Buckets ─────────────────────────────────────────────────────
	// Ready to merge: approved, and nothing known-bad blocks the merge.
	let readyToMerge = $derived(
		pulls
			.filter(
				(p) =>
					p.review_decision === 'approved' &&
					p.mergeable !== false &&
					p.ci_status !== 'failure'
			)
			.sort((a, b) => (a.updated_at < b.updated_at ? -1 : 1))
	);

	// Awaiting re-review: you asked for changes and the author has pushed
	// (or commented) since the decision was recorded — go take a look.
	let awaitingReReview = $derived(
		pulls
			.filter(
				(p) =>
					p.review_decision === 'changes_requested' &&
					p.review_decision_at &&
					p.updated_at > p.review_decision_at
			)
			.sort((a, b) => (a.updated_at < b.updated_at ? -1 : 1))
	);

	// Waiting on author: changes requested, no activity since — nothing for
	// you to do, listed for context.
	let waitingOnAuthor = $derived(
		pulls.filter(
			(p) =>
				p.review_decision === 'changes_requested' &&
				!(p.review_decision_at && p.updated_at > p.review_decision_at)
		)
	);

	// Needs first review: nobody has reviewed yet. Oldest first — those are
	// the ones rotting.
	let needsFirstReview = $derived(
		pulls
			.filter((p) => p.review_decision === 'review_required')
			.sort((a, b) => (a.created_at < b.created_at ? -1 : 1))
	);

	let hasDecisionData = $derived(pulls.some((p) => p.review_decision != null));

	let modalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	function openPr(pr: PullRequest) {
		activePr = pr;
		modalOpen = true;
	}

	function ciBadge(ci: string | null | undefined): { class: string | null; label: string } {
		if (ci === 'success')
			return { class: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400', label: 'CI ✓' };
		if (ci === 'failure')
			return { class: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400', label: 'CI ✗' };
		return { class: null, label: 'CI –' };
	}
</script>

<svelte:head>
	<title>wshm - To Validate</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">To Validate</h2>
	<p class="text-sm text-muted-foreground">Review radar — PRs waiting on you, so nothing slips through</p>
</div>

{#if error}
	<Card.Root class="border-red-500">
		<Card.Content>
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</Card.Content>
	</Card.Root>
{:else if loading}
	<Card.Root>
		<Card.Content>
			<p class="text-muted-foreground text-center py-6">Loading…</p>
		</Card.Content>
	</Card.Root>
{:else}
	{#if !hasDecisionData}
		<Card.Root class="mb-4">
			<Card.Content>
				<p class="text-sm text-muted-foreground">
					No review data yet — decisions are fetched during PR sync.
					<span class="block text-xs text-muted-foreground mt-1">
						Trigger a sync from the sidebar (requires a GitHub token with repo access).
					</span>
				</p>
			</Card.Content>
		</Card.Root>
	{/if}

	<!-- Ready to merge -->
	<div class="mt-2">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-green-600 dark:text-green-400">Ready to merge</h2>
			<span class="text-xs text-muted-foreground mono">{readyToMerge.length}</span>
		</div>
		<p class="text-sm text-muted-foreground mb-3">Approved, no known conflicts, CI not failing — one click away</p>
		{#if readyToMerge.length === 0}
			<Card.Root>
				<Card.Content>
					<p class="text-muted-foreground text-center py-3 text-sm">Nothing approved is waiting.</p>
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[70px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[70px]">CI</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[130px]">Approved</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each readyToMerge as pr}
							{@const ci = ciBadge(pr.ci_status)}
							<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
								<Table.Cell class="px-2 py-1.5 mono">{pr.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{pr.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">
									{#if ci.class}
										<Badge variant="outline" class={ci.class}>{ci.label}</Badge>
									{:else}
										<Badge variant="secondary">{ci.label}</Badge>
									{/if}
								</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground" title={exactTime(pr.review_decision_at)}>
									{timeAgo(pr.review_decision_at)}
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</div>

	<!-- Awaiting re-review -->
	<div class="mt-6">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-yellow-600 dark:text-yellow-400">Awaiting your re-review</h2>
			<span class="text-xs text-muted-foreground mono">{awaitingReReview.length}</span>
		</div>
		<p class="text-sm text-muted-foreground mb-3">You requested changes and the author has since pushed — don't leave them hanging</p>
		{#if awaitingReReview.length === 0}
			<Card.Root>
				<Card.Content>
					<p class="text-muted-foreground text-center py-3 text-sm">No PRs updated since your change requests.</p>
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[70px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[150px]">Changes requested</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[130px]">Updated</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each awaitingReReview as pr}
							<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
								<Table.Cell class="px-2 py-1.5 mono">{pr.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{pr.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground" title={exactTime(pr.review_decision_at)}>
									{timeAgo(pr.review_decision_at)}
								</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-foreground/90" title={exactTime(pr.updated_at)}>
									{timeAgo(pr.updated_at)}
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</div>

	<!-- Needs first review -->
	<div class="mt-6">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-foreground">Needs first review</h2>
			<span class="text-xs text-muted-foreground mono">{needsFirstReview.length}</span>
		</div>
		<p class="text-sm text-muted-foreground mb-3">Never reviewed, oldest first</p>
		{#if needsFirstReview.length === 0}
			<Card.Root>
				<Card.Content>
					<p class="text-muted-foreground text-center py-3 text-sm">Every open PR has at least one review.</p>
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[70px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[70px]">CI</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[110px]">Age</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each needsFirstReview.slice(0, 30) as pr}
							{@const ci = ciBadge(pr.ci_status)}
							<Table.Row class="cursor-pointer" onclick={() => openPr(pr)}>
								<Table.Cell class="px-2 py-1.5 mono">{pr.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{pr.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">
									{#if ci.class}
										<Badge variant="outline" class={ci.class}>{ci.label}</Badge>
									{:else}
										<Badge variant="secondary">{ci.label}</Badge>
									{/if}
								</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground" title={exactTime(pr.created_at)}>
									{timeAgo(pr.created_at)}
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
			{#if needsFirstReview.length > 30}
				<p class="text-xs text-muted-foreground mt-2">Showing the 30 oldest of {needsFirstReview.length} — see <a href="/prs" class="text-primary hover:underline">Pull Requests</a> for the full list.</p>
			{/if}
		{/if}
	</div>

	<!-- Waiting on author (context only) -->
	{#if waitingOnAuthor.length > 0}
		<div class="mt-6">
			<div class="flex items-baseline gap-2 mb-1">
				<h2 class="text-sm font-semibold text-muted-foreground">Waiting on author</h2>
				<span class="text-xs text-muted-foreground mono">{waitingOnAuthor.length}</span>
			</div>
			<p class="text-xs text-muted-foreground mb-2">Changes requested, no activity since — nothing for you to do yet.</p>
			<div class="flex flex-wrap gap-2">
				{#each waitingOnAuthor.slice(0, 20) as pr}
					<button
						type="button"
						class="text-xs px-2 py-1 rounded border bg-card text-muted-foreground hover:text-foreground hover:border-muted-foreground/50"
						onclick={() => openPr(pr)}
						title={pr.title}
					>#{pr.number}</button>
				{/each}
			</div>
		</div>
	{/if}

	<Dialog.Root bind:open={modalOpen}>
		<Dialog.Content class="max-h-[85vh] w-[80vw] max-w-[80vw] overflow-y-auto sm:max-w-[80vw]">
			<Dialog.Header>
				<Dialog.Title class="flex w-full items-center gap-3 pr-2">
					<span class="mono text-muted-foreground text-sm">#{activePr?.number ?? ''}</span>
					<span class="text-base font-semibold text-foreground truncate">{activePr?.title ?? ''}</span>
				</Dialog.Title>
			</Dialog.Header>
			{#if activePr}
				<PrDetail pr={activePr} />
				<div class="text-right pt-2">
					<a href="/prs/{activePr.number}" class="text-xs text-primary hover:text-primary/80">Open full page →</a>
				</div>
			{/if}
		</Dialog.Content>
	</Dialog.Root>
{/if}
