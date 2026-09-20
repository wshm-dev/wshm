<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, fetchIssues, fetchPulls, fetchAuthStatus, type Status, type Issue, type PullRequest, type AuthStatus } from '$lib/api';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Alert from '$lib/components/ui/alert';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Dialog from '$lib/components/ui/dialog';
	import IssueDetail from '$lib/components/IssueDetail.svelte';
	import PrDetail from '$lib/components/PrDetail.svelte';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

	let status: Status | null = $state(null);
	let issues: Issue[] = $state([]);
	let pulls: PullRequest[] = $state([]);
	let auth: AuthStatus | null = $state(null);
	let error: string | null = $state(null);

	let aiMissing = $derived(auth !== null && !auth.anthropic);
	let ghMissing = $derived(auth !== null && !auth.github);

	// Row click opens the same detail modal as /issues and /prs; the Done
	// button stops propagation so marking a row never also opens it.
	let issueModalOpen = $state(false);
	let activeIssue: Issue | null = $state(null);
	let prModalOpen = $state(false);
	let activePr: PullRequest | null = $state(null);
	function openIssue(issue: Issue) { activeIssue = issue; issueModalOpen = true; }
	function openPr(pr: PullRequest) { activePr = pr; prModalOpen = true; }

	const priorityOrder: Record<string, number> = { critical: 0, high: 1, medium: 2, low: 3 };

	function ageDays(dateStr: string): number {
		return Math.floor((Date.now() - new Date(dateStr).getTime()) / 86400000);
	}

	function ageText(dateStr: string): string {
		const d = ageDays(dateStr);
		if (d === 0) return 'today';
		if (d === 1) return '1d';
		return `${d}d`;
	}

	// ── Done tracking ───────────────────────────────────────────────
	// "Done" is a personal, browser-local overlay: marking an item done
	// hides it from the "To do" view without touching the issue/PR on the
	// forge. Persisted in localStorage, keyed by repo + type + number so it
	// survives reloads and stays correct across repo switches. A done item
	// that is later closed on the forge simply drops out of the fetched
	// lists; its stale key is harmless.
	const DONE_KEY = 'wshm.actions.done';
	type View = 'todo' | 'done' | 'all';
	let view: View = $state('todo');
	let doneKeys: Set<string> = $state(new Set());

	function issueKey(i: Issue): string {
		return `${i.repo}:i:${i.number}`;
	}
	function prKey(p: PullRequest): string {
		return `${p.repo}:p:${p.number}`;
	}
	function isDone(key: string): boolean {
		return doneKeys.has(key);
	}
	function toggleDone(key: string) {
		// Reassign a new Set (not .add/.delete on the existing one): Svelte 5
		// `$state` does not track Set/Map mutations, only reassignment — so an
		// in-place mutate leaves the filtered lists stale and the click looks
		// like it does nothing.
		const next = new Set(doneKeys);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		doneKeys = next;
		persistDone();
	}
	function persistDone() {
		try {
			localStorage.setItem(DONE_KEY, JSON.stringify([...doneKeys]));
		} catch {
			/* ignore */
		}
	}
	function loadDone() {
		try {
			const raw = localStorage.getItem(DONE_KEY);
			if (raw) doneKeys = new Set(JSON.parse(raw) as string[]);
		} catch {
			/* ignore */
		}
	}
	function matchesView(key: string): boolean {
		if (view === 'all') return true;
		return view === 'done' ? doneKeys.has(key) : !doneKeys.has(key);
	}

	let actionRequired = $derived(
		issues
			.filter(i => i.state === 'open' && (i.priority === 'critical' || i.priority === 'high'))
			.sort((a, b) => ageDays(b.created_at) - ageDays(a.created_at))
	);

	// Sort the full open set, apply the done/view filter, THEN cap to 10 so
	// the "To do" list always shows up to 10 pending items (done items don't
	// eat into the cap).
	let issuesSorted = $derived(
		issues
			.filter(i => i.state === 'open')
			.sort((a, b) => {
				const pa = priorityOrder[a.priority ?? 'low'] ?? 9;
				const pb = priorityOrder[b.priority ?? 'low'] ?? 9;
				if (pa !== pb) return pa - pb;
				return ageDays(b.created_at) - ageDays(a.created_at);
			})
	);

	let prsSorted = $derived(
		pulls
			.filter(p => p.state === 'open')
			.sort((a, b) => {
				const ca = a.mergeable === false ? 0 : 1;
				const cb = b.mergeable === false ? 0 : 1;
				if (ca !== cb) return ca - cb;
				return ageDays(b.created_at) - ageDays(a.created_at);
			})
	);

	let actionRequiredView = $derived(actionRequired.filter(i => matchesView(issueKey(i))));
	let issuesTodoView = $derived(issuesSorted.filter(i => matchesView(issueKey(i))).slice(0, 10));
	let prsTodoView = $derived(prsSorted.filter(p => matchesView(prKey(p))).slice(0, 10));

	let doneCount = $derived(doneKeys.size);

	async function load() {
		try {
			error = null;
			const [s, i, p, a] = await Promise.all([
				fetchStatus(),
				fetchIssues({ limit: 500 }),
				fetchPulls({ limit: 500 }),
				fetchAuthStatus()
			]);
			status = s;
			issues = i.items;
			pulls = p.items;
			auth = a;
		} catch (e) {
			error = e instanceof Error ? e.message : 'Failed to load data';
		}
	}

	onMount(() => {
		loadDone();
		load();
		const unsub = selectedRepo.subscribe(() => { load(); });
		return unsub;
	});

	function riskBadgeClass(risk: string | null): string {
		if (risk === 'low') return 'border-green-500/30 bg-green-500/15 text-green-600 dark:text-green-400';
		if (risk === 'medium') return 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400';
		if (risk === 'high') return 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400';
		return '';
	}

	const views: { id: View; label: string }[] = [
		{ id: 'todo', label: 'To do' },
		{ id: 'done', label: 'Done' },
		{ id: 'all', label: 'All' }
	];
