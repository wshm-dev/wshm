<script lang="ts">
	interface Props {
		options: string[];
		value: string;
		onchange?: (v: string) => void;
		placeholder?: string;
	}

	let { options, value = $bindable(''), onchange, placeholder = 'All' }: Props = $props();

	// A dropdown pick is an exact value, so emit the `=value` form that
	// matchesFilter treats as equality — otherwise "no" would also match
	// "unknown" by substring. Quick-filter chips set the same `=value` form,
	// so the select reflects a chip-applied filter too. A plain legacy value
	// (e.g. `?conflicts=yes` from a dashboard link) still displays.
	let shown = $derived(value.startsWith('=') ? value.slice(1) : value);

	function handle(e: Event) {
		const v = (e.target as HTMLSelectElement).value;
		value = v ? `=${v}` : '';
		onchange?.(value);
	}
</script>

<select
	value={shown}
	onchange={handle}
	class="w-full rounded border border-border bg-muted px-1 py-0.5 text-xs text-foreground focus:border-primary focus:outline-none"
>
	<option value="">{placeholder}</option>
	{#each options as o (o)}
		<option value={o}>{o}</option>
	{/each}
</select>
