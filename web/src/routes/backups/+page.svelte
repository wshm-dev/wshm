<script lang="ts">
	import { onMount } from 'svelte';
	import { fetchBackups, createBackup, restoreBackup, type BackupsResult } from '$lib/api';
	import { Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell } from 'flowbite-svelte';

	let result = $state<BackupsResult | null>(null);
	let error = $state<string | null>(null);
	let creating = $state(false);
	let restoring = $state<string | null>(null);
	let message = $state<string | null>(null);

	async function load() {
		try {
			error = null;
			result = await fetchBackups();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load backups';
		}
	}

	async function handleCreate() {
		creating = true;
		message = null;
		try {
			const res = await createBackup();
			message = res.message;
			await load();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Backup failed';
		}
		creating = false;
	}

	async function handleRestore(name: string) {
		if (!confirm(`Restore from ${name}? This will overwrite the current database.`)) return;
		restoring = name;
		message = null;
		try {
			const res = await restoreBackup(name);
			message = res.message;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Restore failed';
		}
		restoring = null;
	}

	function formatSize(bytes: number): string {
		if (bytes > 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
		return (bytes / 1024).toFixed(1) + ' KB';
	}

	onMount(load);
</script>

<svelte:head>
	<title>wshm - Backups</title>
</svelte:head>

<div class="mb-6 flex items-center justify-between">
	<div>
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Backups</h2>
		<p class="text-sm text-gray-500">Backup and restore your wshm database</p>
	</div>
	<button
		onclick={handleCreate}
		disabled={creating}
		class="bg-blue-600 hover:bg-blue-700 disabled:opacity-50 text-white text-sm font-semibold px-5 py-2.5 rounded-lg transition"
	>
		{creating ? 'Creating...' : 'Create backup'}
	</button>
</div>

{#if message}
	<div class="rounded-lg border border-green-800 bg-green-900/20 p-4 mb-6">
		<p class="text-sm text-green-400">{message}</p>
	</div>
{/if}

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5 mb-6">
		<p class="text-red-400">{error}</p>
	</div>
{/if}

{#if result && result.backups.length > 0}
	<Table shadow hoverable>
		<TableHead class="bg-gray-800">
			<TableHeadCell>Backup</TableHeadCell>
			<TableHeadCell>Size</TableHeadCell>
			<TableHeadCell>Date</TableHeadCell>
			<TableHeadCell>Actions</TableHeadCell>
		</TableHead>
		<TableBody>
			{#each result.backups as b}
				<TableBodyRow class="border-gray-700">
					<TableBodyCell class="font-mono text-sm">{b.name}</TableBodyCell>
					<TableBodyCell class="text-gray-400">{formatSize(b.size)}</TableBodyCell>
					<TableBodyCell class="text-xs text-gray-400">{b.created_at?.slice(0, 19).replace('T', ' ') ?? ''}</TableBodyCell>
					<TableBodyCell>
						<button
							onclick={() => handleRestore(b.name)}
							disabled={restoring === b.name}
							class="text-xs border border-gray-700 text-gray-300 hover:text-white hover:border-gray-500 px-3 py-1.5 rounded-lg transition"
						>
							{restoring === b.name ? 'Restoring...' : 'Restore'}
						</button>
					</TableBodyCell>
				</TableBodyRow>
			{/each}
		</TableBody>
	</Table>
{:else if result}
	<div class="rounded-lg border border-gray-700 bg-gray-800 p-10 text-center">
		<div class="text-2xl mb-2">&#128230;</div>
		<p class="text-gray-400">No backups yet.</p>
		<p class="text-xs text-gray-500 mt-2">Click "Create backup" to save your database, config, and credentials.</p>
	</div>
{:else}
	<div class="text-center py-10 text-gray-500">Loading...</div>
{/if}

<div class="mt-6 rounded-lg border border-gray-700 bg-gray-800 p-5">
	<p class="text-sm text-gray-400 mb-2">CLI usage:</p>
	<code class="block bg-gray-900 px-4 py-2 rounded text-xs text-gray-300 font-mono mb-1">wshm backup</code>
	<code class="block bg-gray-900 px-4 py-2 rounded text-xs text-gray-300 font-mono">wshm restore .wshm/backup-2026-04-09.tar.gz --force</code>
</div>
