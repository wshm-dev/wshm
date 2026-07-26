<script lang="ts">
	import type { Snippet } from 'svelte';
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import { selectedRepo, theme, collapseSidebarSignal, type Theme } from '$lib/stores';
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
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';
	import * as Alert from '$lib/components/ui/alert';
	import * as Avatar from '$lib/components/ui/avatar';
	import * as Sidebar from '$lib/components/ui/sidebar';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import XIcon from '@lucide/svelte/icons/x';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import LogOutIcon from '@lucide/svelte/icons/log-out';
	import SunIcon from '@lucide/svelte/icons/sun';
	import MoonIcon from '@lucide/svelte/icons/moon';
	import '../app.css';

	let { children }: { children: Snippet } = $props();
	let isLoginRoute = $derived($page.url.pathname === '/login');
	let activeUrl = $derived($page.url.pathname);

	let repos: RepoInfo[] = $state([]);
	let sidebarOpen: boolean = $state(true);
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
		| 'dashboard' | 'summary' | 'issues' | 'prs' | 'prGraph' | 'review' | 'triage' | 'queue'
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
		{ href: '/graphs', label: 'Graphs', icon: 'prGraph', section: 'Insights' },
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
	function setSectionOpen(name: string, open: boolean) {
		foldedSections = { ...foldedSections, [name]: !open };
		try { localStorage.setItem(NAV_FOLD_KEY, JSON.stringify(foldedSections)); } catch { /* ignore */ }
	}
	function sectionOpen(name: string, items: NavItem[]): boolean {
		if (!foldedSections[name]) return true;
		return items.some((i) => i.href === activeUrl);
	}

	function persistSidebarOpen(open: boolean) {
		try { localStorage.setItem('wshm-sidebar-collapsed', String(!open)); } catch { /* ignore */ }
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
			if (saved === 'true') sidebarOpen = false;
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
	// Auto-collapse the sidebar when a content page requests it (e.g. the graph
	// on interaction). Only reacts to real bumps (>0), so it never fires on load.
	let lastCollapseSignal = 0;
	$effect(() => {
		if ($collapseSidebarSignal > lastCollapseSignal) {
			lastCollapseSignal = $collapseSidebarSignal;
			sidebarOpen = false;
		}
	});

	let selectedRepoValue: string = $state('');
	selectedRepo.subscribe((v) => (selectedRepoValue = v ?? ''));
	$effect(() => {
		selectedRepo.set(selectedRepoValue === '' ? null : selectedRepoValue);
	});
</script>

{#snippet navIcon(icon: IconName)}
	<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
		{#if icon === 'dashboard'}
			<rect x="3" y="3" width="7" height="9" rx="1" />
			<rect x="14" y="3" width="7" height="5" rx="1" />
			<rect x="14" y="12" width="7" height="9" rx="1" />
			<rect x="3" y="16" width="7" height="5" rx="1" />
		{:else if icon === 'summary'}
			<path d="M3 3v18h18" />
			<path d="M7 14l4-4 4 4 5-7" />
		{:else if icon === 'issues'}
			<circle cx="12" cy="12" r="9" />
			<path d="M12 8v4M12 16h.01" />
		{:else if icon === 'prs'}
			<circle cx="6" cy="6" r="2" />
			<circle cx="6" cy="18" r="2" />
			<circle cx="18" cy="18" r="2" />
			<path d="M6 8v8" />
			<path d="M11 6h5a2 2 0 0 1 2 2v8" />
		{:else if icon === 'review'}
			<path d="M9 12l2 2 4-4" />
			<circle cx="12" cy="12" r="9" />
		{:else if icon === 'triage'}
			<path d="M3 6h18" />
			<path d="M6 12h12" />
			<path d="M10 18h4" />
		{:else if icon === 'queue'}
			<path d="M8 6h13M8 12h13M8 18h13" />
			<circle cx="3.5" cy="6" r="1.5" />
			<circle cx="3.5" cy="12" r="1.5" />
			<circle cx="3.5" cy="18" r="1.5" />
		{:else if icon === 'changelog'}
			<path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
			<polyline points="14 2 14 8 20 8" />
			<path d="M8 13h8M8 17h5" />
		{:else if icon === 'revert'}
			<path d="M9 14L4 9l5-5" />
			<path d="M4 9h11a5 5 0 0 1 0 10h-3" />
		{:else if icon === 'backups'}
			<ellipse cx="12" cy="5" rx="9" ry="3" />
			<path d="M3 5v6c0 1.7 4 3 9 3s9-1.3 9-3V5" />
			<path d="M3 11v6c0 1.7 4 3 9 3s9-1.3 9-3v-6" />
		{:else if icon === 'activity'}
			<polyline points="22 12 18 12 15 21 9 3 6 12 2 12" />
		{:else if icon === 'actions'}
			<polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
		{:else if icon === 'logs'}
			<path d="M4 4h16v4H4z" />
			<path d="M4 12h16v4H4z" />
			<path d="M4 20h10" />
		{:else if icon === 'prGraph'}
			<circle cx="5" cy="6" r="2.5" />
			<circle cx="19" cy="7" r="2.5" />
			<circle cx="12" cy="18" r="2.5" />
			<path d="M7.2 7.2 10 16M16.9 8.4 13.4 16.4M7.3 6.4h9.4" />
		{:else if icon === 'insights'}
			<circle cx="12" cy="12" r="9" />
			<path d="M12 3v9l6.5 6.5" />
		{:else if icon === 'issueInsights'}
			<path d="M4 20V10" />
			<path d="M12 20V4" />
			<path d="M20 20v-7" />
		{:else if icon === 'search'}
			<circle cx="11" cy="11" r="7" />
			<path d="M21 21l-4.3-4.3" />
		{:else if icon === 'settings'}
			<circle cx="12" cy="12" r="3" />
			<path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 0 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 0 1-4 0v-.1a1.7 1.7 0 0 0-1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 0 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 0 1 0-4h.1a1.7 1.7 0 0 0 1.5-1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 0 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3h.1a1.7 1.7 0 0 0 1-1.5V3a2 2 0 0 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 0 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8v.1a1.7 1.7 0 0 0 1.5 1H21a2 2 0 0 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
		{/if}
	</svg>
{/snippet}

{#if isLoginRoute}
	{@render children()}
{:else}
<Sidebar.Provider
	bind:open={sidebarOpen}
	onOpenChange={persistSidebarOpen}
	style="--sidebar-width: 13rem; --sidebar-width-icon: 3rem;"
>
	<Sidebar.Root collapsible="icon">
		<Sidebar.Header>
			<div class="flex items-center gap-2 px-1 py-0.5">
				<!-- Dark backdrop keeps the (dark) logo art visible on the light theme. -->
				<img src="/wizard-icon.png" alt="wshm" class="h-7 w-7 flex-shrink-0 rounded-md bg-gray-900 p-0.5" />
				<span class="truncate text-base font-bold group-data-[collapsible=icon]:hidden">wshm</span>
			</div>
			<div class="space-y-1.5 group-data-[collapsible=icon]:hidden">
				<Select.Root type="single" bind:value={selectedRepoValue}>
					<Select.Trigger size="sm" class="w-full text-xs">
						{repoOptions.find((o) => o.value === selectedRepoValue)?.name ?? 'All repos'}
					</Select.Trigger>
					<Select.Content>
						{#each repoOptions as o (o.value)}
							<Select.Item value={o.value} label={o.name} />
						{/each}
					</Select.Content>
				</Select.Root>
				<div class="flex w-full gap-1">
					<Button
						variant="outline"
						size="xs"
						class="flex-1"
						disabled={syncing || !can(me?.role, 'syncIncremental')}
						onclick={() => runSync(false)}
						title={can(me?.role, 'syncIncremental')
							? 'Incremental sync (changes since last sync)'
							: 'Requires member role'}
					>{syncing ? '…' : 'Sync'}</Button>
					<Button
						variant="outline"
						size="xs"
						class="flex-1"
						disabled={syncing || !can(me?.role, 'syncFull')}
						onclick={() => runSync(true)}
						title={can(me?.role, 'syncFull')
							? 'Full re-sync (slower)'
							: 'Requires operator role'}
					>Full</Button>
				</div>
				{#if syncMsg}
					<div class="truncate text-[0.65rem] text-muted-foreground" title={syncMsg}>{syncMsg}</div>
				{/if}
			</div>
		</Sidebar.Header>

		<Sidebar.Content>
			{#each navSections as section (section.name)}
				<!-- In icon-collapsed mode the group labels (fold triggers) are not
				     visible, so a folded section would leave orphaned, unreachable
				     items: force every section open while collapsed. -->
				<Collapsible.Root
					open={!sidebarOpen || sectionOpen(section.name, section.items)}
					onOpenChange={(o) => { if (sidebarOpen) setSectionOpen(section.name, o); }}
					class="group/collapsible"
				>
					<Sidebar.Group class="py-1">
						<Sidebar.GroupLabel class="text-[0.625rem] font-semibold tracking-wider uppercase">
							{#snippet child({ props })}
								<Collapsible.Trigger {...props}>
									{section.name}
									<ChevronRightIcon
										class="ml-auto size-3 transition-transform group-data-[state=open]/collapsible:rotate-90"
									/>
								</Collapsible.Trigger>
							{/snippet}
						</Sidebar.GroupLabel>
						<Collapsible.Content>
							<Sidebar.GroupContent>
								<Sidebar.Menu>
									{#each section.items as item (item.href)}
										<Sidebar.MenuItem>
											<Sidebar.MenuButton isActive={activeUrl === item.href} tooltipContent={item.label}>
												{#snippet child({ props })}
													<a
														href={item.href}
														aria-current={activeUrl === item.href ? 'page' : undefined}
														{...props}
													>
														{@render navIcon(item.icon)}
														<span>{item.label}</span>
													</a>
												{/snippet}
											</Sidebar.MenuButton>
										</Sidebar.MenuItem>
									{/each}
								</Sidebar.Menu>
							</Sidebar.GroupContent>
						</Collapsible.Content>
					</Sidebar.Group>
				</Collapsible.Root>
			{/each}
		</Sidebar.Content>

		<Sidebar.Footer>
			{#if me}
				<div class="flex items-center gap-2 px-1" title={meLabel(me)}>
					<Avatar.Root class="size-7 flex-shrink-0">
						<Avatar.Fallback class="bg-primary text-xs font-semibold text-primary-foreground">
							{meInitial(me)}
						</Avatar.Fallback>
					</Avatar.Root>
					<div class="min-w-0 flex-1 group-data-[collapsible=icon]:hidden">
						<div class="truncate text-xs">{meLabel(me)}</div>
						<div class="text-[0.625rem] tracking-wider text-muted-foreground uppercase">
							{me.auth_method === 'sso' ? 'SSO' : 'local'}
						</div>
					</div>
				</div>
			{/if}
			<div class="flex items-center gap-1 group-data-[collapsible=icon]:flex-col">
				<Button
					variant="ghost"
					size="icon-sm"
					class="text-muted-foreground hover:text-foreground"
					onclick={handleLogout}
					title="Sign out"
					aria-label="Sign out"
				>
					<LogOutIcon />
				</Button>
				<Button
					variant="ghost"
					size="icon-sm"
					class="text-muted-foreground hover:text-foreground"
					onclick={toggleTheme}
					title={currentTheme === 'dark' ? 'Switch to light mode' : 'Switch to dark mode'}
					aria-label="Toggle theme"
				>
					{#if currentTheme === 'dark'}
						<SunIcon />
					{:else}
						<MoonIcon />
					{/if}
				</Button>
				<span
					class="mono flex-1 text-center text-[0.625rem] text-muted-foreground group-data-[collapsible=icon]:hidden"
					title={license?.is_pro ? 'wshm-pro' : 'wshm OSS'}
				>v{license?.version ?? '…'}</span>
				<Sidebar.Trigger
					class="text-muted-foreground hover:text-foreground"
					title="Toggle sidebar (⌘B)"
				/>
			</div>
		</Sidebar.Footer>
		<Sidebar.Rail />
	</Sidebar.Root>

	<Sidebar.Inset>
		<main class="p-3">
			{#if authStatus && !authStatus.github && bannerOpen}
				<Alert.Root
					class="mb-3 border-yellow-500/40 bg-yellow-500/10 text-yellow-700 dark:text-yellow-200 [&>svg]:text-yellow-500"
				>
					<TriangleAlertIcon />
					<Alert.Title>Anonymous GitHub mode.</Alert.Title>
					<Alert.Description class="text-yellow-700/90 dark:text-yellow-200/90">
						Public repos sync read-only with a 60 req/h limit; labels, comments, and auto-fix actions are skipped.
						<a href="/settings" class="ml-1 underline">Add a github_token in Settings → Secrets</a>
						for full functionality.
					</Alert.Description>
					<Button
						variant="ghost"
						size="icon-xs"
						class="absolute top-2 right-2 text-current hover:bg-yellow-500/20 hover:text-current"
						onclick={() => {
							bannerOpen = false;
							persistBannerDismiss();
						}}
						aria-label="Dismiss"
					>
						<XIcon />
					</Button>
				</Alert.Root>
			{/if}
			{@render children()}
		</main>
	</Sidebar.Inset>
</Sidebar.Provider>
{/if}
