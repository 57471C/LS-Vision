<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { type DetectRoi, overlayDetections, slotColor } from "$lib/detect-rois";
import {
	detectionCounts,
	extraPartCount,
	hasUnexpected,
} from "$lib/operator/verdict";
import { useOperator } from "$lib/stores/operator";

const {
	barcodeOverlay,
	cameraFeedOn,
	camerasMissing,
	onRoiPointerDown,
	onRoiPointerMove,
	onRoiPointerUp,
	operator,
	refs,
	rememberPaneSize,
	resumeLiveFeed,
	toggleDetectSlot,
	zoom,
} = useOperator();

type SnapshotView = {
	kitCode: string;
	image: string;
	takenAt: string;
	detected?: boolean;
};

type Detection = {
	ic: string;
	confidence: number;
	bbox: number[];
	consumed?: boolean;
	slot?: number;
};

type DetectionMark = {
	key: string;
	ic: string;
	color: string;
	x: number;
	y: number;
	w: number;
	h: number;
	labelX: number;
	labelY: number;
	labelW: number;
	labelH: number;
	unexpected: boolean;
	child: boolean;
	slot?: number;
};

// OpenCV stored these as BGR.
const PART_COLORS = [
	"rgb(60, 180, 0)",
	"rgb(255, 165, 0)",
	"rgb(0, 140, 255)",
	"rgb(230, 60, 60)",
	"rgb(200, 60, 200)",
	"rgb(200, 200, 0)",
	"rgb(0, 180, 180)",
	"rgb(255, 100, 0)",
];
const BOX_STROKE = 2;
const CHILD_STROKE = 1;
const LABEL_FONT = 12;
const LABEL_PAD_X = 4;
const LABEL_PAD_Y = 3;

function imageLoaded(event: Event) {
	const image =
		event.currentTarget instanceof HTMLImageElement
			? event.currentTarget
			: null;
	rememberPaneSize(image);
}

function colorForIc(ic: string): string {
	const bytes = new TextEncoder().encode(ic);
	let total = 0;
	for (const byte of bytes) total += byte;
	return PART_COLORS[total % PART_COLORS.length];
}

function detectionMarks(
	snapshot: SnapshotView | null,
	paneWidth: number,
	paneHeight: number,
	frameWidth: number,
	frameHeight: number,
	detections: Detection[],
	zoom: {
		width: number;
		height: number;
	} | null,
	bomParts: { ic: string }[] | null,
	admin: boolean,
): DetectionMark[] {
	// Positions are CSS pixels of the still. The viewBox matches that size,
	// so the stroke and the label font do not grow when the still is scaled.
	if (!snapshot || paneWidth < 1 || paneHeight < 1 || detections.length === 0)
		return [];
	let scale = 1;
	let originX = 0;
	let originY = 0;
	if (zoom) {
		scale = zoom.width / paneWidth;
	} else if (frameWidth > 0 && frameHeight > 0) {
		scale = Math.min(frameWidth / paneWidth, frameHeight / paneHeight);
		originX = (frameWidth - paneWidth * scale) / 2;
		originY = (frameHeight - paneHeight * scale) / 2;
	} else {
		return [];
	}
	if (!Number.isFinite(scale) || scale <= 0) return [];
	const imageRight = originX + paneWidth * scale;
	const imageBottom = originY + paneHeight * scale;
	const marks: DetectionMark[] = [];
	const known = bomParts ? new Set(bomParts.map((part) => part.ic)) : null;
	const ordered = [...detections].sort((a, b) => a.confidence - b.confidence);
	for (const [index, item] of ordered.entries()) {
		// Operators do not see a child that a matching rule already used.
		if (item.consumed && !admin) continue;
		const box = item.bbox;
		if (box.length < 4) continue;
		const [rawX1, rawY1, rawX2, rawY2] = box;
		if (
			![rawX1, rawY1, rawX2, rawY2].every(
				(value) => typeof value === "number" && Number.isFinite(value),
			)
		) {
			continue;
		}
		const x1 = Math.max(0, Math.min(paneWidth, rawX1));
		const y1 = Math.max(0, Math.min(paneHeight, rawY1));
		const x2 = Math.max(0, Math.min(paneWidth, rawX2));
		const y2 = Math.max(0, Math.min(paneHeight, rawY2));
		if (x2 <= x1 || y2 <= y1) continue;
		const x = originX + x1 * scale;
		const y = originY + y1 * scale;
		const w = (x2 - x1) * scale;
		const h = (y2 - y1) * scale;
		const labelW =
			Math.ceil(item.ic.length * LABEL_FONT * 0.62) + LABEL_PAD_X * 2;
		const labelH = LABEL_FONT + LABEL_PAD_Y * 2;
		let labelX = x;
		let labelY = y - labelH;
		if (labelY < originY) labelY = y;
		if (labelX + labelW > imageRight)
			labelX = Math.max(originX, imageRight - labelW);
		if (labelY + labelH > imageBottom)
			labelY = Math.max(originY, imageBottom - labelH);
		const tint = item.slot ? slotColor(item.slot).stroke : "";
		marks.push({
			key: `${item.slot ?? 0}:${index}:${item.ic}:${x1}:${y1}`,
			ic: item.ic,
			color: tint || colorForIc(item.ic),
			slot: item.slot,
			x,
			y,
			w,
			h,
			labelX,
			labelY,
			labelW,
			labelH,
			unexpected:
				!item.consumed &&
				known !== null &&
				item.ic !== "" &&
				!known.has(item.ic),
			child: item.consumed === true,
		});
	}
	return marks;
}

