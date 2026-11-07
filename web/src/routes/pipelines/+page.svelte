<!--
	Pipelines (#234): the templates and routes of a repo, which template runs
	by default, and the per-repo DB override (shared by every pod), edited as
	JSON and validated by the daemon.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import {
		fetchRepos,
		fetchPipelines,
		updatePipelines,
		type PipelinesSettings,
		type PipelineTemplate
	} from '$lib/api';
	import { selectedRepo } from '$lib/stores';
	import { layers, describeCondition } from '$lib/pipelineGraph';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Label } from '$lib/components/ui/label';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';

	let repos: string[] = $state([]);
	let repo = $state('');
	let data = $state<PipelinesSettings | null>(null);
	let loading = $state(false);
	let loadError: string | null = $state(null);

	let draft = $state('');
	let parseError: string | null = $state(null);
	let saveError: string | null = $state(null);
	let saved: string | null = $state(null);
	let saving = $state(false);

	onMount(async () => {
		try {
			const r = await fetchRepos();
			repos = r.repos.map((x) => x.slug);
			const pick = get(selectedRepo);
			select(pick && repos.includes(pick) ? pick : (repos[0] ?? ''));
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load repositories';
		}
	});

	async function select(slug: string) {
		repo = slug;
		data = null;
		saveError = null;
		saved = null;
		if (!slug) return;
		loading = true;
		loadError = null;
		try {
			apply(await fetchPipelines(slug));
		} catch (e) {
			loadError = e instanceof Error ? e.message : 'Failed to load';
		} finally {
			loading = false;
		}
	}

	function apply(d: PipelinesSettings) {
		data = d;
		draft = d.override ? JSON.stringify(d.override, null, 2) : '';
		parseError = null;
	}

	let defaultTemplate = $derived(data?.default_template ?? 'standard');

	/** User templates first; a built-in replaced by a user one is hidden. */
	let templates = $derived.by((): { t: PipelineTemplate; builtin: boolean; replaces: boolean }[] => {
		if (!data) return [];
		const user = data.effective.pipelines ?? [];
		const names = new Set(user.map((t) => t.name));
		const builtinNames = new Set(data.builtin.map((t) => t.name));
		return [
			...user.map((t) => ({ t, builtin: false, replaces: builtinNames.has(t.name) })),
			...data.builtin.filter((t) => !names.has(t.name)).map((t) => ({ t, builtin: true, replaces: false }))
		];
	});

	let routes = $derived(
		[...(data?.effective.pipeline_routes ?? [])].sort((a, b) => (b.priority ?? 0) - (a.priority ?? 0))
	);

	let problems = $derived.by((): string[] => {
		if (!data) return [];
		return [
			...(data.toml_error ? [`TOML: ${data.toml_error} (the repo runs standard)`] : []),
			...(data.override_errors ?? []).map((e) => `Override: ${e}`),
			...(data.template_errors ?? [])
		];
	});

	function onDraftInput(v: string) {
		draft = v;
		saved = null;
		saveError = null;
		if (!v.trim()) {
			parseError = null;
			return;
		}
		try {
			const p = JSON.parse(v);
			parseError = p && typeof p === 'object' && !Array.isArray(p) ? null : 'The override must be a JSON object.';
		} catch (e) {
			parseError = e instanceof Error ? e.message : 'Invalid JSON';
		}
	}

	async function save(drop = false) {
		if (!repo || (!drop && parseError)) return;
		saving = true;
		saveError = null;
		saved = null;
		try {
			const body = drop || !draft.trim() ? null : JSON.parse(draft);
			apply(await updatePipelines(repo, body));
			saved = body ? 'Override saved: every pod uses it from its next run.' : 'Override removed: the TOML config applies.';
		} catch (e) {
			saveError = e instanceof Error ? e.message : 'Save failed';
		} finally {
			saving = false;
		}
	}

	/** Start an override from the current effective routes and templates. */
	function startFromEffective() {
		if (!data) return;
		const e = data.effective;
		onDraftInput(JSON.stringify({ pipelines: e.pipelines, pipeline_routes: e.pipeline_routes }, null, 2));
	}

	function stepTitle(s: PipelineTemplate['steps'][number]): string {
		const bits = [`uses ${s.uses}`];
		if (s.needs?.length) bits.push(`needs ${s.needs.join(', ')}`);
		const w = describeCondition(s.when);
		if (w) bits.push(`when ${w}`);
		if (s.on_failure === 'continue') bits.push('continues on failure');
		if (s.max_rounds) bits.push(`max ${s.max_rounds} rounds`);
		if (s.max_ai_calls) bits.push(`max ${s.max_ai_calls} AI calls`);
		if (s.with && Object.keys(s.with).length) bits.push(`with ${JSON.stringify(s.with)}`);
		return bits.join('\n');
	}
</script>

<svelte:head>
	<title>wshm - Pipelines</title>
</svelte:head>

