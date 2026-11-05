<script lang="ts">
	import {
		Chart,
		BarController,
		BarElement,
		LinearScale,
		CategoryScale,
		Tooltip
	} from 'chart.js';

	import { chartInk, onThemeChange } from './chartTheme';

	// chart.js is tree-shakeable: register only the bar-chart pieces.
	Chart.register(BarController, BarElement, LinearScale, CategoryScale, Tooltip);

	export type Bar = { label: string; value: number; color: string };

	let { bars }: { bars: Bar[] } = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let chart: Chart | null = null;

	function render() {
		if (!canvas) return;
		const ink = chartInk();
		const data = {
			labels: bars.map((b) => b.label),
			datasets: [
				{
					data: bars.map((b) => b.value),
					backgroundColor: bars.map((b) => b.color),
					borderWidth: 0,
					borderRadius: 3
				}
			]
		};
		if (chart) {
			chart.data = data;
			chart.update();
			return;
		}
		chart = new Chart(canvas, {
			type: 'bar',
			data,
			options: {
				// Horizontal bars — label names read better on the y-axis.
				indexAxis: 'y',
				responsive: true,
				maintainAspectRatio: false,
				plugins: { legend: { display: false } },
				scales: {
					x: {
						beginAtZero: true,
						ticks: { color: ink.tick, font: { size: 10 }, precision: 0 },
						grid: { color: ink.grid }
					},
					y: {
						ticks: { color: ink.label, font: { size: 11 } },
						grid: { display: false }
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
		// Reading the prop makes the effect re-run when data changes.
		void bars;
		render();
		const unsub = onThemeChange(rebuild);
		return () => {
			unsub();
			chart?.destroy();
			chart = null;
		};
	});
</script>

<div class="relative h-[340px]">
	<canvas bind:this={canvas}></canvas>
</div>
