<script lang="ts">
	import { Card, Badge } from 'flowbite-svelte';
	import type { Issue } from '$lib/api';
	import Markdown from './Markdown.svelte';

	let { issue }: { issue: Issue } = $props();

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	// `issue.url` is built server-side from the configured forge
	// (GitHub / GitLab / Gitea / Forgejo / Azure DevOps), so we never
	// need to guess the URL shape from `repo`. Older daemons that
	// didn't yet include the field — we just hide the link.
	let issueUrl = $derived(issue.url ?? null);
</script>

{#if issueUrl}
	<div class="mb-3 flex items-center gap-2 text-xs">
		<a
			href={issueUrl}
			target="_blank"
			rel="noopener noreferrer"
			class="inline-flex items-center gap-1.5 text-blue-400 hover:text-blue-300 underline"
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
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">State</div>
		<Badge color={issue.state === 'open' ? 'green' : 'red'}>{issue.state}</Badge>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Priority</div>
		<span class="text-gray-200">{issue.priority ?? '-'}</span>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Category</div>
		<span class="text-gray-200">{issue.category ?? '-'}</span>
	</Card>
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-1">Age</div>
		<span class="mono text-gray-200">{ageDays(issue.created_at)}d</span>
	</Card>
</div>

{#if issue.labels.length > 0}
	<Card class="bg-gray-800 p-3 mb-4 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-2">Labels</div>
		<div class="flex flex-wrap gap-1">
			{#each issue.labels as label}
				<Badge color="blue">{label}</Badge>
			{/each}
		</div>
	</Card>
{/if}

<Card class="bg-gray-800 p-3 mb-4 max-w-none">
	<div class="text-[0.625rem] uppercase text-gray-500 mb-2">Details</div>
	<div class="grid grid-cols-2 gap-2 text-sm">
		<div><span class="text-gray-500">Author:</span> <span class="text-gray-300">{issue.author ?? '-'}</span></div>
		<div><span class="text-gray-500">Created:</span> <span class="text-gray-300 mono">{issue.created_at?.slice(0, 10)}</span></div>
		<div><span class="text-gray-500">Updated:</span> <span class="text-gray-300 mono">{issue.updated_at?.slice(0, 10)}</span></div>
	</div>
</Card>

{#if issue.body}
	<Card class="bg-gray-800 p-3 max-w-none">
		<div class="text-[0.625rem] uppercase text-gray-500 mb-2">Body</div>
		<Markdown source={issue.body} />
	</Card>
{/if}
