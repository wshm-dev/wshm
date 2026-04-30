<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, fetchIssues, fetchPulls, type Status, type Issue, type PullRequest } from '$lib/api';
	import { Card, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge } from 'flowbite-svelte';

	let status: Status | null = $state(null);
	let issues: Issue[] = $state([]);
	let pulls: PullRequest[] = $state([]);
	let error: string | null = $state(null);

	const priorityOrder: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 };

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	function ageText(dateStr: string): string {
		const d = ageDays(dateStr);
		if (d === 0) return 'today';
		if (d === 1) return '1d';
		return `${d}d`;
	}

	let actionRequired = $derived(
		issues
			.filter(i => i.state === 'open' && (i.priority === 'critical' || i.priority === 'high'))
			.sort((a, b) => ageDays(b.created_at) - ageDays(a.created_at))
	);

	let issuesTodo = $derived(
		issues
			.filter(i => i.state === 'open')
			.sort((a, b) => {
				const pa = priorityOrder[a.priority ?? 'low'] ?? 9;
				const pb = priorityOrder[b.priority ?? 'low'] ?? 9;
				if (pa !== pb) return pa - pb;
				return ageDays(b.created_at) - ageDays(a.created_at);
			})
			.slice(0, 10)
	);

	let prsTodo = $derived(
		pulls
			.filter(p => p.state === 'open')
			.sort((a, b) => {
				const ca = a.mergeable === false ? 0 : 1;
				const cb = b.mergeable === false ? 0 : 1;
				if (ca !== cb) return ca - cb;
				return ageDays(b.created_at) - ageDays(a.created_at);
			})
			.slice(0, 10)
	);

	async function load() {
		try {
			error = null;
			const [s, i, p] = await Promise.all([
				fetchStatus(),
				fetchIssues(),
				fetchPulls()
			]);
			status = s;
			issues = i;
			pulls = p;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
	}
</script>

<svelte:head>
	<title>wshm - Actions</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Actions</h2>
	<p class="text-sm text-gray-500">Priority items requiring attention</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
	</Card>
{:else}
	<div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3 mb-6">
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Open Issues</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.open_issues ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Untriaged</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.untriaged ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Open PRs</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.open_prs ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Unanalyzed</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.unanalyzed ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Conflicts</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.conflicts ?? '--'}</div>
		</Card>
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Action Required</h2>
		<p class="text-sm text-gray-500 mb-3">High/critical priority issues, oldest first</p>
		{#if actionRequired.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-4">No high-priority issues requiring action.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Priority</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each actionRequired as issue}
							<TableBodyRow>
								<TableBodyCell class="px-2 py-1.5 mono">{issue.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">
									<Badge color={issue.priority === 'critical' ? 'red' : 'yellow'}>{issue.priority}</Badge>
								</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(issue.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.title}</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Issues TODO</h2>
		<p class="text-sm text-gray-500 mb-3">Top 10 issues by priority then age</p>
		{#if issuesTodo.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-4">No open issues.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Priority</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each issuesTodo as issue}
							<TableBodyRow>
								<TableBodyCell class="px-2 py-1.5 mono">{issue.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.priority ?? '-'}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(issue.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.title}</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">PRs TODO</h2>
		<p class="text-sm text-gray-500 mb-3">Top 10 PRs by conflicts then age</p>
		{#if prsTodo.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-4">No open pull requests.</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Risk</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each prsTodo as pr}
							<TableBodyRow>
								<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">
									{#if pr.risk}
										<Badge color={riskColor(pr.risk)}>{pr.risk}</Badge>
									{:else}
										<span class="text-gray-500">-</span>
									{/if}
								</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(pr.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{pr.title}</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>
{/if}
