<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";
import { onDestroy, onMount, tick } from "svelte";

const PREVIEW_CHANNEL = "ls-vision-camera-preview";
const DISPLAY_FPS = [15, 20, 25, 30];
const MIN_ZOOM = 1;
const MAX_ZOOM = 8;
const ZOOM_FACTOR = 1.1;

type PreviewBody = {
	status?: string;
	image?: string;
	message?: string;
	width?: number;
	height?: number;
	format?: string;
	fps?: number | null;
};

type TrainingSuggestion = {
	name?: string;
	folder?: string;
	day?: string;
};

type TrainingSave = {
	saved?: boolean;
	name?: string;
	path?: string;
	message?: string;
};

let index = 0;
let displayFps = 15;
let message = "";
let failed = false;
let status = "";
let painted = false;
let canvasFps = "";
let canvasEl: HTMLCanvasElement | undefined;
let viewportEl: HTMLDivElement | undefined;
let pageEl: HTMLDivElement | undefined;
let nameInput: HTMLInputElement | undefined;
let generation = 0;
let displayFpsPinned = false;
const paintStamps: number[] = [];

let zoom = 1;
let panX = 0;
let panY = 0;
let fitLeft = 0;
let fitTop = 0;
let fitW = 0;
let fitH = 0;
let frameW = 0;
let frameH = 0;
let spaceDown = false;
let panning = false;
let panOrigin = { x: 0, y: 0, panX: 0, panY: 0 };

let snapshotOpen = false;
let snapshotBusy = false;
let snapshotName = "";
let snapshotImage = "";
let snapshotFolder = "";
let snapshotDay = "";
let snapshotMessage = "";
let snapshotSaved = "";

function formatStatus(body: PreviewBody, canvasRate: string): string {
	const width = body.width;
	const height = body.height;
	const format = body.format?.trim() ?? "";
	const size =
		typeof width === "number" &&
		typeof height === "number" &&
		width > 0 &&
		height > 0
			? `${width}×${height}`
			: "";
	const capture =
		typeof body.fps === "number" && Number.isFinite(body.fps) && body.fps > 0
			? `capture ${body.fps.toFixed(1)} fps`
			: "";
	const canvas = canvasRate ? `canvas ${canvasRate}` : "";
	return [size, format, capture, canvas]
		.filter((part) => part.length > 0)
		.join(" · ");
}

function readIndex(value: unknown): number | null {
	const next = Number(value);
	if (!Number.isInteger(next) || next < 0 || next > 32) return null;
	return next;
}

function readDisplayFps(value: unknown): number {
	const next = Number(value);
	return DISPLAY_FPS.includes(next) ? next : 15;
}

function frameMs(): number {
	return 1000 / displayFps;
}

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => {
		setTimeout(resolve, ms);
	});
}

function notePaint(now: number) {
	paintStamps.push(now);
	while (paintStamps.length > 0 && now - paintStamps[0] > 1000) {
		paintStamps.shift();
	}
	if (paintStamps.length < 2) {
		canvasFps = "";
		return;
	}
	const span = paintStamps[paintStamps.length - 1] - paintStamps[0];
	if (span < 200) {
		canvasFps = "";
		return;
	}
	canvasFps = `${(((paintStamps.length - 1) * 1000) / span).toFixed(1)} fps`;
}

function fitFrame() {
	const view = viewportEl;
	if (!view || frameW <= 0 || frameH <= 0) return;
	const viewW = view.clientWidth;
	const viewH = view.clientHeight;
	if (viewW <= 0 || viewH <= 0) return;
	const scale = Math.min(viewW / frameW, viewH / frameH);
	fitW = frameW * scale;
	fitH = frameH * scale;
	fitLeft = (viewW - fitW) / 2;
	fitTop = (viewH - fitH) / 2;
	clampPan();
}

function clampAxis(
	pan: number,
	fit: number,
	scaled: number,
	view: number,
): number {
	if (scaled <= view) return 0;
	const min = view - scaled - fit;
	const max = -fit;
	if (pan < min) return min;
	if (pan > max) return max;
	return pan;
}

