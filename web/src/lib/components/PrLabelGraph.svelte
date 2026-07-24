<script lang="ts">
	/**
	 * Bipartite force-directed graph: each label is a hub node (radius ∝ number
	 * of PRs carrying it), each PR is a small satellite linked to every label it
	 * wears. Answers "how many PRs sit under label X" at a glance (e.g. codex).
	 *
	 * Dependency-free. PERF: the SVG structure is rendered ONCE per graph rebuild
	 * (keyed each on `graphVersion`); the rAF sim then moves elements
	 * IMPERATIVELY via setAttribute on refs stored on each node/link. Going back
	 * through Svelte's reactivity every frame (re-slicing deriveds + reconciling
	 * ~hundreds of nodes) is what made this jank in dev — this avoids it entirely.
	 * A tiny velocity-Verlet sim (charge + link spring + gravity) cools via alpha
	 * decay. O(n²) charge is fine for the few-hundred open PRs a repo carries.
	 */
	import { untrack } from 'svelte';
	import type { PullRequest } from '$lib/api';

	type Props = {
		pulls: PullRequest[];
		/** When non-empty, restrict the graph to these labels. */
		focus?: string[];
		colorFor: (label: string) => string;
		onSelectPr?: (pr: PullRequest) => void;
		onToggleLabel?: (label: string) => void;
	};
	let { pulls, focus = [], colorFor, onSelectPr, onToggleLabel }: Props = $props();

	const NODE_SOFT_CAP = 600;

	type Node = {
		id: string;
		kind: 'label' | 'pr';
		/** Label name (label nodes) or dominant label (pr nodes) — drives color. */
		label: string;
		text: string;
		r: number;
		mass: number;
		count: number;
		pr?: PullRequest;
		x: number;
		y: number;
		vx: number;
		vy: number;
		fx: number | null;
		fy: number | null;
		/** DOM ref for imperative positioning: <circle> for PRs, <g> for hubs. */
		el?: SVGGraphicsElement;
	};
	type Link = { source: Node; target: Node; el?: SVGLineElement };

	let width = $state(800);
	let height = $state(600);
	let view = $state({ k: 1, tx: 0, ty: 0 });
	/** Bumped only when the graph is rebuilt, to re-render the SVG structure.
	 *  NOT bumped per frame — positions move imperatively. */
	let graphVersion = $state(0);
	let truncated = $state(false);

	// Plain (non-reactive) sim state; `sim` is a const so array swaps never trip
	// Svelte reactivity. Rendering keys off graphVersion instead.
	const sim: { nodes: Node[]; links: Link[] } = { nodes: [], links: [] };
	let alpha = 0;
	let raf = 0;

	let renderNodes = $derived.by(() => {
		void graphVersion;
		return sim.nodes;
	});
	let renderLinks = $derived.by(() => {
		void graphVersion;
		return sim.links;
	});

	function labelRadius(count: number): number {
		return Math.min(46, 12 + Math.sqrt(count) * 6);
	}

	/** PR satellite radius grows with its 👍 (+1) reaction count. sqrt keeps a
	 *  25-vote PR readable next to a 0-vote one without dwarfing the hubs. */
	function prRadius(votes: number): number {
		return Math.min(22, 5 + Math.sqrt(Math.max(0, votes)) * 2.4);
	}

	function dominantLabel(prLabels: string[], counts: Map<string, number>): string {
		let best = prLabels[0];
		let bestC = -1;
		for (const l of prLabels) {
			const c = counts.get(l) ?? 0;
			if (c > bestC) {
				bestC = c;
				best = l;
			}
		}
		return best;
	}

	// Rebuild only when the DATA or FOCUS changes — NOT on resize. Tracking
	// width/height here would rebuild + reheat the whole graph on every 1px
	// container jitter (scrollbar, HMR), which reads as constant lag; the sim
	// re-centers on live width/height inside tick() instead.
	$effect(() => {
		void pulls;
		void focus;
		return untrack(() => rebuild());
	});

	function rebuild() {
		const focusSet = new Set(focus);
		const active = (l: string) => focusSet.size === 0 || focusSet.has(l);

		const counts = new Map<string, number>();
		for (const pr of pulls) {
			for (const l of pr.labels) if (active(l)) counts.set(l, (counts.get(l) ?? 0) + 1);
		}

		const cx = width / 2;
		const cy = height / 2;
		const labelNames = [...counts.keys()];
		const labelNode = new Map<string, Node>();
		labelNames.forEach((name, i) => {
			const ang = (i / Math.max(1, labelNames.length)) * Math.PI * 2;
			labelNode.set(name, {
				id: `label:${name}`,
				kind: 'label',
				label: name,
				text: name,
				r: labelRadius(counts.get(name) ?? 0),
				mass: 6,
				count: counts.get(name) ?? 0,
				x: cx + Math.cos(ang) * 120,
				y: cy + Math.sin(ang) * 120,
				vx: 0,
				vy: 0,
				fx: null,
				fy: null
			});
		});

		const nextNodes: Node[] = [...labelNode.values()];
		const nextLinks: Link[] = [];
		let dropped = false;
		for (const pr of pulls) {
			const own = pr.labels.filter(active);
			if (own.length === 0) continue;
			if (nextNodes.length >= NODE_SOFT_CAP) {
				dropped = true;
				break;
			}
			const dom = dominantLabel(own, counts);
			const seed = labelNode.get(dom)!;
			const votes = pr.reactions_plus1 ?? 0;
			const prNode: Node = {
				id: `pr:${pr.repo}#${pr.number}`,
				kind: 'pr',
				label: dom,
				text: `#${pr.number}`,
				r: prRadius(votes),
				mass: 1 + Math.sqrt(votes) * 0.15,
				count: votes,
				pr,
				x: seed.x + (Math.random() - 0.5) * 60,
				y: seed.y + (Math.random() - 0.5) * 60,
				vx: 0,
				vy: 0,
				fx: null,
				fy: null
			};
			nextNodes.push(prNode);
			for (const l of own) {
				const ln = labelNode.get(l);
				if (ln) nextLinks.push({ source: prNode, target: ln });
			}
		}

		sim.nodes = nextNodes;
		sim.links = nextLinks;
		truncated = dropped;
		alpha = 0.85;
		graphVersion++;
		startSim();

		return () => {
			if (raf) cancelAnimationFrame(raf);
			raf = 0;
		};
	}

	function startSim() {
		if (raf) return;
		const loop = () => {
			tick();
			paint();
			if (alpha > 0.02) {
				raf = requestAnimationFrame(loop);
			} else {
				raf = 0;
			}
		};
		raf = requestAnimationFrame(loop);
	}

	function tick() {
		const cx = width / 2;
		const cy = height / 2;
		const N = sim.nodes;
		const n = N.length;
		if (n === 0) return;

		for (let i = 0; i < n; i++) {
			const a = N[i];
			for (let j = i + 1; j < n; j++) {
				const b = N[j];
				let dx = a.x - b.x;
				let dy = a.y - b.y;
				let d2 = dx * dx + dy * dy;
				if (d2 < 1) {
					dx = Math.random() - 0.5;
					dy = Math.random() - 0.5;
					d2 = 1;
				}
				const chargeA = a.kind === 'label' ? 1400 : 240;
				const chargeB = b.kind === 'label' ? 1400 : 240;
				const d = Math.sqrt(d2);
				const f = (Math.sqrt(chargeA * chargeB) / d2) * alpha;
				const fx = (dx / d) * f;
				const fy = (dy / d) * f;
				a.vx += fx / a.mass;
				a.vy += fy / a.mass;
				b.vx -= fx / b.mass;
				b.vy -= fy / b.mass;
			}
		}

		for (const l of sim.links) {
			const s = l.source;
			const t = l.target;
			const dx = t.x - s.x;
			const dy = t.y - s.y;
			const d = Math.sqrt(dx * dx + dy * dy) || 1;
			const rest = t.r + 42;
			const f = (d - rest) * 0.06 * alpha;
			const fx = (dx / d) * f;
			const fy = (dy / d) * f;
			s.vx += fx / s.mass;
			s.vy += fy / s.mass;
			t.vx -= fx / t.mass;
			t.vy -= fy / t.mass;
		}

		for (const nd of N) {
			nd.vx += (cx - nd.x) * 0.015 * alpha;
			nd.vy += (cy - nd.y) * 0.015 * alpha;
			if (nd.fx !== null) {
				nd.x = nd.fx;
				nd.vx = 0;
			} else {
				nd.vx *= 0.82;
				nd.x += nd.vx;
			}
			if (nd.fy !== null) {
				nd.y = nd.fy;
				nd.vy = 0;
			} else {
				nd.vy *= 0.82;
				nd.y += nd.vy;
			}
		}

		// Hard collision: separate any overlapping pair by their radii (+pad),
		// distributing the push by inverse mass so PR satellites yield to the
		// heavier hubs. Position-based (like d3 forceCollide) — resolves overlap
		// the charge force alone leaves when the link spring packs nodes onto a
		// hub. Runs regardless of alpha so the final frame is overlap-free.
		const PAD = 3;
		for (let i = 0; i < n; i++) {
			const a = N[i];
			for (let j = i + 1; j < n; j++) {
				const b = N[j];
				const dx = b.x - a.x;
				const dy = b.y - a.y;
				const d = Math.sqrt(dx * dx + dy * dy) || 0.01;
				const min = a.r + b.r + PAD;
				if (d < min) {
					const ux = dx / d;
					const uy = dy / d;
					const overlap = min - d;
					const wa = a.fx !== null ? 0 : b.mass / (a.mass + b.mass);
					const wb = b.fx !== null ? 0 : a.mass / (a.mass + b.mass);
					a.x -= ux * overlap * wa;
					a.y -= uy * overlap * wa;
					b.x += ux * overlap * wb;
					b.y += uy * overlap * wb;
				}
			}
		}

		alpha *= 0.975;
	}

	/** Push positions to the DOM imperatively — no framework reconciliation. */
	function paint() {
		for (const nd of sim.nodes) {
			const el = nd.el;
			if (!el) continue;
			if (nd.kind === 'pr') {
				el.setAttribute('cx', String(nd.x));
				el.setAttribute('cy', String(nd.y));
			} else {
				el.setAttribute('transform', `translate(${nd.x} ${nd.y})`);
			}
		}
		for (const l of sim.links) {
			const el = l.el;
			if (!el) continue;
			el.setAttribute('x1', String(l.source.x));
			el.setAttribute('y1', String(l.source.y));
			el.setAttribute('x2', String(l.target.x));
			el.setAttribute('y2', String(l.target.y));
		}
	}

	// ---- Interaction: pan / zoom / node drag ----------------------------------
	let svgEl: SVGSVGElement;
	let dragNode: Node | null = null;
	let panning = $state(false);
	let pointerStart = { x: 0, y: 0 };
	let moved = false;

	function toGraph(clientX: number, clientY: number) {
		const rect = svgEl.getBoundingClientRect();
		return {
			x: (clientX - rect.left - view.tx) / view.k,
			y: (clientY - rect.top - view.ty) / view.k
		};
	}

	function onWheel(e: WheelEvent) {
		e.preventDefault();
		const rect = svgEl.getBoundingClientRect();
		const px = e.clientX - rect.left;
		const py = e.clientY - rect.top;
		const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
		const k = Math.min(4, Math.max(0.2, view.k * factor));
		view = {
			k,
			tx: px - ((px - view.tx) * k) / view.k,
			ty: py - ((py - view.ty) * k) / view.k
		};
	}

	function nodePointerDown(e: PointerEvent, node: Node) {
		e.stopPropagation();
		(e.target as Element).setPointerCapture?.(e.pointerId);
		dragNode = node;
		moved = false;
		pointerStart = { x: e.clientX, y: e.clientY };
		const g = toGraph(e.clientX, e.clientY);
		node.fx = g.x;
		node.fy = g.y;
	}

	function bgPointerDown(e: PointerEvent) {
		panning = true;
		moved = false;
		pointerStart = { x: e.clientX, y: e.clientY };
	}

	function onPointerMove(e: PointerEvent) {
		if (Math.abs(e.clientX - pointerStart.x) + Math.abs(e.clientY - pointerStart.y) > 4) {
			moved = true;
		}
		if (dragNode) {
			const g = toGraph(e.clientX, e.clientY);
			dragNode.fx = g.x;
			dragNode.fy = g.y;
			alpha = Math.max(alpha, 0.3);
			startSim();
		} else if (panning) {
			view = { ...view, tx: view.tx + e.movementX, ty: view.ty + e.movementY };
		}
	}

	function onPointerUp(node?: Node) {
		if (dragNode) {
			dragNode.fx = null;
			dragNode.fy = null;
			dragNode = null;
		}
		panning = false;
		if (!moved && node) {
			if (node.kind === 'pr' && node.pr) onSelectPr?.(node.pr);
			else if (node.kind === 'label') onToggleLabel?.(node.label);
		}
	}
