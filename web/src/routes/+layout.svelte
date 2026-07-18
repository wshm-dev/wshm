<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { selectedRepo, theme, type Theme } from '$lib/stores';
	import {
		fetchStatus,
		fetchMe,
		fetchAuthStatus,
		fetchLicense,
		syncIncremental,
		syncFull,
		type RepoInfo,
		type Me,
		type AuthStatus,
		type LicenseInfo
	} from '$lib/api';
	import { canAccessRoute, can } from '$lib/permissions';
	import {
		SidebarGroup,
		SidebarItem,
		Avatar,
		Alert,
		Button,
		Select,
		ButtonGroup
	} from 'flowbite-svelte';
	import '../app.css';

	let { children }: { children: Snippet } = $props();
	let isLoginRoute = $derived($page.url.pathname === '/login');
	let activeUrl = $derived($page.url.pathname);

	let repos: RepoInfo[] = $state([]);
	let collapsed: boolean = $state(false);
	let currentTheme: Theme = $state('dark');
	let me: Me | null = $state(null);
	let authStatus: AuthStatus | null = $state(null);
	let license: LicenseInfo | null = $state(null);
	let bannerOpen: boolean = $state(true);
	theme.subscribe((t) => (currentTheme = t));

	function meLabel(m: Me): string {
		return m.email ?? m.username ?? 'signed in';
	}
	function meInitial(m: Me): string {
		const s = m.email ?? m.username ?? '?';
		return s.charAt(0).toUpperCase();
	}

	function toggleTheme() {
		theme.set(currentTheme === 'dark' ? 'light' : 'dark');
	}

	type IconName =
		| 'dashboard' | 'summary' | 'issues' | 'prs' | 'review' | 'triage' | 'queue'
		| 'changelog' | 'revert' | 'backups' | 'activity' | 'actions' | 'logs'
		| 'search' | 'settings' | 'insights' | 'issueInsights';

	type NavSection = 'Overview' | 'Work' | 'Insights' | 'System';

	type NavItem = {
		href: string;
		label: string;
		icon: IconName;
		/** Sidebar section header this item is grouped under. */
		section: NavSection;
		/** When set, hide the item unless `license.features[id].enabled === true`. */
		feature?: string;
	};

	const allNavItems: NavItem[] = [
		{ href: '/', label: 'Dashboard', icon: 'dashboard', section: 'Overview' },
		{ href: '/summary', label: 'Summary', icon: 'summary', section: 'Overview' },
		{ href: '/search', label: 'Search', icon: 'search', section: 'Work', feature: 'search' },
		{ href: '/issues', label: 'Issues', icon: 'issues', section: 'Work' },
		{ href: '/prs', label: 'Pull Requests', icon: 'prs', section: 'Work' },
		{ href: '/review', label: 'To Validate', icon: 'review', section: 'Work' },
		{ href: '/triage', label: 'Triage', icon: 'triage', section: 'Work' },
		{ href: '/queue', label: 'Merge Queue', icon: 'queue', section: 'Work' },
		{ href: '/actions', label: 'Actions', icon: 'actions', section: 'Work' },
		{
			href: '/pr-insights',
			label: 'PR Insights',
			icon: 'insights',
			section: 'Insights',
			feature: 'pr-insights'
		},
		{
			href: '/issue-insights',
			label: 'Issue Insights',
			icon: 'issueInsights',
			section: 'Insights',
			feature: 'issue-insights'
		},
		{ href: '/usage', label: 'Usage', icon: 'activity', section: 'Insights', feature: 'usage-dashboard' },
		{ href: '/changelog', label: 'Changelog', icon: 'changelog', section: 'Insights' },
		{ href: '/activity', label: 'Activity', icon: 'activity', section: 'Insights' },
		{ href: '/logs', label: 'Logs', icon: 'logs', section: 'System' },
		{ href: '/revert', label: 'Revert', icon: 'revert', section: 'System' },
		{ href: '/backups', label: 'Backups', icon: 'backups', section: 'System' },
		{ href: '/settings', label: 'Settings', icon: 'settings', section: 'System' }
	];
	function isFeatureLicensed(featureId: string | undefined): boolean {
		if (!featureId) return true;
		const f = license?.features?.find((x) => x.id === featureId);
		return f?.enabled === true;
	}
	let navItems = $derived(
		allNavItems
			.filter((i) => canAccessRoute(me?.role, i.href))
			.filter((i) => isFeatureLicensed(i.feature))
	);
	const sectionOrder: NavSection[] = ['Overview', 'Work', 'Insights', 'System'];
	let navSections = $derived(
		sectionOrder
			.map((name) => ({ name, items: navItems.filter((i) => i.section === name) }))
			.filter((s) => s.items.length > 0)
	);

	// Per-section fold state (persisted). System starts folded to keep the
	// menu focused on daily work; a section containing the ACTIVE route is
	// always shown expanded so the current page never disappears from the
	// sidebar (it re-folds when you navigate away).
	const NAV_FOLD_KEY = 'wshm-nav-folded-sections';
	let foldedSections: Record<string, boolean> = $state({ System: true });
	function loadFoldedSections() {
		try {
			const raw = localStorage.getItem(NAV_FOLD_KEY);
			if (raw) foldedSections = { System: true, ...JSON.parse(raw) };
		} catch { /* ignore */ }
	}
	function toggleSection(name: string) {
		foldedSections = { ...foldedSections, [name]: !foldedSections[name] };
		try { localStorage.setItem(NAV_FOLD_KEY, JSON.stringify(foldedSections)); } catch { /* ignore */ }
	}
	function sectionOpen(name: string, items: NavItem[]): boolean {
		if (!foldedSections[name]) return true;
		return items.some((i) => i.href === activeUrl);
	}

	function toggleCollapse() {
		collapsed = !collapsed;
		try { localStorage.setItem('wshm-sidebar-collapsed', String(collapsed)); } catch { /* ignore */ }
	}

	async function handleLogout() {
		try {
			await fetch('/api/v1/auth/logout', { method: 'POST' });
		} catch { /* ignore */ }
		try {
			const xhr = new XMLHttpRequest();
			xhr.open('GET', '/api/v1/status', false, 'logout', 'logout');
			xhr.send();
		} catch { /* ignore */ }
		window.location.replace('/login');
	}

	onMount(async () => {
		try {
			const saved = localStorage.getItem('wshm-sidebar-collapsed');
			if (saved === 'true') collapsed = true;
		} catch { /* ignore */ }
		loadFoldedSections();
		theme.update((t) => t);
		try {
			const status = await fetchStatus();
			repos = status.repos;
		} catch { /* ignore */ }
		try {
			me = await fetchMe();
		} catch { /* ignore */ }
		try {
			authStatus = await fetchAuthStatus();
		} catch { /* ignore */ }
		try {
			license = await fetchLicense();
		} catch { /* ignore */ }
		try {
			bannerOpen = localStorage.getItem('wshm-anon-banner-dismissed') !== 'true';
		} catch { /* ignore */ }
	});

	function persistBannerDismiss() {
		try { localStorage.setItem('wshm-anon-banner-dismissed', 'true'); } catch { /* ignore */ }
	}

	let syncing = $state(false);
	let syncMsg: string | null = $state(null);

	async function runSync(full: boolean) {
		if (syncing) return;
		syncing = true;
		syncMsg = full ? 'Full sync...' : 'Sync...';
		try {
			const r = full ? await syncFull() : await syncIncremental();
			const ok = r.errors?.length === 0;
			syncMsg = ok ? `Synced ${r.synced.length} repo(s)` : `Partial: ${r.errors?.length} error(s)`;
		} catch (e) {
			syncMsg = e instanceof Error ? e.message : 'Sync failed';
		}
		syncing = false;
		setTimeout(() => { if (syncMsg) syncMsg = null; }, 4000);
	}

	let repoOptions = $derived([
		{ value: '', name: 'All repos' },
		...repos.map((r) => ({ value: r.slug, name: r.slug }))
	]);
	let selectedRepoValue: string = $state('');
	selectedRepo.subscribe((v) => (selectedRepoValue = v ?? ''));
	$effect(() => {
		selectedRepo.set(selectedRepoValue === '' ? null : selectedRepoValue);
	});
