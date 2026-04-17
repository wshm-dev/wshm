<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, type RepoInfo } from '$lib/api';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	let repos: RepoInfo[] = $state([]);
	let collapsed: boolean = $state(false);

	const navItems = [
		{ href: '/', label: 'Dashboard', icon: 'D' },
		{ href: '/summary', label: 'Summary', icon: 'M' },
		{ href: '/issues', label: 'Issues', icon: 'I' },
		{ href: '/prs', label: 'Pull Requests', icon: 'P' },
		{ href: '/triage', label: 'Triage', icon: 'T' },
		{ href: '/queue', label: 'Merge Queue', icon: 'Q' },
		{ href: '/changelog', label: 'Changelog', icon: 'L' },
		{ href: '/revert', label: 'Revert', icon: 'X' },
		{ href: '/backups', label: 'Backups', icon: 'B' },
		{ href: '/activity', label: 'Activity', icon: 'A' },
		{ href: '/actions', label: 'Actions', icon: '!' },
		{ href: '/settings', label: 'Settings', icon: 'S' }
	];

	function handleRepoChange(event: Event) {
		const value = (event.target as HTMLSelectElement).value;
		selectedRepo.set(value === '' ? null : value);
	}

	function toggleCollapse() {
		collapsed = !collapsed;
		try {
			localStorage.setItem('wshm-sidebar-collapsed', String(collapsed));
		} catch {
			// ignore
		}
	}

	onMount(async () => {
		try {
			const saved = localStorage.getItem('wshm-sidebar-collapsed');
			if (saved === 'true') collapsed = true;
		} catch {
			// ignore
		}
		try {
			const status = await fetchStatus();
			repos = status.repos;
		} catch {
			// silently ignore
		}
	});
</script>

<div class="dark bg-gray-900 text-gray-200 min-h-screen">
	<nav
		class="fixed top-0 left-0 bottom-0 z-40 flex flex-col border-r border-gray-700 bg-gray-800 overflow-y-auto transition-[width] duration-150"
		style="width: {collapsed ? '48px' : '200px'}"
	>
		<div class="flex items-center gap-2 px-3 py-3 border-b border-gray-700">
			<img src="/wizard-icon.png" alt="wshm" class="h-7 w-7 flex-shrink-0" />
			{#if !collapsed}
				<span class="text-base font-bold text-gray-100 truncate">wshm</span>
			{/if}
		</div>

		{#if !collapsed}
			<div class="px-3 py-2 border-b border-gray-700">
				<select
					onchange={handleRepoChange}
					class="w-full rounded border border-gray-600 bg-gray-900 px-1.5 py-1 text-xs text-gray-300 focus:border-blue-500 focus:outline-none"
				>
					<option value="">All repos</option>
					{#each repos as r}
						<option value={r.slug}>{r.slug}</option>
					{/each}
				</select>
			</div>
		{/if}

		<div class="flex-1 py-1">
			{#each navItems as item}
				<a
					href={item.href}
					class="flex items-center gap-2 px-3 py-1.5 text-sm text-gray-400 hover:bg-gray-700 hover:text-gray-100 transition-colors {collapsed ? 'justify-center' : ''}"
					title={item.label}
				>
					<span class="font-mono text-xs font-bold text-gray-500 w-5 text-center">{item.icon}</span>
					{#if !collapsed}
						<span class="truncate">{item.label}</span>
					{/if}
				</a>
			{/each}
		</div>

		<div class="border-t border-gray-700 px-3 py-2 flex items-center {collapsed ? 'justify-center' : 'justify-between'}">
			{#if !collapsed}
				<span class="text-[0.625rem] text-gray-600">v0.26.0</span>
			{/if}
			<button
				onclick={toggleCollapse}
				title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
				class="text-gray-500 hover:text-gray-200 text-xs"
			>
				{collapsed ? '>>' : '<<'}
			</button>
		</div>
	</nav>

	<main
		class="transition-[margin-left] duration-150 p-4"
		style="margin-left: {collapsed ? '48px' : '200px'}"
	>
		{@render children()}
	</main>
</div>
