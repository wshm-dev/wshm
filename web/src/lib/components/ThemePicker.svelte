<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { theme, themeMode, palette, PALETTES, type Palette, type ThemeMode } from '$lib/stores';
	import SunIcon from '@lucide/svelte/icons/sun';
	import MoonIcon from '@lucide/svelte/icons/moon';
	import MonitorIcon from '@lucide/svelte/icons/monitor';
	import CheckIcon from '@lucide/svelte/icons/check';

	const modes: { id: ThemeMode; label: string; icon: typeof SunIcon }[] = [
		{ id: 'light', label: 'Light', icon: SunIcon },
		{ id: 'dark', label: 'Dark', icon: MoonIcon },
		{ id: 'system', label: 'System', icon: MonitorIcon }
	];

	// Swatches for the cards. They mirror the palette blocks in app.css so a
	// card can preview a theme without applying it.
	type Swatch = { accent: string; bg: string; sidebar: string; line: string };
	const previews: Record<Palette, { label: string; hint: string; light: Swatch; dark: Swatch }> = {
		default: {
			label: 'Default',
			hint: 'Blue accent on neutral zinc',
			light: { accent: 'oklch(0.546 0.245 262.881)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.623 0.214 259.815)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		violet: {
			label: 'Violet',
			hint: 'The wshm brand color',
			light: { accent: 'oklch(0.541 0.247 293.009)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.606 0.25 292.717)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		emerald: {
			label: 'Emerald',
			hint: 'Calm green accent',
			light: { accent: 'oklch(0.596 0.145 163.225)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.696 0.17 162.48)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		amber: {
			label: 'Amber',
			hint: 'Warm accent',
			light: { accent: 'oklch(0.666 0.179 58.318)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.769 0.188 70.08)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		rose: {
			label: 'Rose',
			hint: 'Vivid pink-red accent',
			light: { accent: 'oklch(0.586 0.253 17.585)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.645 0.246 16.439)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		graphite: {
			label: 'Graphite',
			hint: 'No color, just ink',
			light: { accent: 'oklch(0.21 0.006 285.885)', bg: 'oklch(1 0 0)', sidebar: 'oklch(0.975 0.002 286)', line: 'oklch(0.92 0.004 286.32)' },
			dark: { accent: 'oklch(0.92 0.004 286.32)', bg: 'oklch(0.141 0.005 285.823)', sidebar: 'oklch(0.185 0.005 285.885)', line: 'oklch(0.274 0.006 286.033)' }
		},
		midnight: {
			label: 'Midnight',
			hint: 'Navy surfaces, cyan accent',
			light: { accent: 'oklch(0.52 0.105 223.128)', bg: 'oklch(0.985 0.004 250)', sidebar: 'oklch(0.955 0.014 252)', line: 'oklch(0.915 0.022 252)' },
			dark: { accent: 'oklch(0.715 0.143 215.221)', bg: 'oklch(0.16 0.03 262)', sidebar: 'oklch(0.185 0.034 262)', line: 'oklch(0.28 0.045 262)' }
		},
		contrast: {
			label: 'High contrast',
			hint: 'Black and white, strong borders',
			light: { accent: 'oklch(0.45 0.25 264)', bg: 'oklch(1 0 0)', sidebar: 'oklch(1 0 0)', line: 'oklch(0.45 0 0)' },
			dark: { accent: 'oklch(0.78 0.16 250)', bg: 'oklch(0 0 0)', sidebar: 'oklch(0.06 0 0)', line: 'oklch(1 0 0 / 50%)' }
		}
	};
</script>

<div class="space-y-4">
	<Card.Root>
		<Card.Header>
			<Card.Title>Mode</Card.Title>
			<Card.Description>System follows your operating system and switches automatically.</Card.Description>
		</Card.Header>
		<Card.Content>
			<div class="inline-flex rounded-lg border p-0.5" role="radiogroup" aria-label="Color mode">
				{#each modes as m (m.id)}
					{@const Icon = m.icon}
					<button
						type="button"
						role="radio"
						aria-checked={$themeMode === m.id}
						class="flex items-center gap-1.5 rounded-md px-3 py-1.5 text-sm transition-colors {$themeMode === m.id
							? 'bg-primary text-primary-foreground'
							: 'text-muted-foreground hover:text-foreground'}"
						onclick={() => themeMode.set(m.id)}
					>
						<Icon class="size-4" />
						{m.label}
					</button>
				{/each}
			</div>
		</Card.Content>
	</Card.Root>

	<Card.Root>
		<Card.Header>
			<Card.Title>Theme</Card.Title>
			<Card.Description>Accent and surface colors. Every theme has a light and a dark variant.</Card.Description>
		</Card.Header>
		<Card.Content>
			<div class="grid grid-cols-2 gap-3 sm:grid-cols-3 xl:grid-cols-4" role="radiogroup" aria-label="Theme">
				{#each PALETTES as id (id)}
					{@const p = previews[id]}
					{@const sw = $theme === 'dark' ? p.dark : p.light}
					{@const selected = $palette === id}
					<button
						type="button"
						role="radio"
						aria-checked={selected}
						class="group overflow-hidden rounded-lg border text-left transition-shadow hover:shadow-md focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {selected
							? 'ring-2 ring-primary'
							: ''}"
						onclick={() => palette.set(id)}
					>
						<!-- Mini app: sidebar with an active item, content with a button. -->
						<div class="flex h-20" style="background: {sw.bg}">
							<div class="flex w-1/3 flex-col gap-1 p-1.5" style="background: {sw.sidebar}; border-right: 1px solid {sw.line}">
								<span class="h-1.5 w-3/4 rounded-full" style="background: {sw.line}"></span>
								<span class="h-1.5 w-full rounded-full" style="background: {sw.accent}"></span>
								<span class="h-1.5 w-2/3 rounded-full" style="background: {sw.line}"></span>
								<span class="h-1.5 w-3/4 rounded-full" style="background: {sw.line}"></span>
							</div>
							<div class="flex flex-1 flex-col gap-1.5 p-2">
								<span class="h-2 w-2/3 rounded-full" style="background: {sw.line}"></span>
								<span class="h-1.5 w-full rounded-full" style="background: {sw.line}"></span>
								<span class="mt-auto h-3 w-10 rounded" style="background: {sw.accent}"></span>
							</div>
						</div>
						<div class="flex items-start gap-2 border-t p-2">
							<div class="min-w-0 flex-1">
								<div class="truncate text-sm font-medium">{p.label}</div>
								<div class="truncate text-xs text-muted-foreground">{p.hint}</div>
							</div>
							{#if selected}<CheckIcon class="size-4 shrink-0 text-primary" />{/if}
						</div>
					</button>
				{/each}
			</div>
		</Card.Content>
	</Card.Root>
</div>
