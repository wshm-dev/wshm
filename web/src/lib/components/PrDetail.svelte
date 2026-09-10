<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import type { PullRequest } from '$lib/api';
	import Markdown from './Markdown.svelte';

	let { pr }: { pr: PullRequest } = $props();

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	const badgeGreen = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const badgeRed = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
	const badgeYellow = 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
	const badgeGray = 'border-transparent bg-secondary text-secondary-foreground';

	function riskClass(risk: string | null): string {
		if (risk === 'high') return badgeRed;
		if (risk === 'medium') return badgeYellow;
		if (risk === 'low') return badgeGreen;
		return badgeGray;
	}

	// `pr.url` is built server-side from the configured forge — no
	// hardcoded github.com pattern so GitLab / Gitea / Forgejo /
	// Azure DevOps deploys all show a usable link.
	let prUrl = $derived(pr.url ?? null);
</script>

{#if prUrl}
	<div class="mb-3 flex items-center gap-2 text-xs">
		<a
			href={prUrl}
			target="_blank"
			rel="noopener noreferrer"
			class="inline-flex items-center gap-1.5 text-primary hover:text-primary/80 underline"
		>
			<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4" aria-hidden="true">
				<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
				<polyline points="15 3 21 3 21 9" />
				<line x1="10" y1="14" x2="21" y2="3" />
			</svg>
			<span class="truncate">{prUrl}</span>
		</a>
	</div>
{/if}

<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">State</div>
			<Badge variant="outline" class={pr.state === 'open' ? badgeGreen : badgeRed}>{pr.state}</Badge>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Risk</div>
			<Badge variant="outline" class={riskClass(pr.risk_level)}>{pr.risk_level ?? '-'}</Badge>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">CI Status</div>
			<span class="text-foreground">{pr.ci_status ?? '-'}</span>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Age</div>
			<span class="mono text-foreground">{ageDays(pr.created_at)}d</span>
		</Card.Content>
	</Card.Root>
</div>

<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Conflicts</div>
			<Badge variant="outline" class={pr.mergeable === false ? badgeRed : badgeGreen}>
				{pr.mergeable === false ? 'Yes' : 'No'}
			</Badge>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Branch</div>
			<span class="text-foreground/90 text-sm mono">{pr.head_ref ?? '-'} → {pr.base_ref ?? '-'}</span>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Author</div>
			<span class="text-foreground/90">{pr.author ?? '-'}</span>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Created</div>
			<span class="mono text-foreground/90">{pr.created_at?.slice(0, 10)}</span>
		</Card.Content>
	</Card.Root>
</div>

{#if pr.labels && pr.labels.length > 0}
	<Card.Root class="py-3 mb-4">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Labels</div>
			<div class="flex flex-wrap gap-1">
				{#each pr.labels as label}
					<Badge variant="outline" class="bg-primary/15 text-primary">{label}</Badge>
				{/each}
			</div>
		</Card.Content>
	</Card.Root>
{/if}

{#if pr.body}
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Description</div>
			<Markdown source={pr.body} />
		</Card.Content>
	</Card.Root>
{/if}
