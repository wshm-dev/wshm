<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, type RepoInfo } from '$lib/api';
	import { Sidebar, SidebarWrapper, SidebarGroup, SidebarItem } from 'flowbite-svelte';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	let repos: RepoInfo[] = $state([]);
	let collapsed: boolean = $state(false);

	const navItems = [
		{ href: '/', label: 'Dashboard', icon: 'D' },
		{ href: '/issues', label: 'Issues', icon: 'I' },
		{ href: '/prs', label: 'Pull Requests', icon: 'P' },
		{ href: '/triage', label: 'Triage', icon: 'T' },
		{ href: '/queue', label: 'Merge Queue', icon: 'Q' },
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

<div class="dark">
	<div class="flex min-h-screen bg-gray-900">
		<aside
			class="fixed top-0 left-0 bottom-0 z-40 flex-shrink-0 border-r border-gray-700 bg-gray-800 overflow-y-auto transition-[width] duration-150 ease-in-out"
			style="width: {collapsed ? '56px' : '220px'}"
		>
			<div class="flex justify-end p-2">
				<button
					onclick={toggleCollapse}
					title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
					class="rounded border border-gray-600 px-1.5 py-0.5 text-xs text-gray-400 hover:border-blue-500 hover:text-gray-200 mono"
				>
					{collapsed ? '>>' : '<<'}
				</button>
			</div>

			<div class="flex items-center gap-2 border-b border-gray-700 px-4 pb-4">
				<img src="/wizard-icon.png" alt="wshm" class="h-8 w-8 flex-shrink-0" />
				{#if !collapsed}
					<div>
						<h1 class="text-xl font-bold text-gray-100 tracking-tight leading-tight">wshm</h1>
						<span class="text-xs text-gray-500">wishmaster</span>
					</div>
				{/if}
			</div>

			{#if !collapsed}
				<div class="border-b border-gray-700 px-4 py-3">
					<label for="repo-select" class="mb-1 block text-[0.6875rem] uppercase tracking-wider text-gray-500">Repository</label>
					<select
						id="repo-select"
						onchange={handleRepoChange}
						class="w-full rounded-md border border-gray-600 bg-gray-900 px-2 py-1.5 text-sm text-gray-300 focus:border-blue-500 focus:outline-none"
					>
						<option value="">All repos</option>
						{#each repos as r}
							<option value={r.slug}>{r.slug}</option>
						{/each}
					</select>
				</div>
			{/if}

			<Sidebar class="w-full">
				<SidebarWrapper class="bg-transparent px-0 py-2">
					<SidebarGroup>
						{#each navItems as item}
							<SidebarItem href={item.href} label={collapsed ? '' : item.label} class="text-gray-400 hover:bg-gray-700 hover:text-gray-100 rounded-none px-4 py-2 text-sm {collapsed ? 'justify-center' : ''}">
								{#snippet icon()}
									{#if collapsed}
										<span class="mono text-xs font-bold">[{item.icon}]</span>
									{/if}
								{/snippet}
							</SidebarItem>
						{/each}
					</SidebarGroup>
				</SidebarWrapper>
			</Sidebar>
		</aside>

		<main
			class="flex-1 p-8 transition-[margin-left] duration-150 ease-in-out"
			style="margin-left: {collapsed ? '56px' : '220px'}"
		>
			{@render children()}
		</main>
	</div>
</div>
