<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchReports, type ReportsResult, type RepoReport } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';

	let result = $state<ReportsResult | null>(null);
	let error = $state<string | null>(null);

	async function load() {
		try {
			error = null;
			result = await fetchReports();
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load reports';
		}
	}

	onMount(() => {
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function ageColor(days: number): string {
		if (days < 3) return 'text-green-600 dark:text-green-400';
		if (days < 7) return 'text-yellow-600 dark:text-yellow-400';
		if (days < 30) return 'text-orange-600 dark:text-orange-400';
		return 'text-red-600 dark:text-red-400';
	}

	function slaStatus(over7d: number, total: number): 'green' | 'yellow' | 'red' {
		if (total === 0) return 'green';
		const pct = over7d / total;
		if (pct < 0.1) return 'green';
		if (pct < 0.3) return 'yellow';
		return 'red';
	}

	const badgeClasses = {
		green: 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400',
		yellow: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400',
		red: 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400'
	} as const;
</script>

<svelte:head>
	<title>wshm - Reports</title>
</svelte:head>

	<div class="mb-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Reports</h2>
		<p class="text-sm text-muted-foreground">Repository health metrics and SLA overview</p>
	</div>

	{#if error}
		<div class="rounded-lg border border-red-500 bg-card p-5">
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</div>
	{:else if result && result.repos.length > 0}
		{#each result.repos as repo}
			<div class="rounded-lg border bg-card p-6 mb-6">
				<h3 class="text-lg font-semibold text-foreground mb-4">{repo.slug}</h3>

				<!-- Overview cards -->
				<div class="grid grid-cols-2 md:grid-cols-5 gap-3 mb-6">
					<div class="rounded-lg bg-muted/40 p-4 text-center">
						<div class="text-xl font-bold text-foreground">{repo.open_issues}</div>
						<div class="text-xs text-muted-foreground">Open Issues</div>
						{#if repo.untriaged > 0}
							<div class="text-xs text-yellow-600 dark:text-yellow-400 mt-1">{repo.untriaged} untriaged</div>
						{/if}
					</div>
					<div class="rounded-lg bg-muted/40 p-4 text-center">
						<div class="text-xl font-bold text-foreground">{repo.open_prs}</div>
						<div class="text-xs text-muted-foreground">Open PRs</div>
						{#if repo.unanalyzed > 0}
							<div class="text-xs text-yellow-600 dark:text-yellow-400 mt-1">{repo.unanalyzed} unanalyzed</div>
						{/if}
					</div>
					<div class="rounded-lg bg-muted/40 p-4 text-center">
						<div class="text-xl font-bold {repo.conflicts > 0 ? 'text-red-600 dark:text-red-400' : 'text-green-600 dark:text-green-400'}">{repo.conflicts}</div>
						<div class="text-xs text-muted-foreground">Conflicts</div>
					</div>
					<div class="rounded-lg bg-muted/40 p-4 text-center">
						<div class="text-xl font-bold {ageColor(repo.sla.avg_pr_age_days)}">{repo.sla.avg_pr_age_days}d</div>
						<div class="text-xs text-muted-foreground">Avg PR Age</div>
					</div>
					<div class="rounded-lg bg-muted/40 p-4 text-center">
						<div class="text-xl font-bold {ageColor(repo.sla.avg_issue_age_days)}">{repo.sla.avg_issue_age_days}d</div>
						<div class="text-xs text-muted-foreground">Avg Issue Age</div>
					</div>
				</div>

				<!-- SLA -->
				<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6">
					<div class="rounded-lg bg-muted/40 p-3">
						<div class="flex items-center justify-between">
							<span class="text-xs text-muted-foreground">PRs &gt; 7 days</span>
							<Badge variant="outline" class={badgeClasses[slaStatus(repo.sla.prs_over_7d, repo.open_prs)]}>{repo.sla.prs_over_7d}</Badge>
						</div>
					</div>
					<div class="rounded-lg bg-muted/40 p-3">
						<div class="flex items-center justify-between">
							<span class="text-xs text-muted-foreground">PRs &gt; 30 days</span>
							<Badge variant="outline" class={badgeClasses[repo.sla.prs_over_30d > 0 ? 'red' : 'green']}>{repo.sla.prs_over_30d}</Badge>
						</div>
					</div>
					<div class="rounded-lg bg-muted/40 p-3">
						<div class="flex items-center justify-between">
							<span class="text-xs text-muted-foreground">Issues &gt; 7 days</span>
							<Badge variant="outline" class={badgeClasses[slaStatus(repo.sla.issues_over_7d, repo.open_issues)]}>{repo.sla.issues_over_7d}</Badge>
						</div>
					</div>
					<div class="rounded-lg bg-muted/40 p-3">
						<div class="flex items-center justify-between">
							<span class="text-xs text-muted-foreground">Issues &gt; 30 days</span>
							<Badge variant="outline" class={badgeClasses[repo.sla.issues_over_30d > 0 ? 'red' : 'green']}>{repo.sla.issues_over_30d}</Badge>
						</div>
					</div>
				</div>

				<!-- Distributions -->
				<div class="grid md:grid-cols-2 gap-4">
					{#if Object.keys(repo.categories).length > 0}
						<div class="rounded-lg bg-muted/40 p-4">
							<h4 class="text-sm font-semibold text-foreground/90 mb-3">Triage Categories</h4>
							<div class="space-y-2">
								{#each Object.entries(repo.categories).sort((a, b) => b[1] - a[1]) as [cat, count]}
									<div class="flex items-center justify-between text-sm">
										<span class="text-muted-foreground">{cat}</span>
										<div class="flex items-center gap-2">
											<div class="w-24 h-1.5 bg-muted rounded-full overflow-hidden">
												<div class="bg-primary h-full rounded-full" style="width: {Math.min(100, count * 10)}%"></div>
											</div>
											<span class="text-muted-foreground text-xs w-6 text-right">{count}</span>
										</div>
									</div>
								{/each}
							</div>
						</div>
					{/if}

					{#if Object.keys(repo.risks).length > 0}
						<div class="rounded-lg bg-muted/40 p-4">
							<h4 class="text-sm font-semibold text-foreground/90 mb-3">PR Risk Distribution</h4>
							<div class="space-y-2">
								{#each Object.entries(repo.risks).sort((a, b) => b[1] - a[1]) as [risk, count]}
									<div class="flex items-center justify-between text-sm">
										<span class="text-muted-foreground">{risk}</span>
										<div class="flex items-center gap-2">
											<div class="w-24 h-1.5 bg-muted rounded-full overflow-hidden">
												<div class="{risk === 'high' ? 'bg-red-500' : risk === 'medium' ? 'bg-yellow-500' : 'bg-green-500'} h-full rounded-full" style="width: {Math.min(100, count * 20)}%"></div>
											</div>
											<span class="text-muted-foreground text-xs w-6 text-right">{count}</span>
										</div>
									</div>
								{/each}
							</div>
						</div>
					{/if}
				</div>
			</div>
		{/each}

		<div class="rounded-lg border bg-card p-5">
			<p class="text-sm text-muted-foreground">
				For a full HTML/PDF report, run: <code class="bg-muted/40 px-2 py-1 rounded text-xs">wshm report --format html</code>
			</p>
		</div>
	{:else if result}
		<div class="rounded-lg border bg-card p-10 text-center">
			<p class="text-muted-foreground">No repo data available. Sync your repos first.</p>
		</div>
	{:else}
		<div class="text-center py-10 text-muted-foreground">Loading...</div>
	{/if}
