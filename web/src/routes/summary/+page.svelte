<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchSummary, type Summary } from '$lib/api';
	import { Card, Badge } from 'flowbite-svelte';

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
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function priorityColor(p: string | null): 'red' | 'yellow' | 'blue' | 'dark' {
		if (p === 'critical' || p === 'high') return 'red';
		if (p === 'medium') return 'yellow';
		if (p === 'low') return 'blue';
		return 'dark';
	}
</script>

<svelte:head>
	<title>wshm - Summary</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Summary</h2>
	<p class="text-sm text-gray-500">Daily digest — same data as Discord notifications</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800">
		<p class="text-red-400">{error}</p>
		<p class="mt-2 text-sm text-gray-500">The wshm daemon must expose <code>/api/v1/summary</code>.</p>
	</Card>
{:else if summary}
	<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 mb-6">
		<Card class="bg-gray-800 border-gray-700 text-center">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Open Issues</div>
			<div class="text-3xl font-bold text-gray-100 mono">{summary.open_issues}</div>
			<div class="text-xs text-gray-500 mt-1">{summary.untriaged_issues} untriaged</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Open PRs</div>
			<div class="text-3xl font-bold text-gray-100 mono">{summary.open_prs}</div>
			<div class="text-xs text-gray-500 mt-1">{summary.unanalyzed_prs} unanalyzed</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Conflicts</div>
			<div class="text-3xl font-bold {summary.conflicts > 0 ? 'text-red-400' : 'text-gray-100'} mono">{summary.conflicts}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Action Required</div>
			<div class="text-3xl font-bold {summary.high_priority_issues.length > 0 ? 'text-red-400' : 'text-gray-100'} mono">{summary.high_priority_issues.length}</div>
		</Card>
	</div>

	{#if summary.high_priority_issues.length > 0}
		<Card class="bg-gray-800 border-gray-700 mb-4">
			<h3 class="text-lg font-semibold text-red-400 mb-3">Action Required</h3>
			<ul class="space-y-2">
				{#each summary.high_priority_issues.slice(0, 10) as issue (issue.number)}
					<li class="flex items-start gap-2 text-sm">
						<a href="/issues/{issue.number}" class="text-yellow-400 mono hover:underline">#{issue.number}</a>
						<Badge color={priorityColor(issue.priority)}>{issue.priority ?? '?'}</Badge>
						<span class="text-gray-300 flex-1">{issue.title}</span>
						{#if issue.age_days > 0}<span class="text-gray-500 text-xs">{issue.age_days}d</span>{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if summary.high_risk_prs.length > 0}
		<Card class="bg-gray-800 border-gray-700 mb-4">
			<h3 class="text-lg font-semibold text-purple-400 mb-3">Attention PRs</h3>
			<ul class="space-y-2">
				{#each summary.high_risk_prs.slice(0, 10) as pr (pr.number)}
					<li class="flex items-start gap-2 text-sm">
						<a href="/prs/{pr.number}" class="text-yellow-400 mono hover:underline">#{pr.number}</a>
						{#if pr.risk_level}<Badge color="purple">risk:{pr.risk_level}</Badge>{/if}
						{#if pr.has_conflicts}<Badge color="red">CONFLICT</Badge>{/if}
						<span class="text-gray-300 flex-1">{pr.title}</span>
						{#if pr.age_days > 0}<span class="text-gray-500 text-xs">{pr.age_days}d</span>{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if summary.top_issues.length > 0}
		<Card class="bg-gray-800 border-gray-700 mb-4">
			<h3 class="text-lg font-semibold text-cyan-400 mb-3">Issues TODO</h3>
			<ul class="space-y-2">
				{#each summary.top_issues as issue (issue.number)}
					<li class="flex items-start gap-2 text-sm">
						<a href="/issues/{issue.number}" class="text-yellow-400 mono hover:underline">#{issue.number}</a>
						<Badge color={priorityColor(issue.priority)}>{issue.priority ?? '-'}</Badge>
						{#if issue.category}<span class="text-gray-500 text-xs">{issue.category}</span>{/if}
						<span class="text-gray-300 flex-1">{issue.title}</span>
						{#if issue.age_days > 0}<span class="text-gray-500 text-xs">{issue.age_days}d</span>{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if summary.top_prs.length > 0}
		<Card class="bg-gray-800 border-gray-700 mb-4">
			<h3 class="text-lg font-semibold text-cyan-400 mb-3">PRs TODO</h3>
			<ul class="space-y-2">
				{#each summary.top_prs as pr (pr.number)}
					<li class="flex items-start gap-2 text-sm">
						<a href="/prs/{pr.number}" class="text-yellow-400 mono hover:underline">#{pr.number}</a>
						{#if pr.risk_level}<Badge color="purple">{pr.risk_level}</Badge>{/if}
						{#if pr.has_conflicts}<Badge color="red">CONFLICT</Badge>{/if}
						<span class="text-gray-300 flex-1">{pr.title}</span>
						{#if pr.age_days > 0}<span class="text-gray-500 text-xs">{pr.age_days}d</span>{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	<p class="text-xs text-gray-500 mt-4">Generated at {summary.timestamp}</p>
{:else}
	<p class="text-gray-500">Loading…</p>
{/if}
