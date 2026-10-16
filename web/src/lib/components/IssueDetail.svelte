<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import { fetchIssueRepro, type Issue, type ReproRun } from '$lib/api';
	import Markdown from './Markdown.svelte';

	let { issue }: { issue: Issue } = $props();

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	const badgeGreen = 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
	const badgeRed = 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';

	// `issue.url` is built server-side from the configured forge
	// (GitHub / GitLab / Gitea / Forgejo / Azure DevOps), so we never
	// need to guess the URL shape from `repo`. Older daemons that
	// didn't yet include the field — we just hide the link.
	let issueUrl = $derived(issue.url ?? null);

	// Latest automatic reproduction (#229). Best effort: older daemons 404.
	let repro: ReproRun | null = $state(null);
	$effect(() => {
		const slug = issue.repo;
		const number = issue.number;
		repro = null;
		if (!slug) return;
		fetchIssueRepro(slug, number)
			.then((r) => (repro = r.run))
			.catch(() => (repro = null));
	});

	const reproStyle: Record<string, string> = {
		confirmed: badgeRed,
		'not-reproduced': badgeGreen,
		'needs-info': 'border-yellow-500/30 bg-yellow-500/15 text-yellow-700 dark:text-yellow-400',
		error: 'border-muted bg-muted text-muted-foreground',
		pending: 'border-blue-500/30 bg-blue-500/15 text-blue-600 dark:text-blue-400'
	};
</script>

{#if issueUrl}
	<div class="mb-3 flex items-center gap-2 text-xs">
		<a
			href={issueUrl}
			target="_blank"
			rel="noopener noreferrer"
			class="inline-flex items-center gap-1.5 text-primary hover:text-primary/80 underline"
		>
			<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4" aria-hidden="true">
				<path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
				<polyline points="15 3 21 3 21 9" />
				<line x1="10" y1="14" x2="21" y2="3" />
			</svg>
			<span class="truncate">{issueUrl}</span>
		</a>
	</div>
{/if}

<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">State</div>
			<Badge variant="outline" class={issue.state === 'open' ? badgeGreen : badgeRed}>{issue.state}</Badge>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Priority</div>
			<span class="text-foreground">{issue.priority ?? '-'}</span>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Category</div>
			<span class="text-foreground">{issue.category ?? '-'}</span>
		</Card.Content>
	</Card.Root>
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-1">Age</div>
			<span class="mono text-foreground">{ageDays(issue.created_at)}d</span>
		</Card.Content>
	</Card.Root>
</div>

{#if issue.labels.length > 0}
	<Card.Root class="py-3 mb-4">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Labels</div>
			<div class="flex flex-wrap gap-1">
				{#each issue.labels as label}
					<Badge variant="outline" class="bg-primary/15 text-primary">{label}</Badge>
				{/each}
			</div>
		</Card.Content>
	</Card.Root>
{/if}

<Card.Root class="py-3 mb-4">
	<Card.Content class="px-3">
		<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Details</div>
		<div class="grid grid-cols-2 gap-2 text-sm">
			<div><span class="text-muted-foreground">Author:</span> <span class="text-foreground/90">{issue.author ?? '-'}</span></div>
			<div><span class="text-muted-foreground">Created:</span> <span class="text-foreground/90 mono">{issue.created_at?.slice(0, 10)}</span></div>
			<div><span class="text-muted-foreground">Updated:</span> <span class="text-foreground/90 mono">{issue.updated_at?.slice(0, 10)}</span></div>
		</div>
	</Card.Content>
</Card.Root>

{#if repro}
	<Card.Root class="py-3 mb-4">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Reproduction</div>
			<div class="flex flex-wrap items-center gap-2 text-sm mb-2">
				<Badge variant="outline" class={reproStyle[repro.outcome] ?? ''}>{repro.outcome}</Badge>
				<span class="text-foreground/90">{repro.reason}</span>
			</div>
			<div class="grid grid-cols-1 md:grid-cols-2 gap-2 text-xs text-muted-foreground mb-2">
				<div>Run: <span class="mono">{repro.created_at}</span></div>
				{#if repro.default_branch}
					<div>On: <span class="mono">{repro.default_branch} @ {repro.head_sha?.slice(0, 12)}</span></div>
				{/if}
				{#if repro.exit_code !== null}<div>Exit code: <span class="mono">{repro.exit_code}</span></div>{/if}
				<div>Comment posted: {repro.comment_posted ? 'yes' : 'no'}{#if repro.test_pr} · test PR #{repro.test_pr}{/if}</div>
			</div>
			{#if repro.missing_info.length > 0}
				<div class="text-sm mb-2">Missing information:</div>
				<ul class="list-disc ml-5 text-sm mb-2">
					{#each repro.missing_info as m}<li>{m}</li>{/each}
				</ul>
			{/if}
			{#if repro.command}
				<div class="text-xs text-muted-foreground">Command</div>
				<pre class="mono text-xs bg-muted/50 rounded p-2 mb-2 overflow-x-auto whitespace-pre-wrap">{repro.command}</pre>
			{/if}
			{#each repro.files as f}
				<details class="mb-2">
					<summary class="text-xs cursor-pointer"><span class="mono">{f.path}</span></summary>
					<pre class="mono text-xs bg-muted/50 rounded p-2 mt-1 overflow-x-auto">{f.content}</pre>
				</details>
			{/each}
			{#if repro.output}
				<details>
					<summary class="text-xs cursor-pointer">Output</summary>
					<pre class="mono text-xs bg-muted/50 rounded p-2 mt-1 overflow-x-auto max-h-96">{repro.output}</pre>
				</details>
			{/if}
		</Card.Content>
	</Card.Root>
{/if}

{#if issue.body}
	<Card.Root class="py-3">
		<Card.Content class="px-3">
			<div class="text-[0.625rem] uppercase text-muted-foreground mb-2">Body</div>
			<Markdown source={issue.body} />
		</Card.Content>
	</Card.Root>
{/if}
