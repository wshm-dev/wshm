<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchRevertPreview, type RevertPreview } from '$lib/api';
	import { Card, Alert, Heading, P } from 'flowbite-svelte';

	let preview = $state<RevertPreview | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			error = null;
			preview = await fetchRevertPreview();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load revert preview';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	let totalActions = $derived(
		preview ? preview.repos.reduce((sum: number, r) => sum + r.triage_results + r.pr_analyses + r.labels_to_remove, 0) : 0
	);
</script>

<svelte:head>
	<title>wshm - Revert</title>
</svelte:head>

<div class="mb-6">
	<Heading tag="h2" class="text-xl mb-1">Revert Actions</Heading>
	<P class="text-sm text-gray-500">Preview and undo all wshm-applied labels, comments, and analyses</P>
</div>

{#if error}
	<Alert color="red">{error}</Alert>
{:else if preview}
	<Alert color="yellow" class="mb-6 !border !bg-yellow-50 !text-yellow-900 !border-yellow-300 dark:!bg-yellow-900/20 dark:!text-yellow-100 dark:!border-yellow-700/50">
		<Heading tag="h3" class="text-sm font-semibold mb-1">Destructive Operation</Heading>
		<P class="text-xs">
			Reverting will remove all wshm comments, labels, triage results, and PR analyses from GitHub.
			This cannot be undone. Use <code class="bg-yellow-900/50 px-1.5 py-0.5 rounded">wshm revert --apply</code> from the CLI to execute.
		</P>
	</Alert>

	{#if totalActions === 0}
		<Card class="bg-gray-800 border-green-800 max-w-none text-center p-10">
			<svg class="h-10 w-10 mx-auto mb-2 text-green-500" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" aria-hidden="true">
				<path stroke-linecap="round" stroke-linejoin="round" d="M9 12.75 11.25 15 15 9.75M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z" />
			</svg>
			<P class="text-green-400 font-semibold">Nothing to revert</P>
			<P class="text-xs text-gray-500 mt-1">No wshm actions found in the database</P>
		</Card>
	{:else}
		<div class="space-y-4">
			{#each preview.repos as repo}
				<Card class="bg-gray-800 border-gray-700 max-w-none">
					<Heading tag="h3" class="text-sm font-semibold mb-4">{repo.repo}</Heading>

					<div class="grid grid-cols-3 gap-4">
						<Card class="bg-gray-900 border-gray-700 max-w-none text-center !p-4">
							<div class="text-2xl font-bold text-orange-400">{repo.triage_results}</div>
							<div class="text-xs text-gray-500 mt-1">Triage Results</div>
							<div class="text-[0.625rem] text-gray-600">Comments + classifications</div>
						</Card>
						<Card class="bg-gray-900 border-gray-700 max-w-none text-center !p-4">
							<div class="text-2xl font-bold text-orange-400">{repo.pr_analyses}</div>
							<div class="text-xs text-gray-500 mt-1">PR Analyses</div>
							<div class="text-[0.625rem] text-gray-600">Risk + type + summary</div>
						</Card>
						<Card class="bg-gray-900 border-gray-700 max-w-none text-center !p-4">
							<div class="text-2xl font-bold text-orange-400">{repo.labels_to_remove}</div>
							<div class="text-xs text-gray-500 mt-1">Labels</div>
							<div class="text-[0.625rem] text-gray-600">wshm-applied labels</div>
						</Card>
					</div>
				</Card>
			{/each}
		</div>

		<Card class="mt-6 bg-gray-800 border-gray-700 max-w-none">
			<P class="text-sm text-gray-400 mb-2">
				To revert all actions, run from the CLI:
			</P>
			<code class="block bg-gray-900 px-4 py-3 rounded text-xs text-gray-300 font-mono">
				wshm revert --apply
			</code>
			<P class="text-xs text-gray-500 mt-3">
				Dry-run first with <code class="bg-gray-900 px-1.5 py-0.5 rounded">wshm revert</code> (no --apply) to see what would happen.
			</P>
		</Card>
	{/if}
{:else}
	<div class="text-center py-10 text-gray-500">Loading...</div>
{/if}
