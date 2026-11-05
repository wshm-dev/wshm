<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchImprovements, type ImprovementsResult, type Suggestion } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';

	let result: ImprovementsResult | null = $state(null);
	let error: string | null = $state(null);

	async function load() {
		try {
			error = null;
			result = await fetchImprovements();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load improvements';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function categoryClass(cat: string): string {
		if (cat.includes('security')) return 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
		if (cat.includes('performance')) return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		if (cat.includes('testing')) return 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
		if (cat.includes('refactor')) return 'bg-primary/15 text-primary';
		if (cat.includes('feature')) return 'border-purple-500/30 bg-purple-500/15 text-purple-600 dark:text-purple-400';
		return 'border-transparent bg-secondary text-secondary-foreground';
	}

	function effortClass(effort: string): string {
		if (effort === 'trivial') return 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
		if (effort === 'small') return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		if (effort === 'medium') return 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
		return 'border-transparent bg-secondary text-secondary-foreground';
	}
</script>

<svelte:head>
	<title>wshm - Improvements</title>
</svelte:head>

	<div class="mb-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Improvement Proposals</h2>
		<p class="text-sm text-muted-foreground">AI-analyzed codebase improvement suggestions</p>
	</div>

	{#if error}
		<div class="rounded-lg border border-red-500 bg-card p-5">
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</div>
	{:else if result && result.suggestions.length > 0}
		<div class="space-y-4">
			{#each result.suggestions as s}
				<div class="rounded-lg border bg-card p-5">
					<div class="flex items-start justify-between mb-3">
						<div class="flex-1">
							<h3 class="text-sm font-semibold text-foreground">{s.title}</h3>
							{#if s.body}
								<p class="text-xs text-muted-foreground mt-1 line-clamp-2">{s.body}</p>
							{/if}
						</div>
						<div class="flex gap-2 ml-4 shrink-0">
							<Badge variant="outline" class={categoryClass(s.category)}>{s.category}</Badge>
							<Badge variant="outline" class={effortClass(s.effort)}>{s.effort}</Badge>
						</div>
					</div>
					<div class="flex items-center gap-4 text-xs text-muted-foreground">
						{#if s.issue_number}
							<span class="font-mono text-primary">#{s.issue_number}</span>
						{/if}
						{#if s.priority}
							<span>Priority: <span class="text-foreground/90">{s.priority}</span></span>
						{/if}
						<span>{s.created_at?.slice(0, 10) ?? ''}</span>
						<span class="text-muted-foreground">{s.repo}</span>
					</div>
				</div>
			{/each}
		</div>
	{:else if result}
		<div class="rounded-lg border bg-card p-10 text-center">
			<div class="text-2xl mb-2">&#128161;</div>
			<p class="text-muted-foreground">No improvement suggestions yet.</p>
			<p class="text-xs text-muted-foreground mt-2">
				Run <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm improve --apply</code> to generate suggestions and create issues.
			</p>
		</div>
	{:else}
		<div class="text-center py-10 text-muted-foreground">Loading...</div>
	{/if}
