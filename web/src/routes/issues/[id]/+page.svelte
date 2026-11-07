<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { fetchIssues, type Issue } from '$lib/api';
	import * as Card from '$lib/components/ui/card';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import RunGraph from '$lib/components/RunGraph.svelte';

	let issue: Issue | null = $state(null);
	let error: string | null = $state(null);
	let id = $derived(Number($page.params.id));

	onMount(async () => {
		try {
			const all = await fetchIssues({ limit: 500 });
			issue = all.items.find(i => i.number === id) ?? null;
			if (!issue) error = `Issue #${id} not found`;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load';
		}
	});
</script>

<svelte:head>
	<title>wshm - Issue #{id}</title>
</svelte:head>

<div class="mb-4">
	<a href="/issues" class="text-sm text-primary hover:underline">← Back to Issues</a>
</div>

{#if error}
	<Card.Root><Card.Content><p class="text-red-600 dark:text-red-400">{error}</p></Card.Content></Card.Root>
{:else if issue}
	<div class="mb-4">
		<h2 class="text-xl font-semibold text-foreground">
			<span class="mono text-muted-foreground">#{issue.number}</span> {issue.title}
		</h2>
	</div>
	<IssueDetail {issue} />
	<RunGraph repo={issue.repo} kind="issue" number={issue.number} />
{:else}
	<p class="text-muted-foreground">Loading...</p>
{/if}
