<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';

	interface Props {
		total: number;
		limit: number;
		offset: number;
		storageKey?: string;
		onChange: (next: { limit: number; offset: number }) => void;
		/** Rows actually shown after client-side text/dropdown filters are
		 * applied to the currently-loaded page. Filtering here only ever
		 * narrows what's on THIS page — Prev/Next still page through the
		 * unfiltered server total — so when this differs from the page's
		 * loaded row count, the summary switches to an honest "N matching
		 * (of M loaded, T total)" instead of implying the filtered set
		 * spans the whole T. */
		filteredCount?: number;
	}

	let { total, limit, offset, storageKey, onChange, filteredCount }: Props = $props();

	const sizes = [25, 50, 100, 250, 500];

	function setLimit(next: number) {
		if (storageKey) {
			try {
				localStorage.setItem(storageKey, String(next));
			} catch {
				/* private mode etc. */
			}
		}
		onChange({ limit: next, offset: 0 });
	}

	function prev() {
		const next = Math.max(0, offset - limit);
		onChange({ limit, offset: next });
	}

	function next() {
		const candidate = offset + limit;
		if (candidate >= total) return;
		onChange({ limit, offset: candidate });
	}

	const start = $derived(total === 0 ? 0 : offset + 1);
	const end = $derived(Math.min(total, offset + limit));
	const loadedOnPage = $derived(total === 0 ? 0 : end - offset);
	const isFiltered = $derived(filteredCount !== undefined && filteredCount !== loadedOnPage);
	const canPrev = $derived(offset > 0);
	const canNext = $derived(offset + limit < total);
</script>

<div class="flex items-center justify-between gap-3 text-xs text-muted-foreground mt-3">
	<div class="flex items-center gap-2">
		<span>Per page</span>
		<Select.Root
			type="single"
			value={String(limit)}
			onValueChange={(v) => { if (v) setLimit(Number(v)); }}
		>
			<Select.Trigger size="sm" class="w-24 text-xs">{limit}</Select.Trigger>
			<Select.Content>
				{#each sizes as s (s)}
					<Select.Item value={String(s)} label={String(s)} />
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	<div>
		{#if total === 0}
			0 items
		{:else if isFiltered}
			{filteredCount} matching (of {loadedOnPage} loaded, {total} total)
		{:else}
			{start}–{end} of {total}
		{/if}
	</div>

	<div class="flex items-center gap-2">
		<Button size="xs" variant="outline" disabled={!canPrev} onclick={prev}>Prev</Button>
		<Button size="xs" variant="outline" disabled={!canNext} onclick={next}>Next</Button>
	</div>
</div>
