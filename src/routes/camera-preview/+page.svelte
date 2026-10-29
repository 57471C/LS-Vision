<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { invoke } from "@tauri-apps/api/core";
import { onDestroy, onMount, tick } from "svelte";
import {
	assignActiveCamera,
	readUvcSettings,
	retainCameraSettings,
	settingsForCamera,
	type UvcSettings,
} from "$lib/camera-settings";
import { boxesFromDetect } from "$lib/detect-labels";
import {
	catalogueMiss,
	readMatchThreshold,
} from "$lib/catalogue-capture";
// biome-ignore lint/correctness/noUnusedImports: the template uses this component
import MeasureOverlay from "$lib/MeasureOverlay.svelte";
import {
	type CameraScale,
	formatPxMm,
	formatReadout,
	formatScale,
	imagePoint,
	type ImageLine,
	lineLength,
	type MeasureDraw,
	measureReadout,
	parseMillimetres,
	scaleFromLength,
} from "$lib/measure";
import { marqueeGesture, topMarkAt } from "$lib/preview-marquee";

const PREVIEW_CHANNEL = "ls-vision-camera-preview";
const ADMIN_FLAG = "ls-vision-settings-open";
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

type DetectBody = {
	detections?: unknown;
	message?: unknown;
};

type DetectMark = {
	ic: string;
	confidence: number;
	x: number;
	y: number;
	w: number;
	h: number;
	color: string;
	index: number;
	embed: boolean;
	sensor: [number, number, number, number];
	catalogueMiss: boolean;
};

type Region = {
	x: number;
	y: number;
	w: number;
	h: number;
};

type Aim = {
	x: number;
	y: number;
	left: number;
	top: number;
};

const MIN_MARQUEE = 8;

const DETECT_COLORS = [
	"rgb(60, 180, 0)",
	"rgb(255, 165, 0)",
	"rgb(0, 140, 255)",
	"rgb(230, 60, 60)",
	"rgb(200, 60, 200)",
	"rgb(200, 200, 0)",
	"rgb(0, 180, 180)",
	"rgb(255, 100, 0)",
];

let lastFrame = "";
let modelReady = false;
let modelChecked = false;
let modelLoadId = 0;
let detectBusy = false;
let detectGeneration = 0;
let detectImage = "";
let detectW = 0;
let detectH = 0;
let detectMarks: DetectMark[] = [];
let detectNote = "";
let admin = false;
// The each block has to read this array itself. A helper is not redrawn when it changes.
let selected: number[] = [];
let saveMode: "append" | "replace" = "append";
let saveBusy = false;
let saveNote = "";
let saveGeneration = 0;
let matchThreshold = 0.7;
let captureMark: DetectMark | null = null;
let captureIc = "";
let captureImage = "";
let captureBusy = false;
let captureNote = "";
let captureGen = 0;
let cataloguePress: DetectMark | null = null;
let sourceW = 0;
let sourceH = 0;
let classNames: string[] = [];
let marquee: Region | null = null;
let marqueeDraft: Region | null = null;
let marqueeIc = "";
let marqueeBusy = false;
let marqueeReady = false;
let marqueeNote = "";
let marqueeGen = 0;
let drawing = false;
let drawOrigin = { x: 0, y: 0 };

let scale: CameraScale | null = null;
let scaleLoadId = 0;
let scaleMode = false;
let scaleModal = false;
let scaleMm = "";
let scaleError = "";
let scaleSaving = false;
let scaleInput: HTMLInputElement | undefined;
let pendingScale: { pixels: number; width: number; height: number } | null =
	null;
let measureOn = false;
let measureLine: ImageLine | null = null;
let crosshairs = false;
let aim: Aim | null = null;
let aimClient: { x: number; y: number } | null = null;
let aimFrame = 0;
let frameEl: HTMLDivElement | undefined;
let pressMark: DetectMark | null = null;

$: shownMarquee = marqueeDraft ?? marquee;

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
	scheduleAim();
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
	scheduleAim();
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
		scheduleAim();
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
let exposureAbsolute = 156;
let gain = 0;
let controlNotice = "";
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
	let uvc: UvcSettings = readUvcSettings(null);
	try {
		const config = await invoke<{ camera?: unknown }>("load_local_config");
		stored = readStoredFocus(config.camera, cameraIndex);
		uvc = readUvcSettings(config.camera);
	} catch {
		stored = null;
	}
	if (closed || loadId !== focusLoadId || cameraIndex !== index) return;
	exposureAbsolute = uvc.exposureAbsolute;
	gain = uvc.gain;
	controlNotice = "";
	focusNotice = "";
	if (stored !== null) {
		focus = stored;
		focusStatus = "";
		sendFocus(cameraIndex, stored);
	} else if (!uvc.autoFocus) {
		focus = uvc.focusAbsolute;
		focusStatus = "";
		sendFocus(cameraIndex, uvc.focusAbsolute);
	} else {
		focus = 50;
		focusStatus = "Autofocus";
	}
	sendExposure(cameraIndex, uvc.exposureAbsolute);
	sendGain(cameraIndex, uvc.gain);
}

function sendExposure(cameraIndex: number, value: number) {
	const run = focusChain.then(async () => {
		if (closed || cameraIndex !== index || exposureAbsolute !== value) return;
		try {
			const body = await invoke<PreviewBody>("set_camera_exposure", {
				index: cameraIndex,
				exposureAbsolute: value,
			});
			if (closed || cameraIndex !== index || exposureAbsolute !== value) return;
			if (body.status === "fail") {
				controlNotice = body.message || "Could not set exposure.";
			}
		} catch (err: unknown) {
			if (closed || cameraIndex !== index || exposureAbsolute !== value) return;
			controlNotice = invokeMessage(err, "Could not set exposure.");
		}
	});
	focusChain = run.then(
		() => undefined,
		() => undefined,
	);
}

function sendGain(cameraIndex: number, value: number) {
	const run = focusChain.then(async () => {
		if (closed || cameraIndex !== index || gain !== value) return;
		try {
			const body = await invoke<PreviewBody>("set_camera_gain", {
				index: cameraIndex,
				gain: value,
			});
			if (closed || cameraIndex !== index || gain !== value) return;
			if (body.status === "fail") {
				controlNotice = body.message || "Could not set gain.";
			}
		} catch (err: unknown) {
			if (closed || cameraIndex !== index || gain !== value) return;
			controlNotice = invokeMessage(err, "Could not set gain.");
		}
	});
	focusChain = run.then(
		() => undefined,
		() => undefined,
	);
}

function onExposureChange(event: Event) {
	const raw = Number((event.currentTarget as HTMLInputElement).value);
	if (!Number.isInteger(raw) || raw < -13 || raw > 20000 || snapshotOpen)
		return;
	exposureAbsolute = raw;
	controlNotice = "";
	sendExposure(index, raw);
}