</script>

<div class="relative h-full w-full" bind:clientWidth={width} bind:clientHeight={height}>
	{#if truncated}
		<div
			class="absolute left-2 top-2 z-10 rounded border border-yellow-500/40 bg-yellow-500/10 px-2 py-1 text-[0.7rem] text-yellow-600 dark:text-yellow-400"
		>
			Showing first {NODE_SOFT_CAP} nodes — narrow the filter to see the rest.
		</div>
	{/if}
	<svg
		bind:this={svgEl}
		class="h-full w-full touch-none select-none"
		style="cursor: {panning ? 'grabbing' : 'grab'}"
		onwheel={onWheel}
		onpointerdown={bgPointerDown}
		onpointermove={onPointerMove}
		onpointerup={() => onPointerUp()}
		role="application"
		aria-label="Pull request label network graph"
	>
		<g transform="translate({view.tx} {view.ty}) scale({view.k})">
			{#each renderLinks as l (l.source.id + '->' + l.target.id)}
				<line
					bind:this={l.el}
					x1={l.source.x}
					y1={l.source.y}
					x2={l.target.x}
					y2={l.target.y}
					stroke="var(--border)"
					stroke-width={0.6}
					opacity={0.5}
				/>
			{/each}

			{#each renderNodes as node (node.id)}
				{#if node.kind === 'pr'}
					<circle
						bind:this={node.el}
						cx={node.x}
						cy={node.y}
						r={node.r}
						fill={colorFor(node.label)}
						fill-opacity={0.85}
						stroke="var(--background)"
						stroke-width={1}
						style="cursor: pointer"
						onpointerdown={(e) => nodePointerDown(e, node)}
						onpointerup={() => onPointerUp(node)}
						role="button"
						tabindex="-1"
					>
						<title>#{node.pr?.number} — {node.pr?.title}{node.count ? ` · 👍 ${node.count}` : ''}</title>
					</circle>
				{:else}
					<g
						bind:this={node.el}
						transform="translate({node.x} {node.y})"
						style="cursor: pointer"
						onpointerdown={(e) => nodePointerDown(e, node)}
						onpointerup={() => onPointerUp(node)}
						role="button"
						tabindex="-1"
					>
						<circle
							r={node.r}
							fill={colorFor(node.label)}
							fill-opacity={0.2}
							stroke={colorFor(node.label)}
							stroke-width={2}
						/>
						<text
							y={-2}
							text-anchor="middle"
							font-size="12"
							font-weight="600"
							fill="var(--foreground)"
							style="pointer-events: none">{node.text}</text
						>
						<text
							y={12}
							text-anchor="middle"
							font-size="11"
							fill="var(--muted-foreground)"
							style="pointer-events: none">{node.count}</text
						>
					</g>
				{/if}
			{/each}
		</g>
	</svg>
</div>
