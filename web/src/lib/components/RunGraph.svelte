<!--
	Pipeline run graph of one issue / PR (#234): the steps of a run as a
	left-to-right flow, steps that do not depend on each other stacked in the
	same column. Each node shows its status, duration, AI calls and findings;
	the skip reason or error shows on hover. A selector picks the run when
	there are several.
-->
<script lang="ts">
	import { fetchRuns, type PipelineRun, type PipelineStepRun } from '$lib/api';
	import { layers, formatDuration } from '$lib/pipelineGraph';
	import * as Card from '$lib/components/ui/card';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { Badge } from '$lib/components/ui/badge';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import CircleXIcon from '@lucide/svelte/icons/circle-x';
	import CircleSlashIcon from '@lucide/svelte/icons/circle-slash';
	import CircleDashedIcon from '@lucide/svelte/icons/circle-dashed';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import WorkflowIcon from '@lucide/svelte/icons/workflow';

	let { repo, kind, number }: { repo: string; kind: 'issue' | 'pr'; number: number } = $props();

	let runs: PipelineRun[] = $state([]);
	let selectedId = $state<number | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);

	$effect(() => {
		const slug = repo;
		const n = number;
		const k = kind;
		loading = true;
		error = null;
		fetchRuns(slug, n, k)
			.then((r) => {
				runs = r.runs;
				selectedId = r.runs[0]?.id ?? null;
			})
			.catch((e) => (error = e instanceof Error ? e.message : 'Failed to load runs'))
			.finally(() => (loading = false));
	});

	let run = $derived(runs.find((r) => r.id === selectedId) ?? null);

	/** Planned steps with their recorded outcome (none yet: pending). */
	type Node = { id: string; uses: string; needs?: string[]; after?: string[]; rec: PipelineStepRun | null };
	let columns = $derived.by((): Node[][] => {
		if (!run) return [];
		const byId = new Map(run.steps.map((s) => [s.step_id, s]));
		const planned = run.plan.length
			? run.plan
			: run.steps.map((s) => ({ id: s.step_id, uses: s.uses, needs: [] }));
		const nodes: Node[] = planned.map((p) => ({ ...p, rec: byId.get(p.id) ?? null }));
		return layers(nodes);
	});

	function status(n: Node): string {
		return n.rec?.status ?? (run?.status === 'running' ? 'running' : 'pending');
	}

	const nodeClass: Record<string, string> = {
		done: 'border-primary/60 bg-primary/5',
		failed: 'border-destructive/70 bg-destructive/5',
		skipped: 'border-dashed border-border bg-muted/40 text-muted-foreground',
		running: 'border-dashed border-primary/50',
		pending: 'border-dashed border-border text-muted-foreground'
	};

	function runLabel(r: PipelineRun): string {
		const when = new Date(r.started_at).toLocaleString();
		return `${when} · ${r.template} · ${r.trigger} · ${r.status}`;
	}

	function shortSha(s: string | null): string {
		return s ? s.slice(0, 7) : '';
	}
</script>