function onGainChange(event: Event) {
	const raw = Number((event.currentTarget as HTMLInputElement).value);
	if (!Number.isInteger(raw) || raw < 0 || raw > 255 || snapshotOpen) return;
	gain = raw;
	controlNotice = "";
	sendGain(index, raw);
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
	if (scaleModal) {
		if (event.key === "Escape") {
			event.preventDefault();
			cancelScaleMode();
		}
		return;
	}
	if (event.key === "Escape" && scaleMode) {
		event.preventDefault();
		cancelScaleMode();
		return;
	}
	if (event.key === "Escape" && measureOn && measureLine) {
		event.preventDefault();
		measureLine = null;
		return;
	}
	if (event.key === "Enter" && !snapshotOpen && !typingTarget(event)) {
		event.preventDefault();
		void takeSnapshot();
		return;
	}
	if (
		event.key === "Escape" &&
		!snapshotOpen &&
		(drawing || marquee || marqueeDraft || marqueeNote)
	) {
		event.preventDefault();
		forgetMarquee("vector");
		return;
	}
	if (event.key === "Enter" && marqueeField(event)) {
		event.preventDefault();
		void saveMarquee();
		return;
	}
	if (event.key === "Enter" && catalogueField(event)) {
		event.preventDefault();
		void saveCatalogueCrop();
		return;
	}
	if (event.key === "Escape" && captureMark) {
		event.preventDefault();
		closeCatalogue();
		return;
	}
	if (event.key === "Escape" && !snapshotOpen && detectImage) {
		event.preventDefault();
		clearDetect();
		return;
	}
	// The shortcut belongs to this window. A focused text field keeps the key.
	if (event.key === "\\" && !snapshotOpen && !textTarget(event)) {
		event.preventDefault();
		void runDetect();
	}
}

function textTarget(event: KeyboardEvent): boolean {
	const target = event.target;
	if (!(target instanceof HTMLElement)) return false;
	if (target.isContentEditable) return true;
	const tag = target.tagName;
	if (tag === "TEXTAREA" || tag === "SELECT") return true;
	if (tag !== "INPUT") return false;
	const type = (target as HTMLInputElement).type;
	return type === "text" || type === "search" || type === "url" || type === "";
}

function onKeyUp(event: KeyboardEvent) {
	if (event.code === "Space") releaseSpace();
}

function marqueeField(event: KeyboardEvent): boolean {
	const target = event.target;
	return target instanceof HTMLInputElement && target.id === "marquee-ic";
}

function catalogueField(event: KeyboardEvent): boolean {
	const target = event.target;
	return target instanceof HTMLInputElement && target.id === "catalogue-ic";
}

function eventOnFrame(event: Event): boolean {
	const node = frameEl;
	if (!node || !(event.target instanceof Node)) return false;
	return node === event.target || node.contains(event.target);
}

function previewPoint(event: PointerEvent): { x: number; y: number } | null {
	const node = frameEl;
	if (!node || detectW < 1 || detectH < 1) return null;
	return imagePoint(
		event.clientX,
		event.clientY,
		node.getBoundingClientRect(),
		detectW,
		detectH,
	);
}

function placeAim(clientX: number, clientY: number, keepOutside: boolean) {
	if (!admin || !crosshairs || snapshotOpen || scaleModal) {
		if (aim) aim = null;
		return;
	}
	const node = frameEl;
	const view = viewportEl;
	if (!node || !view || overlayW < 1 || overlayH < 1) {
		aim = null;
		return;
	}
	const rect = node.getBoundingClientRect();
	const inside =
		clientX >= rect.left &&
		clientX <= rect.right &&
		clientY >= rect.top &&
		clientY <= rect.bottom;
	if (!inside && !keepOutside) {
		if (aim) aim = null;
		return;
	}
	const point = imagePoint(clientX, clientY, rect, overlayW, overlayH);
	if (!point) {
		aim = null;
		return;
	}
	const box = view.getBoundingClientRect();
	const labelW = 220;
	const labelH = drawing && marqueeDraft ? 78 : 42;
	let left = clientX - box.left + 14;
	let top = clientY - box.top + 14;
	if (left + labelW > box.width) left = clientX - box.left - labelW - 8;
	if (top + labelH > box.height) top = clientY - box.top - labelH - 8;
	if (left < 4) left = 4;
	if (top < 4) top = 4;
	aim = { x: point.x, y: point.y, left, top };
}

function notePointer(event: PointerEvent) {
	aimClient = { x: event.clientX, y: event.clientY };
	placeAim(event.clientX, event.clientY, drawing);
}

function scheduleAim() {
	if (!aimClient || !crosshairs || !admin) return;
	if (aimFrame) cancelAnimationFrame(aimFrame);
	aimFrame = requestAnimationFrame(() => {
		aimFrame = 0;
		if (!aimClient) return;
		placeAim(aimClient.x, aimClient.y, drawing);
	});
}

function onViewportLeave() {
	if (drawing) return;
	aim = null;
	aimClient = null;
}

function toggleCrosshairs() {
	crosshairs = !crosshairs;
	if (!crosshairs) {
		aim = null;
		aimClient = null;
	}
}

function regionFrom(
	origin: { x: number; y: number },
	point: { x: number; y: number },
): Region {
	return {
		x: Math.min(origin.x, point.x),
		y: Math.min(origin.y, point.y),
		w: Math.abs(point.x - origin.x),
		h: Math.abs(point.y - origin.y),
	};
}

function toSource(region: Region): Region | null {
	if (detectW < 1 || detectH < 1 || sourceW < 1 || sourceH < 1) return null;
	return {
		x: (region.x * sourceW) / detectW,
		y: (region.y * sourceH) / detectH,
		w: (region.w * sourceW) / detectW,
		h: (region.h * sourceH) / detectH,
	};
}

function cancelDraft() {
	drawing = false;
	marqueeDraft = null;
	pressMark = null;
	cataloguePress = null;
}

function closeCatalogue() {
	captureGen += 1;
	captureMark = null;
	captureIc = "";
	captureImage = "";
	captureBusy = false;
	captureNote = "";
	cataloguePress = null;
}

function forgetMarquee(mode: "vector" | "frame" | "view") {
	marqueeGen += 1;
	marquee = null;
	marqueeDraft = null;
	drawing = false;
	pressMark = null;
	cataloguePress = null;
	marqueeReady = false;
	marqueeBusy = false;
	marqueeNote = "";
	if (mode === "view") return;
	const command =
		mode === "frame" ? "release_preview_frame" : "clear_marquee_reference";
	void invoke(command).catch(() => {
		// The next detect replaces the held still.
	});
}

function onPointerDown(event: PointerEvent) {
	if (snapshotOpen || captureMark || event.button !== 0) return;
	if (spaceDown && zoom > 1) {
		panning = true;
		panOrigin = { x: event.clientX, y: event.clientY, panX, panY };
		viewportEl?.setPointerCapture(event.pointerId);
		return;
	}
	if (!marqueeHit && catalogueHit && eventOnFrame(event)) {
		const point = previewPoint(event);
		const mark = point ? topMarkAt(detectMarks, point.x, point.y) : null;
		if (!mark?.catalogueMiss) return;
		event.preventDefault();
		cataloguePress = mark;
		viewportEl?.setPointerCapture(event.pointerId);
		return;
	}
	if (!marqueeHit || drawing || !eventOnFrame(event)) return;
	const point = previewPoint(event);
	if (!point) return;
	event.preventDefault();
	drawing = true;
	drawOrigin = point;
	pressMark = topMarkAt(detectMarks, point.x, point.y);
	marqueeDraft = { x: point.x, y: point.y, w: 0, h: 0 };
	viewportEl?.setPointerCapture(event.pointerId);
}

