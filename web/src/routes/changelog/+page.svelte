<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchChangelog, type ChangelogResult, type ChangelogSection } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';

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

	function sectionBadge(name: string): { variant: 'outline' | 'secondary'; class: string } {
		if (name === 'Features') return { variant: 'outline', class: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400' };
		if (name === 'Bug Fixes') return { variant: 'outline', class: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400' };
		if (name === 'Refactoring') return { variant: 'outline', class: 'bg-primary/15 text-primary' };
		if (name === 'Documentation') return { variant: 'outline', class: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400' };
		if (name === 'Maintenance') return { variant: 'secondary', class: '' };
		return { variant: 'outline', class: 'border-purple-500/30 bg-purple-500/15 text-purple-600 dark:text-purple-400' };
	}

	let totalPrs: number = $derived(
		result ? result.sections.reduce((sum: number, s: ChangelogSection) => sum + s.pull_requests.length, 0) : 0
	);

	// Each section starts capped so a large merged-PR history (hundreds of
	// entries) doesn't render as one endless scroll — expand per section on
	// demand instead of paginating the whole page.
	const INITIAL_VISIBLE = 15;
	let expanded: Record<string, boolean> = $state({});
</script>

<svelte:head>
	<title>wshm - Changelog</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">Changelog</h2>
	<p class="text-sm text-muted-foreground">Auto-generated from merged pull requests</p>
</div>

{#if error}
	<div class="rounded-lg border border-red-500 bg-card p-5">
		<p class="text-red-600 dark:text-red-400">{error}</p>
	</div>
{:else if result && result.sections.length > 0}
	<div class="mb-6 text-xs text-muted-foreground">
		{totalPrs} merged PR{totalPrs !== 1 ? 's' : ''} across {result.sections.length} categor{result.sections.length !== 1 ? 'ies' : 'y'}
	</div>

	<div class="space-y-8">
		{#each result.sections as section}
			<div>
				<div class="flex items-center gap-2 mb-4">
					<span>{sectionIcon(section.name)}</span>
					<h3 class="text-lg font-semibold text-foreground">{section.name}</h3>
					<Badge variant={sectionBadge(section.name).variant} class={sectionBadge(section.name).class}>{section.pull_requests.length}</Badge>
				</div>

				<div class="space-y-2 ml-7">
					{#each (expanded[section.name] ? section.pull_requests : section.pull_requests.slice(0, INITIAL_VISIBLE)) as pr}
						<div class="flex items-start gap-3 text-sm">
							<span class="font-mono text-primary shrink-0">#{pr.number}</span>
							<div class="flex-1">
								<span class="text-foreground">{pr.title}</span>
								{#if pr.author}
									<span class="text-muted-foreground ml-2">@{pr.author}</span>
								{/if}
							</div>
							<span class="text-xs text-muted-foreground shrink-0">{pr.merged_at?.slice(0, 10) ?? ''}</span>
						</div>
					{/each}
					{#if section.pull_requests.length > INITIAL_VISIBLE}
						<button
							type="button"
							class="text-xs text-primary hover:underline"
							onclick={() => (expanded[section.name] = !expanded[section.name])}
						>
							{expanded[section.name] ? 'Show fewer' : `Show ${section.pull_requests.length - INITIAL_VISIBLE} more`}
						</button>
					{/if}
				</div>
			</div>
		{/each}
	</div>
{:else if result}
	<div class="rounded-lg border bg-card p-10 text-center">
		<svg class="h-10 w-10 mx-auto mb-2 text-muted-foreground" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" aria-hidden="true">
			<path stroke-linecap="round" stroke-linejoin="round" d="M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z" />
		</svg>
		<p class="text-muted-foreground">No merged PRs found in the database.</p>
		<p class="text-xs text-muted-foreground mt-2">
			Run <code class="bg-muted px-2 py-1 rounded text-xs">wshm changelog --days 30</code> to generate a changelog from CLI, or sync your repos to populate the database.
		</p>
	</div>
{:else}
	<div class="text-center py-10 text-muted-foreground">Loading...</div>
{/if}