<Card.Root class="mt-4">
	<Card.Header class="pb-2">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<Card.Title class="flex items-center gap-2 text-base">
				<WorkflowIcon class="size-4 text-muted-foreground" />
				Pipeline runs
			</Card.Title>
			{#if runs.length > 1}
				<select
					class="rounded-md border bg-background px-2 py-1 text-xs"
					aria-label="Select a run"
					value={selectedId}
					onchange={(e) => (selectedId = Number((e.currentTarget as HTMLSelectElement).value))}
				>
					{#each runs as r (r.id)}
						<option value={r.id}>{runLabel(r)}</option>
					{/each}
				</select>
			{/if}
		</div>
		{#if run}
			<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
				<span>template <span class="font-medium text-foreground">{run.template}</span></span>
				<span>trigger <span class="mono">{run.trigger}</span></span>
				<Badge variant={run.status === 'failed' ? 'destructive' : run.status === 'done' ? 'secondary' : 'outline'}>
					{run.status}
				</Badge>
				<span>{formatDuration(run.duration_ms)}</span>
				<span>{run.ai_calls} AI call{run.ai_calls === 1 ? '' : 's'}</span>
				{#if run.head_sha}<span class="mono">@{shortSha(run.head_sha)}</span>{/if}
				<span>{new Date(run.started_at).toLocaleString()}</span>
			</div>
			{#if run.error}
				<p class="text-xs text-destructive">{run.error}</p>
			{/if}
		{/if}
	</Card.Header>
	<Card.Content>
		{#if loading}
			<p class="text-sm text-muted-foreground">Loading…</p>
		{:else if error}
			<p class="text-sm text-destructive">{error}</p>
		{:else if !run}
			<p class="text-sm text-muted-foreground">No pipeline run recorded for this {kind === 'pr' ? 'pull request' : 'issue'} yet.</p>
		{:else if columns.length === 0}
			<p class="text-sm text-muted-foreground">This run had no step for this item.</p>
		{:else}
			<Tooltip.Provider delayDuration={150}>
				<div class="overflow-x-auto pb-2">
					<div class="flex min-w-max items-center gap-1">
						{#each columns as col, ci}
							{#if ci > 0}
								<ChevronRightIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
							{/if}
							<div class="flex flex-col gap-2">
								{#each col as n (n.id)}
									{@const st = status(n)}
									<Tooltip.Root>
										<Tooltip.Trigger
											class="w-44 rounded-md border px-2.5 py-2 text-left text-xs transition-colors hover:bg-accent/40 {nodeClass[st] ?? nodeClass.pending}"
										>
											<div class="flex items-center gap-1.5">
												{#if st === 'done'}
													<CircleCheckIcon class="size-3.5 shrink-0 text-primary" />
												{:else if st === 'failed'}
													<CircleXIcon class="size-3.5 shrink-0 text-destructive" />
												{:else if st === 'skipped'}
													<CircleSlashIcon class="size-3.5 shrink-0 text-muted-foreground" />
												{:else}
													<CircleDashedIcon class="size-3.5 shrink-0 text-muted-foreground" />
												{/if}
												<span class="truncate font-medium {st === 'skipped' ? '' : 'text-foreground'}">{n.id}</span>
											</div>
											{#if n.uses !== n.id}
												<div class="mono mt-0.5 truncate text-[10px] text-muted-foreground">{n.uses}</div>
											{/if}
											<div class="mt-1 flex flex-wrap gap-x-2 text-[10px] text-muted-foreground">
												<span>{st}</span>
												{#if n.rec && st !== 'skipped'}
													<span>{formatDuration(n.rec.duration_ms)}</span>
												{/if}
												{#if n.rec && n.rec.ai_calls > 0}
													<span>{n.rec.ai_calls} AI</span>
												{/if}
												{#if n.rec && n.rec.findings > 0}
													<span class="font-medium text-foreground">{n.rec.findings} finding{n.rec.findings === 1 ? '' : 's'}</span>
												{/if}
											</div>
										</Tooltip.Trigger>
										<Tooltip.Content side="bottom" class="max-w-xs text-xs leading-snug">
											<p class="font-medium">{n.id} <span class="mono opacity-80">({n.uses})</span></p>
											{#if n.rec?.reason}
												<p class="mt-1">{st === 'failed' ? 'Error' : 'Skipped'}: {n.rec.reason}</p>
											{/if}
											{#if n.rec?.summary}
												<p class="mt-1">{n.rec.summary}</p>
											{/if}
											{#if n.needs?.length}
												<p class="mt-1 opacity-80">needs {n.needs.join(', ')}</p>
											{/if}
											{#if n.after?.length}
												<p class="mt-1 opacity-80">after {n.after.join(', ')}</p>
											{/if}
											{#if !n.rec}
												<p class="mt-1 opacity-80">not recorded (run {run.status})</p>
											{/if}
										</Tooltip.Content>
									</Tooltip.Root>
								{/each}
							</div>
						{/each}
					</div>
				</div>
			</Tooltip.Provider>
		{/if}
	</Card.Content>
</Card.Root>
