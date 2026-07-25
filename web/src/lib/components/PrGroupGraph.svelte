<script lang="ts">
	/**
	 * Group → subgroup network graph. Each grand groupe is a large hub node
	 * (radius ∝ number of PRs), each sous-groupe a medium satellite linked to its
	 * parent (radius ∝ its PR count), coloured by group. Clicking a subgroup
	 * surfaces its PRs via `onSelect`. Data is computed server-side across the
	 * whole DB, so this reflects every PR, not a client sample.
	 *
	 * Dependency-free force sim (repulsion + link springs + collision + centering)
	 * on a rAF loop. Node count is small (~groups × subs), so plain SVG is ample.
	 */
	import { onMount } from 'svelte';
	import type { PrGroup, PrSubGroup } from '$lib/api';

	let {
		groups = [],
		selectedId = null,
		onSelect
	}: {
		groups: PrGroup[];
		selectedId?: string | null;
		onSelect?: (payload: { group: PrGroup; sub: PrSubGroup; id: string }) => void;
	} = $props();

	const W = 960;
	const H = 640;

	type Node = {
		id: string;
		kind: 'group' | 'sub';
		name: string;
		count: number;
		r: number;
		hue: number;
		parent?: string;
		group: PrGroup;
		sub?: PrSubGroup;
		x: number;
		y: number;
		vx: number;
		vy: number;
		fx: number | null;
		fy: number | null;
	};

	let nodes = $state<Node[]>([]);
	let links = $state<{ a: Node; b: Node }[]>([]);
	let raf = 0;

	function hueFor(s: string): number {
		let h = 0;
		for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
		return h;
	}

	// Rebuild whenever the incoming groups change.
	let signature = $derived(groups.map((g) => `${g.name}:${g.count}:${g.subgroups.length}`).join('|'));
	let builtFor = '';
	$effect(() => {
		if (signature !== builtFor) {
			builtFor = signature;
			build();
		}
	});

	function build() {
		const ns: Node[] = [];
		const cx = W / 2;
		const cy = H / 2;
		const n = Math.max(1, groups.length);
		groups.forEach((g, gi) => {
			const hue = hueFor(g.name);
			const ang = (gi / n) * Math.PI * 2;
			const gx = cx + Math.cos(ang) * 210;
			const gy = cy + Math.sin(ang) * 190;
			const gid = `g:${g.name}`;
			ns.push({
				id: gid,
				kind: 'group',
				name: g.name,
				count: g.count,
				r: 18 + Math.sqrt(g.count) * 1.7,
				hue,
				group: g,
				x: gx,
				y: gy,
				vx: 0,
				vy: 0,
				fx: null,
				fy: null
			});
			const m = Math.max(1, g.subgroups.length);
			g.subgroups.forEach((s, si) => {
				const sa = ang + (si - (m - 1) / 2) * 0.5;
				ns.push({
					id: `${gid}/s:${s.name}`,
					kind: 'sub',
					name: s.name,
					count: s.count,
					r: 8 + Math.sqrt(s.count) * 1.25,
					hue,
					parent: gid,
					group: g,
					sub: s,
					x: gx + Math.cos(sa) * 85,
					y: gy + Math.sin(sa) * 85,
					vx: 0,
					vy: 0,
					fx: null,
					fy: null
				});
			});
		});
		nodes = ns;
		const byId = new Map(ns.map((nd) => [nd.id, nd]));
		links = ns
			.filter((nd) => nd.parent)
			.map((nd) => ({ a: byId.get(nd.parent!)!, b: nd }))
			.filter((l) => l.a && l.b);
		startSim();
	}

	function startSim() {
		cancelAnimationFrame(raf);
		let alpha = 1;
		const tick = () => {
			const arr = nodes;
			// Repulsion + collision (O(n²), fine for a small node count).
			for (let i = 0; i < arr.length; i++) {
				const a = arr[i];
				for (let j = i + 1; j < arr.length; j++) {
					const b = arr[j];
					let dx = a.x - b.x;
					let dy = a.y - b.y;
					let d2 = dx * dx + dy * dy || 0.01;
					let d = Math.sqrt(d2);
					const ux = dx / d;
					const uy = dy / d;
					const rep = ((a.r * b.r * 0.9) / d2) * 55 * alpha;
					a.vx += ux * rep;
					a.vy += uy * rep;
					b.vx -= ux * rep;
					b.vy -= uy * rep;
					const min = a.r + b.r + 8;
					if (d < min) {
						const o = (min - d) * 0.5;
						a.x += ux * o;
						a.y += uy * o;
						b.x -= ux * o;
						b.y -= uy * o;
					}
				}
			}
			// Link springs pull subgroups to their parent.
			for (const l of links) {
				const a = l.a;
				const b = l.b;
				let dx = b.x - a.x;
				let dy = b.y - a.y;
				let d = Math.hypot(dx, dy) || 0.01;
				const target = a.r + b.r + 26;
				const f = (d - target) * 0.03 * alpha;
				const ux = dx / d;
				const uy = dy / d;
				a.vx += ux * f;
				a.vy += uy * f;
				b.vx -= ux * f;
				b.vy -= uy * f;
			}
			// Centering + integrate.
			for (const nd of arr) {
				if (nd.fx != null) {
					nd.x = nd.fx;
					nd.y = nd.fy!;
					nd.vx = 0;
					nd.vy = 0;
					continue;
				}
				nd.vx += (W / 2 - nd.x) * 0.0011 * alpha;
				nd.vy += (H / 2 - nd.y) * 0.0011 * alpha;
				nd.vx *= 0.86;
				nd.vy *= 0.86;
				nd.x += nd.vx;
				nd.y += nd.vy;
				nd.x = Math.max(nd.r, Math.min(W - nd.r, nd.x));
				nd.y = Math.max(nd.r, Math.min(H - nd.r, nd.y));
			}
			alpha *= 0.994;
			if (alpha > 0.02) raf = requestAnimationFrame(tick);
		};
		raf = requestAnimationFrame(tick);
	}

	onMount(() => () => cancelAnimationFrame(raf));

	// Drag.
	let dragging: Node | null = null;
	let svgEl: SVGSVGElement;
	function toSvg(e: PointerEvent) {
		const rect = svgEl.getBoundingClientRect();
		return {
			x: ((e.clientX - rect.left) / rect.width) * W,
			y: ((e.clientY - rect.top) / rect.height) * H
		};
	}
	function onDown(nd: Node, e: PointerEvent) {
		dragging = nd;
		const p = toSvg(e);
		nd.fx = p.x;
		nd.fy = p.y;
		svgEl.setPointerCapture(e.pointerId);
		startSim();
	}
	function onMove(e: PointerEvent) {
		if (!dragging) return;
		const p = toSvg(e);
		dragging.fx = p.x;
		dragging.fy = p.y;
		dragging.x = p.x;
		dragging.y = p.y;
	}
	function onUp() {
		if (dragging) {
			dragging.fx = null;
			dragging.fy = null;
			dragging = null;
		}
	}

	function pick(nd: Node) {
		if (nd.kind === 'sub' && nd.sub) onSelect?.({ group: nd.group, sub: nd.sub, id: nd.id });
	}
