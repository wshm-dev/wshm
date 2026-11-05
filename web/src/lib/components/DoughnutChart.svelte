<script lang="ts">
	import { Chart, ArcElement, DoughnutController, Tooltip, Legend } from 'chart.js';
	import { chartInk, onThemeChange } from './chartTheme';

	// chart.js is tree-shakeable: only the doughnut pieces are registered.
	Chart.register(ArcElement, DoughnutController, Tooltip, Legend);

	export type Segment = {
		label: string;
		value: number;
		color: string;
		/** Opaque payload echoed back via `onSegmentClick`; lets the parent map
		 *  a clicked slice to its filter predicate without parsing the label. */
		meta?: unknown;
	};

	let {
		segments,
		centerLabel = '',
		onSegmentClick
	}: {
		segments: Segment[];
		centerLabel?: string;
		onSegmentClick?: (segment: Segment, index: number) => void;
	} = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let chart: Chart | null = null;

	function render() {
		if (!canvas) return;
		const ink = chartInk();
		const data = {
			labels: segments.map((s) => s.label),
			datasets: [
				{
					data: segments.map((s) => s.value),
					backgroundColor: segments.map((s) => s.color),
					// Card-surface border keeps the 2px spacer gap between
					// slices in both themes.
					borderColor: ink.surface,
					borderWidth: 2
				}
			]
		};
		if (chart) {
			chart.data = data;
			chart.update();
			return;
		}
		chart = new Chart(canvas, {
			type: 'doughnut',
			data,
			options: {
				responsive: true,
				maintainAspectRatio: false,
				cutout: '62%',
				onClick: (_evt, elements) => {
					if (!onSegmentClick || elements.length === 0) return;
					const idx = elements[0].index;
					const seg = segments[idx];
					if (seg) onSegmentClick(seg, idx);
				},
				onHover: (evt, elements) => {
					const target = evt.native?.target as HTMLElement | undefined;
					if (target) {
						target.style.cursor =
							onSegmentClick && elements.length > 0 ? 'pointer' : 'default';
					}
				},
				plugins: {
					legend: {
						position: 'bottom',
						labels: {
							color: ink.label,
							font: { size: 11 },
							boxWidth: 12,
							padding: 12
						}
					},
					tooltip: {
						callbacks: {
							label: (ctx) => ` ${ctx.label}: ${ctx.parsed}`
						}
					}
				}
			}
		});
	}

	function rebuild() {
		chart?.destroy();
		chart = null;
		render();
	}

	$effect(() => {
		// Reading `segments` here makes the effect re-run when data changes.
		void segments;
		render();
		const unsub = onThemeChange(rebuild);
		return () => {
			unsub();
			chart?.destroy();
			chart = null;
		};
	});
</script>

<div class="relative h-[260px]">
	<canvas bind:this={canvas}></canvas>
	{#if centerLabel}
		<div
			class="pointer-events-none absolute inset-x-0 top-[42%] -translate-y-1/2 text-center"
		>
			<div class="text-2xl font-bold text-foreground mono">{centerLabel}</div>
		</div>
	{/if}
</div>
