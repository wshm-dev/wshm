<script lang="ts">
	import type { QuickChip } from '$lib/filter';

	// One-click presets that patch the page's `filters` state (the same object
	// the header filter row binds to), so a chip and a typed filter compose
	// naturally and "Clear" resets both. A chip is active when every key in
	// its patch currently holds the patch value; clicking an active chip
	// clears just those keys.
	let {
		chips,
		filters = $bindable()
	}: { chips: QuickChip[]; filters: Record<string, string> } = $props();

	function isActive(chip: QuickChip): boolean {
		return Object.entries(chip.patch).every(([k, v]) => filters[k] === v);
	}

	function toggle(chip: QuickChip) {
		if (isActive(chip)) {
			for (const k of Object.keys(chip.patch)) filters[k] = '';
		} else {
			for (const [k, v] of Object.entries(chip.patch)) filters[k] = v;
		}
	}

	let anyActive = $derived(Object.values(filters).some((v) => v.trim() !== ''));

	function clearAll() {
		for (const k of Object.keys(filters)) filters[k] = '';
	}
</script>

<div class="flex flex-wrap items-center gap-1.5 mb-3" role="group" aria-label="Quick filters">
	{#each chips as chip (chip.label)}
		{@const active = isActive(chip)}
		<button
			type="button"
			aria-pressed={active}
			onclick={() => toggle(chip)}
			class="inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs transition-colors
				{active
					? 'border-primary/40 bg-primary/15 text-primary'
					: 'border-border bg-muted/40 text-muted-foreground hover:bg-muted hover:text-foreground'}"
		>
			{chip.label}
			{#if chip.count !== undefined}
				<span class="rounded-full px-1.5 text-[0.65rem] leading-4 {active ? 'bg-primary/20' : 'bg-background/60'}">{chip.count}</span>
			{/if}
		</button>
	{/each}
	{#if anyActive}
		<button
			type="button"
			onclick={clearAll}
			class="ml-1 text-xs text-muted-foreground underline-offset-2 hover:text-foreground hover:underline"
		>
			Clear filters
		</button>
	{/if}
</div>