</script>

{#if isLoginRoute}
	{@render children()}
{:else}
<div class="bg-gray-900 text-gray-200 min-h-screen">
	<aside
		aria-label="Main navigation"
		class="fixed top-0 left-0 bottom-0 z-40 border-r border-gray-700 bg-gray-800 overflow-y-auto transition-[width] duration-150 flex flex-col"
		style="width: {collapsed ? '52px' : '180px'}"
	>
		<div class="flex items-center gap-2 px-3 py-3 border-b border-gray-700">
			<!-- Dark backdrop keeps the (dark) logo art visible on the light theme. -->
			<img src="/wizard-icon.png" alt="wshm" class="h-7 w-7 flex-shrink-0 rounded-md bg-gray-900 p-0.5" />
			{#if !collapsed}
				<span class="text-base font-bold text-gray-100 truncate">wshm</span>
			{/if}
		</div>

		{#if !collapsed}
			<div class="px-3 py-2 border-b border-gray-700 space-y-1.5">
				<!-- Empty placeholder: otherwise Flowbite prepends its own
				     "Choose option ..." entry which wins over the value=''
				     "All repos" item and makes the active repo unreadable. -->
				<Select
					bind:value={selectedRepoValue}
					items={repoOptions}
					placeholder=""
					size="sm"
					class="bg-gray-900 border-gray-600 text-gray-300 text-xs"
				/>
				<ButtonGroup class="w-full">
					<Button
						color="alternative"
						size="xs"
						class="flex-1"
						disabled={syncing || !can(me?.role, 'syncIncremental')}
						onclick={() => runSync(false)}
						title={can(me?.role, 'syncIncremental')
							? 'Incremental sync (changes since last sync)'
							: 'Requires member role'}
					>{syncing ? '…' : 'Sync'}</Button>
					<Button
						color="alternative"
						size="xs"
						class="flex-1"
						disabled={syncing || !can(me?.role, 'syncFull')}
						onclick={() => runSync(true)}
						title={can(me?.role, 'syncFull')
							? 'Full re-sync (slower)'
							: 'Requires operator role'}
					>Full</Button>
				</ButtonGroup>
				{#if syncMsg}
					<div class="text-[0.65rem] text-gray-500 truncate" title={syncMsg}>{syncMsg}</div>
				{/if}
			</div>
		{/if}

		<div class="flex-1 py-1">
			{#each navSections as section (section.name)}
				{@const open = sectionOpen(section.name, section.items)}
				{#if !collapsed}
					<button
						type="button"
						class="w-full flex items-center justify-between px-3 pt-3 pb-1 text-[0.625rem] font-semibold uppercase tracking-wider text-gray-600 hover:text-gray-400 select-none"
						aria-expanded={open}
						onclick={() => toggleSection(section.name)}
					>
						{section.name}
						<svg class="h-2.5 w-2.5 transition-transform {open ? 'rotate-90' : ''}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
							<path d="m9 6 6 6-6 6" />
						</svg>
					</button>
				{:else}
					<div class="mx-3 my-2 border-t border-gray-700" aria-hidden="true"></div>
				{/if}
			{#if open || collapsed}
			<SidebarGroup class="space-y-0">
				{#each section.items as item}
					<SidebarItem
						href={item.href}
						label={item.label}
						active={activeUrl === item.href}
						spanClass={collapsed ? 'sr-only' : 'ms-3 truncate'}
						class={collapsed ? 'justify-center' : ''}
						aClass="flex items-center gap-2.5 px-3 py-2 text-sm rounded-none"
						activeClass="flex items-center gap-2.5 px-3 py-2 text-sm rounded-none bg-gray-700 text-gray-100"
						nonActiveClass="flex items-center gap-2.5 px-3 py-2 text-sm rounded-none text-gray-400 hover:bg-gray-700 hover:text-gray-100"
						title={item.label}
					>
						{#snippet icon()}
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
								{:else if item.icon === 'review'}
									<path d="M9 12l2 2 4-4" />
									<circle cx="12" cy="12" r="9" />
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
								{:else if item.icon === 'logs'}
									<path d="M4 4h16v4H4z" />
									<path d="M4 12h16v4H4z" />
									<path d="M4 20h10" />
								{:else if item.icon === 'insights'}
									<circle cx="12" cy="12" r="9" />
									<path d="M12 3v9l6.5 6.5" />
								{:else if item.icon === 'issueInsights'}
									<path d="M4 20V10" />
									<path d="M12 20V4" />
									<path d="M20 20v-7" />
								{:else if item.icon === 'search'}
									<circle cx="11" cy="11" r="7" />
									<path d="M21 21l-4.3-4.3" />
								{:else if item.icon === 'settings'}
									<circle cx="12" cy="12" r="3" />
									<path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 0 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 0 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 0 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 0 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
								{/if}
							</svg>
						{/snippet}
					</SidebarItem>
				{/each}
			</SidebarGroup>
			{/if}
			{/each}
		</div>

		{#if me}
			<div
				class="border-t border-gray-700 px-2 py-2 flex items-center gap-2 {collapsed ? 'justify-center' : ''}"
				title={meLabel(me)}
			>
				<Avatar size="sm" class="bg-blue-600 text-xs font-semibold text-white">
					{meInitial(me)}
				</Avatar>
				{#if !collapsed}
					<div class="min-w-0 flex-1">
						<div class="truncate text-xs text-gray-200">{meLabel(me)}</div>
						<div class="text-[0.625rem] uppercase tracking-wider text-gray-500">
							{me.auth_method === 'sso' ? 'SSO' : 'local'}
						</div>
					</div>
				{/if}
			</div>
		{/if}

		<div
			class="border-t border-gray-700 px-2 py-2 flex items-center gap-1 {collapsed ? 'flex-col' : 'justify-between'}"
		>
			<Button
				color="alternative"
				size="xs"
				class="!p-1.5 border-0 bg-transparent text-gray-400 hover:bg-gray-700 hover:text-gray-100"
				onclick={handleLogout}
				title="Sign out"
				aria-label="Sign out"
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
					<path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
					<polyline points="16 17 21 12 16 7" />
					<line x1="21" y1="12" x2="9" y2="12" />
				</svg>
			</Button>
			<Button
				color="alternative"
				size="xs"
				class="!p-1.5 border-0 bg-transparent text-gray-400 hover:bg-gray-700 hover:text-gray-100"
				onclick={toggleTheme}
				title={currentTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
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
			</Button>
			{#if !collapsed}
				<span
					class="text-[0.625rem] text-gray-500 mono"
					title={license?.is_pro ? 'wshm-pro' : 'wshm OSS'}
				>v{license?.version ?? '…'}</span>
			{/if}
			<Button
				color="alternative"
				size="xs"
				class="!p-1.5 border-0 bg-transparent text-gray-400 hover:bg-gray-700 hover:text-gray-100"
				onclick={toggleCollapse}
				title={collapsed ? 'Expand sidebar' : 'Collapse sidebar'}
				aria-label="Toggle sidebar"
			>
				<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" class="h-4 w-4">
					{#if collapsed}<polyline points="9 18 15 12 9 6" />{:else}<polyline points="15 18 9 12 15 6" />{/if}
				</svg>
			</Button>
		</div>
	</aside>

	<main class="transition-[margin-left] duration-150 p-3 max-w-none" style="margin-left: {collapsed ? '52px' : '180px'}">
		{#if authStatus && !authStatus.github && bannerOpen}
			<Alert
				color="yellow"
				dismissable
				bind:alertStatus={bannerOpen}
				onclose={persistBannerDismiss}
				class="mb-3 text-sm !border !bg-yellow-50 !text-yellow-900 !border-yellow-300 dark:!bg-yellow-900/20 dark:!text-yellow-100 dark:!border-yellow-700/50"
			>
				<span class="font-semibold">Anonymous GitHub mode.</span>
				Public repos sync read-only with a 60 req/h limit; labels, comments, and auto-fix actions are skipped.
				<a href="/settings" class="ml-1 underline">Add a github_token in Settings → Secrets</a>
				for full functionality.
			</Alert>
		{/if}
		{@render children()}
	</main>
</div>
{/if}