function clampPan() {
	const view = viewportEl;
	if (!view || fitW <= 0 || fitH <= 0) return;
	panX = clampAxis(panX, fitLeft, fitW * zoom, view.clientWidth);
	panY = clampAxis(panY, fitTop, fitH * zoom, view.clientHeight);
}

function resetView() {
	zoom = 1;
	panX = 0;
	panY = 0;
	spaceDown = false;
	panning = false;
}

function applyZoom(next: number, clientX: number, clientY: number) {
	const view = viewportEl;
	let clamped = next;
	if (clamped < MIN_ZOOM) clamped = MIN_ZOOM;
	if (clamped > MAX_ZOOM) clamped = MAX_ZOOM;
	if (!view || fitW <= 0 || fitH <= 0) {
		zoom = clamped <= MIN_ZOOM + 0.0001 ? 1 : clamped;
		if (zoom === 1) {
			panX = 0;
			panY = 0;
		}
		return;
	}
	if (Math.abs(clamped - zoom) < 0.0001) return;
	const rect = view.getBoundingClientRect();
	const cx = clientX - rect.left;
	const cy = clientY - rect.top;
	const contentX = (cx - fitLeft - panX) / zoom;
	const contentY = (cy - fitTop - panY) / zoom;
	zoom = clamped;
	if (zoom <= MIN_ZOOM + 0.0001) {
		zoom = 1;
		panX = 0;
		panY = 0;
		return;
	}
	panX = cx - fitLeft - contentX * zoom;
	panY = cy - fitTop - contentY * zoom;
	clampPan();
}

function zoomAt(clientX: number, clientY: number, deltaY: number) {
	if (snapshotOpen || fitW <= 0 || fitH <= 0) return;
	const factor = deltaY < 0 ? ZOOM_FACTOR : 1 / ZOOM_FACTOR;
	applyZoom(zoom * factor, clientX, clientY);
}

let focus = 50;
let focusNotice = "";
let focusStatus = "";
let focusTimer: ReturnType<typeof setTimeout> | null = null;
let focusLoadId = 0;
let focusEpoch = 0;
let focusChain: Promise<void> = Promise.resolve();
let closed = false;

function objectField(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== "object" || Array.isArray(value)) return {};
	return value as Record<string, unknown>;
}

function levelFocus(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value)) return null;
	if (value < 0 || value > 100) return null;
	return value;
}

/** Null means the camera record does not set focus, so autofocus stays. */
function readStoredFocus(camera: unknown, cameraIndex: number): number | null {
	const data = objectField(camera);
	const byIndex = objectField(data.byIndex);
	const key = String(cameraIndex);
	if (Object.hasOwn(byIndex, key))
		return levelFocus(objectField(byIndex[key]).focus);
	const active = Number(data.index);
	if (Number.isInteger(active) && active === cameraIndex)
		return levelFocus(data.focus);
	return null;
}

function scheduleFocus(value: number) {
	focusEpoch += 1;
	focusLoadId += 1;
	focusStatus = "";
	if (focusTimer) clearTimeout(focusTimer);
	const cameraIndex = index;
	focusTimer = setTimeout(() => {
		focusTimer = null;
		if (closed || cameraIndex !== index) return;
		sendFocus(cameraIndex, value);
	}, 80);
}

function sendFocus(cameraIndex: number, value: number) {
	const run = focusChain.then(async () => {
		if (closed || cameraIndex !== index || focus !== value) return;
		try {
			const body = await invoke<PreviewBody>("set_camera_focus", {
				index: cameraIndex,
				focus: value,
			});
			if (closed || cameraIndex !== index || focus !== value) return;
			if (body.status === "fail") {
				focusNotice = body.message || "Could not set focus.";
				return;
			}
			focusNotice = "";
			const channel = new BroadcastChannel(PREVIEW_CHANNEL);
			channel.postMessage({ index: cameraIndex, focus: value });
			channel.close();
		} catch (err: unknown) {
			if (closed || cameraIndex !== index || focus !== value) return;
			focusNotice = invokeMessage(err, "Could not set focus.");
		}
	});
	focusChain = run.then(
		() => undefined,
		() => undefined,
	);
}

