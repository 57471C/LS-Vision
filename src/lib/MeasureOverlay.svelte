<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { createEventDispatcher } from "svelte";
import {
	finishLine,
	type ImageLine,
	type ImagePoint,
	imagePoint,
	type MeasureDraw,
} from "$lib/measure";

export let active = false;
export let imageWidth = 0;
export let imageHeight = 0;
export let line: ImageLine | null = null;
export let label = "";

const dispatch = createEventDispatcher<{ line: MeasureDraw }>();

let surface: HTMLDivElement | undefined;
let draft: ImageLine | null = null;
let drawing = false;
let origin: ImagePoint = { x: 0, y: 0 };

$: if (!active && drawing) {
	drawing = false;
	draft = null;
}
$: shown = draft ?? line;
$: labelLeft =
	shown && imageWidth > 0 ? ((shown.x1 + shown.x2) / 2 / imageWidth) * 100 : 0;
$: labelTop =
	shown && imageHeight > 0
		? ((shown.y1 + shown.y2) / 2 / imageHeight) * 100
		: 0;
$: labelAbove = labelTop > 14;

function pointOf(event: PointerEvent): ImagePoint | null {
	const node = surface;
	if (!node) return null;
	return imagePoint(
		event.clientX,
		event.clientY,
		node.getBoundingClientRect(),
		imageWidth,
		imageHeight,
	);
}

function onPointerDown(event: PointerEvent) {
	if (!active || event.button !== 0) return;
	const point = pointOf(event);
	if (!point) return;
	event.preventDefault();
	event.stopPropagation();
	drawing = true;
	origin = point;
	draft = { x1: point.x, y1: point.y, x2: point.x, y2: point.y };
	surface?.setPointerCapture(event.pointerId);
}

function onPointerMove(event: PointerEvent) {
	if (!drawing) return;
	const point = pointOf(event);
	if (!point) return;
	draft = { x1: origin.x, y1: origin.y, x2: point.x, y2: point.y };
}

function onPointerUp(event: PointerEvent) {
	if (!drawing) return;
	drawing = false;
	const point = pointOf(event) ?? {
		x: draft?.x2 ?? origin.x,
		y: draft?.y2 ?? origin.y,
	};
	draft = null;
	const finished = finishLine(origin, point);
	if (!finished) return;
	dispatch("line", {
		line: finished,
		width: imageWidth,
		height: imageHeight,
	});
}

function onPointerCancel() {
	drawing = false;
	draft = null;
}
</script>

<div
	bind:this={surface}
	class="absolute inset-0 touch-none"
	class:cursor-crosshair={active}
	class:pointer-events-none={!active}
	on:pointerdown={onPointerDown}
	on:pointermove={onPointerMove}
	on:pointerup={onPointerUp}
	on:pointercancel={onPointerCancel}
>
	{#if shown && imageWidth > 0 && imageHeight > 0}
		<svg
			class="pointer-events-none absolute inset-0 h-full w-full"
			viewBox="0 0 {imageWidth} {imageHeight}"
			preserveAspectRatio="none"
			aria-hidden="true"
		>
			<line
				x1={shown.x1}
				y1={shown.y1}
				x2={shown.x2}
				y2={shown.y2}
				stroke="#facc15"
				stroke-width="2"
				stroke-linecap="round"
				vector-effect="non-scaling-stroke"
			/>
			<circle cx={shown.x1} cy={shown.y1} r="3" fill="#facc15" />
			<circle cx={shown.x2} cy={shown.y2} r="3" fill="#facc15" />
		</svg>
		{#if label}
			<p
				class="pointer-events-none absolute z-[1] max-w-[90%] bg-neutral-950/90 px-2 py-1 text-xs font-semibold text-neutral-100 {labelAbove
					? '-translate-x-1/2 -translate-y-full'
					: '-translate-x-1/2 translate-y-1'}"
				style:left="{labelLeft}%"
				style:top="{labelTop}%"
			>
				{label}
			</p>
		{/if}
	{/if}
</div>
