<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { timeAgo, exactTime } from '$lib/time';
	import { Card, Badge, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Modal } from 'flowbite-svelte';
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

	function ciBadge(ci: string | null | undefined): { color: 'green' | 'red' | 'gray'; label: string } {
		if (ci === 'success') return { color: 'green', label: 'CI ✓' };
		if (ci === 'failure') return { color: 'red', label: 'CI ✗' };
		return { color: 'gray', label: 'CI –' };
	}
</script>

<svelte:head>
	<title>wshm - To Validate</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">To Validate</h2>
	<p class="text-sm text-gray-500">Review radar — PRs waiting on you, so nothing slips through</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
	</Card>
{:else if loading}
	<Card class="bg-gray-800 border-gray-700 max-w-none">
		<p class="text-gray-500 text-center py-6">Loading…</p>
	</Card>
{:else}
	{#if !hasDecisionData}
		<Card class="bg-gray-800 border-gray-700 max-w-none mb-4">
			<p class="text-sm text-gray-400">
				No review data yet — decisions are fetched during PR sync.
				<span class="block text-xs text-gray-500 mt-1">
					Trigger a sync from the sidebar (requires a GitHub token with repo access).
				</span>
			</p>
		</Card>
	{/if}

	<!-- Ready to merge -->
	<div class="mt-2">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-green-400">Ready to merge</h2>
			<span class="text-xs text-gray-500 mono">{readyToMerge.length}</span>
		</div>
		<p class="text-sm text-gray-500 mb-3">Approved, no known conflicts, CI not failing — one click away</p>
		{#if readyToMerge.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-3 text-sm">Nothing approved is waiting.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[70px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">CI</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[130px]">Approved</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each readyToMerge as pr}
							{@const ci = ciBadge(pr.ci_status)}
							<TableBodyRow class="cursor-pointer" onclick={() => openPr(pr)}>
								<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{pr.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5"><Badge color={ci.color}>{ci.label}</Badge></TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500" title={exactTime(pr.review_decision_at)}>
									{timeAgo(pr.review_decision_at)}
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<!-- Awaiting re-review -->
	<div class="mt-6">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-yellow-400">Awaiting your re-review</h2>
			<span class="text-xs text-gray-500 mono">{awaitingReReview.length}</span>
		</div>
		<p class="text-sm text-gray-500 mb-3">You requested changes and the author has since pushed — don't leave them hanging</p>
		{#if awaitingReReview.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-3 text-sm">No PRs updated since your change requests.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[70px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[150px]">Changes requested</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[130px]">Updated</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each awaitingReReview as pr}
							<TableBodyRow class="cursor-pointer" onclick={() => openPr(pr)}>
								<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{pr.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500" title={exactTime(pr.review_decision_at)}>
									{timeAgo(pr.review_decision_at)}
								</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-300" title={exactTime(pr.updated_at)}>
									{timeAgo(pr.updated_at)}
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<!-- Needs first review -->
	<div class="mt-6">
		<div class="flex items-baseline gap-2 mb-1">
			<h2 class="text-lg font-semibold text-gray-200">Needs first review</h2>
			<span class="text-xs text-gray-500 mono">{needsFirstReview.length}</span>
		</div>
		<p class="text-sm text-gray-500 mb-3">Never reviewed, oldest first</p>
		{#if needsFirstReview.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-3 text-sm">Every open PR has at least one review.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[70px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">CI</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[110px]">Age</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each needsFirstReview.slice(0, 30) as pr}
							{@const ci = ciBadge(pr.ci_status)}
							<TableBodyRow class="cursor-pointer" onclick={() => openPr(pr)}>
								<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{pr.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5"><Badge color={ci.color}>{ci.label}</Badge></TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500" title={exactTime(pr.created_at)}>
									{timeAgo(pr.created_at)}
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
			{#if needsFirstReview.length > 30}
				<p class="text-xs text-gray-600 mt-2">Showing the 30 oldest of {needsFirstReview.length} — see <a href="/prs" class="text-blue-400 hover:underline">Pull Requests</a> for the full list.</p>
			{/if}
		{/if}
	</div>

	<!-- Waiting on author (context only) -->
	{#if waitingOnAuthor.length > 0}
		<div class="mt-6">
			<div class="flex items-baseline gap-2 mb-1">
				<h2 class="text-sm font-semibold text-gray-400">Waiting on author</h2>
				<span class="text-xs text-gray-600 mono">{waitingOnAuthor.length}</span>
			</div>
			<p class="text-xs text-gray-600 mb-2">Changes requested, no activity since — nothing for you to do yet.</p>
			<div class="flex flex-wrap gap-2">
				{#each waitingOnAuthor.slice(0, 20) as pr}
					<button
						type="button"
						class="text-xs px-2 py-1 rounded border border-gray-700 bg-gray-800 text-gray-400 hover:text-gray-200 hover:border-gray-500"
						onclick={() => openPr(pr)}
						title={pr.title}
					>#{pr.number}</button>
				{/each}
			</div>
		</div>
	{/if}

	<Modal
		bind:open={modalOpen}
		size="xl"
		dismissable
		class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
		bodyClass="text-gray-200"
	>
		{#snippet header()}
			<div class="flex w-full items-center gap-3 pr-2">
				<span class="mono text-gray-500 text-sm">#{activePr?.number ?? ''}</span>
				<span class="text-base font-semibold text-gray-100 truncate">{activePr?.title ?? ''}</span>
			</div>
		{/snippet}
		{#if activePr}
			<PrDetail pr={activePr} />
			<div class="text-right pt-2">
				<a href="/prs/{activePr.number}" class="text-xs text-blue-400 hover:text-blue-300">Open full page →</a>
			</div>
		{/if}
	</Modal>
{/if}
