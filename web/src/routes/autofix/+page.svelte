<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchAutofixCandidates, type AutofixResult, type AutofixCandidate } from '$lib/api';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';

	let result = $state<AutofixResult | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			error = null;
			result = await fetchAutofixCandidates();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load auto-fix candidates';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function confidenceBar(conf: number): string {
		const pct = Math.round(conf * 100);
		if (pct >= 90) return 'bg-green-500';
		if (pct >= 80) return 'bg-yellow-500';
		return 'bg-red-500';
	}

	const badgeClasses = {
		red: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400',
		yellow: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
		green: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400'
	} as const;
</script>

<svelte:head>
	<title>wshm - Auto-Fix</title>
</svelte:head>

	<div class="mb-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Auto-Fix</h2>
		<p class="text-sm text-muted-foreground">Issues eligible for automated PR generation (confidence &ge; 85%, simple fix)</p>
	</div>

	{#if error}
		<div class="rounded-lg border border-red-500 bg-card p-5">
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</div>
	{:else if result && result.candidates.length > 0}
		<div class="mb-4 text-xs text-muted-foreground">
			{result.candidates.length} candidate{result.candidates.length !== 1 ? 's' : ''} ready for auto-fix
		</div>

		<div class="rounded-lg border">
			<Table.Root class="table-fixed">
				<Table.Header>
					<Table.Row>
						<Table.Head class="w-[70px]">Issue</Table.Head>
						<Table.Head>Title</Table.Head>
						<Table.Head class="w-[110px]">Category</Table.Head>
						<Table.Head class="w-[90px]">Priority</Table.Head>
						<Table.Head class="w-[130px]">Confidence</Table.Head>
						<Table.Head class="w-[100px]">Created</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each result.candidates as c}
						<Table.Row>
							<Table.Cell class="font-mono text-primary">#{c.number}</Table.Cell>
							<Table.Cell class="overflow-hidden">
								<div class="truncate">{c.title}</div>
								{#if c.summary}
									<div class="text-xs text-muted-foreground mt-1 truncate">{c.summary}</div>
								{/if}
							</Table.Cell>
							<Table.Cell><Badge variant="outline" class={badgeClasses.red}>{c.category}</Badge></Table.Cell>
							<Table.Cell>
								{#if c.priority}
									<Badge variant="outline" class={badgeClasses[c.priority === 'high' ? 'red' : c.priority === 'medium' ? 'yellow' : 'green']}>{c.priority}</Badge>
								{:else}
									<span class="text-muted-foreground">-</span>
								{/if}
							</Table.Cell>
							<Table.Cell>
								<div class="flex items-center gap-2">
									<div class="w-16 h-1.5 bg-muted rounded-full overflow-hidden">
										<div class="{confidenceBar(c.confidence)} h-full rounded-full" style="width: {Math.round(c.confidence * 100)}%"></div>
									</div>
									<span class="text-xs text-muted-foreground">{Math.round(c.confidence * 100)}%</span>
								</div>
							</Table.Cell>
							<Table.Cell class="text-xs text-muted-foreground">{c.created_at?.slice(0, 10) ?? ''}</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		</div>

		<div class="mt-6 rounded-lg border bg-card p-5">
			<p class="text-sm text-muted-foreground">
				To auto-fix an issue, run: <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm fix --issue N --apply</code>
			</p>
			<p class="text-xs text-muted-foreground mt-2">
				Uses AI to generate a fix, run tests, and open a draft PR. Requires a configured AI provider.
			</p>
		</div>
	{:else if result}
		<div class="rounded-lg border bg-card p-10 text-center">
			<div class="text-2xl mb-2">&#9889;</div>
			<p class="text-muted-foreground">No auto-fix candidates found.</p>
			<p class="text-xs text-muted-foreground mt-2">
				Candidates appear when triage identifies simple bugs with high confidence (&ge; 85%).
				Run <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm triage --apply</code> first.
			</p>
		</div>
	{:else}
		<div class="text-center py-10 text-muted-foreground">Loading...</div>
	{/if}
