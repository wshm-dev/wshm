<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import {
		fetchSummary,
		fetchIssues,
		fetchPulls,
		type Summary,
		type Issue,
		type PullRequest
	} from '$lib/api';
	import { Card, Badge, Modal } from 'flowbite-svelte';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';

	let summary: Summary | null = $state(null);
	let error: string | null = $state(null);

	async function load() {
		try {
			error = null;
			summary = await fetchSummary();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load summary';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => {
			load();
		});
		return unsub;
	});

	function priorityColor(p: string | null | undefined): 'red' | 'yellow' | 'blue' | 'dark' {
		if (p === 'critical' || p === 'high') return 'red';
		if (p === 'medium') return 'yellow';
		if (p === 'low') return 'blue';
		return 'dark';
	}

	// Bucket items by urgency. The page answers "what should I do right now?",
	// not "what's the full inventory" — that's what /issues and /prs are for.
	// NOW = critical / high priority issues, plus PRs that are either flagged
	// high-risk or have merge conflicts. TODAY = the rest of the curated
	// top_* lists, deduped against NOW. THIS WEEK is just counters.
	type Bucketed = {
		nowIssues: Issue[];
		nowPrs: PullRequest[];
		todayIssues: Issue[];
		todayPrs: PullRequest[];
	};

	let bucketed = $derived.by<Bucketed>(() => {
		if (!summary) return { nowIssues: [], nowPrs: [], todayIssues: [], todayPrs: [] };
		const nowIssues = summary.high_priority_issues.slice(0, 10);
		const nowIssueSet = new Set(nowIssues.map((i) => i.number));

		const nowPrSet = new Set<number>();
		const nowPrs: PullRequest[] = [];
		for (const pr of summary.high_risk_prs) {
			if (!nowPrSet.has(pr.number)) {
				nowPrSet.add(pr.number);
				nowPrs.push(pr);
			}
		}
		for (const pr of summary.top_prs) {
			if (pr.has_conflicts && !nowPrSet.has(pr.number)) {
				nowPrSet.add(pr.number);
				nowPrs.push(pr);
			}
		}

		const todayIssues = summary.top_issues
			.filter((i) => !nowIssueSet.has(i.number))
			.slice(0, 8);
		const todayPrs = summary.top_prs.filter((p) => !nowPrSet.has(p.number)).slice(0, 8);

		return { nowIssues, nowPrs: nowPrs.slice(0, 10), todayIssues, todayPrs };
	});

	function relativeMinutes(iso: string | undefined): string {
		if (!iso) return '';
		const t = new Date(iso).getTime();
		if (isNaN(t)) return '';
		const mins = Math.max(0, Math.floor((Date.now() - t) / 60000));
		if (mins < 1) return 'just now';
		if (mins < 60) return `${mins} min ago`;
		const hrs = Math.floor(mins / 60);
		if (hrs < 24) return `${hrs}h ago`;
		return `${Math.floor(hrs / 24)}d ago`;
	}

	let issueModalOpen = $state(false);
	let activeIssue: Issue | null = $state(null);
	let issueLoading = $state(false);
	let issueError: string | null = $state(null);

	let prModalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	let prLoading = $state(false);
	let prError: string | null = $state(null);

	async function openIssue(num: number) {
		issueModalOpen = true;
		activeIssue = null;
		issueError = null;
		issueLoading = true;
		try {
			const all = await fetchIssues({ limit: 500 });
			activeIssue = all.items.find((i) => i.number === num) ?? null;
			if (!activeIssue) issueError = `Issue #${num} not found`;
		} catch (e) {
			issueError = e instanceof Error ? e.message : 'Failed to load';
		}
		issueLoading = false;
	}

	async function openPr(num: number) {
		prModalOpen = true;
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
	<title>wshm - Summary</title>
</svelte:head>

<div class="mb-6 flex flex-wrap items-end justify-between gap-3">
	<div>
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Daily summary</h2>
		<p class="text-sm text-gray-500">What needs your attention, sorted by urgency.</p>
	</div>
	{#if summary?.timestamp}
		<p class="text-xs text-gray-500 mono">Synced {relativeMinutes(summary.timestamp)}</p>
	{/if}
</div>

{#snippet issueRow(issue: Issue)}
	<li
		class="flex items-center gap-2 text-sm cursor-pointer hover:bg-gray-700/50 rounded px-2 py-1"
		onclick={() => openIssue(issue.number)}
		onkeydown={(e) => e.key === 'Enter' && openIssue(issue.number)}
		role="button"
		tabindex="0"
	>
		<span class="mono text-yellow-400 text-xs w-12 shrink-0">#{issue.number}</span>
		{#if issue.priority}
			<Badge color={priorityColor(issue.priority)} class="shrink-0">{issue.priority}</Badge>
		{/if}
		<span class="text-gray-300 flex-1 truncate">{issue.title}</span>
		{#if issue.age_days > 0}
			<span class="text-gray-500 text-xs mono shrink-0">{issue.age_days}d</span>
		{/if}
		<span class="text-gray-600 shrink-0">→</span>
	</li>
{/snippet}

{#snippet prRow(pr: PullRequest)}
	<li
		class="flex items-center gap-2 text-sm cursor-pointer hover:bg-gray-700/50 rounded px-2 py-1"
		onclick={() => openPr(pr.number)}
		onkeydown={(e) => e.key === 'Enter' && openPr(pr.number)}
		role="button"
		tabindex="0"
	>
		<span class="mono text-yellow-400 text-xs w-12 shrink-0">#{pr.number}</span>
		{#if pr.has_conflicts}
			<Badge color="red" class="shrink-0">conflict</Badge>
		{:else if pr.risk_level}
			<Badge color={priorityColor(pr.risk_level)} class="shrink-0">{pr.risk_level}</Badge>
		{/if}
		<span class="text-gray-300 flex-1 truncate">{pr.title}</span>
		{#if pr.age_days > 0}
			<span class="text-gray-500 text-xs mono shrink-0">{pr.age_days}d</span>
		{/if}
		<span class="text-gray-600 shrink-0">→</span>
	</li>
{/snippet}

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
		<p class="mt-2 text-sm text-gray-500">
			The wshm daemon must expose <code>/api/v1/summary</code>.
		</p>
	</Card>
{:else if summary}
	{@const nowCount = bucketed.nowIssues.length + bucketed.nowPrs.length}
	{@const todayCount = bucketed.todayIssues.length + bucketed.todayPrs.length}

	<div class="grid grid-cols-1 lg:grid-cols-3 gap-4 mb-4">
		<!-- NOW column: red accent — drop everything and look at this. -->
		<Card
			class="border-l-4 {nowCount > 0
				? 'border-l-red-500'
				: 'border-l-gray-700'} bg-gray-800 border-gray-700 max-w-none"
		>
			<div class="flex items-baseline justify-between mb-3">
				<h3 class="text-sm font-semibold {nowCount > 0 ? 'text-red-400' : 'text-gray-400'}">
					NOW
				</h3>
				<span class="text-xs text-gray-500 mono">{nowCount}</span>
			</div>
			<p class="text-xs text-gray-500 mb-3">Critical priority &middot; conflicts &middot; high risk</p>
			{#if nowCount === 0}
				<p class="text-sm text-gray-600 italic">Nothing urgent. Nice.</p>
			{:else}
				<ul class="space-y-1">
					{#each bucketed.nowIssues as issue (issue.number)}
						{@render issueRow(issue)}
					{/each}
					{#each bucketed.nowPrs as pr (pr.number)}
						{@render prRow(pr)}
					{/each}
				</ul>
			{/if}
		</Card>

		<!-- TODAY column: amber accent — work through these next. -->
		<Card
			class="border-l-4 {todayCount > 0
				? 'border-l-amber-500'
				: 'border-l-gray-700'} bg-gray-800 border-gray-700 max-w-none"
		>
			<div class="flex items-baseline justify-between mb-3">
				<h3 class="text-sm font-semibold {todayCount > 0 ? 'text-amber-400' : 'text-gray-400'}">
					TODAY
				</h3>
				<span class="text-xs text-gray-500 mono">{todayCount}</span>
			</div>
			<p class="text-xs text-gray-500 mb-3">Medium priority &middot; PRs awaiting review</p>
			{#if todayCount === 0}
				<p class="text-sm text-gray-600 italic">Inbox zero on the medium list.</p>
			{:else}
				<ul class="space-y-1">
					{#each bucketed.todayIssues as issue (issue.number)}
						{@render issueRow(issue)}
					{/each}
					{#each bucketed.todayPrs as pr (pr.number)}
						{@render prRow(pr)}
					{/each}
				</ul>
			{/if}
		</Card>

		<!-- THIS WEEK column: counters — context, not action. -->
		<Card class="border-l-4 border-l-gray-700 bg-gray-800 border-gray-700 max-w-none">
			<div class="flex items-baseline justify-between mb-3">
				<h3 class="text-sm font-semibold text-gray-300">THIS WEEK</h3>
				<span class="text-xs text-gray-500 mono">backlog</span>
			</div>
			<p class="text-xs text-gray-500 mb-3">Volume across the repo, not an action list.</p>
			<dl class="space-y-2 text-sm">
				<div class="flex items-center justify-between">
					<dt class="text-gray-400">Open issues</dt>
					<dd>
						<a href="/issues" class="mono text-blue-400 hover:text-blue-300">
							{summary.open_issues}
						</a>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-gray-400">Untriaged</dt>
					<dd>
						<a href="/triage" class="mono text-blue-400 hover:text-blue-300">
							{summary.untriaged_issues}
						</a>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-gray-400">Open PRs</dt>
					<dd>
						<a href="/prs" class="mono text-blue-400 hover:text-blue-300">{summary.open_prs}</a>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-gray-400">Unanalyzed</dt>
					<dd>
						<a href="/prs" class="mono text-blue-400 hover:text-blue-300">
							{summary.unanalyzed_prs}
						</a>
					</dd>
				</div>
				<div class="flex items-center justify-between">
					<dt class="text-gray-400">Conflicts</dt>
					<dd>
						<span
							class="mono {summary.conflicts > 0 ? 'text-red-400' : 'text-gray-500'}"
						>{summary.conflicts}</span>
					</dd>
				</div>
			</dl>
		</Card>
	</div>
{:else}
	<p class="text-gray-500">Loading…</p>
{/if}

<Modal
	bind:open={issueModalOpen}
	size="xl"
	dismissable
	class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	{#snippet header()}
		<div class="flex w-full items-center gap-3 pr-2">
			<span class="mono text-gray-500 text-sm">#{activeIssue?.number ?? ''}</span>
			<span class="text-base font-semibold text-gray-100 truncate">
				{activeIssue?.title ?? (issueLoading ? 'Loading…' : '')}
			</span>
		</div>
	{/snippet}
	{#if issueLoading}
		<p class="text-gray-500 text-sm">Loading…</p>
	{:else if issueError}
		<p class="text-red-400 text-sm">{issueError}</p>
	{:else if activeIssue}
		<IssueDetail issue={activeIssue} />
		<div class="text-right pt-2">
			<a href="/issues/{activeIssue.number}" class="text-xs text-blue-400 hover:text-blue-300">
				Open full page →
			</a>
		</div>
	{/if}
</Modal>

<Modal
	bind:open={prModalOpen}
	size="xl"
	dismissable
	class="!max-w-[80vw] w-[80vw] bg-gray-900 border-gray-700"
	bodyClass="text-gray-200"
>
	{#snippet header()}
		<div class="flex w-full items-center gap-3 pr-2">
			<span class="mono text-gray-500 text-sm">#{activePr?.number ?? ''}</span>
			<span class="text-base font-semibold text-gray-100 truncate">
				{activePr?.title ?? (prLoading ? 'Loading…' : '')}
			</span>
		</div>
	{/snippet}
	{#if prLoading}
		<p class="text-gray-500 text-sm">Loading…</p>
	{:else if prError}
		<p class="text-red-400 text-sm">{prError}</p>
	{:else if activePr}
		<PrDetail pr={activePr} />
		<div class="text-right pt-2">
			<a href="/prs/{activePr.number}" class="text-xs text-blue-400 hover:text-blue-300">
				Open full page →
			</a>
		</div>
	{/if}
</Modal>
