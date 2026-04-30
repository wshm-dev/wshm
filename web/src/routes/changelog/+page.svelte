<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchChangelog, type ChangelogResult, type ChangelogSection } from '$lib/api';
	import { Badge } from 'flowbite-svelte';

	let result = $state<ChangelogResult | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			error = null;
			result = await fetchChangelog();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load changelog';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function sectionIcon(name: string): string {
		if (name === 'Features') return '\u2728';
		if (name === 'Bug Fixes') return '\uD83D\uDC1B';
		if (name === 'Refactoring') return '\u267B\uFE0F';
		if (name === 'Documentation') return '\uD83D\uDCDD';
		if (name === 'Maintenance') return '\uD83D\uDD27';
		return '\uD83D\uDCE6';
	}

	function sectionColor(name: string): 'green' | 'red' | 'blue' | 'yellow' | 'gray' | 'indigo' {
		if (name === 'Features') return 'green';
		if (name === 'Bug Fixes') return 'red';
		if (name === 'Refactoring') return 'blue';
		if (name === 'Documentation') return 'yellow';
		if (name === 'Maintenance') return 'gray';
		return 'indigo';
	}

	let totalPrs: number = $derived(
		result ? result.sections.reduce((sum: number, s: ChangelogSection) => sum + s.pull_requests.length, 0) : 0
	);
</script>

<svelte:head>
	<title>wshm - Changelog</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-gray-100 mb-1">Changelog</h2>
	<p class="text-sm text-gray-500">Auto-generated from merged pull requests</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-gray-800 p-5">
		<p class="text-red-400">{error}</p>
	</div>
{:else if result && result.sections.length > 0}
	<div class="mb-6 text-xs text-gray-500">
		{totalPrs} merged PR{totalPrs !== 1 ? 's' : ''} across {result.sections.length} categor{result.sections.length !== 1 ? 'ies' : 'y'}
	</div>

	<div class="space-y-8">
		{#each result.sections as section}
			<div>
				<div class="flex items-center gap-2 mb-4">
					<span>{sectionIcon(section.name)}</span>
					<h3 class="text-lg font-semibold text-gray-100">{section.name}</h3>
					<Badge color={sectionColor(section.name)}>{section.pull_requests.length}</Badge>
				</div>

				<div class="space-y-2 ml-7">
					{#each section.pull_requests as pr}
						<div class="flex items-start gap-3 text-sm">
							<span class="font-mono text-blue-400 shrink-0">#{pr.number}</span>
							<div class="flex-1">
								<span class="text-gray-200">{pr.title}</span>
								{#if pr.author}
									<span class="text-gray-600 ml-2">@{pr.author}</span>
								{/if}
							</div>
							<span class="text-xs text-gray-600 shrink-0">{pr.merged_at?.slice(0, 10) ?? ''}</span>
						</div>
					{/each}
				</div>
			</div>
		{/each}
	</div>
{:else if result}
	<div class="rounded-lg border border-gray-700 bg-gray-800 p-10 text-center">
		<div class="text-2xl mb-2">&#128220;</div>
		<p class="text-gray-400">No merged PRs found in the database.</p>
		<p class="text-xs text-gray-500 mt-2">
			Run <code class="bg-gray-900 px-2 py-1 rounded text-xs">wshm changelog --days 30</code> to generate a changelog from CLI, or sync your repos to populate the database.
		</p>
	</div>
{:else}
	<div class="text-center py-10 text-gray-500">Loading...</div>
{/if}
