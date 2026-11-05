<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '$lib/components/ui/button';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Alert from '$lib/components/ui/alert';
	import * as Tabs from '$lib/components/ui/tabs';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import {
		fetchUsageLlm,
		fetchUsageGithub,
		type UsageLlmResponse,
		type UsageGithubResponse,
	} from '$lib/api';

	let llm = $state<UsageLlmResponse | null>(null);
	let gh = $state<UsageGithubResponse | null>(null);
	let llmError = $state<string | null>(null);
	let ghError = $state<string | null>(null);
	let loading = $state(true);
	let activeTab = $state('ai-calls');

	async function load() {
		loading = true;
		llmError = null;
		ghError = null;
		const [a, b] = await Promise.allSettled([fetchUsageLlm(), fetchUsageGithub()]);
		if (a.status === 'fulfilled') llm = a.value;
		else llmError = String(a.reason);
		if (b.status === 'fulfilled') gh = b.value;
		else ghError = String(b.reason);
		loading = false;
	}

	onMount(load);

	// GitHub /rate_limit nests counters under `resources.core` (REST) and a few
	// other buckets (search, graphql, ...). We surface `core` because that's
	// where wshm-pro's reads land; the others are shown raw in a table.
	function coreLimit(g: UsageGithubResponse | null) {
		return g?.rate_limit?.resources?.core ?? g?.rate_limit?.rate ?? null;
	}

	function pctRemaining(c: { remaining: number; limit: number } | null): number {
		if (!c || c.limit <= 0) return 0;
		return Math.round((c.remaining / c.limit) * 100);
	}

	function resetClock(epoch: number): string {
		const ms = epoch * 1000 - Date.now();
		if (ms <= 0) return 'now';
		const m = Math.floor(ms / 60000);
		const s = Math.floor((ms % 60000) / 1000);
		return m > 0 ? `${m}m ${s}s` : `${s}s`;
	}
</script>

<svelte:head>
	<title>Usage — wshm</title>
</svelte:head>