</script>

<svelte:head>
	<title>wshm - Actions</title>
</svelte:head>

<div class="mb-6 flex items-start justify-between gap-4 flex-wrap">
	<div>
		<h2 class="text-xl font-semibold text-foreground mb-1">Actions</h2>
		<p class="text-sm text-muted-foreground">Priority items requiring attention</p>
	</div>
	<div class="inline-flex rounded-md border overflow-hidden text-xs" role="group" aria-label="Filter tasks">
		{#each views as v}
			<button
				type="button"
				class="px-3 py-1.5 select-none {view === v.id
					? 'bg-primary text-primary-foreground'
					: 'bg-card text-muted-foreground hover:text-foreground hover:bg-muted'}"
				aria-pressed={view === v.id}
				onclick={() => (view = v.id)}
			>
				{v.label}{#if v.id === 'done' && doneCount > 0}<span class="ml-1 opacity-70">({doneCount})</span>{/if}
			</button>
		{/each}
	</div>
</div>

{#if error}
	<Card.Root class="border-red-500">
		<Card.Content>
			<p class="text-red-600 dark:text-red-400">{error}</p>
		</Card.Content>
	</Card.Root>
{:else}
	{#if aiMissing || ghMissing}
		<Alert.Root class="mb-4 border-yellow-500/40 bg-yellow-500/10 text-yellow-700 dark:text-yellow-200 [&>svg]:text-yellow-500">
			<TriangleAlertIcon />
			<Alert.Title>Automatic actions disabled</Alert.Title>
			<Alert.Description class="text-yellow-700 dark:text-yellow-200">
				<ul class="text-sm list-disc ml-5 space-y-0.5">
					{#if ghMissing}
						<li>No GitHub token configured — wshm cannot read issues/PRs from private repos or post comments. <a href="/settings" class="underline hover:text-yellow-800 dark:hover:text-yellow-100">Settings → Git providers</a>.</li>
					{/if}
					{#if aiMissing}
						<li>No AI provider configured — issues won't be triaged (no <code>priority</code>) and PRs won't be analyzed (no <code>risk</code>), so the lists below stay empty. <a href="/settings" class="underline hover:text-yellow-800 dark:hover:text-yellow-100">Settings → AI providers</a>.</li>
					{/if}
				</ul>
			</Alert.Description>
		</Alert.Root>
	{/if}
	<div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3 mb-6">
		<Card.Root class="py-4 text-center">
			<Card.Content class="px-4">
				<div class="text-[0.6875rem] uppercase tracking-wider text-muted-foreground mb-1">Open Issues</div>
				<div class="text-2xl font-bold text-foreground mono">{status?.open_issues ?? '--'}</div>
			</Card.Content>
		</Card.Root>
		<Card.Root class="py-4 text-center">
			<Card.Content class="px-4">
				<div class="text-[0.6875rem] uppercase tracking-wider text-muted-foreground mb-1">Untriaged</div>
				<div class="text-2xl font-bold text-foreground mono">{status?.untriaged ?? '--'}</div>
			</Card.Content>
		</Card.Root>
		<Card.Root class="py-4 text-center">
			<Card.Content class="px-4">
				<div class="text-[0.6875rem] uppercase tracking-wider text-muted-foreground mb-1">Open PRs</div>
				<div class="text-2xl font-bold text-foreground mono">{status?.open_prs ?? '--'}</div>
			</Card.Content>
		</Card.Root>
		<Card.Root class="py-4 text-center">
			<Card.Content class="px-4">
				<div class="text-[0.6875rem] uppercase tracking-wider text-muted-foreground mb-1">Unanalyzed</div>
				<div class="text-2xl font-bold text-foreground mono">{status?.unanalyzed ?? '--'}</div>
			</Card.Content>
		</Card.Root>
		<Card.Root class="py-4 text-center">
			<Card.Content class="px-4">
				<div class="text-[0.6875rem] uppercase tracking-wider text-muted-foreground mb-1">Conflicts</div>
				<div class="text-2xl font-bold text-foreground mono">{status?.conflicts ?? '--'}</div>
			</Card.Content>
		</Card.Root>
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Action Required</h2>
		<p class="text-sm text-muted-foreground mb-3">High/critical priority issues, oldest first</p>
		{#if actionRequiredView.length === 0}
			<Card.Root>
				<Card.Content>
					{#if view === 'done'}
						<p class="text-muted-foreground text-center py-4">Nothing marked done here yet.</p>
					{:else if aiMissing}
						<p class="text-muted-foreground text-center py-4 text-sm">
							Issues are not triaged because no AI provider is configured.<br />
							Set one in <a href="/settings" class="text-primary hover:underline">Settings → AI providers</a> to populate this list.
						</p>
					{:else}
						<p class="text-muted-foreground text-center py-4">No high-priority issues requiring action.</p>
					{/if}
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[60px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[70px]">Priority</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[50px]">Age</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px] text-right">Done</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each actionRequiredView as issue}
							{@const key = issueKey(issue)}
							<Table.Row class="cursor-pointer {isDone(key) ? 'opacity-50' : ''}" onclick={() => openIssue(issue)}>
								<Table.Cell class="px-2 py-1.5 mono">{issue.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">
									<Badge
										variant="outline"
										class={issue.priority === 'critical'
											? 'border-red-500/30 bg-red-500/15 text-red-600 dark:text-red-400'
											: 'border-yellow-500/30 bg-yellow-500/15 text-yellow-600 dark:text-yellow-400'}
									>{issue.priority}</Badge>
								</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground mono">{ageText(issue.created_at)}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{issue.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-right">
									<Button size="xs" variant="outline" onclick={(e: MouseEvent) => { e.stopPropagation(); toggleDone(key); }}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">Issues TODO</h2>
		<p class="text-sm text-muted-foreground mb-3">Top 10 issues by priority then age</p>
		{#if issuesTodoView.length === 0}
			<Card.Root>
				<Card.Content>
					<p class="text-muted-foreground text-center py-4">
						{view === 'done' ? 'Nothing marked done here yet.' : 'No open issues.'}
					</p>
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[60px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[70px]">Priority</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[50px]">Age</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px] text-right">Done</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each issuesTodoView as issue}
							{@const key = issueKey(issue)}
							<Table.Row class="cursor-pointer {isDone(key) ? 'opacity-50' : ''}" onclick={() => openIssue(issue)}>
								<Table.Cell class="px-2 py-1.5 mono">{issue.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{issue.priority ?? '-'}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground mono">{ageText(issue.created_at)}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{issue.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-right">
									<Button size="xs" variant="outline" onclick={(e: MouseEvent) => { e.stopPropagation(); toggleDone(key); }}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-foreground mb-1">PRs TODO</h2>
		<p class="text-sm text-muted-foreground mb-3">Top 10 PRs by conflicts then age</p>
		{#if prsTodoView.length === 0}
			<Card.Root>
				<Card.Content>
					<p class="text-muted-foreground text-center py-4">
						{view === 'done' ? 'Nothing marked done here yet.' : 'No open pull requests.'}
					</p>
				</Card.Content>
			</Card.Root>
		{:else}
			<div class="w-full overflow-x-auto rounded-lg border">
				<Table.Root class="w-full">
					<Table.Header class="text-xs uppercase text-muted-foreground">
						<Table.Row>
							<Table.Head class="px-2 py-1.5 w-[60px]">#</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[70px]">Risk</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[50px]">Age</Table.Head>
							<Table.Head class="px-2 py-1.5">Title</Table.Head>
							<Table.Head class="px-2 py-1.5 w-[80px] text-right">Done</Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each prsTodoView as pr}
							{@const key = prKey(pr)}
							<Table.Row class="cursor-pointer {isDone(key) ? 'opacity-50' : ''}" onclick={() => openPr(pr)}>
								<Table.Cell class="px-2 py-1.5 mono">{pr.number}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">
									{#if pr.risk_level}
										{#if riskBadgeClass(pr.risk_level)}
											<Badge variant="outline" class={riskBadgeClass(pr.risk_level)}>{pr.risk_level}</Badge>
										{:else}
											<Badge variant="secondary">{pr.risk_level}</Badge>
										{/if}
									{:else}
										<span class="text-muted-foreground">-</span>
									{/if}
								</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-muted-foreground mono">{ageText(pr.created_at)}</Table.Cell>
								<Table.Cell class="px-2 py-1.5">{pr.title}</Table.Cell>
								<Table.Cell class="px-2 py-1.5 text-right">
									<Button size="xs" variant="outline" onclick={(e: MouseEvent) => { e.stopPropagation(); toggleDone(key); }}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</div>
{/if}

<Dialog.Root bind:open={issueModalOpen}>
	<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
		<Dialog.Header>
			<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
				<span class="mono text-muted-foreground text-sm font-normal">#{activeIssue?.number}</span>
				<span class="truncate">{activeIssue?.title}</span>
			</Dialog.Title>
		</Dialog.Header>
		{#if activeIssue}
			<IssueDetail issue={activeIssue} />
			<div class="text-right pt-2">
				<a href="/issues/{activeIssue.number}" class="text-xs text-primary hover:underline">Open full page →</a>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<Dialog.Root bind:open={prModalOpen}>
	<Dialog.Content class="sm:max-w-[80vw] max-h-[85vh] overflow-y-auto">
		<Dialog.Header>
			<Dialog.Title class="flex w-full items-center gap-3 pr-2 text-base font-semibold">
				<span class="mono text-muted-foreground text-sm font-normal">#{activePr?.number}</span>
				<span class="truncate">{activePr?.title}</span>
			</Dialog.Title>
		</Dialog.Header>
		{#if activePr}
			<PrDetail pr={activePr} />
			<div class="text-right pt-2">
				<a href="/prs/{activePr.number}" class="text-xs text-primary hover:underline">Open full page →</a>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