function onPointerMove(event: PointerEvent) {
	if (crosshairs) notePointer(event);
	if (panning) {
		panX = panOrigin.panX + (event.clientX - panOrigin.x);
		panY = panOrigin.panY + (event.clientY - panOrigin.y);
		clampPan();
		return;
	}
	if (!drawing) return;
	const point = previewPoint(event);
	if (!point) return;
	marqueeDraft = regionFrom(drawOrigin, point);
}

function onPointerUp(event: PointerEvent) {
	if (panning) {
		panning = false;
		return;
	}
	if (cataloguePress && !drawing) {
		const mark = cataloguePress;
		cataloguePress = null;
		void openCatalogueCapture(mark);
		return;
	}
	if (!drawing) return;
	drawing = false;
	const point = previewPoint(event);
	const draft = point ? regionFrom(drawOrigin, point) : marqueeDraft;
	const pressed = pressMark;
	marqueeDraft = null;
	pressMark = null;
	const gesture = marqueeGesture(
		draft?.w ?? 0,
		draft?.h ?? 0,
		pressed,
		MIN_MARQUEE,
	);
	if (pressed?.catalogueMiss && gesture.type !== "marquee") {
		void openCatalogueCapture(pressed);
		return;
	}
	if (gesture.type === "select") {
		toggleMark(gesture.index);
		return;
	}
	if (gesture.type === "marquee" && draft) void commitMarquee(draft);
}

function onPointerCancel() {
	drawing = false;
	marqueeDraft = null;
	pressMark = null;
	cataloguePress = null;
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
			if (frameW > 0 && frameH > 0) measureLine = null;
			frameW = bitmap.width;
			frameH = bitmap.height;
			fitFrame();
		}
		const context = node.getContext("2d", { alpha: false });
		if (!context) return false;
		context.drawImage(bitmap, 0, 0);
		painted = true;
		lastFrame = url;
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
	lastFrame = "";
	clearDetect();
	cancelScaleMode();
	measureLine = null;
	scale = null;
	focusNotice = "";
	focusStatus = "";
	if (focusTimer) {
		clearTimeout(focusTimer);
		focusTimer = null;
	}
	resetView();
	void retitle(next);
	void loadStoredScale(next);
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

async function loadStoredScale(cameraIndex: number) {
	const loadId = ++scaleLoadId;
	try {
		const config = await invoke<{ camera?: unknown }>("load_local_config");
		if (closed || loadId !== scaleLoadId || cameraIndex !== index) return;
		scale = settingsForCamera(config.camera, cameraIndex).scale;
	} catch {
		if (closed || loadId !== scaleLoadId || cameraIndex !== index) return;
		scale = null;
	}
}

async function loadDisplayFps() {
	const loadId = ++scaleLoadId;
	try {
		const config = await invoke<{ camera?: { displayFps?: unknown } }>(
			"load_local_config",
		);
		if (!displayFpsPinned) {
			displayFps = readDisplayFps(config.camera?.displayFps);
		}
		if (!closed && loadId === scaleLoadId) {
			scale = settingsForCamera(config.camera, index).scale;
		}
	} catch {
		if (!displayFpsPinned) displayFps = 15;
	}
	await applyStoredFocus(index);
}

function cancelScaleMode() {
	scaleMode = false;
	scaleModal = false;
	scaleMm = "";
	scaleError = "";
	scaleSaving = false;
	pendingScale = null;
	if (!measureOn) measureLine = null;
}

function toggleScaleMode() {
	if (!admin) return;
	cancelDraft();
	if (scaleMode || scaleModal) {
		cancelScaleMode();
		return;
	}
	measureOn = false;
	measureLine = null;
	scaleMode = true;
}

function toggleMeasure() {
	cancelDraft();
	if (measureOn) {
		measureOn = false;
		measureLine = null;
		return;
	}
	cancelScaleMode();
	measureOn = true;
}

function onMeasureLine(event: CustomEvent<MeasureDraw>) {
	const drawn = event.detail;
	measureLine = drawn.line;
	if (!scaleMode) return;
	pendingScale = {
		pixels: lineLength(drawn.line),
		width: drawn.width,
		height: drawn.height,
	};
	scaleMm = "";
	scaleError = "";
	scaleModal = true;
	void tick().then(() => {
		scaleInput?.focus();
		scaleInput?.select();
	});
}

async function persistScale(next: CameraScale) {
	const config = await invoke<Record<string, unknown>>("load_local_config");
	const camera = config.camera;
	const active = Number(
		camera && typeof camera === "object"
			? (camera as { index?: unknown }).index
			: Number.NaN,
	);
	const settings = settingsForCamera(camera, index);
	settings.scale = next;
	if (active === index) assignActiveCamera(config, settings);
	else retainCameraSettings(config, settings);
	await invoke("save_local_config", { config });
	const saved = await invoke<{ camera?: unknown }>("load_local_config");
	const stored = settingsForCamera(saved.camera, index).scale;
	if (!stored || Math.abs(stored.mmPerPx - next.mmPerPx) > 1e-9) {
		throw new Error("The scale was not stored.");
	}
	scale = stored;
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.postMessage({ index, scale: stored });
	channel.close();
}

async function confirmScale() {
	const pending = pendingScale;
	if (!pending || scaleSaving || !admin) return;
	const millimetres = parseMillimetres(scaleMm);
	const next = millimetres
		? scaleFromLength(
				millimetres,
				pending.pixels,
				pending.width,
				pending.height,
			)
		: null;
	if (!next) {
		scaleError = "Enter a length in millimetres.";
		return;
	}
	scaleSaving = true;
	scaleError = "";
	try {
		await persistScale(next);
		scaleModal = false;
		scaleMode = false;
		pendingScale = null;
		measureLine = null;
		scaleMm = "";
	} catch (err: unknown) {
		scaleError = invokeMessage(err, "Could not save the scale.");
	} finally {
		scaleSaving = false;
	}
}

$: scaleText = scale ? formatScale(scale, frameW) : "";
$: measureLabel = measureLine
	? formatReadout(measureReadout(lineLength(measureLine), frameW, scale))
	: "";
$: lineActive =
	(scaleMode || measureOn) &&
	!scaleModal &&
	!snapshotOpen &&
	!spaceDown &&
	!panning;
$: overlayW = detectImage && detectW > 0 ? detectW : frameW;
$: overlayH = detectImage && detectH > 0 ? detectH : frameH;
// The measure layer sits on the still. This flag is the drag surface, so a
// detection box does not take the press and measuring keeps the line.
$: marqueeHit =
	admin &&
	!!detectImage &&
	!lineActive &&
	!spaceDown &&
	!panning &&
	!snapshotOpen &&
	!scaleModal &&
	!captureMark;
