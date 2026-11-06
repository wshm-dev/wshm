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
		type Status,
		type Me,
		type AuthStatus,
		type LicenseInfo
	} from '$lib/api';
	import { canAccessRoute, can } from '$lib/permissions';
	import { timeAgo } from '$lib/time';
	import { goto, afterNavigate } from '$app/navigation';
	import {
		allNavItems,
		sectionOrder,
		settingsItem,
		searchItem,
		activeHref,
		type NavItem
	} from '$lib/nav';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import { Button } from '$lib/components/ui/button';
	import * as Alert from '$lib/components/ui/alert';
	import * as Avatar from '$lib/components/ui/avatar';
	import * as Sidebar from '$lib/components/ui/sidebar';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import XIcon from '@lucide/svelte/icons/x';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
	import CheckIcon from '@lucide/svelte/icons/check';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import RefreshCcwIcon from '@lucide/svelte/icons/refresh-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import LogOutIcon from '@lucide/svelte/icons/log-out';
	import SunIcon from '@lucide/svelte/icons/sun';
	import MoonIcon from '@lucide/svelte/icons/moon';
	import '../app.css';

	let { children }: { children: Snippet } = $props();
	let isLoginRoute = $derived($page.url.pathname === '/login');

	let repos: RepoInfo[] = $state([]);
	let status: Status | null = $state(null);
	let selectedRepoValue: string = $state('');
	selectedRepo.subscribe((v) => (selectedRepoValue = v ?? ''));
	let sidebarOpen: boolean = $state(true);
	let currentTheme: Theme = $state('dark');
	let me: Me | null = $state(null);
	let authStatus: AuthStatus | null = $state(null);
	let license: LicenseInfo | null = $state(null);
	let bannerOpen: boolean = $state(true);
	let paletteOpen = $state(false);
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

	function isFeatureLicensed(featureId: string | undefined): boolean {
		if (!featureId) return true;
		const f = license?.features?.find((x) => x.id === featureId);
		return f?.enabled === true;
	}
	function visible(i: NavItem): boolean {
		return (
			canAccessRoute(me?.role, i.href) &&
			isFeatureLicensed(i.feature) &&
			(!i.proOnly || license?.is_pro === true)
		);
	}
	let navItems = $derived(allNavItems.filter(visible));
	let showSettings = $derived(visible(settingsItem));
	// Everything the palette can jump to, in sidebar order.
	let paletteItems = $derived([
		...navItems,
		...(showSettings ? [settingsItem] : []),
		searchItem
	]);
	let activeUrl = $derived(activeHref($page.url.pathname, paletteItems));

	type Section = { name: string; items: { item: NavItem; children: NavItem[] }[] };
	let navSections: Section[] = $derived(
		sectionOrder
			.map((name) => ({
				name,
				items: navItems
					.filter((i) => i.section === name && !i.parent)
					.map((item) => ({
						item,
						children: navItems.filter((c) => c.parent === item.href)
					}))
			}))
			.filter((s) => s.items.length > 0)
	);

	// Per-section fold state (persisted). Admin starts folded to keep the
	// menu focused on daily work; a section containing the ACTIVE route is
	// always shown expanded so the current page never disappears from the
	// sidebar (it re-folds when you navigate away).
	const NAV_FOLD_KEY = 'wshm-nav-folded-sections-v2';
	let foldedSections: Record<string, boolean> = $state({ Admin: true });
	function loadFoldedSections() {
		try {
			const raw = localStorage.getItem(NAV_FOLD_KEY);
			if (raw) foldedSections = { Admin: true, ...JSON.parse(raw) };
		} catch { /* ignore */ }
	}
	function setSectionOpen(name: string, open: boolean) {
		foldedSections = { ...foldedSections, [name]: !open };
		try { localStorage.setItem(NAV_FOLD_KEY, JSON.stringify(foldedSections)); } catch { /* ignore */ }
	}
	function sectionOpen(section: Section): boolean {
		if (!foldedSections[section.name]) return true;
		return section.items.some(
			(e) => e.item.href === activeUrl || e.children.some((c) => c.href === activeUrl)
		);
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

	// "G then <key>" jumps (Linear, GitHub). The G arms the sequence for
	// one second; typing in a field never triggers it.
	let gArmedUntil = 0;
	function isTyping(t: EventTarget | null): boolean {
		const el = t as HTMLElement | null;
		return !!el && (el.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(el.tagName));
	}
	function onGlobalKeydown(e: KeyboardEvent) {
		if (isLoginRoute) return;
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
			e.preventDefault();
			paletteOpen = !paletteOpen;
			return;
		}
		if (e.metaKey || e.ctrlKey || e.altKey || paletteOpen || isTyping(e.target)) return;
		const key = e.key.toLowerCase();
		if (Date.now() < gArmedUntil) {
			gArmedUntil = 0;
			const target = paletteItems.find((i) => i.shortcut === key);
			if (target) {
				e.preventDefault();
				goto(target.href);
			}
		} else if (key === 'g') {
			gArmedUntil = Date.now() + 1000;
		}
	}

	// Recently visited pages, newest first, for the palette.
	const RECENT_KEY = 'wshm-recent-pages';
	let recentPages: string[] = $state([]);
	afterNavigate(() => {
		const href = activeHref($page.url.pathname, paletteItems);
		if (!href || href === '/search') return;
		recentPages = [href, ...recentPages.filter((h) => h !== href)].slice(0, 4);
		try { localStorage.setItem(RECENT_KEY, JSON.stringify(recentPages)); } catch { /* ignore */ }
	});

	// Badge counts follow the repo picked in the switcher.
	let counts = $derived.by(() => {
		if (!status) return null;
		if (selectedRepoValue === '') return status;
		return status.repos.find((r) => r.slug === selectedRepoValue) ?? null;
	});
	function countFor(item: NavItem): number | null {
		if (!item.count || !counts) return null;
		const n = counts[item.count];
		return typeof n === 'number' && n > 0 ? n : null;
	}
	function formatCount(n: number): string {
		return n > 999 ? `${Math.floor(n / 1000)}k` : String(n);
	}
	// Status and RepoInfo both carry last_sync.
	let lastSync = $derived(counts?.last_sync ?? null);

	async function refreshStatus() {
		try {
			status = await fetchStatus();
			repos = status.repos;
		} catch { /* ignore */ }
	}

	let isMac = $state(true);

	onMount(async () => {
		isMac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
		try {
			const saved = localStorage.getItem('wshm-sidebar-collapsed');
			if (saved === 'true') sidebarOpen = false;
		} catch { /* ignore */ }
		loadFoldedSections();
		try {
			recentPages = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]');
		} catch { /* ignore */ }
		theme.update((t) => t);
		await refreshStatus();
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

	// Keep the badge counts fresh while the app stays open.
	onMount(() => {
		const poll = setInterval(refreshStatus, 60_000);
		return () => clearInterval(poll);
	});

	function persistBannerDismiss() {
		try { localStorage.setItem('wshm-anon-banner-dismissed', 'true'); } catch { /* ignore */ }
	}

	let syncing = $state(false);
	let syncMsg: string | null = $state(null);

	async function runSync(full: boolean) {
		if (syncing) return;
		syncing = true;
		syncMsg = full ? 'Full sync…' : 'Syncing…';
		try {
			const r = full ? await syncFull() : await syncIncremental();
			const ok = r.errors?.length === 0;
			syncMsg = ok ? `Synced ${r.synced.length} repo(s)` : `Partial: ${r.errors?.length} error(s)`;
			await refreshStatus();
		} catch (e) {
			syncMsg = e instanceof Error ? e.message : 'Sync failed';
		}
		syncing = false;
		setTimeout(() => { if (syncMsg) syncMsg = null; }, 4000);
	}

	// Auto-collapse the sidebar when a content page requests it (e.g. the graph
	// on interaction). Only reacts to real bumps (>0), so it never fires on load.
	let lastCollapseSignal = 0;
	$effect(() => {
		if ($collapseSidebarSignal > lastCollapseSignal) {
			lastCollapseSignal = $collapseSidebarSignal;
			sidebarOpen = false;
		}
	});

	$effect(() => {
		selectedRepo.set(selectedRepoValue === '' ? null : selectedRepoValue);
	});
	let repoLabel = $derived(selectedRepoValue === '' ? 'All repos' : selectedRepoValue);
	let repoSubtitle = $derived(
		syncMsg ?? (lastSync ? `${repoLabel} · ${timeAgo(lastSync)}` : repoLabel)
	);
</script>

<svelte:window onkeydown={onGlobalKeydown} />

{#snippet navLink(item: NavItem)}
	{@const Icon = item.icon}
	{@const n = countFor(item)}
	<Sidebar.MenuButton
		isActive={activeUrl === item.href}
		tooltipContent={item.shortcut ? `${item.label}  ·  G ${item.shortcut.toUpperCase()}` : item.label}
		class="relative h-7 text-[0.8125rem] text-sidebar-foreground/75 data-active:text-sidebar-accent-foreground data-active:before:absolute data-active:before:inset-y-1.5 data-active:before:left-0 data-active:before:w-0.5 data-active:before:rounded-full data-active:before:bg-primary {n !== null ? 'pr-10' : ''}"
	>
		{#snippet child({ props })}
			<a href={item.href} aria-current={activeUrl === item.href ? 'page' : undefined} {...props}>
				<Icon />
				<span>{item.label}</span>
			</a>
		{/snippet}
	</Sidebar.MenuButton>
	{#if n !== null}
		<Sidebar.MenuBadge
			class="top-1 h-5 font-normal tabular-nums {item.countAttention
				? 'bg-primary/15 text-primary'
				: 'text-muted-foreground'}"
			title="{n} {item.count?.replace('_', ' ')}"
		>{formatCount(n)}</Sidebar.MenuBadge>
	{/if}
{/snippet}

{#if isLoginRoute}
	{@render children()}
{:else}
<Sidebar.Provider
	bind:open={sidebarOpen}
	onOpenChange={persistSidebarOpen}
	style="--sidebar-width: 15rem; --sidebar-width-icon: 3rem;"
>
	<Sidebar.Root collapsible="icon">
		<Sidebar.Header class="gap-1.5">
			<!-- Repo switcher: the "workspace" picker of Linear / Vercel. Sync
			     lives here because it acts on the selected repos. -->
			<Sidebar.Menu>
				<Sidebar.MenuItem>
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							{#snippet child({ props })}
								<Sidebar.MenuButton size="lg" class="data-[state=open]:bg-sidebar-accent" {...props}>
									<!-- Dark backdrop keeps the (dark) logo art visible on the light theme. -->
									<img src="/wizard-icon.png" alt="" class="size-8 shrink-0 rounded-md bg-gray-900 p-0.5" />
									<div class="grid min-w-0 flex-1 text-left leading-tight">
										<span class="truncate text-sm font-semibold">wshm</span>
										<span class="truncate text-xs text-muted-foreground" title={lastSync ?? undefined}>{repoSubtitle}</span>
									</div>
									<ChevronsUpDownIcon class="ml-auto text-muted-foreground" />
								</Sidebar.MenuButton>
							{/snippet}
						</DropdownMenu.Trigger>
						<DropdownMenu.Content class="w-(--bits-dropdown-menu-anchor-width) min-w-56" align="start" sideOffset={4}>
							<DropdownMenu.Label class="text-xs text-muted-foreground">Repositories</DropdownMenu.Label>
							<DropdownMenu.Item onSelect={() => (selectedRepoValue = '')}>
								<span class="flex-1 truncate">All repos</span>
								{#if selectedRepoValue === ''}<CheckIcon class="size-4" />{/if}
							</DropdownMenu.Item>
							{#each repos as r (r.slug)}
								<DropdownMenu.Item onSelect={() => (selectedRepoValue = r.slug)}>
									<span class="flex-1 truncate">{r.slug}</span>
									{#if selectedRepoValue === r.slug}<CheckIcon class="size-4" />{/if}
								</DropdownMenu.Item>
							{/each}
							<DropdownMenu.Separator />
							<DropdownMenu.Item
								disabled={syncing || !can(me?.role, 'syncIncremental')}
								onSelect={() => runSync(false)}
								title={can(me?.role, 'syncIncremental') ? undefined : 'Requires member role'}
							>
								<RefreshCwIcon class="size-4" />
								<span class="flex-1">Sync now</span>
								<span class="text-xs text-muted-foreground">changes</span>
							</DropdownMenu.Item>
							<DropdownMenu.Item
								disabled={syncing || !can(me?.role, 'syncFull')}
								onSelect={() => runSync(true)}
								title={can(me?.role, 'syncFull') ? undefined : 'Requires operator role'}
							>
								<RefreshCcwIcon class="size-4" />
								<span class="flex-1">Full re-sync</span>
								<span class="text-xs text-muted-foreground">slower</span>
							</DropdownMenu.Item>
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				</Sidebar.MenuItem>
			</Sidebar.Menu>
			<div class="flex items-center gap-1 group-data-[collapsible=icon]:flex-col">
				<Sidebar.Menu class="min-w-0 flex-1">
					<Sidebar.MenuItem>
						<Sidebar.MenuButton
							onclick={() => (paletteOpen = true)}
							tooltipContent="Search ({isMac ? '⌘' : 'Ctrl+'}K)"
							class="h-7 border border-sidebar-border text-[0.8125rem] text-muted-foreground group-data-[collapsible=icon]:border-0"
						>
							<SearchIcon />
							<span class="flex-1">Search…</span>
							<kbd class="pointer-events-none rounded border bg-background px-1 font-mono text-[0.625rem] group-data-[collapsible=icon]:hidden">
								{isMac ? '⌘' : 'Ctrl'} K
							</kbd>
						</Sidebar.MenuButton>
					</Sidebar.MenuItem>
				</Sidebar.Menu>
				<Sidebar.Trigger
					class="shrink-0 text-muted-foreground hover:text-foreground"
					title="Toggle sidebar ({isMac ? '⌘' : 'Ctrl+'}B)"
				/>
			</div>
		</Sidebar.Header>

		<Sidebar.Content class="gap-0">
			{#each navSections as section (section.name)}
				{#if section.name === 'Main'}
					<Sidebar.Group class="py-1">
						<Sidebar.Menu>
							{#each section.items as { item } (item.href)}
								<Sidebar.MenuItem>{@render navLink(item)}</Sidebar.MenuItem>
							{/each}
						</Sidebar.Menu>
					</Sidebar.Group>
				{:else}
					<!-- In icon-collapsed mode the group labels (fold triggers) are not
					     visible, so a folded section would leave orphaned, unreachable
					     items: force every section open while collapsed. -->
					<Collapsible.Root
						open={!sidebarOpen || sectionOpen(section)}
						onOpenChange={(o) => { if (sidebarOpen) setSectionOpen(section.name, o); }}
						class="group/collapsible"
					>
						<Sidebar.Group class="py-1">
							<Sidebar.GroupLabel class="h-7 text-xs font-medium text-muted-foreground hover:text-foreground">
								{#snippet child({ props })}
									<Collapsible.Trigger {...props}>
										{section.name}
										<ChevronRightIcon
											class="ml-1 size-3 opacity-0 transition-[transform,opacity] group-hover/collapsible:opacity-100 group-data-[state=closed]/collapsible:opacity-100 group-data-[state=open]/collapsible:rotate-90"
										/>
									</Collapsible.Trigger>
								{/snippet}
							</Sidebar.GroupLabel>
							<Collapsible.Content>
								<Sidebar.GroupContent>
									<Sidebar.Menu>
										{#each section.items as { item, children: subs } (item.href)}
											<Sidebar.MenuItem>
												{@render navLink(item)}
												{#if subs.length > 0}
													<Sidebar.MenuSub>
														{#each subs as sub (sub.href)}
															{@const SubIcon = sub.icon}
															<Sidebar.MenuSubItem>
																<Sidebar.MenuSubButton
																	href={sub.href}
																	isActive={activeUrl === sub.href}
																	class="text-[0.8125rem] text-sidebar-foreground/75 data-active:text-sidebar-accent-foreground"
																	aria-current={activeUrl === sub.href ? 'page' : undefined}
																>
																	<SubIcon />
																	<span>{sub.label}</span>
																</Sidebar.MenuSubButton>
															</Sidebar.MenuSubItem>
														{/each}
													</Sidebar.MenuSub>
												{/if}
											</Sidebar.MenuItem>
										{/each}
									</Sidebar.Menu>
								</Sidebar.GroupContent>
							</Collapsible.Content>
						</Sidebar.Group>
					</Collapsible.Root>
				{/if}
			{/each}
		</Sidebar.Content>

		<Sidebar.Footer class="gap-1 border-t border-sidebar-border">
			<Sidebar.Menu>
				{#if showSettings}
					<Sidebar.MenuItem>{@render navLink(settingsItem)}</Sidebar.MenuItem>
				{/if}
				<Sidebar.MenuItem>
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							{#snippet child({ props })}
								<Sidebar.MenuButton size="lg" class="data-[state=open]:bg-sidebar-accent" {...props}>
									<Avatar.Root class="size-8 shrink-0 rounded-md">
										<Avatar.Fallback class="rounded-md bg-primary text-xs font-semibold text-primary-foreground">
											{me ? meInitial(me) : '?'}
										</Avatar.Fallback>
									</Avatar.Root>
									<div class="grid min-w-0 flex-1 text-left leading-tight">
										<span class="truncate text-sm">{me ? meLabel(me) : 'Not signed in'}</span>
										<span class="truncate text-xs text-muted-foreground">
											{me?.role ?? ''}{me?.role ? ' · ' : ''}{license?.is_pro ? 'Enterprise' : 'OSS'} v{license?.version ?? '…'}
										</span>
									</div>
									<ChevronsUpDownIcon class="ml-auto text-muted-foreground" />
								</Sidebar.MenuButton>
							{/snippet}
						</DropdownMenu.Trigger>
						<DropdownMenu.Content class="w-(--bits-dropdown-menu-anchor-width) min-w-56" side="top" align="start" sideOffset={4}>
							{#if me}
								<DropdownMenu.Label class="font-normal">
									<div class="truncate text-sm">{meLabel(me)}</div>
									<div class="text-xs text-muted-foreground">
										{me.auth_method === 'sso' ? 'Signed in with SSO' : 'Local account'}
									</div>
								</DropdownMenu.Label>
								<DropdownMenu.Separator />
							{/if}
							<DropdownMenu.Item onSelect={toggleTheme}>
								{#if currentTheme === 'dark'}<SunIcon class="size-4" />{:else}<MoonIcon class="size-4" />{/if}
								<span class="flex-1">{currentTheme === 'dark' ? 'Light mode' : 'Dark mode'}</span>
							</DropdownMenu.Item>
							<DropdownMenu.Separator />
							<DropdownMenu.Item onSelect={handleLogout}>
								<LogOutIcon class="size-4" />
								<span>Sign out</span>
							</DropdownMenu.Item>
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				</Sidebar.MenuItem>
			</Sidebar.Menu>
		</Sidebar.Footer>
		<Sidebar.Rail />
	</Sidebar.Root>

	<CommandPalette bind:open={paletteOpen} items={paletteItems} recent={recentPages} />

	<Sidebar.Inset class="min-w-0">
		<main class="min-w-0 p-3">
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
