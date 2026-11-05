<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchConflicts, type ConflictScan, type ConflictPr } from '$lib/api';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { paginate, totalPages } from '$lib/paginate';

	let scan: ConflictScan | null = $state(null);
	let error: string | null = $state(null);
	let page = $state(0);

	async function load() {
		page = 0;
		try {
			error = null;
			scan = await fetchConflicts();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load conflicts';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});
</script>

<svelte:head>
	<title>wshm - Conflicts</title>
</svelte:head>

	<div class="mb-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Conflict Resolution</h2>
		<p class="text-sm text-muted-foreground">Detect and resolve merge conflicts across open PRs</p>
	</div>

	{#if error}
		<div class="rounded-lg border border-red-500 bg-card p-5">
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</div>
	{:else if scan}
		<!-- Summary cards -->
		<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
			<div class="rounded-lg border bg-card p-5 text-center">
				<div class="text-2xl font-bold text-foreground">{scan.total}</div>
				<div class="text-xs text-muted-foreground">Total PRs</div>
			</div>
			<div class="rounded-lg border border-green-500/30 bg-card p-5 text-center">
				<div class="text-2xl font-bold text-green-600 dark:text-green-400">{scan.clean}</div>
				<div class="text-xs text-muted-foreground">Clean</div>
			</div>
			<div class="rounded-lg border border-red-500/30 bg-card p-5 text-center">
				<div class="text-2xl font-bold text-red-600 dark:text-red-400">{scan.conflicts.length}</div>
				<div class="text-xs text-muted-foreground">Conflicts</div>
			</div>
			<div class="rounded-lg border bg-card p-5 text-center">
				<div class="text-2xl font-bold text-muted-foreground">{scan.unknown}</div>
				<div class="text-xs text-muted-foreground">Unknown</div>
			</div>
		</div>

		{@const pages = totalPages(scan.conflicts.length)}
		{@const paged = paginate(scan.conflicts, page)}
		{#if scan.conflicts.length === 0}
			<div class="rounded-lg border border-green-500/30 bg-card p-10 text-center">
				<div class="text-2xl mb-2">&#9989;</div>
				<p class="text-green-600 dark:text-green-400 font-semibold">No conflicts detected</p>
				<p class="text-xs text-muted-foreground mt-1">All open PRs can be merged cleanly</p>
			</div>
		{:else}
			<div class="rounded-lg border">
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head>PR</Table.Head>
							<Table.Head>Title</Table.Head>
							<Table.Head>Author</Table.Head>
							<Table.Head>Branches</Table.Head>
							<Table.Head>CI</Table.Head>
							<Table.Head>Updated</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each paged as c}
							<Table.Row>
								<Table.Cell class="font-mono text-red-600 dark:text-red-400">#{c.number}</Table.Cell>
								<Table.Cell>
									<div class="max-w-xs truncate">{c.title}</div>
								</Table.Cell>
								<Table.Cell class="text-muted-foreground">{c.author ?? ''}</Table.Cell>
								<Table.Cell class="text-xs font-mono text-muted-foreground">
									{c.head_ref ?? '?'} &larr; {c.base_ref ?? '?'}
								</Table.Cell>
								<Table.Cell>
									{#if c.ci_status === 'success'}
										<Badge variant="outline" class="border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400">passing</Badge>
									{:else if c.ci_status === 'failure'}
										<Badge variant="outline" class="border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400">failing</Badge>
									{:else}
										<Badge variant="secondary">pending</Badge>
									{/if}
								</Table.Cell>
								<Table.Cell class="text-xs text-muted-foreground">{c.updated_at?.slice(0, 10) ?? ''}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>

			{#if pages > 1}
				<div class="flex justify-center gap-2 mt-4">
					<button disabled={page === 0} onclick={() => page--} class="px-3 py-1 text-xs rounded bg-muted disabled:opacity-40">&laquo;</button>
					<span class="text-xs text-muted-foreground py-1">{page + 1} / {pages}</span>
					<button disabled={page >= pages - 1} onclick={() => page++} class="px-3 py-1 text-xs rounded bg-muted disabled:opacity-40">&raquo;</button>
				</div>
			{/if}

			<div class="mt-6 rounded-lg border bg-card p-5">
				<p class="text-sm text-muted-foreground">
					To resolve conflicts, run: <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm conflicts scan --apply</code>
				</p>
			</div>
		{/if}
	{:else}
		<div class="text-center py-10 text-muted-foreground">Loading...</div>
	{/if}
