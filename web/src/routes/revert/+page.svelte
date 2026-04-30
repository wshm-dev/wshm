<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchRevertPreview, type RevertPreview } from '$lib/api';

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
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Revert Actions</h2>
	<p class="text-sm text-gray-500">Preview and undo all wshm-applied labels, comments, and analyses</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
{:else if preview}
	<!-- Warning banner -->
	<div class="rounded-lg border border-yellow-700 bg-yellow-900/20 p-5 mb-6">
		<div class="flex items-start gap-3">
			<span class="text-xl">&#9888;&#65039;</span>
			<div>
				<h3 class="text-sm font-semibold text-yellow-300 mb-1">Destructive Operation</h3>
				<p class="text-xs text-yellow-200/70">
					Reverting will remove all wshm comments, labels, triage results, and PR analyses from GitHub.
					This cannot be undone. Use <code class="bg-yellow-900/50 px-1.5 py-0.5 rounded">wshm revert --apply</code> from the CLI to execute.
				</p>
			</div>
		</div>
	</div>

	{#if totalActions === 0}
		<div class="rounded-lg border border-green-800 bg-gray-800 p-10 text-center">
			<div class="text-2xl mb-2">&#9989;</div>
			<p class="text-green-400 font-semibold">Nothing to revert</p>
			<p class="text-xs text-gray-500 mt-1">No wshm actions found in the database</p>
		</div>
	{:else}
		<div class="space-y-4">
			{#each preview.repos as repo}
				<div class="rounded-lg border border-gray-700 bg-gray-800 p-6">
					<h3 class="text-sm font-semibold text-gray-100 mb-4">{repo.repo}</h3>

					<div class="grid grid-cols-3 gap-4">
						<div class="rounded-lg bg-gray-900 p-4 text-center">
							<div class="text-2xl font-bold text-orange-400">{repo.triage_results}</div>
							<div class="text-xs text-gray-500 mt-1">Triage Results</div>
							<div class="text-[0.625rem] text-gray-600">Comments + classifications</div>
						</div>
						<div class="rounded-lg bg-gray-900 p-4 text-center">
							<div class="text-2xl font-bold text-orange-400">{repo.pr_analyses}</div>
							<div class="text-xs text-gray-500 mt-1">PR Analyses</div>
							<div class="text-[0.625rem] text-gray-600">Risk + type + summary</div>
						</div>
						<div class="rounded-lg bg-gray-900 p-4 text-center">
							<div class="text-2xl font-bold text-orange-400">{repo.labels_to_remove}</div>
							<div class="text-xs text-gray-500 mt-1">Labels</div>
							<div class="text-[0.625rem] text-gray-600">wshm-applied labels</div>
						</div>
					</div>
				</div>
			{/each}
		</div>

		<div class="mt-6 rounded-lg border border-gray-700 bg-gray-800 p-5">
			<p class="text-sm text-gray-400 mb-2">
				To revert all actions, run from the CLI:
			</p>
			<code class="block bg-gray-900 px-4 py-3 rounded text-xs text-gray-300 font-mono">
				wshm revert --apply
			</code>
			<p class="text-xs text-gray-500 mt-3">
				Dry-run first with <code class="bg-gray-900 px-1.5 py-0.5 rounded">wshm revert</code> (no --apply) to see what would happen.
			</p>
		</div>
	{/if}
{:else}
	<div class="text-center py-10 text-gray-500">Loading...</div>
{/if}