<div class="p-6 max-w-6xl mx-auto space-y-4">
	<div class="flex items-center justify-between">
		<h1 class="text-2xl font-bold tracking-tight">Usage</h1>
		<Button variant="outline" size="sm" onclick={load} disabled={loading}>
			{loading ? 'Loading…' : 'Refresh'}
		</Button>
	</div>

	<Tabs.Root bind:value={activeTab}>
		<Tabs.List>
			<Tabs.Trigger value="ai-calls">AI calls</Tabs.Trigger>
			<Tabs.Trigger value="github-api">GitHub API</Tabs.Trigger>
		</Tabs.List>

		<Tabs.Content value="ai-calls">
			{#if llmError}
				<Alert.Root variant="destructive" class="mb-4">
					<Alert.Description>Failed to load LLM usage: {llmError}</Alert.Description>
				</Alert.Root>
			{:else if !llm}
				<p class="text-muted-foreground">Loading…</p>
			{:else}
				<!-- Top-level totals -->
				<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6">
					<Card.Root size="sm">
						<Card.Content>
							<div class="text-xs uppercase text-muted-foreground">Last 24h</div>
							<div class="text-3xl font-bold">{llm.last_24h.toLocaleString()}</div>
						</Card.Content>
					</Card.Root>
					<Card.Root size="sm">
						<Card.Content>
							<div class="text-xs uppercase text-muted-foreground">Last 7d</div>
							<div class="text-3xl font-bold">{llm.last_7d.toLocaleString()}</div>
						</Card.Content>
					</Card.Root>
					<Card.Root size="sm">
						<Card.Content>
							<div class="text-xs uppercase text-muted-foreground">Last 30d</div>
							<div class="text-3xl font-bold">{llm.last_30d.toLocaleString()}</div>
						</Card.Content>
					</Card.Root>
					<Card.Root size="sm">
						<Card.Content>
							<div class="text-xs uppercase text-muted-foreground">All-time</div>
							<div class="text-3xl font-bold">{llm.total.toLocaleString()}</div>
						</Card.Content>
					</Card.Root>
				</div>

				<!-- Per-kind -->
				<h2 class="text-lg font-bold tracking-tight mb-2">By kind</h2>
				{#if llm.by_kind.length === 0}
					<p class="text-muted-foreground mb-6">No LLM calls recorded yet. The accounting hook fires on every successful triage / PR analysis — content-hash cache hits don't count.</p>
				{:else}
					<div class="rounded-lg border mb-6">
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head>Kind</Table.Head>
									<Table.Head>24h</Table.Head>
									<Table.Head>7d</Table.Head>
									<Table.Head>30d</Table.Head>
									<Table.Head>Total</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each llm.by_kind as k (k.kind)}
									<Table.Row>
										<Table.Cell><Badge class="bg-primary/15 text-primary" variant="outline">{k.kind}</Badge></Table.Cell>
										<Table.Cell>{k.last_24h.toLocaleString()}</Table.Cell>
										<Table.Cell>{k.last_7d.toLocaleString()}</Table.Cell>
										<Table.Cell>{k.last_30d.toLocaleString()}</Table.Cell>
										<Table.Cell class="font-semibold">{k.total.toLocaleString()}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					</div>
				{/if}

				<!-- Per-repo -->
				<h2 class="text-lg font-bold tracking-tight mb-2">By repo</h2>
				{#if llm.by_repo.length === 0}
					<p class="text-muted-foreground">No repos configured.</p>
				{:else}
					<div class="rounded-lg border">
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head>Repo</Table.Head>
									<Table.Head>24h</Table.Head>
									<Table.Head>7d</Table.Head>
									<Table.Head>30d</Table.Head>
									<Table.Head>Total</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each llm.by_repo as r (r.repo)}
									<Table.Row>
										<Table.Cell class="font-mono text-sm">{r.repo}</Table.Cell>
										<Table.Cell>{r.last_24h.toLocaleString()}</Table.Cell>
										<Table.Cell>{r.last_7d.toLocaleString()}</Table.Cell>
										<Table.Cell>{r.last_30d.toLocaleString()}</Table.Cell>
										<Table.Cell class="font-semibold">{r.total.toLocaleString()}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					</div>
				{/if}
			{/if}
		</Tabs.Content>

		<Tabs.Content value="github-api">
			{#if ghError}
				<Alert.Root variant="destructive" class="mb-4">
					<Alert.Description>Failed to load GitHub usage: {ghError}</Alert.Description>
				</Alert.Root>
			{:else if !gh}
				<p class="text-muted-foreground">Loading…</p>
			{:else if gh.rate_limit?.error}
				<Alert.Root class="border-yellow-500/40 bg-yellow-500/10 text-yellow-700 dark:text-yellow-200 [&>svg]:text-yellow-500">
					<TriangleAlertIcon />
					<Alert.Description>GitHub /rate_limit unavailable: {gh.rate_limit.error}</Alert.Description>
				</Alert.Root>
			{:else}
				{@const core = coreLimit(gh)}
				{#if core}
					<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6">
						<Card.Root size="sm">
							<Card.Content>
								<div class="text-xs uppercase text-muted-foreground">Remaining</div>
								<div class="text-3xl font-bold">{core.remaining.toLocaleString()}</div>
								<div class="text-sm text-muted-foreground">/ {core.limit.toLocaleString()}</div>
							</Card.Content>
						</Card.Root>
						<Card.Root size="sm">
							<Card.Content>
								<div class="text-xs uppercase text-muted-foreground">Used</div>
								<div class="text-3xl font-bold">{core.used.toLocaleString()}</div>
							</Card.Content>
						</Card.Root>
						<Card.Root size="sm">
							<Card.Content>
								<div class="text-xs uppercase text-muted-foreground">% Remaining</div>
								<div class="text-3xl font-bold">{pctRemaining(core)}%</div>
							</Card.Content>
						</Card.Root>
						<Card.Root size="sm">
							<Card.Content>
								<div class="text-xs uppercase text-muted-foreground">Resets in</div>
								<div class="text-3xl font-bold">{resetClock(core.reset)}</div>
							</Card.Content>
						</Card.Root>
					</div>
				{/if}

				<h2 class="text-lg font-bold tracking-tight mb-2">All buckets</h2>
				{#if gh.rate_limit?.resources}
					<div class="rounded-lg border mb-6">
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head>Bucket</Table.Head>
									<Table.Head>Remaining</Table.Head>
									<Table.Head>Limit</Table.Head>
									<Table.Head>Used</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each Object.entries(gh.rate_limit.resources) as [name, r] (name)}
									<Table.Row>
										<Table.Cell><Badge variant="secondary">{name}</Badge></Table.Cell>
										<Table.Cell>{r.remaining.toLocaleString()}</Table.Cell>
										<Table.Cell>{r.limit.toLocaleString()}</Table.Cell>
										<Table.Cell>{r.used.toLocaleString()}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					</div>
				{/if}

				<h2 class="text-lg font-bold tracking-tight mb-2">Per-repo last sync</h2>
				<div class="rounded-lg border">
					<Table.Root>
						<Table.Header>
							<Table.Row>
								<Table.Head>Repo</Table.Head>
								<Table.Head>Last issue sync</Table.Head>
								<Table.Head>Last pull sync</Table.Head>
							</Table.Row>
						</Table.Header>
						<Table.Body>
							{#each gh.by_repo as r (r.repo)}
								<Table.Row>
									<Table.Cell class="font-mono text-sm">{r.repo}</Table.Cell>
									<Table.Cell>{r.last_issue_sync ?? '—'}</Table.Cell>
									<Table.Cell>{r.last_pull_sync ?? '—'}</Table.Cell>
								</Table.Row>
							{/each}
						</Table.Body>
					</Table.Root>
				</div>
			{/if}
		</Tabs.Content>
	</Tabs.Root>
</div>