<div class="mb-4 flex flex-wrap items-end justify-between gap-3">
	<div>
		<h2 class="text-xl font-semibold text-foreground">Pipelines</h2>
		<p class="text-sm text-muted-foreground">
			How each issue and pull request is processed: templates of steps, and routes that pick one.
		</p>
	</div>
	<div class="w-72">
		<Label class="mb-1 text-xs">Repository</Label>
		<select
			class="w-full rounded-md border bg-background px-2 py-1.5 text-sm"
			value={repo}
			onchange={(e) => select((e.currentTarget as HTMLSelectElement).value)}
		>
			{#if repos.length === 0}<option value="">No repository</option>{/if}
			{#each repos as r}
				<option value={r}>{r}</option>
			{/each}
		</select>
	</div>
</div>

{#if loadError}
	<Alert.Root variant="destructive" class="mb-4"><Alert.Description>{loadError}</Alert.Description></Alert.Root>
{/if}

{#if loading}
	<p class="text-sm text-muted-foreground">Loading…</p>
{:else if data}
	{#if problems.length || data.warnings.length}
		<Card.Root class="mb-4">
			<Card.Content class="space-y-1 text-sm">
				{#each problems as p}
					<p class="text-destructive">{p}</p>
				{/each}
				{#each data.warnings as w}
					<p class="text-muted-foreground">Warning: {w}</p>
				{/each}
			</Card.Content>
		</Card.Root>
	{/if}

	<div class="grid gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
		<div class="space-y-4">
			<Card.Root>
				<Card.Header>
					<Card.Title class="text-base">Templates</Card.Title>
					<Card.Description class="text-xs">
						Runs not picked by a route use <span class="font-medium text-foreground">{defaultTemplate}</span>.
						Steps in the same column do not depend on each other; hover a step for its conditions.
					</Card.Description>
				</Card.Header>
				<Card.Content class="space-y-3">
					{#each templates as { t, builtin, replaces } (t.name)}
						<div class="rounded-md border p-3 {t.name === defaultTemplate ? 'border-primary/60' : ''}">
							<div class="flex flex-wrap items-center gap-2">
								<span class="font-medium text-foreground">{t.name}</span>
								{#if t.name === defaultTemplate}<Badge>default</Badge>{/if}
								<Badge variant="outline">{builtin ? 'built-in' : replaces ? 'replaces built-in' : 'custom'}</Badge>
								{#if t.parallel}<Badge variant="secondary">parallel</Badge>{/if}
								{#if t.max_ai_calls}<span class="text-xs text-muted-foreground">≤ {t.max_ai_calls} AI calls</span>{/if}
							</div>
							{#if t.description}
								<p class="mt-1 text-xs text-muted-foreground">{t.description}</p>
							{/if}
							<div class="mt-2 overflow-x-auto">
								<div class="flex min-w-max items-center gap-1">
									{#each layers(t.steps) as col, ci}
										{#if ci > 0}<ChevronRightIcon class="size-3.5 shrink-0 text-muted-foreground" />{/if}
										<div class="flex flex-col gap-1">
											{#each col as s (s.id)}
												<span
													class="rounded border bg-muted/40 px-1.5 py-0.5 text-[11px] text-foreground"
													title={stepTitle(s)}
												>
													{s.id}{#if s.when?.item}<span class="ml-1 text-muted-foreground">{s.when.item}</span>{/if}
												</span>
											{/each}
										</div>
									{/each}
								</div>
							</div>
						</div>
					{/each}
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title class="text-base">Routes</Card.Title>
					<Card.Description class="text-xs">
						Tried by priority, highest first; the first whose conditions hold picks the template.
					</Card.Description>
				</Card.Header>
				<Card.Content>
					{#if routes.length === 0}
						<p class="text-sm text-muted-foreground">No route: every run uses {defaultTemplate}.</p>
					{:else}
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head class="w-20">Priority</Table.Head>
									<Table.Head>Template</Table.Head>
									<Table.Head>When</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each routes as r}
									<Table.Row>
										<Table.Cell class="mono">{r.priority ?? 0}</Table.Cell>
										<Table.Cell class="font-medium">{r.template}</Table.Cell>
										<Table.Cell class="text-xs text-muted-foreground">{describeCondition(r.when) || 'always'}</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
					{/if}
				</Card.Content>
			</Card.Root>
		</div>

		<Card.Root class="h-fit">
			<Card.Header>
				<Card.Title class="text-base">Override</Card.Title>
				<Card.Description class="text-xs">
					JSON with any of <span class="mono">pipelines</span>, <span class="mono">pipeline_routes</span>,
					<span class="mono">pipeline_runs</span>, <span class="mono">pipeline_nightly</span>. Each replaces the
					TOML value of <span class="mono">.wshm/config.toml</span>. Stored in the database, so every pod uses it.
				</Card.Description>
			</Card.Header>
			<Card.Content class="space-y-2">
				<p class="text-xs text-muted-foreground">
					{data.override ? 'An override is active.' : 'No override: the TOML config applies.'}
				</p>
				<textarea
					class="h-80 w-full rounded-md border bg-background p-2 font-mono text-xs text-foreground {parseError || saveError ? 'border-destructive' : ''}"
					spellcheck="false"
					placeholder={'{\n  "pipeline_routes": [\n    { "template": "anthropic", "priority": 10, "when": { "item": "pr" } }\n  ]\n}'}
					value={draft}
					oninput={(e) => onDraftInput((e.currentTarget as HTMLTextAreaElement).value)}
					aria-invalid={!!(parseError || saveError)}
				></textarea>
				{#if parseError}
					<p class="text-xs text-destructive">Invalid JSON: {parseError}</p>
				{/if}
				{#if saveError}
					<p class="text-xs text-destructive">{saveError}</p>
				{/if}
				{#if saved}
					<p class="text-xs text-primary">{saved}</p>
				{/if}
				<div class="flex flex-wrap gap-2">
					<Button size="sm" disabled={saving || !!parseError || !draft.trim()} onclick={() => save()}>Save override</Button>
					<Button size="sm" variant="outline" disabled={saving || !data.override} onclick={() => save(true)}>
						Remove override
					</Button>
					<Button size="sm" variant="ghost" disabled={saving} onclick={startFromEffective}>Start from current</Button>
				</div>
				<p class="text-xs text-muted-foreground">Step kinds: {data.step_kinds.join(', ')}</p>
			</Card.Content>
		</Card.Root>
	</div>
{/if}
