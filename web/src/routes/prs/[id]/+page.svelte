<script lang="ts">
	import { page } from '$app/stores';
	import { onMount } from 'svelte';
	import { fetchPulls, type PullRequest } from '$lib/api';
	import { itemRef } from '$lib/stores';
	import * as Card from '$lib/components/ui/card';
	import PrDetail from '$lib/components/PrDetail.svelte';

	let pr: PullRequest | null = $state(null);
	let error: string | null = $state(null);
	let id = $derived(Number($page.params.id));

	onMount(async () => {
		try {
			const all = await fetchPulls({ limit: 500 });
			pr = all.items.find(p => p.number === id) ?? null;
			if (!pr) error = `PR #${id} not found`;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load';
		}
	});
</script>

<svelte:head>
	<title>wshm - PR #{id}</title>
</svelte:head>

<div class="mb-4">
	<a href="/prs" class="text-sm text-primary hover:underline">← Back to Pull Requests</a>
</div>

{#if error}
	<Card.Root><Card.Content><p class="text-red-600 dark:text-red-400">{error}</p></Card.Content></Card.Root>
{:else if pr}
	<div class="mb-4">
		<h2 class="text-xl font-semibold text-foreground">
			<span class="mono text-muted-foreground">{$itemRef(pr.number, 'pr', pr.repo)}</span> {pr.title}
		</h2>
	</div>
	<PrDetail {pr} />
{:else}
	<p class="text-muted-foreground">Loading...</p>
{/if}