async function applyStoredFocus(cameraIndex: number) {
	const loadId = ++focusLoadId;
	let stored: number | null = null;
	try {
		const config = await invoke<{ camera?: unknown }>("load_local_config");
		stored = readStoredFocus(config.camera, cameraIndex);
	} catch {
		stored = null;
	}
	if (closed || loadId !== focusLoadId || cameraIndex !== index) return;
	focus = stored ?? 50;
	focusNotice = "";
	focusStatus = stored === null ? "Autofocus" : "";
	if (stored !== null) sendFocus(cameraIndex, stored);
}

function onAutoFocus() {
	if (snapshotOpen) return;
	const cameraIndex = index;
	const previous = focus;
	const epoch = ++focusEpoch;
	focusLoadId += 1;
	if (focusTimer) {
		clearTimeout(focusTimer);
		focusTimer = null;
	}
	focus = 50;
	focusNotice = "";
	focusStatus = "";
	const run = focusChain.then(async () => {
		if (closed || cameraIndex !== index || epoch !== focusEpoch) return;
		try {
			const body = await invoke<PreviewBody>("set_camera_autofocus", {
				index: cameraIndex,
			});
			if (closed || cameraIndex !== index || epoch !== focusEpoch) return;
			if (body.status === "fail") {
				focus = previous;
				focusNotice = body.message || "Could not set autofocus.";
				return;
			}
			focus = 50;
			focusNotice = "";
			focusStatus = "Autofocus";
			const channel = new BroadcastChannel(PREVIEW_CHANNEL);
			channel.postMessage({ index: cameraIndex, focus: null });
			channel.close();
		} catch (err: unknown) {
			if (closed || cameraIndex !== index || epoch !== focusEpoch) return;
			focus = previous;
			focusNotice = invokeMessage(err, "Could not set autofocus.");
		}
	});
	focusChain = run.then(
		() => undefined,
		() => undefined,
	);
}

function onFocusInput(event: Event) {
	const raw = Number((event.currentTarget as HTMLInputElement).value);
	if (!Number.isInteger(raw) || raw < 0 || raw > 100 || snapshotOpen) return;
	focus = raw;
	scheduleFocus(raw);
}

function onWheel(event: WheelEvent) {
	if (!event.ctrlKey) return;
	event.preventDefault();
	zoomAt(event.clientX, event.clientY, event.deltaY);
}

function typingTarget(event: KeyboardEvent): boolean {
	const target = event.target;
	if (!(target instanceof HTMLElement)) return false;
	const tag = target.tagName;
	return (
		tag === "INPUT" ||
		tag === "TEXTAREA" ||
		tag === "SELECT" ||
		target.isContentEditable
	);
}

function releaseSpace() {
	spaceDown = false;
	panning = false;
}

function onKeyDown(event: KeyboardEvent) {
	if (event.code === "Space") {
		if (snapshotOpen || typingTarget(event)) return;
		event.preventDefault();
		spaceDown = true;
		return;
	}
	if (event.key === "Escape" && snapshotOpen) {
		event.preventDefault();
		cancelSnapshot();
		return;
	}
	if (event.key === "Enter" && !snapshotOpen && !typingTarget(event)) {
		event.preventDefault();
		void takeSnapshot();
	}
}

function onKeyUp(event: KeyboardEvent) {
	if (event.code === "Space") releaseSpace();
}

function onPointerDown(event: PointerEvent) {
	if (!spaceDown || event.button !== 0 || zoom <= 1 || snapshotOpen) return;
	panning = true;
	panOrigin = { x: event.clientX, y: event.clientY, panX, panY };
	viewportEl?.setPointerCapture(event.pointerId);
}

function onPointerMove(event: PointerEvent) {
	if (!panning) return;
	panX = panOrigin.panX + (event.clientX - panOrigin.x);
	panY = panOrigin.panY + (event.clientY - panOrigin.y);
	clampPan();
}

function endPan() {
	panning = false;
}

