<script lang="ts">
	import { onMount } from 'svelte';
	import { selectedRepo } from '$lib/stores';
	import { fetchStatus, fetchIssues, fetchPulls, fetchAuthStatus, type Status, type Issue, type PullRequest, type AuthStatus } from '$lib/api';
	import { Alert, Card, Table, TableHead, TableHeadCell, TableBody, TableBodyRow, TableBodyCell, Badge, Button } from 'flowbite-svelte';

	let status: Status | null = $state(null);
	let issues: Issue[] = $state([]);
	let pulls: PullRequest[] = $state([]);
	let auth: AuthStatus | null = $state(null);
	let error: string | null = $state(null);

	let aiMissing = $derived(auth !== null && !auth.anthropic);
	let ghMissing = $derived(auth !== null && !auth.github);

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

	function riskColor(risk: string | null): 'green' | 'yellow' | 'red' | 'gray' {
		if (risk === 'low') return 'green';
		if (risk === 'medium') return 'yellow';
		if (risk === 'high') return 'red';
		return 'gray';
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
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Actions</h2>
		<p class="text-sm text-gray-500">Priority items requiring attention</p>
	</div>
	<div class="inline-flex rounded-md border border-gray-700 overflow-hidden text-xs" role="group" aria-label="Filter tasks">
		{#each views as v}
			<button
				type="button"
				class="px-3 py-1.5 select-none {view === v.id
					? 'bg-blue-600 text-white'
					: 'bg-gray-800 text-gray-400 hover:text-gray-200 hover:bg-gray-700'}"
				aria-pressed={view === v.id}
				onclick={() => (view = v.id)}
			>
				{v.label}{#if v.id === 'done' && doneCount > 0}<span class="ml-1 opacity-70">({doneCount})</span>{/if}
			</button>
		{/each}
	</div>
</div>

{#if error}
	<Card class="border-red-500 bg-gray-800 max-w-none">
		<p class="text-red-400">{error}</p>
	</Card>
{:else}
	{#if aiMissing || ghMissing}
		<Alert color="yellow" class="mb-4 !border !bg-yellow-50 !text-yellow-900 !border-yellow-300 dark:!bg-yellow-900/20 dark:!text-yellow-100 dark:!border-yellow-700/50">
			<div class="font-semibold mb-1">⚠️ Automatic actions disabled</div>
			<ul class="text-sm list-disc ml-5 space-y-0.5">
				{#if ghMissing}
					<li>No GitHub token configured — wshm cannot read issues/PRs from private repos or post comments. <a href="/settings" class="underline hover:text-yellow-200">Settings → Git providers</a>.</li>
				{/if}
				{#if aiMissing}
					<li>No AI provider configured — issues won't be triaged (no <code>priority</code>) and PRs won't be analyzed (no <code>risk</code>), so the lists below stay empty. <a href="/settings" class="underline hover:text-yellow-200">Settings → AI providers</a>.</li>
				{/if}
			</ul>
		</Alert>
	{/if}
	<div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3 mb-6">
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Open Issues</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.open_issues ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Untriaged</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.untriaged ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Open PRs</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.open_prs ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Unanalyzed</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.unanalyzed ?? '--'}</div>
		</Card>
		<Card class="bg-gray-800 border-gray-700 text-center !p-4 max-w-none">
			<div class="text-[0.6875rem] uppercase tracking-wider text-gray-500 mb-1">Conflicts</div>
			<div class="text-2xl font-bold text-gray-100 mono">{status?.conflicts ?? '--'}</div>
		</Card>
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Action Required</h2>
		<p class="text-sm text-gray-500 mb-3">High/critical priority issues, oldest first</p>
		{#if actionRequiredView.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				{#if view === 'done'}
					<p class="text-gray-600 text-center py-4">Nothing marked done here yet.</p>
				{:else if aiMissing}
					<p class="text-gray-500 text-center py-4 text-sm">
						Issues are not triaged because no AI provider is configured.<br />
						Set one in <a href="/settings" class="text-blue-400 hover:underline">Settings → AI providers</a> to populate this list.
					</p>
				{:else}
					<p class="text-gray-600 text-center py-4">No high-priority issues requiring action.</p>
				{/if}
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Priority</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px] text-right">Done</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each actionRequiredView as issue}
							{@const key = issueKey(issue)}
							<TableBodyRow class={isDone(key) ? 'opacity-50' : ''}>
								<TableBodyCell class="px-2 py-1.5 mono">{issue.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">
									<Badge color={issue.priority === 'critical' ? 'red' : 'yellow'}>{issue.priority}</Badge>
								</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(issue.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-right">
									<Button size="xs" color="alternative" class="!py-0.5 !px-2" onclick={() => toggleDone(key)}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">Issues TODO</h2>
		<p class="text-sm text-gray-500 mb-3">Top 10 issues by priority then age</p>
		{#if issuesTodoView.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-4">
					{view === 'done' ? 'Nothing marked done here yet.' : 'No open issues.'}
				</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Priority</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px] text-right">Done</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each issuesTodoView as issue}
							{@const key = issueKey(issue)}
							<TableBodyRow class={isDone(key) ? 'opacity-50' : ''}>
								<TableBodyCell class="px-2 py-1.5 mono">{issue.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.priority ?? '-'}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(issue.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{issue.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-right">
									<Button size="xs" color="alternative" class="!py-0.5 !px-2" onclick={() => toggleDone(key)}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>

	<div class="mt-6">
		<h2 class="text-xl font-semibold text-gray-100 mb-1">PRs TODO</h2>
		<p class="text-sm text-gray-500 mb-3">Top 10 PRs by conflicts then age</p>
		{#if prsTodoView.length === 0}
			<Card class="bg-gray-800 border-gray-700 max-w-none">
				<p class="text-gray-600 text-center py-4">
					{view === 'done' ? 'Nothing marked done here yet.' : 'No open pull requests.'}
				</p>
			</Card>
		{:else}
			<div class="w-full overflow-x-auto">
				<Table striped hoverable class="w-full">
					<TableHead class="text-xs uppercase text-gray-400">
						<TableHeadCell class="px-2 py-1.5 w-[60px]">#</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[70px]">Risk</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[50px]">Age</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5">Title</TableHeadCell>
						<TableHeadCell class="px-2 py-1.5 w-[80px] text-right">Done</TableHeadCell>
					</TableHead>
					<TableBody>
						{#each prsTodoView as pr}
							{@const key = prKey(pr)}
							<TableBodyRow class={isDone(key) ? 'opacity-50' : ''}>
								<TableBodyCell class="px-2 py-1.5 mono">{pr.number}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">
									{#if pr.risk}
										<Badge color={riskColor(pr.risk)}>{pr.risk}</Badge>
									{:else}
										<span class="text-gray-500">-</span>
									{/if}
								</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-gray-500 mono">{ageText(pr.created_at)}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5">{pr.title}</TableBodyCell>
								<TableBodyCell class="px-2 py-1.5 text-right">
									<Button size="xs" color="alternative" class="!py-0.5 !px-2" onclick={() => toggleDone(key)}>
										{isDone(key) ? 'Undo' : '✓ Done'}
									</Button>
								</TableBodyCell>
							</TableBodyRow>
						{/each}
					</TableBody>
				</Table>
			</div>
		{/if}
	</div>
{/if}