$: catalogueHit =
	!!detectImage &&
	detectMarks.some((mark) => mark.catalogueMiss) &&
	!lineActive &&
	!spaceDown &&
	!panning &&
	!snapshotOpen &&
	!scaleModal &&
	!captureMark;

function invokeMessage(err: unknown, fallback: string): string {
	return typeof err === "string" && err.trim() ? err : fallback;
}

function readNames(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter(
		(item): item is string => typeof item === "string" && item.length > 0,
	);
}

function matchName(name: string, files: readonly string[]): string {
	const cleaned = name.trim().toLowerCase();
	if (!cleaned) return "";
	return files.find((item) => item.toLowerCase() === cleaned) ?? "";
}

// The same choice detect_image uses: the saved name when it is an ONNX file
// in the model folder, otherwise the first ONNX file.
function activeOnnx(stored: unknown, files: readonly string[]): string {
	const saved = typeof stored === "string" ? matchName(stored, files) : "";
	if (saved) return saved.toLowerCase().endsWith(".onnx") ? saved : "";
	return (
		matchName("best.onnx", files) ||
		files.find((item) => item.toLowerCase().endsWith(".onnx")) ||
		""
	);
}

function colorForIc(ic: string): string {
	const bytes = new TextEncoder().encode(ic);
	let total = 0;
	for (const byte of bytes) total += byte;
	return DETECT_COLORS[total % DETECT_COLORS.length];
}

function readDetections(value: unknown): DetectMark[] {
	if (!Array.isArray(value)) return [];
	const marks: DetectMark[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const row = item as {
			ic?: unknown;
			confidence?: unknown;
			bbox?: unknown;
			embed?: unknown;
			twoPass?: unknown;
		};
		const ic = typeof row.ic === "string" ? row.ic : "";
		const confidence = typeof row.confidence === "number" ? row.confidence : 0;
		const bbox = Array.isArray(row.bbox) ? row.bbox : [];
		if (bbox.length < 4) continue;
		const [rawX1, rawY1, rawX2, rawY2] = bbox;
		if (
			![rawX1, rawY1, rawX2, rawY2].every(
				(value) => typeof value === "number" && Number.isFinite(value),
			)
		) {
			continue;
		}
		const x1 = Math.min(rawX1, rawX2);
		const y1 = Math.min(rawY1, rawY2);
		const x2 = Math.max(rawX1, rawX2);
		const y2 = Math.max(rawY1, rawY2);
		if (x2 <= x1 || y2 <= y1) continue;
		marks.push({
			ic,
			confidence,
			x: x1,
			y: y1,
			w: x2 - x1,
			h: y2 - y1,
			color: colorForIc(ic),
			index: marks.length,
			embed: row.embed === true,
			sensor: [x1, y1, x2, y2],
			catalogueMiss: catalogueMiss(row.twoPass, matchThreshold),
		});
	}
	marks.sort((left, right) => left.confidence - right.confidence);
	return marks;
}

function scaleMarks(
	marks: DetectMark[],
	fromW: number,
	fromH: number,
	toW: number,
	toH: number,
): DetectMark[] {
	if (fromW < 1 || fromH < 1 || toW < 1 || toH < 1) return marks;
	if (fromW === toW && fromH === toH) return marks;
	const sx = toW / fromW;
	const sy = toH / fromH;
	return marks.map((mark) => ({
		...mark,
		x: mark.x * sx,
		y: mark.y * sy,
		w: mark.w * sx,
		h: mark.h * sy,
	}));
}

function clearDetect() {
	detectGeneration += 1;
	detectBusy = false;
	detectImage = "";
	detectW = 0;
	detectH = 0;
	sourceW = 0;
	sourceH = 0;
	detectMarks = [];
	detectNote = "";
	closeCatalogue();
	selected = [];
	saveNote = "";
	saveGeneration += 1;
	saveBusy = false;
	forgetMarquee("frame");
}

async function refreshModel() {
	const loadId = ++modelLoadId;
	try {
		const listed = await invoke<unknown>("list_vision_models");
		const config = await invoke<{ model?: unknown; matchThreshold?: unknown }>(
			"load_local_config",
		);
		if (closed || loadId !== modelLoadId) return;
		modelReady = activeOnnx(config.model, readNames(listed)).length > 0;
		matchThreshold = readMatchThreshold(config.matchThreshold);
	} catch {
		if (closed || loadId !== modelLoadId) return;
		modelReady = false;
	}
	if (loadId === modelLoadId) modelChecked = true;
}

// Preview check only. The operator page still owns BoM comparison and banners.
async function runDetect() {
	if (!modelReady || detectBusy || snapshotOpen) return;
	const image = lastFrame;
	const width = frameW;
	const height = frameH;
	if (!image || width < 1 || height < 1) {
		detectNote = "No preview frame.";
		return;
	}
	const gen = ++detectGeneration;
	const camera = index;
	detectBusy = true;
	detectNote = "";
	closeCatalogue();
	selected = [];
	saveNote = "";
	saveGeneration += 1;
	forgetMarquee("vector");
	try {
		const snap = await invoke<PreviewBody>("capture_snapshot", {
			index: camera,
		});
		if (closed || gen !== detectGeneration || camera !== index) return;
		if (snap.status === "fail") {
			detectImage = "";
			sourceW = 0;
			sourceH = 0;
			detectMarks = [];
			detectNote = snap.message || "No frame.";
			forgetMarquee("frame");
			return;
		}
		const body = await invoke<DetectBody>("detect_image", {
			holdReferences: true,
		});
		if (closed || gen !== detectGeneration || camera !== index) return;
		const message = typeof body.message === "string" ? body.message.trim() : "";
		if (message) {
			detectImage = "";
			sourceW = 0;
			sourceH = 0;
			detectMarks = [];
			detectNote = message;
			forgetMarquee("frame");
			if (
				message === "Model is missing." ||
				message.includes("is a .pt file") ||
				message.includes("not an ONNX")
			) {
				modelReady = false;
			}
			return;
		}
		const fullW = typeof snap.width === "number" ? snap.width : width;
		const fullH = typeof snap.height === "number" ? snap.height : height;
		forgetMarquee("view");
		sourceW = fullW;
		sourceH = fullH;
		detectImage = image;
		detectW = width;
		detectH = height;
		detectMarks = scaleMarks(
			readDetections(body.detections),
			fullW,
			fullH,
			width,
			height,
		);
		detectNote = "";
		// Label file only. The Enter dialog still saves the JPEG.
		const labelW = typeof snap.width === "number" ? Math.round(snap.width) : 0;
		const labelH = typeof snap.height === "number" ? Math.round(snap.height) : 0;
		if (labelW > 0 && labelH > 0) {
			try {
				await invoke("save_detect_labels", {
					width: labelW,
					height: labelH,
					boxes: boxesFromDetect(body),
				});
			} catch (err: unknown) {
				if (closed || gen !== detectGeneration) return;
				detectNote = invokeMessage(err, "Could not save the label file.");
			}
		} else if (!closed && gen === detectGeneration) {
			detectNote = "Could not save the label file.";
		}
		if (admin && gen === detectGeneration) void loadClassNames();
	} catch (err: unknown) {
		if (closed || gen !== detectGeneration) return;
		detectImage = "";
		sourceW = 0;
		sourceH = 0;
		detectMarks = [];
		detectNote = invokeMessage(err, "Detect failed.");
		forgetMarquee("frame");
	} finally {
		if (gen === detectGeneration) detectBusy = false;
	}
}

