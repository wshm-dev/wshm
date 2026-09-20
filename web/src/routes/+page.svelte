<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, type Status } from '$lib/api';
	import { timeAgo, exactTime } from '$lib/time';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import ActivitySparkline from '$lib/components/ActivitySparkline.svelte';

	let status: Status | null = $state(null);
	let error: string | null = $state(null);

	async function load() {
		try {
			error = null;
			status = await fetchStatus();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load status';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});
</script>

<svelte:head>
	<title>wshm - Dashboard</title>
</svelte:head>

<div class="mb-6">
	<h2 class="text-xl font-semibold text-foreground mb-1">Dashboard</h2>
	<p class="text-sm text-muted-foreground">Repository status overview</p>
</div>

{#if error}
	<Card.Root class="border-red-500">
		<Card.Content>
			<p class="text-red-600 dark:text-red-400">{error}</p>
			<p class="mt-2 text-sm text-muted-foreground">Make sure the wshm server is running.</p>
		</Card.Content>
	</Card.Root>
{:else}
	<div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
		<a href="/issues" class="block">
			<Card.Root class="text-center transition-colors hover:border-primary/50 hover:bg-accent/30">
				<Card.Content>
					<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">Open Issues</div>
					<div class="text-3xl font-bold text-foreground mono">{status?.open_issues ?? '--'}</div>
					{#if status && status.issues_new_7d > 0}
						<div class="text-xs text-green-600 dark:text-green-400 mt-1">+{status.issues_new_7d} this week</div>
					{/if}
				</Card.Content>
			</Card.Root>
		</a>
		<a href="/prs" class="block">
			<Card.Root class="text-center transition-colors hover:border-primary/50 hover:bg-accent/30">
				<Card.Content>
					<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">Open PRs</div>
					<div class="text-3xl font-bold text-foreground mono">{status?.open_prs ?? '--'}</div>
					{#if status && status.prs_new_7d > 0}
						<div class="text-xs text-green-600 dark:text-green-400 mt-1">+{status.prs_new_7d} this week</div>
					{/if}
				</Card.Content>
			</Card.Root>
		</a>
		<a href="/issues" class="block">
			<Card.Root class="text-center transition-colors hover:border-primary/50 hover:bg-accent/30">
				<Card.Content>
					<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">Untriaged</div>
					<div class="text-3xl font-bold text-foreground mono">{status?.untriaged ?? '--'}</div>
				</Card.Content>
			</Card.Root>
		</a>
		<a href="/prs?conflicts=yes" class="block">
			<Card.Root class="text-center transition-colors hover:border-primary/50 hover:bg-accent/30">
				<Card.Content>
					<div class="text-xs uppercase tracking-wider text-muted-foreground mb-2">Conflicts</div>
					<div
						class="text-3xl font-bold mono {status?.conflicts ? 'text-red-600 dark:text-red-400' : 'text-foreground'}"
					>{status?.conflicts ?? '--'}</div>
				</Card.Content>
			</Card.Root>
		</a>
	</div>

	{#if status && status.daily_activity.length > 0}
		<Card.Root class="mt-6">
			<Card.Header>
				<Card.Title class="text-base">Activity — last 14 days</Card.Title>
			</Card.Header>
			<Card.Content>
				<ActivitySparkline data={status.daily_activity} />
			</Card.Content>
		</Card.Root>
	{/if}

	<Card.Root class="mt-6">
		<Card.Header>
			<Card.Title class="text-xl">Repositories</Card.Title>
			<Card.Action>
				<span class="text-xs text-muted-foreground" title={exactTime(status?.last_sync)}>
					Last sync: {timeAgo(status?.last_sync)}
				</span>
			</Card.Action>
		</Card.Header>
		<Card.Content>
			{#if status && status.repos.length > 0}
				<div class="w-full overflow-x-auto">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5">Repository</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px] text-right">Issues</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px] text-right">PRs</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[90px] text-right">Untriaged</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[90px] text-right">Conflicts</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[110px]">Last sync</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px]">Mode</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each status.repos as repo}
							<Table.Row>
								<Table.Cell class="px-2 py-1.5 mono text-foreground">{repo.slug}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 mono text-right">{repo.open_issues}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 mono text-right">{repo.open_prs}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 mono text-right">{repo.untriaged}</Table.Cell>
								<Table.Cell
									class="px-2 py-1.5 mono text-right {repo.conflicts ? 'text-red-600 dark:text-red-400' : ''}"
								>{repo.conflicts}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground" title={exactTime(repo.last_sync)}>{timeAgo(repo.last_sync)}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">
									{#if repo.apply}
										<Badge variant="outline" class="border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400">apply</Badge>
									{:else}
										<Badge variant="secondary">dry-run</Badge>
									{/if}
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
				</div>
			{:else}
				<p class="text-sm text-muted-foreground">No repositories configured.</p>
			{/if}
		</Card.Content>
	</Card.Root>
{/if}