async function drawFrame(url: string): Promise<boolean> {
	const bitmap = await createImageBitmap(await (await fetch(url)).blob());
	try {
		const node = canvasEl;
		if (!node) return false;
		if (node.width !== bitmap.width || node.height !== bitmap.height) {
			node.width = bitmap.width;
			node.height = bitmap.height;
		}
		if (frameW !== bitmap.width || frameH !== bitmap.height) {
			frameW = bitmap.width;
			frameH = bitmap.height;
			fitFrame();
		}
		const context = node.getContext("2d", { alpha: false });
		if (!context) return false;
		context.drawImage(bitmap, 0, 0);
		painted = true;
		return true;
	} finally {
		bitmap.close();
	}
}

function useIndex(next: number) {
	if (next === index) return;
	index = next;
	message = "";
	failed = false;
	status = "";
	painted = false;
	canvasFps = "";
	paintStamps.length = 0;
	frameW = 0;
	frameH = 0;
	focusNotice = "";
	focusStatus = "";
	if (focusTimer) {
		clearTimeout(focusTimer);
		focusTimer = null;
	}
	resetView();
	void retitle(next);
	void applyStoredFocus(next);
	generation += 1;
	void runPreview(generation);
}

async function retitle(next: number) {
	if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
		return;
	try {
		const { getCurrentWindow } = await import("@tauri-apps/api/window");
		await getCurrentWindow().setTitle(`Camera ${next}`);
	} catch {
		// The page heading still shows the index when the window title cannot change.
	}
}

async function runPreview(gen: number) {
	while (gen === generation) {
		const started = performance.now();
		const current = index;
		try {
			const body = await invoke<PreviewBody>("capture_preview", {
				index: current,
			});
			if (gen !== generation || current !== index) return;
			if (body.status === "fail" || !body.image) {
				failed = true;
				status = "";
				message = body.message || "The camera did not open.";
				await sleep(400);
				continue;
			}
			const drew = await drawFrame(body.image);
			if (gen !== generation || current !== index) return;
			if (drew) notePaint(performance.now());
			status = formatStatus(body, canvasFps);
			message = "";
			failed = false;
		} catch (err: unknown) {
			if (gen !== generation || current !== index) return;
			failed = true;
			status = "";
			message =
				typeof err === "string" && err.trim()
					? err
					: "The camera did not open.";
			await sleep(400);
			continue;
		}
		const remain = frameMs() - (performance.now() - started);
		if (gen === generation && remain > 0) await sleep(remain);
	}
}

async function loadDisplayFps() {
	try {
		const config = await invoke<{ camera?: { displayFps?: unknown } }>(
			"load_local_config",
		);
		if (!displayFpsPinned) {
			displayFps = readDisplayFps(config.camera?.displayFps);
		}
	} catch {
		if (!displayFpsPinned) displayFps = 15;
	}
	await applyStoredFocus(index);
}

function invokeMessage(err: unknown, fallback: string): string {
	return typeof err === "string" && err.trim() ? err : fallback;
}

async function takeSnapshot() {
	if (snapshotOpen || snapshotBusy) return;
	snapshotBusy = true;
	snapshotSaved = "";
	snapshotMessage = "";
	try {
		const body = await invoke<PreviewBody>("capture_snapshot", { index });
		if (body.status === "fail" || !body.image) {
			snapshotSaved = body.message || "The camera did not open.";
			return;
		}
		const suggestion = await invoke<TrainingSuggestion>(
			"suggest_training_filename",
		);
		snapshotImage = body.image;
		snapshotName = suggestion.name?.trim() || "image";
		snapshotFolder = suggestion.folder?.trim() || "";
		snapshotDay = suggestion.day?.trim() || "";
		snapshotOpen = true;
		await tick();
		nameInput?.focus();
		nameInput?.select();
	} catch (err: unknown) {
		snapshotSaved = invokeMessage(err, "Could not take a snapshot.");
	} finally {
		snapshotBusy = false;
	}
}

function cancelSnapshot() {
	snapshotOpen = false;
	snapshotImage = "";
	snapshotMessage = "";
	snapshotBusy = false;
}

