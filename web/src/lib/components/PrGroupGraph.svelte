<script lang="ts">
	/**
	 * Group → subgroup network graph. Each grand groupe is a large hub node
	 * (radius ∝ number of PRs); each sous-groupe is a medium satellite that hugs
	 * its parent (radius ∝ its PR count), coloured by group. Clicking a subgroup
	 * surfaces its PRs via `onSelect`. Data is computed server-side across the
	 * whole DB, so this reflects every PR, not a client sample.
	 *
	 * Dependency-free force sim: a STRONG spring pins each subgroup near its
	 * parent, groups repel each other to spread out, and a collision pass keeps
	 * bubbles from overlapping. Small node count → plain reactive SVG is ample.
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
		onSelect?: (group: PrGroup, sub: PrSubGroup) => void;
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
		parent: string | null;
		group: PrGroup;
		sub: PrSubGroup | null;
		x: number;
		y: number;
		vx: number;
		vy: number;
		fx: number | null;
		fy: number | null;
	};

	let nodes = $state<Node[]>([]);
	// Links are index pairs into `nodes`, resolved in the template so the lines
	// track the reactive node positions as the sim moves them.
	let links = $state<{ a: number; b: number }[]>([]);
	let raf = 0;

	function hueFor(s: string): number {
		let h = 0;
		for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
		return h;
	}

	const groupR = (count: number) => 18 + Math.sqrt(count) * 1.7;
	const subR = (count: number) => 8 + Math.sqrt(count) * 1.2;

	// Rebuild whenever the incoming groups change.
	let signature = $derived(
		groups.map((g) => `${g.name}:${g.count}:${g.subgroups.map((s) => s.name).join(',')}`).join('|')
	);
	let builtFor = '';
	$effect(() => {
		if (signature !== builtFor) {
			builtFor = signature;
			build();
		}
	});

	function build() {
		const ns: Node[] = [];
		const idx = new Map<string, number>();
		const cx = W / 2;
		const cy = H / 2;
		const gN = Math.max(1, groups.length);
		const ringR = Math.min(W, H) * 0.32;

		groups.forEach((g, gi) => {
			const hue = hueFor(g.name);
			const ang = (gi / gN) * Math.PI * 2 - Math.PI / 2;
			const gx = cx + Math.cos(ang) * ringR;
			const gy = cy + Math.sin(ang) * ringR;
			const gid = `g:${g.name}`;
			idx.set(gid, ns.length);
			ns.push({
				id: gid,
				kind: 'group',
				name: g.name,
				count: g.count,
				r: groupR(g.count),
				hue,
				parent: null,
				group: g,
				sub: null,
				x: gx,
				y: gy,
				vx: 0,
				vy: 0,
				fx: null,
				fy: null
			});
			const m = Math.max(1, g.subgroups.length);
			g.subgroups.forEach((s, si) => {
				const sa = ang + (si - (m - 1) / 2) * 0.7;
				const gr = groupR(g.count);
				idx.set(`${gid}/s:${s.name}`, ns.length);
				ns.push({
					id: `${gid}/s:${s.name}`,
					kind: 'sub',
					name: s.name,
					count: s.count,
					r: subR(s.count),
					hue,
					parent: gid,
					group: g,
					sub: s,
					x: gx + Math.cos(sa) * (gr + 34),
					y: gy + Math.sin(sa) * (gr + 34),
					vx: 0,
					vy: 0,
					fx: null,
					fy: null
				});
			});
		});

		nodes = ns;
		links = ns
			.map((nd, i) => (nd.parent ? { a: idx.get(nd.parent)!, b: i } : null))
			.filter((l): l is { a: number; b: number } => !!l && l.a != null);
		startSim();
	}

	function startSim() {
		cancelAnimationFrame(raf);
		let alpha = 1;
		const tick = () => {
			const arr = nodes;

			// 1) Strong spring: each subgroup is pulled to hug its parent group.
			for (const l of links) {
				const a = arr[l.a];
				const b = arr[l.b];
				let dx = b.x - a.x;
				let dy = b.y - a.y;
				let d = Math.hypot(dx, dy) || 0.01;
				const target = a.r + b.r + 14;
				const f = ((d - target) / d) * 0.12 * alpha;
				const fx = dx * f;
				const fy = dy * f;
				// Groups are heavy: nudge them little, move subgroups more.
				a.vx += fx * 0.25;
				a.vy += fy * 0.25;
				b.vx -= fx * 1.0;
				b.vy -= fy * 1.0;
			}

			// 2) Repulsion: groups spread from each other; siblings fan out. Kept
			//    short-range and gentle so nothing is flung to the walls.
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

					// Long-range charge only between the big group hubs.
					if (a.kind === 'group' && b.kind === 'group') {
						const rep = ((a.r * b.r) / d2) * 40 * alpha;
						a.vx += ux * rep;
						a.vy += uy * rep;
						b.vx -= ux * rep;
						b.vy -= uy * rep;
					}

					// Collision: never let two bubbles overlap.
					const min = a.r + b.r + 4;
					if (d < min) {
						const push = (min - d) * 0.5;
						a.x += ux * push;
						a.y += uy * push;
						b.x -= ux * push;
						b.y -= uy * push;
					}
				}
			}

			// 3) Gentle centering + integrate.
			for (const nd of arr) {
				if (nd.fx != null && nd.fy != null) {
					nd.x = nd.fx;
					nd.y = nd.fy;
					nd.vx = 0;
					nd.vy = 0;
					continue;
				}
				if (nd.kind === 'group') {
					nd.vx += (W / 2 - nd.x) * 0.004 * alpha;
					nd.vy += (H / 2 - nd.y) * 0.004 * alpha;
				}
				nd.vx *= 0.8;
				nd.vy *= 0.8;
				nd.x += nd.vx;
				nd.y += nd.vy;
				nd.x = Math.max(nd.r, Math.min(W - nd.r, nd.x));
				nd.y = Math.max(nd.r, Math.min(H - nd.r, nd.y));
			}

			alpha *= 0.99;
			if (alpha > 0.015) raf = requestAnimationFrame(tick);
		};
		raf = requestAnimationFrame(tick);
	}

	onMount(() => () => cancelAnimationFrame(raf));

	// --- drag ---
	let dragId: string | null = null;
	let svgEl: SVGSVGElement | null = null;

	function toSvg(e: PointerEvent): { x: number; y: number } {
		if (!svgEl) return { x: 0, y: 0 };
		const rect = svgEl.getBoundingClientRect();
		return {
			x: ((e.clientX - rect.left) / rect.width) * W,
			y: ((e.clientY - rect.top) / rect.height) * H
		};
	}
	function onDown(nd: Node, e: PointerEvent) {
		dragId = nd.id;
		const p = toSvg(e);
		nd.fx = p.x;
		nd.fy = p.y;
		(e.target as Element).setPointerCapture?.(e.pointerId);
		startSim();
	}
	function onMove(e: PointerEvent) {
		if (!dragId) return;
		const nd = nodes.find((n) => n.id === dragId);
		if (!nd) return;
		const p = toSvg(e);
		nd.fx = p.x;
		nd.fy = p.y;
		nd.x = p.x;
		nd.y = p.y;
	}
	function onUp() {
		if (!dragId) return;
		const nd = nodes.find((n) => n.id === dragId);
		if (nd) {
			nd.fx = null;
			nd.fy = null;
		}
		dragId = null;
	}

	function pick(nd: Node) {
		if (nd.kind === 'sub' && nd.sub) onSelect?.(nd.group, nd.sub);
	}
</script>

{#if nodes.length === 0}
	<div
		class="flex h-64 items-center justify-center rounded-lg border bg-card text-sm text-muted-foreground"
	>
		Aucun groupe à afficher.
	</div>
{:else}
	<svg
		bind:this={svgEl}
		viewBox="0 0 {W} {H}"
		class="w-full touch-none select-none rounded-lg border bg-card"
		onpointermove={onMove}
		onpointerup={onUp}
		onpointerleave={onUp}
		role="presentation"
	>
		{#each links as l (l.a + '-' + l.b)}
			<line
				x1={nodes[l.a].x}
				y1={nodes[l.a].y}
				x2={nodes[l.b].x}
				y2={nodes[l.b].y}
				stroke="hsl({nodes[l.a].hue} 45% 55% / 0.4)"
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
					fill="hsl({nd.hue} {nd.kind === 'group' ? 60 : 52}% {nd.kind === 'group'
						? 52
						: 62}% / {nd.kind === 'group' ? 0.9 : 0.85})"
					stroke={selectedId === nd.id ? 'white' : `hsl(${nd.hue} 55% 40%)`}
					stroke-width={selectedId === nd.id ? 3 : nd.kind === 'group' ? 2 : 1}
				/>
				<text
					x={nd.x}
					y={nd.kind === 'group' ? nd.y - nd.r - 4 : nd.y + nd.r + 10}
					text-anchor="middle"
					class="pointer-events-none fill-foreground font-medium"
					font-size={nd.kind === 'group' ? 13 : 10}
				>
					{nd.name}
					<tspan class="fill-muted-foreground">· {nd.count}</tspan>
				</text>
			</g>
		{/each}
	</svg>
{/if}