function slotSquareClass(roi: DetectRoi | null): string {
	if (!roi) return "border-neutral-700 bg-neutral-800 text-neutral-500";
	if (roi.enabled) return "border-green-400 bg-green-500 text-neutral-950";
	return "border-red-500 bg-red-600 text-white";
}

function paintedSlots(
	slots: readonly (DetectRoi | null)[],
): { slot: number; roi: DetectRoi }[] {
	const found: { slot: number; roi: DetectRoi }[] = [];
	slots.forEach((roi, index) => {
		if (roi) found.push({ slot: index + 1, roi });
	});
	// Every saved slot is off, so the outlines stay hidden.
	if (found.length > 0 && found.every((item) => !item.roi.enabled)) return [];
	return found;
}

$: bannerParts = $operator.bom?.parts ?? [];
$: overlayBoxes = overlayDetections(
	$operator.detections,
	$operator.slotResults,
);
$: bannerCounts = detectionCounts($operator.detections);
$: extraParts =
	$operator.detectState !== "done"
		? 0
		: $operator.slotResults.length > 0
			? $operator.slotResults.reduce((max, slot) => {
					return Math.max(
						max,
						extraPartCount(bannerParts, detectionCounts(slot.detections)),
					);
				}, 0)
			: extraPartCount(bannerParts, bannerCounts);
$: wrongParts =
	$operator.detectState === "done" &&
	($operator.slotResults.length > 0
		? $operator.slotResults.some((slot) =>
				hasUnexpected(slot.detections, bannerParts),
			)
		: hasUnexpected($operator.detections, bannerParts));
$: draftSlot = slotColor($operator.detectSlot);
$: draftStyle = $operator.draftMark
	? `left:${$operator.draftMark.left}px;top:${$operator.draftMark.top}px;width:${$operator.draftMark.width}px;height:${$operator.draftMark.height}px;box-sizing:border-box;border:${
			$operator.barcodeDrawing
				? "3px dotted #fde68a"
				: `3px solid ${draftSlot.bright}`
		};background:${
			$operator.barcodeDrawing ? "rgba(253,230,138,0.2)" : draftSlot.fill
		};box-shadow:0 0 0 1px #0a0a0a;`
	: "";
$: bannerCount = (wrongParts ? 1 : 0) + (extraParts > 0 ? 1 : 0);
</script>

