<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { selectedRepo, theme, type Theme } from '$lib/stores';
	import { fetchStatus, type RepoInfo } from '$lib/api';
	import '../app.css';

	let { children }: { children: Snippet } = $props();

	let repos: RepoInfo[] = $state([]);
	let collapsed: boolean = $state(false);
	let currentTheme: Theme = $state('dark');
	theme.subscribe((t) => (currentTheme = t));

	function toggleTheme() {
		theme.set(currentTheme === 'dark' ? 'light' : 'dark');
	}

	type IconName =
		| 'dashboard' | 'summary' | 'issues' | 'prs' | 'triage' | 'queue'
		| 'changelog' | 'revert' | 'backups' | 'activity' | 'actions' | 'settings';

	const navItems: { href: string; label: string; icon: IconName }[] = [
		{ href: '/', label: 'Dashboard', icon: 'dashboard' },
		{ href: '/summary', label: 'Summary', icon: 'summary' },
		{ href: '/issues', label: 'Issues', icon: 'issues' },
		{ href: '/prs', label: 'Pull Requests', icon: 'prs' },
		{ href: '/triage', label: 'Triage', icon: 'triage' },
		{ href: '/queue', label: 'Merge Queue', icon: 'queue' },
		{ href: '/changelog', label: 'Changelog', icon: 'changelog' },
		{ href: '/revert', label: 'Revert', icon: 'revert' },
		{ href: '/backups', label: 'Backups', icon: 'backups' },
		{ href: '/activity', label: 'Activity', icon: 'activity' },
		{ href: '/actions', label: 'Actions', icon: 'actions' },
		{ href: '/settings', label: 'Settings', icon: 'settings' }
	];

	function handleRepoChange(event: Event) {
		const value = (event.target as HTMLSelectElement).value;
		selectedRepo.set(value === '' ? null : value);
	}

	function toggleCollapse() {
		collapsed = !collapsed;
		try { localStorage.setItem('wshm-sidebar-collapsed', String(collapsed)); } catch { /* ignore */ }
	}

	function handleLogout() {
		if (!window.confirm('Sign out?')) return;
		// HTTP Basic Auth has no real "logout" — the browser caches creds for
		// the origin until restart. Trick: send a sync XHR with deliberately
		// invalid credentials so the browser updates its cache (= forgets the
		// good ones). Then reload — the daemon answers 401 and the browser
		// re-prompts.
		try {
			const xhr = new XMLHttpRequest();
			xhr.open('GET', '/api/v1/status', false, 'logout', 'logout');
			xhr.send();
		} catch {
			// expected — browser may throw if blocked
		}
		window.location.replace('/');
	}

	onMount(async () => {
		try {
			const saved = localStorage.getItem('wshm-sidebar-collapsed');
			if (saved === 'true') collapsed = true;
		} catch { /* ignore */ }
		theme.update((t) => t);
		try {
			const status = await fetchStatus();
			repos = status.repos;
		} catch { /* ignore */ }
	});
</script>

<div class="bg-gray-900 text-gray-200 min-h-screen">
	<nav
		class="fixed top-0 left-0 bottom-0 z-40 flex flex-col border-r border-gray-700 bg-gray-800 overflow-y-auto transition-[width] duration-150"
		style="width: {collapsed ? '52px' : '180px'}"
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
					class="flex items-center gap-2.5 px-3 py-2 text-sm text-gray-400 hover:bg-gray-700 hover:text-gray-100 transition-colors {collapsed ? 'justify-center' : ''}"
					title={item.label}
				>
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4 flex-shrink-0" aria-hidden="true">
						{#if item.icon === 'dashboard'}
							<rect x="3" y="3" width="7" height="9" rx="1" />
							<rect x="14" y="3" width="7" height="5" rx="1" />
							<rect x="14" y="12" width="7" height="9" rx="1" />
							<rect x="3" y="16" width="7" height="5" rx="1" />
						{:else if item.icon === 'summary'}
							<path d="M3 3v18h18" />
							<path d="M7 14l4-4 4 4 5-7" />
						{:else if item.icon === 'issues'}
							<circle cx="12" cy="12" r="9" />
							<path d="M12 8v4M12 16h.01" />
						{:else if item.icon === 'prs'}
							<circle cx="6" cy="6" r="2" />
							<circle cx="6" cy="18" r="2" />
							<circle cx="18" cy="18" r="2" />
							<path d="M6 8v8" />
							<path d="M11 6h5a2 2 0 0 1 2 2v8" />
						{:else if item.icon === 'triage'}
							<path d="M3 6h18" />
							<path d="M6 12h12" />
							<path d="M10 18h4" />
						{:else if item.icon === 'queue'}
							<path d="M8 6h13M8 12h13M8 18h13" />
							<circle cx="3.5" cy="6" r="1.5" />
							<circle cx="3.5" cy="12" r="1.5" />
							<circle cx="3.5" cy="18" r="1.5" />
						{:else if item.icon === 'changelog'}
							<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
							<polyline points="14 2 14 8 20 8" />
							<path d="M8 13h8M8 17h5" />
						{:else if item.icon === 'revert'}
							<path d="M9 14L4 9l5-5" />
							<path d="M4 9h11a5 5 0 0 1 0 10h-3" />
						{:else if item.icon === 'backups'}
							<ellipse cx="12" cy="5" rx="9" ry="3" />
							<path d="M3 5v6c0 1.7 4 3 9 3s9-1.3 9-3V5" />
							<path d="M3 11v6c0 1.7 4 3 9 3s9-1.3 9-3v-6" />
						{:else if item.icon === 'activity'}
							<polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
						{:else if item.icon === 'actions'}
							<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
						{:else if item.icon === 'settings'}
							<circle cx="12" cy="12" r="3" />
							<path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 0 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 0 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 0 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 0 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
						{/if}
					</svg>
					{#if !collapsed}
						<span class="truncate">{item.label}</span>
					{/if}
				</a>
			{/each}
		</div>

		<div class="border-t border-gray-700 px-2 py-2 flex items-center gap-1 {collapsed ? 'flex-col' : 'justify-between'}">
			<button
				onclick={handleLogout}
				title="Sign out"
				class="rounded p-1.5 text-gray-400 hover:bg-gray-700 hover:text-gray-100 transition-colors"
				aria-label="Sign out"
			>
				<!-- Logout icon (door + arrow) -->
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
					<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
					<polyline points="16 17 21 12 16 7" />
					<line x1="21" y1="12" x2="9" y2="12" />
				</svg>
			</button>
			<button
				onclick={toggleTheme}
				title={currentTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
				class="rounded p-1.5 text-gray-400 hover:bg-gray-700 hover:text-gray-100 transition-colors"
				aria-label="Toggle theme"
			>
				{#if currentTheme === 'dark'}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
						<circle cx="12" cy="12" r="4" />
						<path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
					</svg>
				{:else}
					<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
						<path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
					</svg>
				{/if}
			</button>
			{#if !collapsed}
				<span class="text-[0.625rem] text-gray-500 mono">v0.28.3</span>
			{/if}
			<button
				onclick={toggleCollapse}
				title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
				class="rounded p-1.5 text-gray-400 hover:bg-gray-700 hover:text-gray-100 transition-colors"
				aria-label="Toggle sidebar"
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
					{#if collapsed}<polyline points="9 18 15 12 9 6" />{:else}<polyline points="15 18 9 12 15 6" />{/if}
				</svg>
			</button>
		</div>
	</nav>

	<main class="transition-[margin-left] duration-150 p-3 max-w-none" style="margin-left: {collapsed ? '52px' : '180px'}">
		{@render children()}
	</main>
</div>
