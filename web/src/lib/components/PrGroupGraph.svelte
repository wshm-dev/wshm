<script lang="ts">
	/**
	 * Group → subgroup network graph with pan / zoom.
	 *
	 * Each grand groupe is a large hub node (radius ∝ number of PRs); each
	 * sous-groupe is a medium satellite that hugs its parent, coloured by group.
	 * Clicking a node fires `onSelect(group, sub|null)` — `sub` is null when a
	 * grand groupe itself is clicked. The view supports wheel-zoom (toward the
	 * cursor), background drag to pan, and node drag to reposition.
	 *
	 * Dependency-free force sim: a strong spring pins each subgroup near its
	 * parent, groups repel only each other to spread out, and a collision pass
	 * keeps bubbles from overlapping. Links reference the reactive $state nodes so
	 * connectors track motion.
	 */
	import { onMount } from 'svelte';
	import type { PrGroup, PrSubGroup } from '$lib/api';

	let {
		groups = [],
		selectedId = null,
		onSelect,
		onInteract
	}: {
		groups?: PrGroup[];
		selectedId?: string | null;
		onSelect?: (group: PrGroup, sub: PrSubGroup | null) => void;
		onInteract?: () => void;
	} = $props();

	// World (viewBox) size — large so there is room to spread and zoom into.
	const W = 1280;
	const H = 820;

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

	// View transform.
	let zoom = $state(1);
	let panX = $state(0);
	let panY = $state(0);

	let svgEl: SVGSVGElement | undefined = $state();

	function hueFor(s: string): number {
		let h = 0;
		for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
		return h;
	}

	// Rebuild whenever the incoming groups change.
	let signature = $derived(
		groups.map((g) => `${g.name}:${g.count}:${g.subgroups.length}`).join('|')
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
		const cx = W / 2;
		const cy = H / 2;
		const ring = Math.min(W, H) * 0.34;
		const n = Math.max(1, groups.length);
		groups.forEach((g, gi) => {
			const hue = hueFor(g.name);
			const ang = (gi / n) * Math.PI * 2 - Math.PI / 2;
			const gx = cx + Math.cos(ang) * ring;
			const gy = cy + Math.sin(ang) * ring;
			const gid = `g:${g.name}`;
			ns.push({
				id: gid,
				kind: 'group',
				name: g.name,
				count: g.count,
				r: 20 + Math.sqrt(g.count) * 1.8,
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
				const sa = ang + (si - (m - 1) / 2) * 0.6;
				ns.push({
					id: `${gid}/s:${s.name}`,
					kind: 'sub',
					name: s.name,
					count: s.count,
					r: 9 + Math.sqrt(s.count) * 1.3,
					hue,
					parent: gid,
					group: g,
					sub: s,
					x: gx + Math.cos(sa) * 95,
					y: gy + Math.sin(sa) * 95,
					vx: 0,
					vy: 0,
					fx: null,
					fy: null
				});
			});
		});
		nodes = ns;
		const byId = new Map(nodes.map((nd) => [nd.id, nd]));
		links = nodes
			.filter((nd) => nd.parent)
			.map((nd) => ({ a: byId.get(nd.parent!)!, b: nd }))
			.filter((l) => l.a && l.b);
		// Reset the view so a fresh graph is centred and fully visible.
		zoom = 1;
		panX = 0;
		panY = 0;
		startSim();
	}

	function startSim() {
		cancelAnimationFrame(raf);
		let alpha = 1;
		const tick = () => {
			const arr = nodes;
			// Strong spring: each subgroup hugs its parent group.
			for (const l of links) {
				const a = l.a;
				const b = l.b;
				let dx = b.x - a.x;
				let dy = b.y - a.y;
				let d = Math.sqrt(dx * dx + dy * dy) || 0.01;
				const target = a.r + b.r + 20;
				const f = ((d - target) / d) * 0.09 * alpha;
				const fxv = dx * f;
				const fyv = dy * f;
				a.vx += fxv;
				a.vy += fyv;
				b.vx -= fxv;
				b.vy -= fyv;
			}
			// Repulsion between GROUPS only (spread the hubs); collision for all.
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
					if (a.kind === 'group' && b.kind === 'group') {
						const rep = ((a.r * b.r) / d2) * 160 * alpha;
						a.vx += ux * rep;
						a.vy += uy * rep;
						b.vx -= ux * rep;
						b.vy -= uy * rep;
					}
					const min = a.r + b.r + 6;
					if (d < min) {
						const push = (min - d) * 0.5;
						a.x += ux * push;
						a.y += uy * push;
						b.x -= ux * push;
						b.y -= uy * push;
					}
				}
			}
			// Gentle centring + integrate.
			for (const nd of arr) {
				if (nd.fx !== null) {
					nd.x = nd.fx;
					nd.y = nd.fy!;
					nd.vx = 0;
					nd.vy = 0;
					continue;
				}
				nd.vx += (W / 2 - nd.x) * 0.0012 * alpha;
				nd.vy += (H / 2 - nd.y) * 0.0012 * alpha;
				nd.vx *= 0.82;
				nd.vy *= 0.82;
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

	// ---- coordinate helpers -------------------------------------------------
	/** Pixels → viewBox units (approx; assumes width-driven scale). */
	function unit(): number {
		const rect = svgEl?.getBoundingClientRect();
		return rect && rect.width ? W / rect.width : 1;
	}

	// ---- interactions -------------------------------------------------------
	let dragNode: Node | null = null;
	let panning = false;
	let lastX = 0;
	let lastY = 0;
	let moved = false;

	function onNodeDown(nd: Node, e: PointerEvent) {
		e.stopPropagation();
		dragNode = nd;
		moved = false;
		lastX = e.clientX;
		lastY = e.clientY;
		nd.fx = nd.x;
		nd.fy = nd.y;
		(e.currentTarget as Element).setPointerCapture?.(e.pointerId);
		onInteract?.();
	}

	function onBgDown(e: PointerEvent) {
		panning = true;
		moved = false;
		lastX = e.clientX;
		lastY = e.clientY;
		onInteract?.();
	}

	function onMove(e: PointerEvent) {
		if (dragNode) {
			const k = unit() / zoom;
			const nx = dragNode.x + (e.clientX - lastX) * k;
			const ny = dragNode.y + (e.clientY - lastY) * k;
			dragNode.x = nx;
			dragNode.y = ny;
			dragNode.fx = nx;
			dragNode.fy = ny;
			lastX = e.clientX;
			lastY = e.clientY;
			if (Math.abs(e.movementX) + Math.abs(e.movementY) > 2) moved = true;
			startSim();
		} else if (panning) {
			const k = unit();
			panX += (e.clientX - lastX) * k;
			panY += (e.clientY - lastY) * k;
			lastX = e.clientX;
			lastY = e.clientY;
			if (Math.abs(e.movementX) + Math.abs(e.movementY) > 2) moved = true;
		}
	}

	function endDrag() {
		if (dragNode) dragNode.fx = null;
		if (dragNode) dragNode.fy = null;
		dragNode = null;
		panning = false;
	}

	function onNodeClick(nd: Node) {
		if (moved) return; // it was a drag, not a click
		onSelect?.(nd.group, nd.kind === 'sub' ? (nd.sub ?? null) : null);
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		const rect = svgEl?.getBoundingClientRect();
		if (!rect) return;
		// Cursor in viewBox coords.
		const sx = ((e.clientX - rect.left) / rect.width) * W;
		const sy = ((e.clientY - rect.top) / rect.height) * H;
		const worldX = (sx - panX) / zoom;
		const worldY = (sy - panY) / zoom;
		const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
		const nz = Math.max(0.35, Math.min(5, zoom * factor));
		panX = sx - worldX * nz;
		panY = sy - worldY * nz;
		zoom = nz;
		onInteract?.();
	}

	function zoomBy(factor: number) {
		const nz = Math.max(0.35, Math.min(5, zoom * factor));
		// Zoom around the centre.
		const worldX = (W / 2 - panX) / zoom;
		const worldY = (H / 2 - panY) / zoom;
		panX = W / 2 - worldX * nz;
		panY = H / 2 - worldY * nz;
		zoom = nz;
		onInteract?.();
	}
	function resetView() {
		zoom = 1;
		panX = 0;
		panY = 0;
	}

	function fill(nd: Node): string {
		return nd.kind === 'group' ? `hsl(${nd.hue} 62% 52%)` : `hsl(${nd.hue} 52% 62%)`;
	}
</script>

<div class="relative">
	<!-- zoom controls -->
	<div class="absolute right-2 top-2 z-10 flex flex-col gap-1">
		<button
			type="button"
			class="h-7 w-7 rounded border bg-card text-sm shadow-sm hover:bg-muted"
			onclick={() => zoomBy(1.25)}
			aria-label="zoom in">+</button
		>
		<button
			type="button"
			class="h-7 w-7 rounded border bg-card text-sm shadow-sm hover:bg-muted"
			onclick={() => zoomBy(1 / 1.25)}
			aria-label="zoom out">−</button
		>
		<button
			type="button"
			class="h-7 w-7 rounded border bg-card text-[0.6rem] shadow-sm hover:bg-muted"
			onclick={resetView}
			aria-label="reset view">⟲</button
		>
	</div>

	{#if nodes.length === 0}
		<div
			class="flex h-[70vh] items-center justify-center rounded-lg border bg-card text-sm text-muted-foreground"
		>
			No PRs to group yet.
		</div>
	{:else}
		<svg
			bind:this={svgEl}
			viewBox="0 0 {W} {H}"
			class="h-[74vh] w-full touch-none select-none rounded-lg border bg-card"
			role="application"
			aria-label="PR groups network graph"
			onwheel={onWheel}
			onpointerdown={onBgDown}
			onpointermove={onMove}
			onpointerup={endDrag}
			onpointerleave={endDrag}
		>
			<!-- Transparent hit-area so background drag (pan) works over EMPTY space:
			     an inline SVG doesn't dispatch pointer events where nothing is painted. -->
			<rect x="0" y="0" width={W} height={H} fill="transparent" style="pointer-events: all" />
			<g transform="translate({panX} {panY}) scale({zoom})">
				{#each links as l (l.b.id)}
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
						onpointerdown={(e) => onNodeDown(nd, e)}
						onclick={() => onNodeClick(nd)}
						onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && onNodeClick(nd)}
						role="button"
						tabindex="-1"
					>
						<circle
							cx={nd.x}
							cy={nd.y}
							r={nd.r}
							fill={fill(nd)}
							stroke={selectedId === nd.id ? 'white' : `hsl(${nd.hue} 55% 35%)`}
							stroke-width={selectedId === nd.id ? 3 : nd.kind === 'group' ? 2 : 1}
							opacity={nd.kind === 'group' ? 0.92 : 0.85}
						/>
						<text
							x={nd.x}
							y={nd.kind === 'group' ? nd.y - nd.r - 4 : nd.y - nd.r - 3}
							text-anchor="middle"
							class="pointer-events-none fill-foreground font-medium"
							font-size={nd.kind === 'group' ? 15 : 11}
						>
							{nd.name}
							<tspan class="fill-muted-foreground">· {nd.count}</tspan>
						</text>
					</g>
				{/each}
			</g>
		</svg>
	{/if}
</div>
