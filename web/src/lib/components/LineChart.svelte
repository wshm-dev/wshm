<script lang="ts">
	import {
		Chart,
		LineController,
		LineElement,
		PointElement,
		LinearScale,
		CategoryScale,
		Tooltip,
		Legend,
		Filler
	} from 'chart.js';

	// chart.js is tree-shakeable: register only the line-chart pieces.
	Chart.register(
		LineController,
		LineElement,
		PointElement,
		LinearScale,
		CategoryScale,
		Tooltip,
		Legend,
		Filler
	);

	import { chartInk, onThemeChange } from './chartTheme';

	export type Line = { label: string; data: number[]; color: string };

	let { labels, lines }: { labels: string[]; lines: Line[] } = $props();

	let canvas: HTMLCanvasElement | undefined = $state();
	let chart: Chart | null = null;

	function render() {
		if (!canvas) return;
		const ink = chartInk();
		const data = {
			labels,
			datasets: lines.map((l) => ({
				label: l.label,
				data: l.data,
				borderColor: l.color,
				backgroundColor: l.color + '22',
				fill: false,
				tension: 0.3,
				pointRadius: 3,
				borderWidth: 2
			}))
		};
		if (chart) {
			chart.data = data;
			chart.update();
			return;
		}
		chart = new Chart(canvas, {
			type: 'line',
			data,
			options: {
				responsive: true,
				maintainAspectRatio: false,
				interaction: { mode: 'index', intersect: false },
				plugins: {
					legend: {
						position: 'bottom',
						labels: {
							color: ink.label,
							font: { size: 11 },
							boxWidth: 12,
							padding: 12
						}
					}
				},
				scales: {
					x: {
						ticks: { color: ink.tick, font: { size: 10 } },
						grid: { color: ink.grid }
					},
					y: {
						beginAtZero: true,
						ticks: { color: ink.tick, font: { size: 10 }, precision: 0 },
						grid: { color: ink.grid }
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
		// Reading the props makes the effect re-run when data changes.
		void labels;
		void lines;
		render();
		const unsub = onThemeChange(rebuild);
		return () => {
			unsub();
			chart?.destroy();
			chart = null;
		};
	});
</script>

<div class="relative h-[300px]">
	<canvas bind:this={canvas}></canvas>
</div>