async function confirmSnapshot() {
	if (!snapshotOpen || snapshotBusy || !snapshotImage) return;
	snapshotBusy = true;
	snapshotMessage = "";
	try {
		const result = await invoke<TrainingSave>("save_training_image", {
			name: snapshotName,
			image: snapshotImage,
		});
		if (!result.saved) {
			if (result.name?.trim()) snapshotName = result.name.trim();
			snapshotMessage = result.message || "That file already exists.";
			await tick();
			nameInput?.focus();
			nameInput?.select();
			return;
		}
		snapshotOpen = false;
		snapshotImage = "";
		snapshotSaved =
			result.message || `Saved ${result.name || snapshotName}.jpg`;
	} catch (err: unknown) {
		snapshotMessage = invokeMessage(err, "Could not save the image.");
	} finally {
		snapshotBusy = false;
	}
}

onMount(() => {
	const params = new URLSearchParams(window.location.search);
	const initial = readIndex(params.get("index"));
	if (initial !== null) index = initial;
	void retitle(index);
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.onmessage = (event) => {
		if (event.data && "displayFps" in event.data) {
			displayFpsPinned = true;
			const next = readDisplayFps(event.data.displayFps);
			if (next !== displayFps) {
				displayFps = next;
				paintStamps.length = 0;
				canvasFps = "";
			}
		}
		const next = readIndex(event.data?.index);
		if (next !== null) useIndex(next);
	};
	const page = pageEl;
	page?.addEventListener("wheel", onWheel, { passive: false });
	const observer = new ResizeObserver(() => fitFrame());
	if (viewportEl) observer.observe(viewportEl);
	void loadDisplayFps().then(() => {
		if (generation === 0) void runPreview(generation);
	});
	return () => {
		generation += 1;
		channel.close();
		observer.disconnect();
		page?.removeEventListener("wheel", onWheel);
	};
});

onDestroy(() => {
	closed = true;
	generation += 1;
	if (focusTimer) clearTimeout(focusTimer);
});
</script>