function applyAdmin(next: boolean) {
	admin = next;
	if (!next) {
		selected = [];
		forgetMarquee("vector");
		cancelScaleMode();
		crosshairs = false;
		aim = null;
		aimClient = null;
	} else if (detectImage) {
		void loadClassNames();
	}
	try {
		if (next) sessionStorage.setItem(ADMIN_FLAG, "1");
		else sessionStorage.removeItem(ADMIN_FLAG);
	} catch {
		// The window can still use the flag it already applied.
	}
	const url = new URL(window.location.href);
	if (next) url.searchParams.set("admin", "1");
	else url.searchParams.delete("admin");
	const nextUrl = `${url.pathname}${url.search}${url.hash}`;
	const current = `${window.location.pathname}${window.location.search}${window.location.hash}`;
	if (current !== nextUrl) history.replaceState(null, "", nextUrl);
}

function readAdmin(params: URLSearchParams): boolean {
	try {
		if (sessionStorage.getItem(ADMIN_FLAG) === "1") return true;
	} catch {
		// A blocked store still leaves the query flag.
	}
	return params.get("admin") === "1";
}

function toggleMark(index: number) {
	if (!admin) return;
	const mark = detectMarks.find((item) => item.index === index);
	if (!mark?.embed) return;
	selected = selected.includes(index)
		? selected.filter((item) => item !== index)
		: [...selected, index];
}

async function openCatalogueCapture(mark: DetectMark) {
	if (!mark.catalogueMiss) return;
	const gen = ++captureGen;
	captureMark = mark;
	captureIc = "";
	captureImage = "";
	captureNote = "";
	captureBusy = true;
	if (classNames.length === 0) void loadClassNames();
	try {
		const body = await invoke<{ image?: unknown }>("preview_catalogue_crop", {
			x1: mark.sensor[0],
			y1: mark.sensor[1],
			x2: mark.sensor[2],
			y2: mark.sensor[3],
		});
		if (closed || gen !== captureGen) return;
		captureImage = typeof body.image === "string" ? body.image : "";
		if (!captureImage) captureNote = "Could not read that crop.";
	} catch (err: unknown) {
		if (closed || gen !== captureGen) return;
		captureImage = "";
		captureNote = invokeMessage(err, "Could not read that crop.");
	} finally {
		if (gen === captureGen) captureBusy = false;
	}
}

async function saveCatalogueCrop() {
	const mark = captureMark;
	const ic = captureIc.trim();
	if (!mark || captureBusy || ic.length === 0 || !captureImage) return;
	const gen = captureGen;
	captureBusy = true;
	captureNote = "";
	try {
		const result = await invoke<{ message?: unknown }>("save_catalogue_crop", {
			ic,
			x1: mark.sensor[0],
			y1: mark.sensor[1],
			x2: mark.sensor[2],
			y2: mark.sensor[3],
		});
		if (closed || gen !== captureGen) return;
		const message = typeof result.message === "string" ? result.message.trim() : "";
		closeCatalogue();
		detectNote = message || "Saved to the catalogue.";
	} catch (err: unknown) {
		if (closed || gen !== captureGen) return;
		captureNote = invokeMessage(err, "Could not save the crop.");
	} finally {
		if (gen === captureGen) captureBusy = false;
	}
}

async function loadClassNames() {
	try {
		const data = await invoke<{ classes?: unknown }>("model_class_names");
		if (closed) return;
		classNames = Array.isArray(data.classes)
			? data.classes.filter(
					(item): item is string =>
						typeof item === "string" && item.trim().length > 0,
				)
			: [];
	} catch {
		if (!closed) classNames = [];
	}
}

async function commitMarquee(region: Region) {
	const source = toSource(region);
	const gen = ++marqueeGen;
	marquee = region;
	marqueeDraft = null;
	marqueeReady = false;
	marqueeNote = "";
	if (!source || source.w < 1 || source.h < 1) {
		marqueeNote = "Draw a larger region.";
		return;
	}
	marqueeBusy = true;
	try {
		await invoke("embed_marquee", {
			x: source.x,
			y: source.y,
			w: source.w,
			h: source.h,
		});
		if (closed || gen !== marqueeGen) return;
		marqueeReady = true;
	} catch (err: unknown) {
		if (closed || gen !== marqueeGen) return;
		marqueeReady = false;
		marqueeNote = invokeMessage(err, "Could not read that region.");
	} finally {
		if (gen === marqueeGen) marqueeBusy = false;
	}
}

async function saveMarquee() {
	const ic = marqueeIc.trim();
	if (!admin || marqueeBusy || !marqueeReady || !marquee || ic.length === 0)
		return;
	const gen = marqueeGen;
	const replace = saveMode === "replace";
	marqueeBusy = true;
	marqueeNote = "";
	try {
		const message = await invoke<string>("save_marquee_reference", {
			ic,
			replace,
		});
		if (closed || gen !== marqueeGen) return;
		marquee = null;
		marqueeReady = false;
		marqueeNote = message.trim() || "Saved.";
	} catch (err: unknown) {
		if (closed || gen !== marqueeGen) return;
		marqueeNote = invokeMessage(err, "Could not save the reference.");
	} finally {
		if (gen === marqueeGen) marqueeBusy = false;
	}
}

async function saveReferences() {
	if (!admin || saveBusy || selected.length === 0) return;
	const gen = saveGeneration;
	const indices = selected.slice();
	const replace = saveMode === "replace";
	saveBusy = true;
	saveNote = "";
	try {
		const message = await invoke<string>("save_detection_references", {
			indices,
			replace,
		});
		if (closed || gen !== saveGeneration) return;
		saveNote = message.trim() || "Saved.";
		selected = [];
	} catch (err: unknown) {
		if (closed || gen !== saveGeneration) return;
		saveNote = invokeMessage(err, "Could not save the reference.");
	} finally {
		if (gen === saveGeneration) saveBusy = false;
	}
}