</script>

<svg
	bind:this={svgEl}
	viewBox="0 0 {W} {H}"
	class="w-full touch-none select-none rounded-lg border bg-card"
	style="aspect-ratio: {W}/{H};"
	onpointermove={onMove}
	onpointerup={onUp}
	onpointerleave={onUp}
	role="presentation"
>
	{#each links as l}
		<line
			x1={l.a.x}
			y1={l.a.y}
			x2={l.b.x}
			y2={l.b.y}
			stroke="hsl({l.a.hue} 45% 55% / 0.35)"
			stroke-width="1.5"
		/>
	{/each}
	{#each nodes as nd (nd.id)}
		<g
			class="cursor-pointer"
			onpointerdown={(e) => onDown(nd, e)}
			onclick={() => pick(nd)}
			onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && pick(nd)}
			role="button"
			tabindex="-1"
		>
			<circle
				cx={nd.x}
				cy={nd.y}
				r={nd.r}
				fill="hsl({nd.hue} {nd.kind === 'group' ? 60 : 55}% {nd.kind === 'group' ? 48 : 62}% / {nd.kind ===
				'group'
					? 0.9
					: 0.8})"
				stroke={selectedId === nd.id ? 'hsl(var(--foreground))' : 'white'}
				stroke-width={selectedId === nd.id ? 3 : nd.kind === 'group' ? 2 : 1}
			/>
			<text
				x={nd.x}
				y={nd.y - nd.r - 3}
				text-anchor="middle"
				class="pointer-events-none fill-foreground font-medium"
				style="font-size: {nd.kind === 'group' ? 13 : 10}px;"
			>
				{nd.name}
				<tspan class="fill-muted-foreground">· {nd.count}</tspan>
			</text>
		</g>
	{/each}
</svg>