<section class="flex min-h-0 min-w-0 flex-col overflow-hidden border border-neutral-700 bg-black">
	<div class="flex shrink-0 items-center justify-between border-b border-neutral-800 px-3 py-1.5">
		<div class="flex items-center gap-2">
			<h2 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
				{$operator.snapshot ? "Snapshot" : "Camera"}
			</h2>
			{#if !$operator.admin && $operator.detectRois.some((roi) => roi)}
				<div class="flex items-center gap-1" role="group" aria-label="Detect slots">
					{#each [1, 2, 3, 4] as slot (slot)}
						{@const box = $operator.detectRois[slot - 1]}
						{#if box}
							<button
								type="button"
								class="inline-flex h-6 w-6 cursor-pointer items-center justify-center border text-[11px] font-semibold {slotSquareClass(box)}"
								aria-pressed={box.enabled}
								aria-label="Slot {slot}"
								on:click={() => toggleDetectSlot(slot)}
							>
								{slot}
							</button>
						{:else}
							<span
								class="inline-flex h-6 w-6 cursor-default items-center justify-center border border-neutral-700 bg-neutral-800 text-[11px] font-semibold text-neutral-500"
								aria-hidden="true"
							>
								{slot}
							</span>
						{/if}
					{/each}
				</div>
			{/if}
		</div>
		{#if $operator.roiMessage}
			<p class="text-xs text-amber-200">{$operator.roiMessage}</p>
		{:else if $operator.snapshot}
			<p class="text-xs text-amber-200">Automatic stream is stopped</p>
		{/if}
	</div>
	<div bind:this={refs.paneFrame} class="relative min-h-0 flex-1 overflow-hidden">
		{#if $operator.snapshot || ($operator.feedOn && $cameraFeedOn)}
			<div
				class="absolute {$zoom ? '' : 'inset-0'}"
				style={$zoom
					? `left:${$zoom.left}px;top:${$zoom.top}px;width:${$zoom.width}px;height:${$zoom.height}px;`
					: ""}
			>
				{#if $operator.feedOn && $cameraFeedOn}
					<canvas
						bind:this={refs.liveCanvas}
						class="h-full w-full {$zoom ? 'object-fill' : 'object-contain'}"
						class:invisible={Boolean($operator.snapshot) || !$operator.livePainted}
						aria-label="Camera feed"
					></canvas>
					{#if !$operator.snapshot && ($operator.liveMessage || !$operator.livePainted)}
						<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
							{$operator.liveMessage || "Opening camera…"}
						</p>
					{/if}
				{/if}
				{#if $operator.snapshot}
					<img
						bind:this={refs.paneImage}
						src={$operator.snapshot.image}
						alt={$operator.snapshot.detected
							? `Detected parts for kit ${$operator.snapshot.kitCode}`
							: `Snapshot of kit ${$operator.snapshot.kitCode}`}
						class="{$operator.feedOn && $cameraFeedOn
							? 'absolute inset-0'
							: ''} h-full w-full {$zoom ? 'object-fill' : 'object-contain'}"
						on:load={imageLoaded}
					/>
				{/if}
				{#if ($operator.snapshot || $cameraFeedOn) && $operator.paneWidth > 0 && $operator.paneHeight > 0}
					{@const slots = paintedSlots($operator.detectRois)}
					{#if slots.length > 0}
						<svg
							class="pointer-events-none absolute inset-0 z-10 h-full w-full"
							viewBox="0 0 {$operator.paneWidth} {$operator.paneHeight}"
							preserveAspectRatio={$zoom ? "none" : "xMidYMid meet"}
						>
							{#if slots.length === 1 && $zoom}
								{@const only = slots[0].roi}
								{@const x = only.x * $operator.paneWidth}
								{@const y = only.y * $operator.paneHeight}
								{@const w = only.w * $operator.paneWidth}
								{@const h = only.h * $operator.paneHeight}
								<rect x="0" y="0" width={$operator.paneWidth} height={y} fill="black" fill-opacity="0.45" />
								<rect
									x="0"
									y={y + h}
									width={$operator.paneWidth}
									height={$operator.paneHeight - (y + h)}
									fill="black"
									fill-opacity="0.45"
								/>
								<rect x="0" y={y} width={x} height={h} fill="black" fill-opacity="0.45" />
								<rect
									x={x + w}
									y={y}
									width={$operator.paneWidth - (x + w)}
									height={h}
									fill="black"
									fill-opacity="0.45"
								/>
							{/if}
							{#each slots as item (item.slot)}
								{@const x = item.roi.x * $operator.paneWidth}
								{@const y = item.roi.y * $operator.paneHeight}
								{@const w = item.roi.w * $operator.paneWidth}
								{@const h = item.roi.h * $operator.paneHeight}
								{@const color = slotColor(item.slot)}
								{@const active = item.slot === $operator.detectSlot}
								<rect
									x={x}
									y={y}
									width={w}
									height={h}
									fill="none"
									stroke={active ? color.bright : color.stroke}
									stroke-opacity={active ? 1 : 0.9}
									stroke-width={active ? 3 : 2}
									stroke-dasharray={active ? undefined : "6 4"}
									vector-effect="non-scaling-stroke"
								/>
								<rect x={x} y={y} width="18" height="16" fill={color.stroke} />
								<text
									x={x + 9}
									y={y + 12}
									fill="#0a0a0a"
									font-size="11"
									font-family="ui-monospace, monospace"
									font-weight="700"
									text-anchor="middle"
								>
									{item.slot}
								</text>
							{/each}
						</svg>
					{/if}
				{/if}
				{#if !$operator.snapshot && !$operator.drawing && !$operator.barcodeDrawing && $cameraFeedOn && $operator.paneWidth > 0 && $operator.paneHeight > 0}
					{@const box = $barcodeOverlay}
					{#if box}
						<svg
							class="pointer-events-none absolute inset-0 z-10 h-full w-full"
							viewBox="0 0 {$operator.paneWidth} {$operator.paneHeight}"
							preserveAspectRatio="xMidYMid meet"
						>
							<rect
								x={box.x}
								y={box.y}
								width={box.width}
								height={box.height}
								fill="none"
								stroke="#67e8f9"
								stroke-width="2"
								vector-effect="non-scaling-stroke"
							/>
						</svg>
					{/if}
				{/if}
				{#if $operator.snapshot && $operator.paneWidth > 0 && $operator.paneHeight > 0 && overlayBoxes.length > 0}
					{@const marks = detectionMarks($operator.snapshot, $operator.paneWidth, $operator.paneHeight, $operator.frameWidth, $operator.frameHeight, overlayBoxes, $zoom, $operator.bom?.parts ?? null, $operator.admin)}
					{@const viewW = $zoom ? $zoom.width : $operator.frameWidth}
					{@const viewH = $zoom ? $zoom.height : $operator.frameHeight}
					{#if marks.length > 0 && viewW > 0 && viewH > 0}
						<svg
							class="pointer-events-none absolute left-0 top-0 z-10"
							width={viewW}
							height={viewH}
							viewBox="0 0 {viewW} {viewH}"
							preserveAspectRatio="none"
							aria-hidden="true"
						>
							{#each marks as mark (mark.key)}
								{#if mark.child}
									<rect
										x={mark.x}
										y={mark.y}
										width={mark.w}
										height={mark.h}
										fill="none"
										stroke="#d4d4d4"
										stroke-opacity="0.55"
										stroke-width={CHILD_STROKE}
										data-part-role="child"
									/>
									<text
										x={mark.x + LABEL_PAD_X}
										y={mark.y + LABEL_FONT}
										fill="#d4d4d4"
										fill-opacity="0.7"
										font-size={LABEL_FONT}
										font-family="ui-monospace, monospace"
										font-weight="600"
										data-part-role="child"
									>
										{mark.ic}
									</text>
								{:else}
									{#if mark.unexpected}
										<line
											x1={mark.x}
											y1={mark.y}
											x2={mark.x + mark.w}
											y2={mark.y + mark.h}
											stroke="#dc2626"
											stroke-width={BOX_STROKE}
										/>
										<line
											x1={mark.x + mark.w}
											y1={mark.y}
											x2={mark.x}
											y2={mark.y + mark.h}
											stroke="#dc2626"
											stroke-width={BOX_STROKE}
										/>
									{:else}
										<rect
											x={mark.x}
											y={mark.y}
											width={mark.w}
											height={mark.h}
											fill="none"
											stroke={mark.color}
											stroke-width={BOX_STROKE}
										/>
									{/if}
									<rect
										x={mark.labelX}
										y={mark.labelY}
										width={mark.labelW}
										height={mark.labelH}
										fill={mark.unexpected ? "#dc2626" : mark.color}
									/>
									<text
										x={mark.labelX + LABEL_PAD_X}
										y={mark.labelY + mark.labelH / 2}
										fill="#ffffff"
										font-size={LABEL_FONT}
										font-family="ui-monospace, monospace"
										font-weight="600"
										dominant-baseline="central"
									>
										{mark.ic}
									</text>
								{/if}
								{#if mark.slot}
									<rect
										x={mark.x}
										y={mark.y}
										width="14"
										height="14"
										fill={slotColor(mark.slot).stroke}
									/>
									<text
										x={mark.x + 7}
										y={mark.y + 11}
										fill="#0a0a0a"
										font-size="10"
										font-family="ui-monospace, monospace"
										font-weight="700"
										text-anchor="middle"
									>
										{mark.slot}
									</text>
								{/if}
							{/each}
						</svg>
					{/if}
				{/if}
			</div>
		{:else if $operator.feedOn && $camerasMissing}
			<div
				class="absolute inset-0 flex items-center justify-center px-6 text-center text-lg font-semibold text-amber-200"
				role="status"
			>
				No cameras found
			</div>
		{:else if $operator.feedOn && !$operator.camerasReady}
			<div class="absolute inset-0 flex items-center justify-center text-lg text-neutral-400">
				Looking for cameras...
			</div>
		{:else if !$operator.feedOn}
			<div class="absolute inset-0 flex items-center justify-center text-lg text-neutral-500">
				Feed stopped
			</div>
		{/if}
		{#if $operator.snapshot}
			<div
				class="pointer-events-none absolute left-3 top-3 z-20 bg-neutral-950/80 {$zoom
					? 'px-2 py-1'
					: 'px-3 py-2'}"
			>
				<p
					class="font-mono font-semibold leading-none text-white {$zoom
						? 'text-sm'
						: 'text-3xl'}"
				>
					{$operator.snapshot.kitCode}
				</p>
				<p class="mt-1 text-neutral-300 {$zoom ? 'text-[11px]' : 'text-sm'}">
					Taken {$operator.snapshot.takenAt}
				</p>
			</div>
			{#if $operator.detectState === "running"}
				<p class="pointer-events-none absolute bottom-3 left-3 z-40 bg-neutral-950/80 px-3 py-2 text-sm font-semibold text-amber-200">
					Detecting parts...
				</p>
			{/if}
			{#if bannerCount > 0}
				<div class="pointer-events-none absolute inset-x-0 bottom-0 z-30 flex flex-col">
					{#if wrongParts}
						<p class="flex h-14 items-center justify-center bg-red-600 px-4 text-center text-4xl font-black leading-none text-white">
							Wrong parts detected
						</p>
					{/if}
					{#if extraParts > 0}
						<p class="flex h-14 items-center justify-center bg-amber-400 px-4 text-center text-4xl font-black leading-none text-neutral-950">
							Too many parts ({extraParts} extra)
						</p>
					{/if}
				</div>
			{/if}
			<button
				type="button"
				class="absolute right-3 z-40 inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950 {bannerCount === 2
					? 'bottom-[7.75rem]'
					: bannerCount === 1
						? 'bottom-[4.25rem]'
						: 'bottom-3'}"
				on:click={resumeLiveFeed}
			>
				{$operator.feedOn ? "Resume live feed" : "Back to feed"}
			</button>
		{/if}
		{#if ($operator.drawing && $operator.snapshot) || ($operator.barcodeDrawing && !$operator.snapshot)}
			<div
				class="absolute inset-0 z-30 cursor-crosshair"
				on:pointerdown={onRoiPointerDown}
				on:pointermove={onRoiPointerMove}
				on:pointerup={onRoiPointerUp}
				on:pointercancel={onRoiPointerUp}
			></div>
			<p class="pointer-events-none absolute left-3 top-3 z-40 bg-neutral-950/80 px-3 py-2 text-sm text-amber-200">
				{$operator.barcodeDrawing
					? "Drag a rectangle on the camera."
					: `Drag a rectangle for slot ${$operator.detectSlot}.`}
			</p>
			{#if $operator.draftMark}
				<div class="pointer-events-none absolute z-50" style={draftStyle}></div>
			{/if}
		{/if}
	</div>
</section>
