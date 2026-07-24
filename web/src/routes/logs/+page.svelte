<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import { fetchLogs, type LogEntry } from '$lib/api';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import { Switch } from '$lib/components/ui/switch';
	import * as Select from '$lib/components/ui/select';

	const POLL_MS = 2000;
	const TAIL_INITIAL = 100;
	const MAX_VISIBLE = 500;

	let entries: LogEntry[] = $state([]);
	let lastId: number | null = $state(null);
	let level: string = $state('INFO');
	let paused: boolean = $state(false);
	let autoscroll: boolean = $state(true);
	let error: string | null = $state(null);
	let loading: boolean = $state(false);
	let logContainer: HTMLDivElement | undefined = $state();

	const LEVELS = [
		{ value: 'TRACE', name: 'Trace' },
		{ value: 'DEBUG', name: 'Debug' },
		{ value: 'INFO', name: 'Info' },
		{ value: 'WARN', name: 'Warn' },
		{ value: 'ERROR', name: 'Error' }
	];

	// In-flight guard: refuses to fire a new fetch while one is pending.
	// Without this, a slow poll + a fast next-tick can race and append
	// entries out of order.
	let inFlight = false;
	// Each fetch carries the level it was started with; if `level` changes
	// while a fetch is in flight, we discard that response on arrival.
	let pollTimer: ReturnType<typeof setInterval> | null = null;

	async function fetchOnce(opts: { tail?: number; reset: boolean }) {
		if (inFlight) return;
		const requestedLevel = level;
		const reset = opts.reset;
		inFlight = true;
		if (reset) loading = true;
		try {
			const r = await fetchLogs({
				tail: opts.tail,
				level: requestedLevel,
				since: reset ? undefined : (lastId ?? undefined)
			});
			// If level changed mid-flight, ignore the response — a fresh
			// reset fetch is already on its way.
			if (level !== requestedLevel) return;
			if (reset) {
				entries = r.entries;
				lastId = r.last_id;
			} else if (r.entries.length > 0) {
				const next = entries.length + r.entries.length > MAX_VISIBLE
					? [...entries, ...r.entries].slice(-MAX_VISIBLE)
					: [...entries, ...r.entries];
				entries = next;
				lastId = r.last_id;
			}
			if (error) error = null;
			if (r.entries.length > 0 || reset) await scheduleScroll();
		} catch (e) {
			const msg = e instanceof Error ? e.message : 'load failed';
			// The logs endpoint is gated at the Operator role — surface that
			// clearly instead of a raw "API error: 403" that looks like a bug.
			error = /\b40[13]\b/.test(msg)
				? 'Access to the daemon logs is restricted to Operator and Admin roles. Ask an admin to raise your role.'
				: msg;
		} finally {
			inFlight = false;
			if (reset) loading = false;
		}
	}

	async function scheduleScroll() {
		if (!autoscroll) return;
		await tick();
		if (logContainer) {
			logContainer.scrollTop = logContainer.scrollHeight;
		}
	}

	function clearLogs() {
		entries = [];
	}

	function levelClass(lvl: string): string {
		switch (lvl) {
			case 'ERROR': return 'text-red-600 dark:text-red-400';
			case 'WARN':  return 'text-yellow-600 dark:text-yellow-400';
			case 'INFO':  return 'text-primary';
			case 'DEBUG': return 'text-muted-foreground';
			case 'TRACE': return 'text-muted-foreground';
			default:      return 'text-foreground';
		}
	}

	function formatTime(at: string): string {
		try {
			return new Date(at).toLocaleTimeString();
		} catch {
			return at;
		}
	}

	function copyAll() {
		const text = entries
			.map(e => `${e.at} ${e.level} ${e.target}: ${e.message}`)
			.join('\n');
		navigator.clipboard.writeText(text).catch(() => {});
	}

	function shouldPoll(): boolean {
		if (paused) return false;
		// Don't poll when the tab is hidden — the user isn't watching, and
		// it lets the daemon's broadcast channel cool down.
		if (typeof document !== 'undefined' && document.hidden) return false;
		return true;
	}

	// Reload from scratch whenever the level changes (or on first mount).
	let prevLevel: string | null = null;
	$effect(() => {
		if (prevLevel === level) return;
		prevLevel = level;
		entries = [];
		lastId = null;
		fetchOnce({ tail: TAIL_INITIAL, reset: true });
	});

	onMount(() => {
		pollTimer = setInterval(() => {
			if (shouldPoll()) fetchOnce({ reset: false });
		}, POLL_MS);
	});

	onDestroy(() => {
		if (pollTimer) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
	});
</script>

<svelte:head>
	<title>wshm — Logs</title>
</svelte:head>

<div class="mb-4">
	<h2 class="text-xl font-bold tracking-tight mb-1">Daemon logs</h2>
	<p class="text-sm text-muted-foreground">Tail of the in-memory log buffer (resets on daemon restart). Polls every {POLL_MS / 1000}s when this tab is visible.</p>
</div>

<div class="flex flex-wrap items-center gap-3 mb-3">
	<div class="flex items-center gap-2">
		<span class="text-xs text-muted-foreground">Min level</span>
		<Select.Root type="single" bind:value={level}>
			<Select.Trigger class="w-28" size="sm">{LEVELS.find((l) => l.value === level)?.name ?? 'Level'}</Select.Trigger>
			<Select.Content>
				{#each LEVELS as l}
					<Select.Item value={l.value} label={l.name} />
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	<div class="flex items-center gap-2">
		<Switch id="logs-pause" bind:checked={paused} />
		<Label for="logs-pause">Pause</Label>
	</div>
	<div class="flex items-center gap-2">
		<Switch id="logs-autoscroll" bind:checked={autoscroll} />
		<Label for="logs-autoscroll">Autoscroll</Label>
	</div>

	<div class="ml-auto flex gap-2">
		<Button variant="outline" size="xs" disabled={loading} onclick={() => fetchOnce({ tail: TAIL_INITIAL, reset: true })}>
			{loading ? 'Loading…' : 'Reload'}
		</Button>
		<Button variant="outline" size="xs" onclick={copyAll}>Copy all</Button>
		<Button variant="outline" size="xs" onclick={clearLogs}>Clear view</Button>
	</div>
</div>

{#if error}
	<div class="rounded border border-red-500/40 bg-red-500/15 px-3 py-2 text-xs text-red-700 dark:text-red-300 mb-3">
		{error}
	</div>
{/if}

<div
	bind:this={logContainer}
	class="rounded border bg-muted/40 p-3 font-mono text-xs leading-5 overflow-auto"
	style="height: calc(100vh - 200px); min-height: 280px; max-height: 75vh;"
>
	{#if entries.length === 0}
		<div class="text-muted-foreground">{loading ? 'Loading…' : 'No log entries yet.'}</div>
	{:else}
		{#each entries as entry (entry.id)}
			<div class="flex gap-2 hover:bg-muted/50 px-1">
				<span class="text-muted-foreground shrink-0">{formatTime(entry.at)}</span>
				<span class="font-semibold w-12 shrink-0 {levelClass(entry.level)}">{entry.level}</span>
				<span class="text-muted-foreground shrink-0 max-w-[260px] truncate" title={entry.target}>{entry.target}</span>
				<span class="text-foreground break-all">{entry.message}</span>
			</div>
		{/each}
	{/if}
</div>

<div class="mt-2 text-xs text-muted-foreground">
	{entries.length} entries · last id {lastId ?? '—'} · {paused ? 'paused' : 'live'}
</div>
