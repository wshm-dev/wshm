<script lang="ts">
	import { Card, Badge } from 'flowbite-svelte';
	import type { PullRequest } from '$lib/api';
	import Markdown from './Markdown.svelte';

	let { pr }: { pr: PullRequest } = $props();

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	function riskColor(risk: string | null): 'red' | 'yellow' | 'green' {
		if (risk === 'high') return 'red';
		if (risk === 'medium') return 'yellow';
		return 'green';
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
			class="inline-flex items-center gap-1.5 text-blue-400 hover:text-blue-300 underline"
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
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">State</div>
		<Badge color={pr.state === 'open' ? 'green' : 'red'}>{pr.state}</Badge>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Risk</div>
		<Badge color={riskColor(pr.risk_level)}>{pr.risk_level ?? '-'}</Badge>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">CI Status</div>
		<span class="text-gray-200">{pr.ci_status ?? '-'}</span>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Age</div>
		<span class="mono text-gray-200">{ageDays(pr.created_at)}d</span>
	</Card>
</div>

<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Conflicts</div>
		<Badge color={pr.mergeable === false ? 'red' : 'green'}>
			{pr.mergeable === false ? 'Yes' : 'No'}
		</Badge>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Branch</div>
		<span class="text-gray-300 text-sm mono">{pr.head_ref ?? '-'} → {pr.base_ref ?? '-'}</span>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Author</div>
		<span class="text-gray-300">{pr.author ?? '-'}</span>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Created</div>
		<span class="mono text-gray-300">{pr.created_at?.slice(0, 10)}</span>
	</Card>
</div>

{#if pr.labels && pr.labels.length > 0}
	<Card class="bg-gray-800 p-3 mb-4 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-2">Labels</div>
		<div class="flex flex-wrap gap-1">
			{#each pr.labels as label}
				<Badge color="blue">{label}</Badge>
			{/each}
		</div>
	</Card>
{/if}

{#if pr.body}
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-2">Description</div>
		<Markdown source={pr.body} />
	</Card>
{/if}
