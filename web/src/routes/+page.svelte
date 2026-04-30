<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, type Status } from '$lib/api';
	import { Card } from 'flowbite-svelte';

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
		<h2 class="text-xl font-semibold text-gray-100 mb-2">Sync Status</h2>
		<p class="text-sm text-gray-500">
			Last sync: {status?.last_sync ?? 'Never'}
		</p>
	</Card>
{/if}