<svelte:head>
	<title>Camera {index}</title>
	<style>
		html {
			color-scheme: dark;
		}

		.focus-slider {
			-webkit-appearance: none;
			appearance: none;
			height: 18px;
			background: transparent;
			cursor: pointer;
		}

		.focus-slider::-webkit-slider-runnable-track {
			height: 4px;
			border-radius: 999px;
			background: linear-gradient(to right, #3b82f6 var(--fill), #d4d4d4 var(--fill));
		}

		.focus-slider::-webkit-slider-thumb {
			-webkit-appearance: none;
			appearance: none;
			width: 16px;
			height: 16px;
			margin-top: -6px;
			border: 2px solid #f5f5f5;
			border-radius: 999px;
			background: #3b82f6;
		}

		.focus-slider:focus-visible::-webkit-slider-thumb {
			outline: 2px solid #93c5fd;
			outline-offset: 2px;
		}

		.focus-slider::-moz-range-track {
			height: 4px;
			border: none;
			border-radius: 999px;
			background: #d4d4d4;
		}

		.focus-slider::-moz-range-progress {
			height: 4px;
			border-radius: 999px;
			background: #3b82f6;
		}

		.focus-slider::-moz-range-thumb {
			width: 16px;
			height: 16px;
			border: 2px solid #f5f5f5;
			border-radius: 999px;
			background: #3b82f6;
		}
	</style>
</svelte:head>

<svelte:window on:keydown={onKeyDown} on:keyup={onKeyUp} on:blur={releaseSpace} />

<div bind:this={pageEl} class="relative flex h-dvh flex-col overflow-hidden bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<h1 class="text-sm font-semibold tracking-tight">Camera {index}</h1>
		{#if status}
			<p class="min-w-0 truncate text-xs tabular-nums text-neutral-200">{status}</p>
		{:else}
			<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Preview</p>
		{/if}
	</header>
	<p class="shrink-0 border-b border-neutral-800 px-4 py-1 text-[11px] text-neutral-500">
		Focus slider or Auto · Ctrl+wheel zoom · Space+drag pan · Enter snapshot
	</p>
	{#if snapshotSaved}
		<p class="shrink-0 px-4 py-1 text-xs text-neutral-200">{snapshotSaved}</p>
	{/if}
	<div
		bind:this={viewportEl}
		class="relative min-h-0 flex-1 select-none overflow-hidden bg-black"
		class:cursor-grab={spaceDown && zoom > 1 && !panning}
		class:cursor-grabbing={panning}
		on:pointerdown={onPointerDown}
		on:pointermove={onPointerMove}
		on:pointerup={endPan}
		on:pointercancel={endPan}
	>
		<canvas
			bind:this={canvasEl}
			class="absolute"
			class:invisible={!painted}
			style:left="{fitLeft}px"
			style:top="{fitTop}px"
			style:width="{fitW}px"
			style:height="{fitH}px"
			style:transform="translate({panX}px, {panY}px) scale({zoom})"
			style:transform-origin="0 0"
			aria-label="Camera {index} preview"
		></canvas>
		{#if failed || !painted}
			<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
				{message || "Opening camera…"}
			</p>
		{/if}
		<div
			class="absolute inset-x-0 bottom-0 z-[1] flex items-center gap-3 bg-neutral-950/80 px-4 py-2"
			on:pointerdown|stopPropagation
		>
			<span class="shrink-0 text-[11px] font-semibold uppercase tracking-widest text-neutral-300">Focus</span>
			<input
				type="range"
				class="focus-slider min-w-0 flex-1"
				min="0"
				max="100"
				step="1"
				value={focus}
				style="--fill: {focus}%"
				aria-label="Focus"
				disabled={snapshotOpen}
				on:input={onFocusInput}
			/>
			<button
				type="button"
				class="shrink-0 border border-neutral-500 bg-neutral-800 px-2 py-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-100 disabled:opacity-50"
				aria-label="Autofocus"
				disabled={snapshotOpen}
				on:click={onAutoFocus}
			>
				Auto
			</button>
			<span class="w-8 shrink-0 text-right font-mono text-sm tabular-nums text-neutral-100">{focus}</span>
			{#if focusStatus}
				<span class="shrink-0 text-xs text-neutral-200">{focusStatus}</span>
			{/if}
			{#if focusNotice}
				<span class="max-w-36 truncate text-xs text-amber-300" title={focusNotice}>{focusNotice}</span>
			{/if}
		</div>
	</div>
	{#if snapshotOpen}
		<div class="absolute inset-0 z-10 flex items-center justify-center bg-black/70 p-4">
			<form
				class="w-full max-w-md border border-neutral-700 bg-neutral-900 p-4"
				role="dialog"
				aria-modal="true"
				aria-labelledby="snapshot-title"
				on:submit|preventDefault={confirmSnapshot}
			>
				<h2 id="snapshot-title" class="text-base font-semibold tracking-tight">Save training image</h2>
				<p class="mt-2 break-all font-mono text-xs text-neutral-400">
					{snapshotFolder}{snapshotFolder ? "\\" : ""}{snapshotDay}
				</p>
				<label class="mt-3 flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
					File name
					<span class="flex items-center gap-2 normal-case tracking-normal">
						<input
							bind:this={nameInput}
							bind:value={snapshotName}
							id="training-snapshot-name"
							class="min-w-0 flex-1 border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-base font-medium text-neutral-100"
							disabled={snapshotBusy}
							autocomplete="off"
							spellcheck="false"
						/>
						<span class="font-mono text-sm text-neutral-300">.jpg</span>
					</span>
				</label>
				{#if snapshotMessage}
					<p class="mt-2 text-sm text-neutral-200">{snapshotMessage}</p>
				{/if}
				<div class="mt-4 flex flex-wrap items-center gap-2">
					<button
						type="submit"
						class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
						disabled={snapshotBusy}
					>
						{snapshotBusy ? "Saving..." : "Save"}
					</button>
					<button
						type="button"
						class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
						on:click={cancelSnapshot}
						disabled={snapshotBusy}
					>
						Cancel
					</button>
				</div>
			</form>
		</div>
	{/if}
</div>
