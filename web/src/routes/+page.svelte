<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, type Status } from '$lib/api';
	import { timeAgo, exactTime } from '$lib/time';
	import { Card, Badge, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell } from 'flowbite-svelte';

	let status: Status | null = $state(null);
	let error: string | null = $state(null);

	async function load() {
		try {
			error = null;
			status = await fetchStatus();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load status';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});
</script>

<svelte:head>
	<title>wshm - Dashboard</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Dashboard</h2>
	<p class="text-sm text-gray-500">Repository status overview</p>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
		<p class="mt-2 text-sm text-gray-500">Make sure the wshm server is running.</p>
	</Card>
{:else}
	<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
		<Card class="bg-gray-800 border-gray-700 text-center max-w-none">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Open Issues</div>
			<div class="text-3xl font-bold text-gray-100 mono">{status?.open_issues ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center max-w-none">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Open PRs</div>
			<div class="text-3xl font-bold text-gray-100 mono">{status?.open_prs ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center max-w-none">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Untriaged</div>
			<div class="text-3xl font-bold text-gray-100 mono">{status?.untriaged ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center max-w-none">
			<div class="text-xs uppercase tracking-wider text-gray-500 mb-2">Conflicts</div>
			<div class="text-3xl font-bold text-gray-100 mono">{status?.conflicts ?? '--'}</div>
		</Card>
	</div>

	<Card class="mt-6 bg-gray-800 border-gray-700 max-w-none">
		<div class="flex items-baseline justify-between mb-3">
			<h2 class="text-xl font-semibold text-gray-100">Repositories</h2>
			<span class="text-xs text-gray-500" title={exactTime(status?.last_sync)}>
				Last sync: {timeAgo(status?.last_sync)}
			</span>
		</div>
		{#if status && status.repos.length > 0}
			<div class="w-full overflow-x-auto">
				<Table class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5">Repository</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px] text-right">Issues</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px] text-right">PRs</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[90px] text-right">Untriaged</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[90px] text-right">Conflicts</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[110px]">Last sync</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px]">Mode</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each status.repos as repo}
							<TableBodyRow>
								<TableBodyCell class="px-2 py-1.5 mono text-gray-200">{repo.slug}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 mono text-right">{repo.open_issues}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 mono text-right">{repo.open_prs}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 mono text-right">{repo.untriaged}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 mono text-right">{repo.conflicts}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500" title={exactTime(repo.last_sync)}>{timeAgo(repo.last_sync)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">
									{#if repo.apply}
										<Badge color="green">apply</Badge>
									{:else}
										<Badge color="gray">dry-run</Badge>
									{/if}
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{:else}
			<p class="text-sm text-gray-500">No repositories configured.</p>
		{/if}
	</Card>
{/if}
