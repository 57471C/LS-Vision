<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

type SnapshotView = {
	kitCode: string;
	image: string;
	takenAt: string;
	detected?: boolean;
};

type DetectRoi = {
	x: number;
	y: number;
	w: number;
	h: number;
};

type BarcodeRoi = {
	x: number;
	y: number;
	width: number;
	height: number;
};

type Detection = {
	ic: string;
	confidence: number;
	bbox: number[];
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
const LABEL_FONT = 12;
const LABEL_PAD_X = 4;
const LABEL_PAD_Y = 3;

export let paneFrame: HTMLElement | null = null;
export let liveCanvas: HTMLCanvasElement | undefined = undefined;
export let paneImage: HTMLImageElement | null = null;
export let snapshot: SnapshotView | null = null;
export let feedOn = false;
export let cameraFeedOn = false;
export let camerasMissing = false;
export let camerasReady = false;
export let liveMessage = "";
export let livePainted = false;
export let zoom: {
	left: number;
	top: number;
	width: number;
	height: number;
} | null = null;
export let paneWidth = 0;
export let paneHeight = 0;
export let frameWidth = 0;
export let frameHeight = 0;
export let detectRoi: DetectRoi | null = null;
export let drawing = false;
export let barcodeDrawing = false;
export let barcodeOverlay: BarcodeRoi | null = null;
export let detections: Detection[] = [];
export let detectState: "idle" | "running" | "done" = "idle";
export let draftMark: {
	left: number;
	top: number;
	width: number;
	height: number;
} | null = null;
export let roiMessage = "";
export let onImageLoad: (image: HTMLImageElement | null) => void = () => {};
export let onResume: () => void = () => {};
export let onPointerDown: (event: PointerEvent) => void = () => {};
export let onPointerMove: (event: PointerEvent) => void = () => {};
export let onPointerUp: (event: PointerEvent) => void = () => {};

function imageLoaded(event: Event) {
	const image =
		event.currentTarget instanceof HTMLImageElement
			? event.currentTarget
			: null;
	onImageLoad(image);
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
	const ordered = [...detections].sort((a, b) => a.confidence - b.confidence);
	for (const [index, item] of ordered.entries()) {
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
		marks.push({
			key: `${index}:${item.ic}:${x1}:${y1}`,
			ic: item.ic,
			color: colorForIc(item.ic),
			x,
			y,
			w,
			h,
			labelX,
			labelY,
			labelW,
			labelH,
		});
	}
	return marks;
}
</script>

<section class="flex min-h-0 min-w-0 flex-col overflow-hidden border border-neutral-700 bg-black">
	<div class="flex shrink-0 items-center justify-between border-b border-neutral-800 px-3 py-1.5">
		<h2 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
			{snapshot ? "Snapshot" : "Camera"}
		</h2>
		{#if roiMessage}
			<p class="text-xs text-amber-200">{roiMessage}</p>
		{:else if snapshot}
			<p class="text-xs text-amber-200">Automatic stream is stopped</p>
		{/if}
	</div>
	<div bind:this={paneFrame} class="relative min-h-0 flex-1 overflow-hidden">
		{#if snapshot || (feedOn && cameraFeedOn)}
			<div
				class="absolute {zoom ? '' : 'inset-0'}"
				style={zoom
					? `left:${zoom.left}px;top:${zoom.top}px;width:${zoom.width}px;height:${zoom.height}px;`
					: ""}
			>
				{#if feedOn && cameraFeedOn}
					<canvas
						bind:this={liveCanvas}
						class="h-full w-full {zoom ? 'object-fill' : 'object-contain'}"
						class:invisible={Boolean(snapshot) || !livePainted}
						aria-label="Camera feed"
					></canvas>
					{#if !snapshot && (liveMessage || !livePainted)}
						<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
							{liveMessage || "Opening camera…"}
						</p>
					{/if}
				{/if}
				{#if snapshot}
					<img
						bind:this={paneImage}
						src={snapshot.image}
						alt={snapshot.detected
							? `Detected parts for kit ${snapshot.kitCode}`
							: `Snapshot of kit ${snapshot.kitCode}`}
						class="{feedOn && cameraFeedOn
							? 'absolute inset-0'
							: ''} h-full w-full {zoom ? 'object-fill' : 'object-contain'}"
						on:load={imageLoaded}
					/>
				{/if}
				{#if (snapshot || cameraFeedOn) && paneWidth > 0 && paneHeight > 0 && !drawing && detectRoi}
					{@const x = detectRoi.x * paneWidth}
					{@const y = detectRoi.y * paneHeight}
					{@const w = detectRoi.w * paneWidth}
					{@const h = detectRoi.h * paneHeight}
					<svg
						class="pointer-events-none absolute inset-0 z-10 h-full w-full"
						viewBox="0 0 {paneWidth} {paneHeight}"
						preserveAspectRatio={zoom ? "none" : "xMidYMid meet"}
					>
						<rect x="0" y="0" width={paneWidth} height={y} fill="black" fill-opacity="0.45" />
						<rect
							x="0"
							y={y + h}
							width={paneWidth}
							height={paneHeight - (y + h)}
							fill="black"
							fill-opacity="0.45"
						/>
						<rect x="0" y={y} width={x} height={h} fill="black" fill-opacity="0.45" />
						<rect
							x={x + w}
							y={y}
							width={paneWidth - (x + w)}
							height={h}
							fill="black"
							fill-opacity="0.45"
						/>
						<rect
							x={x}
							y={y}
							width={w}
							height={h}
							fill="none"
							stroke="#fde68a"
							stroke-opacity="0.75"
							stroke-width="2"
							vector-effect="non-scaling-stroke"
						/>
					</svg>
				{/if}
				{#if !snapshot && !drawing && !barcodeDrawing && cameraFeedOn && paneWidth > 0 && paneHeight > 0}
					{@const box = barcodeOverlay}
					{#if box}
						<svg
							class="pointer-events-none absolute inset-0 z-10 h-full w-full"
							viewBox="0 0 {paneWidth} {paneHeight}"
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
				{#if snapshot && paneWidth > 0 && paneHeight > 0 && detections.length > 0}
					{@const marks = detectionMarks(snapshot, paneWidth, paneHeight, frameWidth, frameHeight, detections, zoom)}
					{@const viewW = zoom ? zoom.width : frameWidth}
					{@const viewH = zoom ? zoom.height : frameHeight}
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
								<rect
									x={mark.x}
									y={mark.y}
									width={mark.w}
									height={mark.h}
									fill="none"
									stroke={mark.color}
									stroke-width={BOX_STROKE}
								/>
								<rect
									x={mark.labelX}
									y={mark.labelY}
									width={mark.labelW}
									height={mark.labelH}
									fill={mark.color}
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
							{/each}
						</svg>
					{/if}
				{/if}
			</div>
		{:else if feedOn && camerasMissing}
			<div
				class="absolute inset-0 flex items-center justify-center px-6 text-center text-lg font-semibold text-amber-200"
				role="status"
			>
				No cameras found
			</div>
		{:else if feedOn && !camerasReady}
			<div class="absolute inset-0 flex items-center justify-center text-lg text-neutral-400">
				Looking for cameras...
			</div>
		{:else if !feedOn}
			<div class="absolute inset-0 flex items-center justify-center text-lg text-neutral-500">
				Feed stopped
			</div>
		{/if}
		{#if snapshot}
			<div
				class="pointer-events-none absolute left-3 top-3 z-20 bg-neutral-950/80 {detectRoi &&
				!drawing
					? 'px-2 py-1'
					: 'px-3 py-2'}"
			>
				<p
					class="font-mono font-semibold leading-none text-white {detectRoi && !drawing
						? 'text-sm'
						: 'text-3xl'}"
				>
					{snapshot.kitCode}
				</p>
				<p
					class="mt-1 text-neutral-300 {detectRoi && !drawing ? 'text-[11px]' : 'text-sm'}"
				>
					Taken {snapshot.takenAt}
				</p>
			</div>
			{#if detectState === "running"}
				<p class="pointer-events-none absolute bottom-3 left-3 z-40 bg-neutral-950/80 px-3 py-2 text-sm font-semibold text-amber-200">
					Detecting parts...
				</p>
			{/if}
			<button
				type="button"
				class="absolute bottom-3 right-3 z-40 inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950"
				on:click={onResume}
			>
				{feedOn ? "Resume live feed" : "Back to feed"}
			</button>
		{/if}
		{#if (drawing && snapshot) || (barcodeDrawing && !snapshot)}
			<div
				class="absolute inset-0 z-30 cursor-crosshair"
				on:pointerdown={onPointerDown}
				on:pointermove={onPointerMove}
				on:pointerup={onPointerUp}
				on:pointercancel={onPointerUp}
			></div>
			<p class="pointer-events-none absolute left-3 top-3 z-40 bg-neutral-950/80 px-3 py-2 text-sm text-amber-200">
				{barcodeDrawing
					? "Drag a rectangle on the camera."
					: "Drag a rectangle on the snapshot."}
			</p>
			{#if draftMark}
				<div
					class="pointer-events-none absolute z-50"
					style="left:{draftMark.left}px;top:{draftMark.top}px;width:{draftMark.width}px;height:{draftMark.height}px;box-sizing:border-box;border:3px dotted #fde68a;background:rgba(253,230,138,0.2);box-shadow:0 0 0 1px #0a0a0a;"
				></div>
			{/if}
		{/if}
	</div>
</section>