async function takeSnapshot() {
	if (snapshotOpen || snapshotBusy) return;
	snapshotBusy = true;
	snapshotSaved = "";
	snapshotMessage = "";
	try {
		const body = await invoke<PreviewBody>("capture_snapshot", { index });
		if (body.status === "fail") {
			snapshotSaved = body.message || "The camera did not open.";
			return;
		}
		if (!lastFrame) {
			snapshotSaved = "No preview frame.";
			return;
		}
		const suggestion = await invoke<TrainingSuggestion>(
			"suggest_training_filename",
		);
		snapshotImage = lastFrame;
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
	applyAdmin(readAdmin(params));
	void retitle(index);
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.onmessage = (event) => {
		if (event.data && typeof event.data.admin === "boolean") {
			applyAdmin(event.data.admin);
		}
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
	channel.postMessage({ ready: true });
	const page = pageEl;
	page?.addEventListener("wheel", onWheel, { passive: false });
	const observer = new ResizeObserver(() => fitFrame());
	if (viewportEl) observer.observe(viewportEl);
	void refreshModel();
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
	if (aimFrame) cancelAnimationFrame(aimFrame);
	void invoke("release_preview_frame").catch(() => {
		// The window is closing. The held still can stay until the process exits.
	});
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

<svelte:window
	on:keydown={onKeyDown}
	on:keyup={onKeyUp}
	on:blur={releaseSpace}
	on:focus={refreshModel}
/>

<div bind:this={pageEl} class="relative flex h-dvh flex-col overflow-hidden bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex min-w-0 items-center gap-2">
			<h1 class="shrink-0 text-sm font-semibold tracking-tight">Camera {index}</h1>
			{#if admin}
				<button
					type="button"
					class="inline-flex h-7 shrink-0 items-center border px-2 text-xs font-semibold disabled:opacity-50 {scaleMode
						? 'border-amber-300 bg-amber-400 text-neutral-950'
						: 'border-neutral-600 bg-neutral-800 text-neutral-100'}"
					aria-pressed={scaleMode}
					aria-label="Set scale"
					title={painted
						? "Draw a line of a known length"
						: "Wait for a preview frame"}
					disabled={!painted || snapshotOpen}
					on:click={toggleScaleMode}
				>
					{scaleMode ? "Cancel scale" : "Set scale"}
				</button>
			{/if}
			{#if scaleText}
				<span class="truncate font-mono text-xs text-neutral-200" title={scaleText}>{scaleText}</span>
			{/if}
		</div>
		<div class="flex min-w-0 items-center gap-2">
			<button
				type="button"
				class="inline-flex h-7 shrink-0 items-center border px-2 text-xs font-semibold {measureOn
					? 'border-blue-500 bg-blue-600 text-white'
					: 'border-neutral-600 bg-neutral-800 text-neutral-100'}"
				aria-pressed={measureOn}
				aria-label="Measure"
				disabled={snapshotOpen}
				on:click={toggleMeasure}
			>
				Measure
			</button>
			{#if admin}
				<button
					type="button"
					class="inline-flex h-7 w-7 shrink-0 items-center justify-center border text-xs font-semibold {crosshairs
						? 'border-blue-500 bg-blue-600 text-white'
						: 'border-neutral-600 bg-neutral-800 text-neutral-100'}"
					aria-pressed={crosshairs}
					aria-label="Crosshairs"
					title="Crosshairs"
					disabled={snapshotOpen}
					on:click={toggleCrosshairs}
				>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 24 24"
						class="h-4 w-4"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						stroke-linecap="round"
						stroke-linejoin="round"
						aria-hidden="true"
					>
						<circle cx="12" cy="12" r="8" />
						<line x1="12" y1="2" x2="12" y2="6" />
						<line x1="12" y1="18" x2="12" y2="22" />
						<line x1="2" y1="12" x2="6" y2="12" />
						<line x1="18" y1="12" x2="22" y2="12" />
						<circle cx="12" cy="12" r="1.5" fill="currentColor" stroke="none" />
					</svg>
				</button>
			{/if}
			{#if status}
				<p class="min-w-0 truncate text-xs tabular-nums text-neutral-200">{status}</p>
			{:else}
				<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Preview</p>
			{/if}
		</div>
	</header>
	<p class="shrink-0 border-b border-neutral-800 px-4 py-1 text-[11px] text-neutral-500">
		Exposure, gain, and focus · Ctrl+wheel zoom · Space+drag pan · Enter snapshot · \ detect
	</p>
	{#if scaleMode}
		<p class="shrink-0 px-4 py-1 text-xs text-amber-200">Draw a straight line of a known length.</p>
	{:else if measureOn}
		<p class="shrink-0 px-4 py-1 text-xs {scale ? 'text-neutral-200' : 'text-amber-200'}">
			{measureLabel || (scale ? "Draw a straight line to measure." : "No scale set for this camera. Draw a line to see pixels.")}
		</p>
	{/if}
	{#if snapshotSaved}
		<p class="shrink-0 px-4 py-1 text-xs text-neutral-200">{snapshotSaved}</p>
	{/if}
	<div
		bind:this={viewportEl}
		class="relative min-h-0 flex-1 select-none overflow-hidden bg-black"
		class:cursor-crosshair={marqueeHit}
		class:cursor-pointer={catalogueHit && !marqueeHit}
		class:cursor-grab={spaceDown && zoom > 1 && !panning}
		class:cursor-grabbing={panning}
		on:pointerdown={onPointerDown}
		on:pointermove={onPointerMove}
		on:pointerup={onPointerUp}
		on:pointercancel={onPointerCancel}
		on:pointerleave={onViewportLeave}
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
		{#if painted && frameW > 0 && frameH > 0}
			<div
				class="pointer-events-none absolute z-[3]"
				style:left="{fitLeft}px"
				style:top="{fitTop}px"
				style:width="{fitW}px"
				style:height="{fitH}px"
				style:transform="translate({panX}px, {panY}px) scale({zoom})"
				style:transform-origin="0 0"
			>
				<MeasureOverlay
					active={lineActive}
					imageWidth={frameW}
					imageHeight={frameH}
					line={measureLine}
					label={measureLabel}
					on:line={onMeasureLine}
				/>
			</div>
			<div
				bind:this={frameEl}
				class="absolute z-[2] touch-none"
				class:pointer-events-none={!marqueeHit && !catalogueHit}
				class:cursor-crosshair={marqueeHit}
				class:cursor-pointer={catalogueHit && !marqueeHit}
				style:left="{fitLeft}px"
				style:top="{fitTop}px"
				style:width="{fitW}px"
				style:height="{fitH}px"
				style:transform="translate({panX}px, {panY}px) scale({zoom})"
				style:transform-origin="0 0"
				data-preview-frame
				data-marquee-hit={marqueeHit ? "true" : "false"}
				data-catalogue-hit={catalogueHit ? "true" : "false"}
			>
				{#if overlayW > 0 && overlayH > 0 && (shownMarquee || (crosshairs && aim))}
					<svg
						class="pointer-events-none absolute inset-0 h-full w-full"
						viewBox="0 0 {overlayW} {overlayH}"
						preserveAspectRatio="none"
						aria-hidden="true"
					>
						{#if crosshairs && aim}
							<line
								x1={aim.x}
								y1="0"
								x2={aim.x}
								y2={overlayH}
								stroke="#ef4444"
								stroke-width="1"
								vector-effect="non-scaling-stroke"
								data-crosshair="v"
							/>
							<line
								x1="0"
								y1={aim.y}
								x2={overlayW}
								y2={aim.y}
								stroke="#ef4444"
								stroke-width="1"
								vector-effect="non-scaling-stroke"
								data-crosshair="h"
							/>
						{/if}
						{#if shownMarquee}
							<rect
								x={shownMarquee.x}
								y={shownMarquee.y}
								width={shownMarquee.w}
								height={shownMarquee.h}
								fill="rgba(250, 204, 21, 0.18)"
								stroke="#facc15"
								stroke-width="2"
								stroke-dasharray="8 4"
								vector-effect="non-scaling-stroke"
								data-marquee
							/>
						{/if}
					</svg>
				{/if}
			</div>
		{/if}
		{#if detectImage}
			<img
				src={detectImage}
				alt=""
				class="pointer-events-none absolute"
				style:left="{fitLeft}px"
				style:top="{fitTop}px"
				style:width="{fitW}px"
				style:height="{fitH}px"
				style:transform="translate({panX}px, {panY}px) scale({zoom})"
				style:transform-origin="0 0"
			/>
			<svg
				class="pointer-events-none absolute"
				style:left="{fitLeft}px"
				style:top="{fitTop}px"
				style:width="{fitW}px"
				style:height="{fitH}px"
				style:transform="translate({panX}px, {panY}px) scale({zoom})"
				style:transform-origin="0 0"
				viewBox="0 0 {detectW} {detectH}"
				preserveAspectRatio="none"
				aria-hidden="true"
			>
				{#each detectMarks as mark, markIndex (`${markIndex}:${mark.ic}`)}
					{#if selected.includes(mark.index)}
						<rect
							x={mark.x}
							y={mark.y}
							width={mark.w}
							height={mark.h}
							fill="none"
							stroke="#ffffff"
							stroke-width="5"
							vector-effect="non-scaling-stroke"
							pointer-events="none"
						/>
					{/if}
					<rect
						x={mark.x}
						y={mark.y}
						width={mark.w}
						height={mark.h}
						fill="none"
						stroke={mark.color}
						stroke-width="2"
						stroke-dasharray={mark.catalogueMiss ? "6 3" : "0"}
						vector-effect="non-scaling-stroke"
						pointer-events="none"
						data-detection={mark.ic}
						data-catalogue-miss={mark.catalogueMiss ? "true" : "false"}
						data-selected={selected.includes(mark.index) ? "true" : "false"}
					/>
					<text
						x={mark.x}
						y={mark.y + 16}
						fill={mark.color}
						font-size="16"
						font-family="ui-monospace, monospace"
						font-weight="600"
						pointer-events="none"
					>
						{mark.ic}
					</text>
				{/each}
			</svg>
			<div
				class="absolute left-3 top-3 z-[4] max-h-80 max-w-xs overflow-auto border border-neutral-700 bg-neutral-950/90 px-3 py-2"
				on:pointerdown|stopPropagation
			>
				<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
					Detections
				</p>
				{#if detectMarks.length === 0}
					<p class="mt-1 text-sm text-neutral-200">No detections.</p>
				{:else}
					<ul class="mt-1 space-y-1">
						{#each detectMarks as mark, markIndex (`${markIndex}:${mark.ic}:${mark.confidence}`)}
							<li>
								{#if admin && mark.embed}
									<button
										type="button"
										class="w-full px-1 text-left font-mono text-sm text-neutral-100"
										class:bg-neutral-800={selected.includes(mark.index)}
										aria-pressed={selected.includes(mark.index)}
										on:click={() => toggleMark(mark.index)}
									>
										{mark.ic}
										<span class="text-neutral-400">{mark.confidence.toFixed(2)}</span>
									</button>
								{:else}
									<p class="px-1 font-mono text-sm text-neutral-100">
										{mark.ic}
										<span class="text-neutral-400">{mark.confidence.toFixed(2)}</span>
									</p>
								{/if}
								{#if mark.catalogueMiss}
									<button
										type="button"
										class="mt-0.5 px-1 text-left text-xs font-semibold text-amber-200"
										on:click={() => openCatalogueCapture(mark)}
									>
										Save to catalogue
									</button>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				{#if admin && detectMarks.some((mark) => mark.embed)}
					<div class="mt-2 flex flex-wrap items-center gap-2">
						<label class="flex items-center gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							For this IC
							<select
								bind:value={saveMode}
								class="border border-neutral-600 bg-neutral-950 px-1 py-1 font-sans text-xs font-medium normal-case tracking-normal text-neutral-100"
								disabled={saveBusy}
								title="Append adds these crops to the class. Replace keeps only these crops for that class."
							>
								<option value="append">Append</option>
								<option value="replace">Replace</option>
							</select>
						</label>
						<button
							type="button"
							class="border border-blue-500 bg-blue-600 px-2 py-1 text-xs font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
							disabled={saveBusy || selected.length === 0}
							on:click={saveReferences}
						>
							{saveBusy ? "Saving..." : "Save as reference"}
						</button>
					</div>
					{#if saveNote}
						<p class="mt-1 text-xs text-neutral-200">{saveNote}</p>
					{:else if selected.length === 0}
						<p class="mt-1 text-xs text-neutral-400">Select one or more boxes.</p>
					{/if}
				{:else if admin && detectMarks.length > 0}
					<p class="mt-2 text-xs text-amber-200">DINOv2 is not loaded.</p>
				{/if}
				{#if admin}
					<p class="mt-2 text-xs text-neutral-400">Drag a region to capture a missed part.</p>
				{/if}
				<button
					type="button"
					class="mt-2 border border-neutral-600 bg-neutral-800 px-2 py-1 text-xs font-semibold text-neutral-100"
					on:click={clearDetect}
				>
					Clear
				</button>
			</div>
			{#if admin && (marquee || marqueeNote)}
				<div
					class="absolute right-3 top-3 z-[5] w-56 border border-neutral-700 bg-neutral-950/95 px-3 py-2"
					on:pointerdown|stopPropagation
				>
					<label
						class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400"
						for="marquee-ic"
					>
						IC
						<input
							id="marquee-ic"
							bind:value={marqueeIc}
							list="preview-class-names"
							class="border border-neutral-600 bg-neutral-950 px-2 py-1 font-mono text-sm font-medium normal-case tracking-normal text-neutral-100"
							disabled={marqueeBusy}
							autocomplete="off"
							spellcheck="false"
						/>
					</label>
					<datalist id="preview-class-names">
						{#each classNames as name, nameIndex (nameIndex)}
							<option value={name}></option>
						{/each}
					</datalist>
					<div class="mt-2 flex items-center gap-2">
						<select
							bind:value={saveMode}
							class="min-w-0 flex-1 border border-neutral-600 bg-neutral-950 px-1 py-1 text-xs font-medium text-neutral-100"
							disabled={marqueeBusy}
							title="Append adds this crop to the class. Replace keeps only this crop for that class."
							aria-label="For this IC"
						>
							<option value="append">Append</option>
							<option value="replace">Replace</option>
						</select>
						<button
							type="button"
							class="border border-blue-500 bg-blue-600 px-2 py-1 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
							aria-label="Save this region"
							disabled={marqueeBusy || !marqueeReady || marqueeIc.trim().length === 0}
							on:click={saveMarquee}
						>
							✓
						</button>
						<button
							type="button"
							class="border border-neutral-600 bg-neutral-800 px-2 py-1 text-sm font-semibold text-neutral-100"
							aria-label="Discard this region"
							on:click={() => forgetMarquee("vector")}
						>
							✕
						</button>
					</div>
					{#if marqueeBusy && !marqueeReady}
						<p class="mt-1 text-xs text-neutral-300">Reading the region...</p>
					{:else if marqueeNote}
						<p class="mt-1 text-xs text-neutral-200">{marqueeNote}</p>
					{/if}
				</div>
			{/if}
		{/if}
		{#if captureMark}
			<div
				class="absolute inset-0 z-[7] flex items-center justify-center bg-black/60 px-4"
				role="dialog"
				aria-label="Save catalogue crop"
				on:pointerdown|stopPropagation
			>
				<div class="w-72 border border-neutral-700 bg-neutral-950 px-3 py-3">
					<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						Catalogue
					</p>
					<p class="mt-1 font-mono text-sm text-neutral-100">{captureMark.ic}</p>
					{#if captureImage}
						<img
							src={captureImage}
							alt="Crop"
							class="mt-2 max-h-40 w-full border border-neutral-800 object-contain"
						/>
					{:else if captureBusy}
						<p class="mt-2 text-xs text-neutral-300">Reading the crop...</p>
					{/if}
					<label
						class="mt-2 flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400"
						for="catalogue-ic"
					>
						IC
						<input
							id="catalogue-ic"
							bind:value={captureIc}
							list="catalogue-class-names"
							class="border border-neutral-600 bg-neutral-950 px-2 py-1 font-mono text-sm font-medium normal-case tracking-normal text-neutral-100"
							disabled={captureBusy}
							autocomplete="off"
							spellcheck="false"
						/>
					</label>
					<datalist id="catalogue-class-names">
						{#each classNames as name, nameIndex (nameIndex)}
							<option value={name}></option>
						{/each}
					</datalist>
					<div class="mt-2 flex items-center gap-2">
						<button
							type="button"
							class="border border-blue-500 bg-blue-600 px-2 py-1 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
							disabled={captureBusy || captureIc.trim().length === 0 || !captureImage}
							on:click={saveCatalogueCrop}
						>
							{captureBusy && captureImage ? "Saving..." : "Save"}
						</button>
						<button
							type="button"
							class="border border-neutral-600 bg-neutral-800 px-2 py-1 text-sm font-semibold text-neutral-100"
							disabled={captureBusy}
							on:click={closeCatalogue}
						>
							Cancel
						</button>
					</div>
					{#if captureNote}
						<p class="mt-1 text-xs text-amber-200">{captureNote}</p>
					{/if}
				</div>
			</div>
		{/if}
		{#if failed || !painted}
			<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
				{message || "Opening camera…"}
			</p>
		{/if}
		{#if admin && crosshairs && aim}
			<p
				class="pointer-events-none absolute z-[6] bg-neutral-950/80 px-2 py-1 font-mono text-xs leading-5 text-neutral-100"
				style:left="{aim.left}px"
				style:top="{aim.top}px"
				data-crosshair-readout
			>
				x {formatPxMm(aim.x, overlayW, scale)}<br />
				y {formatPxMm(aim.y, overlayW, scale)}
				{#if drawing && marqueeDraft}
					<br />
					w {formatPxMm(marqueeDraft.w, overlayW, scale)}<br />
					h {formatPxMm(marqueeDraft.h, overlayW, scale)}
				{/if}
			</p>
		{/if}
		<div
			class="absolute inset-x-0 bottom-0 z-[4] flex flex-col gap-2 bg-neutral-950/80 px-4 py-2"
			on:pointerdown|stopPropagation
		>
			<div class="flex items-center gap-3">
				<label class="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-widest text-neutral-300" for="preview-exposure">
					Exposure
					<input
						id="preview-exposure"
						type="number"
						class="w-20 border border-neutral-600 bg-neutral-950 px-2 py-1 font-mono text-sm font-medium normal-case tracking-normal text-neutral-100"
						min="-13"
						max="20000"
						step="1"
						value={exposureAbsolute}
						aria-label="Exposure"
						disabled={snapshotOpen}
						on:change={onExposureChange}
					/>
				</label>
				<label class="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-widest text-neutral-300" for="preview-gain">
					Gain
					<input
						id="preview-gain"
						type="number"
						class="w-16 border border-neutral-600 bg-neutral-950 px-2 py-1 font-mono text-sm font-medium normal-case tracking-normal text-neutral-100"
						min="0"
						max="255"
						step="1"
						value={gain}
						aria-label="Gain"
						disabled={snapshotOpen}
						on:change={onGainChange}
					/>
				</label>
				{#if controlNotice}
					<span class="min-w-0 truncate text-xs text-amber-300" title={controlNotice}>{controlNotice}</span>
				{/if}
			</div>
			<div class="flex items-center gap-3">
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
			<div class="ml-auto flex shrink-0 items-center gap-2">
				<button
					type="button"
					class="inline-flex min-h-9 items-center gap-2 border border-blue-500 bg-blue-600 px-3 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
					title="Detect"
					disabled={!modelReady || detectBusy || snapshotOpen}
					on:click={runDetect}
				>
					{detectBusy ? "Detecting..." : "Detect"}
				</button>
				{#if modelChecked && !modelReady}
					<span class="max-w-40 text-xs text-amber-200">No model loaded.</span>
				{:else if detectNote}
					<span class="max-w-40 truncate text-xs text-amber-200" title={detectNote}>{detectNote}</span>
				{/if}
			</div>
			</div>
		</div>
	</div>
	{#if scaleModal}
		<div class="absolute inset-0 z-20 flex items-center justify-center bg-black/70 p-4">
			<form
				class="w-full max-w-md border border-neutral-700 bg-neutral-900 p-4"
				role="dialog"
				aria-modal="true"
				aria-labelledby="scale-title"
				on:submit|preventDefault={confirmScale}
			>
				<h2 id="scale-title" class="text-base font-semibold tracking-tight">Known length</h2>
				<p class="mt-2 text-sm text-neutral-300">
					The line is {pendingScale ? Math.round(pendingScale.pixels) : 0} px. Enter its real length.
				</p>
				<label class="mt-3 flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400" for="scale-length">
					Length
					<span class="flex items-center gap-2 normal-case tracking-normal">
						<input
							bind:this={scaleInput}
							bind:value={scaleMm}
							id="scale-length"
							class="min-w-0 flex-1 border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-base font-medium text-neutral-100"
							inputmode="decimal"
							autocomplete="off"
							spellcheck="false"
							disabled={scaleSaving}
						/>
						<span class="font-mono text-sm text-neutral-300">mm</span>
					</span>
				</label>
				{#if scaleError}
					<p class="mt-2 text-sm text-amber-200">{scaleError}</p>
				{/if}
				<div class="mt-4 flex flex-wrap items-center gap-2">
					<button
						type="submit"
						class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
						disabled={scaleSaving}
					>
						{scaleSaving ? "Saving..." : "Save"}
					</button>
					<button
						type="button"
						class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
						on:click={cancelScaleMode}
						disabled={scaleSaving}
					>
						Cancel
					</button>
				</div>
			</form>
		</div>
	{/if}
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
