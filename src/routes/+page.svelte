<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { onDestroy, onMount, tick } from "svelte";

type Part = { ic: string; des: string; qty: number };
type InspectView = {
	status: string;
	message?: string;
	kitCode?: string;
	des?: string;
	parts?: Part[];
	kitsFound?: number;
	expectedKitCount?: number;
	info?: string;
	image?: string;
};

type WatchBody = InspectView & {
	snapshot?: boolean;
	barcode?: string;
	image?: string;
	jobCardNumber?: string;
};

type SnapshotView = {
	kitCode: string;
	image: string;
	source: string;
	takenAt: string;
	detected?: boolean;
};

type DetectRoi = {
	x: number;
	y: number;
	w: number;
	h: number;
};

type Detection = {
	// Part IC from the YOLO class name. Kit codes come from the barcode lookup.
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

// Same order the sidecar used when it painted boxes. OpenCV stored these as BGR.
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

type BomView = {
	kitCode: string;
	des?: string;
	parts: Part[];
};

type DetectedCount = {
	ic: string;
	count: number;
};

const DEFAULT_SCAN_SECONDS = 8;
// Settings unlock is the admin flag for this tab. It is not stored in config.
const ADMIN_FLAG = "ls-vision-settings-open";

let barcode = "";
let bomSource: "json" | "mssql" | "api" = "json";
let bomSourceReady: Promise<void> | null = null;
let result: InspectView | null = null;
let bom: BomView | null = null;
let loading = false;
let cameraIndex = 0;
// Set when the saved camera is missing, so focus does not put that index back.
let skippedSavedIndex: number | null = null;
let cameraIndexes: number[] = [];
let camerasReady = false;
let cameraLookupFailed = false;
let camerasPromise: Promise<boolean> | null = null;
// The live view reads these names so it updates when the camera list arrives.
$: camerasMissing = camerasReady && cameraIndexes.length === 0;
$: cameraFeedOn =
	(camerasReady && cameraIndexes.includes(Number(cameraIndex))) ||
	(cameraLookupFailed && !camerasReady);
let feedOn = true;
let snapshot: SnapshotView | null = null;
let detections: Detection[] = [];
let detectMessage = "";
let detectState: "idle" | "running" | "done" = "idle";
let detectGeneration = 0;
let testMode = false;
let admin = false;
let detectRoi: DetectRoi | null = null;
let draftRoi: DetectRoi | null = null;
let draftMark: {
	left: number;
	top: number;
	width: number;
	height: number;
} | null = null;
let drawing = false;
let savingRoi = false;
let roiMessage = "";
let roiDrag: { x: number; y: number } | null = null;
let paneImage: HTMLImageElement | null = null;
let paneFrame: HTMLElement | null = null;
let paneWidth = 0;
let paneHeight = 0;
let frameWidth = 0;
let frameHeight = 0;
let frameObserver: ResizeObserver | null = null;
let imagePicker: HTMLInputElement | null = null;
let imagePick: ((file: File | null) => void) | null = null;
let streamMs = DEFAULT_SCAN_SECONDS * 1000;
let streamLeft = DEFAULT_SCAN_SECONDS;
let streamTimer: ReturnType<typeof setInterval> | null = null;
let streamEpoch = 0;
let streamRetries = 0;
let streamRetry: ReturnType<typeof setTimeout> | null = null;
let ignoreStreamErrorUntil = 0;
let watchInFlight = false;
let pageAlive = true;
let sidecarUp = false;
let sidecarStarting = false;
let sidecarSeconds = 0;
let sidecarTick: ReturnType<typeof setInterval> | null = null;
let healthTimer: ReturnType<typeof setInterval> | null = null;
let erpName = "";
let erpUrl = "";
let erpOpen = false;
let iframeFailed = false;
let erpChecking = false;
let erpAttempt = 0;
let erpLoadedUrl = "";
let erpCloseButton: HTMLButtonElement | null = null;
let modelName = "best.pt";

function stopWatch() {
	if (streamTimer !== null) {
		clearInterval(streamTimer);
		streamTimer = null;
	}
}

function startWatch() {
	// A dead sidecar uses the same paused countdown as Stop feed.
	if (snapshot || camerasMissing || !sidecarUp || !feedOn) return;
	stopWatch();
	streamLeft = streamMs / 1000;
	void watchFeed();
	streamTimer = setInterval(() => {
		if (!feedOn || snapshot || !sidecarUp) return;
		if (streamLeft <= 1) {
			streamLeft = streamMs / 1000;
			void watchFeed();
		} else {
			streamLeft -= 1;
		}
	}, 1000);
}

function clearStreamRetry() {
	if (streamRetry !== null) {
		clearTimeout(streamRetry);
		streamRetry = null;
	}
}

function showLiveFeed() {
	if (camerasMissing) return;
	clearStreamRetry();
	streamRetries = 0;
	// Changing the img src aborts the previous request and fires its error.
	ignoreStreamErrorUntil = Date.now() + 1000;
	streamEpoch += 1;
	startWatch();
}

function retryStream() {
	// A request that started before the sidecar was listening stays broken.
	// Stop/Start feed fixes that by loading a new stream URL. Do the same once
	// the sidecar is up, and again if that attempt fails.
	if (Date.now() < ignoreStreamErrorUntil) return;
	if (!pageAlive || !feedOn || snapshot || !sidecarUp || streamRetry !== null)
		return;
	if (streamRetries >= 3) return;
	streamRetry = setTimeout(() => {
		streamRetry = null;
		if (!pageAlive || !feedOn || snapshot || !sidecarUp) return;
		streamRetries += 1;
		ignoreStreamErrorUntil = Date.now() + 1000;
		streamEpoch += 1;
	}, 2000);
}

function toggleFeed() {
	feedOn = !feedOn;
	if (feedOn) showLiveFeed();
	else {
		clearStreamRetry();
		stopWatch();
	}
}

function resumeLiveFeed() {
	cancelRoiDraw();
	detectGeneration += 1;
	snapshot = null;
	detections = [];
	detectMessage = "";
	detectState = "idle";
	paneWidth = 0;
	paneHeight = 0;
	if (feedOn) startWatch();
}

function holdStill(image: string, kitCode: string) {
	cancelRoiDraw();
	detectGeneration += 1;
	paneWidth = 0;
	paneHeight = 0;
	snapshot = {
		kitCode,
		image,
		source: image,
		takenAt: new Date().toLocaleTimeString(),
		detected: false,
	};
	stopWatch();
	return detectGeneration;
}

function detectionCounts(items: Detection[]): Record<string, number> {
	const counts: Record<string, number> = {};
	for (const item of items) {
		if (!item.ic) continue;
		counts[item.ic] = (counts[item.ic] ?? 0) + 1;
	}
	return counts;
}

function storeBom(view: InspectView) {
	// Only a passing barcode lookup loads a kit. Model class names do not.
	if (view.status !== "pass" || !view.kitCode || !view.parts?.length) return;
	bom = {
		kitCode: view.kitCode,
		des: view.des,
		parts: view.parts,
	};
}

function dropBom() {
	bom = null;
}

function isLoadedBom(view: InspectView | null): boolean {
	return !!bom && !!view?.parts?.length && view.kitCode === bom.kitCode;
}

function partsOutsideBom(
	items: Detection[],
	parts: Part[] | null,
): DetectedCount[] {
	const counts = detectionCounts(items);
	const known = new Set((parts ?? []).map((part) => part.ic));
	return Object.entries(counts)
		.filter(([ic]) => !known.has(ic))
		.sort((a, b) => a[0].localeCompare(b[0]))
		.map(([ic, count]) => ({ ic, count }));
}

function matchSummary(parts: Part[], items: Detection[]): string {
	const counts = detectionCounts(items);
	let matched = 0;
	for (const part of parts) {
		if ((counts[part.ic] ?? 0) === part.qty) matched += 1;
	}
	const lines = parts.length === 1 ? "line" : "lines";
	return `${matched} of ${parts.length} BoM ${lines} match`;
}

function asDetections(value: unknown): Detection[] {
	if (!Array.isArray(value)) return [];
	const found: Detection[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const row = item as Detection;
		if (typeof row.ic !== "string" || !row.ic) continue;
		found.push({
			ic: row.ic,
			confidence: typeof row.confidence === "number" ? row.confidence : 0,
			bbox: Array.isArray(row.bbox) ? row.bbox : [],
		});
	}
	return found;
}

function detectedImageSrc(encoded: string): string {
	const text = encoded.trim();
	if (text.startsWith("data:")) return text;
	return `data:image/jpeg;base64,${text.replace(/\s+/g, "")}`;
}

function colorForIc(ic: string): string {
	const bytes = new TextEncoder().encode(ic);
	let total = 0;
	for (const byte of bytes) total += byte;
	return PART_COLORS[total % PART_COLORS.length];
}

function detectionMarks(
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

async function runDetect(generation: number, body: { image?: string }) {
	if (generation !== detectGeneration) return;
	detectState = "running";
	detections = [];
	detectMessage = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/detect", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify(body),
		});
		const payload = (await res.json()) as {
			detections?: unknown;
			image_base64?: unknown;
			message?: unknown;
		};
		if (generation !== detectGeneration || !pageAlive) return;
		if (!res.ok) {
			throw new Error(
				typeof payload.message === "string" && payload.message
					? payload.message
					: "Detect failed",
			);
		}
		detections = asDetections(payload.detections);
		detectMessage = typeof payload.message === "string" ? payload.message : "";
		const encoded = payload.image_base64;
		if (snapshot) {
			snapshot = {
				...snapshot,
				image:
					typeof encoded === "string" && encoded
						? detectedImageSrc(encoded)
						: snapshot.image,
				detected: detections.length > 0,
			};
		}
		detectState = "done";
	} catch (err: unknown) {
		if (generation !== detectGeneration || !pageAlive) return;
		detectMessage = errorMessage(err, "Detect failed");
		detectState = "done";
	}
}

function inspectView(body: WatchBody): InspectView {
	return {
		status: body.status,
		message: body.message,
		kitCode: body.kitCode,
		des: body.des,
		parts: body.parts,
		kitsFound: body.kitsFound,
		expectedKitCount: body.expectedKitCount,
		info: body.info,
	};
}

async function showSnapshot(body: WatchBody) {
	if (!body.image) return;
	const kitCode = body.kitCode || body.barcode || barcode;
	// API keeps the decoded job card in the field. The still is labelled with the kit.
	if (bomSource === "api") {
		const card = body.jobCardNumber || body.barcode;
		if (card) barcode = card;
	} else {
		barcode = kitCode;
	}
	result = inspectView(body);
	storeBom(result);
	const generation = holdStill(body.image, kitCode);
	await runDetect(generation, { image: body.image });
}

async function watchFeed() {
	if (!feedOn || snapshot || watchInFlight || loading) return;
	// Wait for the camera list. A snapshot open before that would grab index 0
	// and make the probe fight the reader.
	if (!cameraFeedOn) return;
	watchInFlight = true;
	const index = cameraIndex;
	try {
		// Test mode is decided with the saved source. The sidecar skips decode
		// for that mode and still logs the path. Every other source decodes.
		await ensureBomSource();
		await loadTestMode();
		if (!pageAlive || !feedOn || snapshot || loading || index !== cameraIndex) {
			return;
		}
		const res = await fetch(
			`http://127.0.0.1:8008/camera/snapshot?index=${index}`,
		);
		if (!res.ok) return;
		const body = (await res.json()) as WatchBody;
		if (
			!pageAlive ||
			loading ||
			!feedOn ||
			snapshot ||
			testMode ||
			index !== cameraIndex ||
			!body.snapshot ||
			!body.image
		) {
			return;
		}
		await showSnapshot(body);
	} catch {
		// A missed poll leaves the last still and the last BoM result.
	} finally {
		watchInFlight = false;
	}
}

async function loadScan() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/scan");
		if (!res.ok) return;
		const data = await res.json();
		const seconds = Number(data.seconds);
		if (!Number.isInteger(seconds) || seconds < 1 || seconds > 60) return;
		streamMs = seconds * 1000;
		if (feedOn && !snapshot) streamLeft = seconds;
		else if (streamLeft > seconds) streamLeft = seconds;
	} catch {
		// A missed read keeps the current interval.
	}
}

async function loadErp() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp");
		if (!res.ok) return;
		const data = await res.json();
		const nextName = typeof data.name === "string" ? data.name.trim() : "";
		const nextUrl = typeof data.url === "string" ? data.url.trim() : "";
		if (nextUrl !== erpUrl) {
			erpAttempt += 1;
			erpOpen = false;
			iframeFailed = false;
			erpChecking = false;
			erpLoadedUrl = "";
		}
		erpName = nextName;
		erpUrl = nextUrl;
	} catch {
		// A missed read leaves the last button in place.
	}
}

function cameraList(value: unknown): number[] {
	if (!Array.isArray(value)) return [];
	const found: number[] = [];
	for (const item of value) {
		if (typeof item !== "number" || !Number.isInteger(item) || item < 0)
			continue;
		if (!found.includes(item)) found.push(item);
	}
	found.sort((left, right) => left - right);
	return found;
}

function noteCameraLookupFailed() {
	// A later missed read must not clear a list that is already on screen.
	if (camerasReady) return;
	cameraLookupFailed = true;
}

async function loadCamerasOnce(): Promise<boolean> {
	const selected = Number(cameraIndex);
	try {
		const res = await fetch(
			`http://127.0.0.1:8008/cameras?selected=${selected}`,
		);
		if (!pageAlive) return false;
		if (!res.ok) {
			noteCameraLookupFailed();
			return false;
		}
		const data = (await res.json()) as { cameras?: unknown };
		if (!pageAlive) return false;
		const found = cameraList(data.cameras);
		const current = Number(cameraIndex);
		// They picked another index while the probe was running. Keep it.
		if (
			current !== selected &&
			Number.isInteger(current) &&
			current >= 0 &&
			!found.includes(current)
		) {
			cameraIndexes = cameraList([...found, current]);
			camerasReady = true;
			cameraLookupFailed = false;
			return true;
		}
		cameraIndexes = found;
		camerasReady = true;
		cameraLookupFailed = false;
		if (current === selected && found.length > 0 && !found.includes(selected)) {
			skippedSavedIndex = selected;
			cameraIndex = found[0];
		}
		return true;
	} catch {
		if (pageAlive) noteCameraLookupFailed();
		return false;
	}
}

function loadCameras(): Promise<boolean> {
	if (!camerasPromise) {
		camerasPromise = loadCamerasOnce().finally(() => {
			camerasPromise = null;
		});
	}
	return camerasPromise;
}

async function connectCameras() {
	await loadCameras();
	if (!pageAlive || snapshot || !feedOn) return;
	if (camerasMissing) {
		stopWatch();
		clearStreamRetry();
		return;
	}
	if (!cameraFeedOn) return;
	showLiveFeed();
}

async function checkSidecar() {
	if (!pageAlive) return;
	const wasUp = sidecarUp;
	try {
		const res = await fetch("http://127.0.0.1:8008/health");
		sidecarUp = res.ok;
	} catch {
		sidecarUp = false;
	}
	if (sidecarUp) stopSidecarWait();
	else {
		stopWatch();
		clearStreamRetry();
	}
	if (pageAlive && sidecarUp && !wasUp) {
		void loadErp();
		void loadScan();
		void loadTestMode();
		void loadDetectRoi();
		void loadBomSource();
		void loadModel();
		void (async () => {
			await loadCamera();
			if (pageAlive) await connectCameras();
		})();
	}
}

function ensureBomSource(): Promise<void> {
	if (!bomSourceReady) bomSourceReady = loadBomSource();
	return bomSourceReady;
}

async function loadBomSource() {
	try {
		const data = await invoke<{ source?: unknown }>("load_local_config");
		if (
			data.source === "api" ||
			data.source === "mssql" ||
			data.source === "json"
		) {
			bomSource = data.source;
		}
	} catch {
		// A missed read keeps the current label.
	}
}

async function loadTestMode() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/test-mode");
		if (!res.ok) return;
		const data = await res.json();
		testMode = data.enabled === true;
	} catch {
		// A missed read keeps the current mode.
	}
}

function asRoi(value: unknown): DetectRoi | null {
	if (!value || typeof value !== "object") return null;
	const row = value as DetectRoi;
	if (
		typeof row.x !== "number" ||
		typeof row.y !== "number" ||
		typeof row.w !== "number" ||
		typeof row.h !== "number"
	) {
		return null;
	}
	if (![row.x, row.y, row.w, row.h].every((n) => Number.isFinite(n)))
		return null;
	if (row.w < 0.01 || row.h < 0.01) return null;
	if (
		row.x < 0 ||
		row.y < 0 ||
		row.x + row.w > 1.001 ||
		row.y + row.h > 1.001
	) {
		return null;
	}
	return { x: row.x, y: row.y, w: row.w, h: row.h };
}

async function loadCamera() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/camera");
		if (!res.ok) return;
		const data = await res.json();
		const index = Number(data.index);
		if (!Number.isInteger(index) || index < 0 || index > 32) return;
		if (index === skippedSavedIndex) return;
		cameraIndex = index;
	} catch {
		// A missed read keeps the current camera.
	}
}

async function applySavedCamera() {
	const previous = Number(cameraIndex);
	await loadCamera();
	if (!pageAlive || !sidecarUp) return;
	if (Number(cameraIndex) !== previous) await loadCameras();
}

async function saveCameraIndex() {
	skippedSavedIndex = null;
	const index = Number(cameraIndex);
	cameraIndex = index;
	if (!Number.isInteger(index) || index < 0 || index > 32) return;
	try {
		await fetch("http://127.0.0.1:8008/settings/camera", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ index }),
		});
	} catch {
		// The live view still uses the picked index. The next launch reads the last save.
	}
}

function readAdminFlag() {
	admin = sessionStorage.getItem(ADMIN_FLAG) === "1";
	bomSourceReady = loadBomSource();
	void loadTestMode();
	void loadModel();
	if (sidecarUp) void applySavedCamera();
}

function matchModelName(name: string, files: string[]): string {
	const cleaned = name.trim().toLowerCase();
	if (!cleaned) return "";
	return files.find((item) => item.toLowerCase() === cleaned) ?? "";
}

async function loadModel() {
	try {
		const listed = await invoke<unknown>("list_vision_models");
		const config = await invoke<{ model?: unknown }>("load_local_config");
		const files = Array.isArray(listed)
			? listed.filter(
					(item): item is string => typeof item === "string" && item.length > 0,
				)
			: [];
		const stored = typeof config.model === "string" ? config.model : "";
		const match = matchModelName(stored, files);
		const fallback = matchModelName("best.pt", files);
		modelName = match || fallback || "best.pt";
	} catch {
		// A missed read keeps the last model name.
	}
}

function rememberPaneSize() {
	paneWidth = paneImage?.naturalWidth ?? 0;
	paneHeight = paneImage?.naturalHeight ?? 0;
	measureFrame();
}

function measureFrame() {
	frameWidth = paneFrame?.clientWidth ?? 0;
	frameHeight = paneFrame?.clientHeight ?? 0;
}

function zoomFrame(
	drawing: boolean,
	detectRoi: DetectRoi | null,
	paneWidth: number,
	paneHeight: number,
	frameWidth: number,
	frameHeight: number,
): {
	left: number;
	top: number;
	width: number;
	height: number;
} | null {
	// While drawing, keep the whole still so the new box can start anywhere.
	if (drawing || !detectRoi) return null;
	if (paneWidth < 1 || paneHeight < 1 || frameWidth < 1 || frameHeight < 1)
		return null;
	const roiW = detectRoi.w * paneWidth;
	const roiH = detectRoi.h * paneHeight;
	if (roiW < 1 || roiH < 1) return null;
	const scale = Math.min(frameWidth / roiW, frameHeight / roiH);
	const roiViewW = roiW * scale;
	const roiViewH = roiH * scale;
	return {
		left: (frameWidth - roiViewW) / 2 - detectRoi.x * paneWidth * scale,
		top: (frameHeight - roiViewH) / 2 - detectRoi.y * paneHeight * scale,
		width: paneWidth * scale,
		height: paneHeight * scale,
	};
}

// The still reads this value. {@const zoom = zoomFrame()} is compiled with
// untrack, so it kept the first result and a saved region never zoomed.
$: zoom = zoomFrame(
	drawing,
	detectRoi,
	paneWidth,
	paneHeight,
	frameWidth,
	frameHeight,
);

function showDraft(box: DetectRoi) {
	const img = paneImage;
	const frame = paneFrame;
	if (!img || !frame || (box.w <= 0 && box.h <= 0)) {
		draftMark = null;
		return;
	}
	const nw = img.naturalWidth;
	const nh = img.naturalHeight;
	if (nw < 1 || nh < 1) {
		draftMark = null;
		return;
	}
	const rect = img.getBoundingClientRect();
	const frameRect = frame.getBoundingClientRect();
	if (rect.width < 1 || rect.height < 1) {
		draftMark = null;
		return;
	}
	const scale = Math.min(rect.width / nw, rect.height / nh);
	const drawnW = nw * scale;
	const drawnH = nh * scale;
	const originX = rect.left + (rect.width - drawnW) / 2 - frameRect.left;
	const originY = rect.top + (rect.height - drawnH) / 2 - frameRect.top;
	draftMark = {
		left: originX + box.x * drawnW,
		top: originY + box.y * drawnH,
		width: Math.max(2, box.w * drawnW),
		height: Math.max(2, box.h * drawnH),
	};
}

function cancelRoiDraw() {
	drawing = false;
	draftRoi = null;
	draftMark = null;
	roiDrag = null;
}

function beginSetRoi() {
	if (!admin) return;
	if (drawing) {
		cancelRoiDraw();
		roiMessage = "";
		return;
	}
	if (!snapshot) {
		roiMessage = "Hold a snapshot, then draw the region.";
		return;
	}
	roiMessage = "";
	drawing = true;
	draftRoi = null;
	draftMark = null;
	roiDrag = null;
}

function pointInImage(event: PointerEvent): { x: number; y: number } | null {
	const img = paneImage;
	if (!img) return null;
	const rect = img.getBoundingClientRect();
	const nw = img.naturalWidth;
	const nh = img.naturalHeight;
	if (!nw || !nh || rect.width < 1 || rect.height < 1) return null;
	const scale = Math.min(rect.width / nw, rect.height / nh);
	const drawnW = nw * scale;
	const drawnH = nh * scale;
	const originX = rect.left + (rect.width - drawnW) / 2;
	const originY = rect.top + (rect.height - drawnH) / 2;
	return {
		x: Math.min(1, Math.max(0, (event.clientX - originX) / drawnW)),
		y: Math.min(1, Math.max(0, (event.clientY - originY) / drawnH)),
	};
}

function rectFromPoints(
	start: { x: number; y: number },
	end: { x: number; y: number },
): DetectRoi {
	return {
		x: Math.min(start.x, end.x),
		y: Math.min(start.y, end.y),
		w: Math.abs(end.x - start.x),
		h: Math.abs(end.y - start.y),
	};
}

function onRoiPointerDown(event: PointerEvent) {
	if (!drawing) return;
	const point = pointInImage(event);
	if (!point) return;
	(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	roiDrag = point;
	draftRoi = { x: point.x, y: point.y, w: 0, h: 0 };
	draftMark = null;
}

function onRoiPointerMove(event: PointerEvent) {
	if (!roiDrag) return;
	const point = pointInImage(event);
	if (!point) return;
	const box = rectFromPoints(roiDrag, point);
	draftRoi = box;
	showDraft(box);
}

function onRoiPointerUp(event: PointerEvent) {
	if (!roiDrag) return;
	const start = roiDrag;
	const point = pointInImage(event) ?? start;
	roiDrag = null;
	const roi = rectFromPoints(start, point);
	if (roi.w < 0.01 || roi.h < 0.01) {
		draftRoi = null;
		draftMark = null;
		roiMessage = "Drag a larger rectangle.";
		return;
	}
	draftRoi = roi;
	showDraft(roi);
	void saveDetectRoi(roi);
}

async function loadDetectRoi() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/detect-roi");
		if (!res.ok) return;
		const data = await res.json();
		detectRoi = asRoi(data.roi);
	} catch {
		// A missed read keeps the current region.
	}
}

function roundRoi(roi: DetectRoi): DetectRoi {
	const round = (value: number) => Math.round(value * 10000) / 10000;
	return { x: round(roi.x), y: round(roi.y), w: round(roi.w), h: round(roi.h) };
}

async function saveDetectRoi(roi: DetectRoi | null) {
	if (!admin) return;
	savingRoi = true;
	roiMessage = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/detect-roi", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ roi: roi ? roundRoi(roi) : null }),
		});
		const data = (await res.json().catch(() => ({}))) as {
			status?: string;
			message?: string;
			roi?: unknown;
		};
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Could not save the ROI");
		}
		detectRoi = asRoi(data.roi);
		draftRoi = null;
		draftMark = null;
		drawing = false;
		roiDrag = null;
		roiMessage = data.message || "Saved.";
		rerunDetect();
	} catch (err: unknown) {
		roiMessage = errorMessage(err, "Could not save the ROI");
	} finally {
		savingRoi = false;
	}
}

function clearDetectRoi() {
	if (!admin || savingRoi) return;
	cancelRoiDraw();
	void saveDetectRoi(null);
}

function rerunDetect() {
	if (!snapshot?.source || !pageAlive) return;
	detectGeneration += 1;
	const generation = detectGeneration;
	void runDetect(generation, { image: snapshot.source });
}

function chooseImageFile(): Promise<File | null> {
	const input = imagePicker;
	if (!input) return Promise.resolve(null);
	if (imagePick) imagePick(null);
	input.value = "";
	return new Promise((resolve) => {
		imagePick = resolve;
		try {
			input.click();
		} catch {
			imagePick = null;
			resolve(null);
		}
	});
}

function finishImagePick(file: File | null) {
	const resolve = imagePick;
	imagePick = null;
	resolve?.(file);
}

function onImagePicked(event: Event) {
	const input = event.currentTarget as HTMLInputElement;
	finishImagePick(input.files?.[0] ?? null);
}

function onImageCancel() {
	finishImagePick(null);
}

function readImageFile(file: File): Promise<string> {
	return new Promise((resolve, reject) => {
		const reader = new FileReader();
		reader.onload = () => {
			if (typeof reader.result === "string" && reader.result)
				resolve(reader.result);
			else reject(new Error("Could not read the image"));
		};
		reader.onerror = () =>
			reject(reader.error ?? new Error("Could not read the image"));
		reader.readAsDataURL(file);
	});
}

async function chooseAndDetect() {
	if (!bom || !testMode) return;
	const kitCode = bom.kitCode;
	const file = await chooseImageFile();
	if (!file || !pageAlive || !bom || bom.kitCode !== kitCode) return;
	try {
		const dataUrl = await readImageFile(file);
		if (!pageAlive || !bom || bom.kitCode !== kitCode) return;
		const generation = holdStill(dataUrl, kitCode);
		await runDetect(generation, { image: dataUrl });
	} catch (err: unknown) {
		if (!pageAlive) return;
		detectMessage = errorMessage(err, "Could not read the image");
		detectState = "done";
	}
}

async function openErp() {
	if (!erpUrl) return;
	erpOpen = true;
	await tick();
	erpCloseButton?.focus();
	// The ERP page stays loaded behind the vision view. Opening it again
	// must not navigate the iframe, or a menu selection is lost.
	if (erpLoadedUrl === erpUrl && !iframeFailed) return;
	const attempt = ++erpAttempt;
	iframeFailed = false;
	erpChecking = true;
	erpLoadedUrl = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp/frame", {
			signal: AbortSignal.timeout(12000),
		});
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		if (attempt !== erpAttempt) return;
		if (data.frame === true) {
			erpLoadedUrl = erpUrl;
		} else {
			iframeFailed = true;
		}
		erpChecking = false;
	} catch {
		if (attempt !== erpAttempt) return;
		iframeFailed = true;
		erpChecking = false;
	}
}

function closeErp() {
	erpOpen = false;
}

function onErpKey(event: KeyboardEvent) {
	if (event.key === "Escape" && erpOpen) closeErp();
	else if (event.key === "Escape" && drawing) cancelRoiDraw();
}

function onIframeError() {
	iframeFailed = true;
}

async function openErpInBrowser() {
	if (!erpUrl) return;
	try {
		await openUrl(erpUrl);
	} catch {
		window.open(erpUrl, "_blank", "noopener,noreferrer");
	}
}

onMount(() => {
	readAdminFlag();
	window.addEventListener("focus", readAdminFlag);
	measureFrame();
	if (paneFrame) {
		frameObserver = new ResizeObserver(() => measureFrame());
		frameObserver.observe(paneFrame);
	}
	void checkSidecar();
	healthTimer = setInterval(() => {
		void checkSidecar();
	}, 3000);
});

onDestroy(() => {
	pageAlive = false;
	window.removeEventListener("focus", readAdminFlag);
	frameObserver?.disconnect();
	frameObserver = null;
	stopWatch();
	clearStreamRetry();
	stopSidecarWait();
	if (healthTimer !== null) {
		clearInterval(healthTimer);
		healthTimer = null;
	}
});

function errorMessage(err: unknown, fallback: string): string {
	return err instanceof Error && err.message ? err.message : fallback;
}

function invokeError(err: unknown, fallback: string): string {
	if (typeof err === "string" && err.trim()) return err;
	return errorMessage(err, fallback);
}

function stopSidecarWait() {
	sidecarStarting = false;
	if (sidecarTick !== null) {
		clearInterval(sidecarTick);
		sidecarTick = null;
	}
}

function beginSidecarWait() {
	sidecarStarting = true;
	sidecarSeconds = 0;
	if (sidecarTick !== null) clearInterval(sidecarTick);
	sidecarTick = setInterval(() => {
		sidecarSeconds += 1;
	}, 1000);
}

async function startSidecar() {
	if (sidecarStarting || sidecarUp) return;
	beginSidecarWait();
	try {
		const res = await invoke("start_vision_sidecar");
		console.log(res);
	} catch (err) {
		console.error(err);
		stopSidecarWait();
		return;
	}
	void checkSidecar();
	setTimeout(() => {
		void checkSidecar();
	}, 1500);
}

function apiField(row: Record<string, unknown>, names: string[]): unknown {
	const lookup = new Map(
		Object.entries(row).map(([key, value]) => [key.toLowerCase(), value]),
	);
	for (const name of names) {
		const key = name.toLowerCase();
		if (lookup.has(key)) return lookup.get(key);
	}
	return undefined;
}

function apiText(row: Record<string, unknown>, names: string[]): string {
	const value = apiField(row, names);
	if (value === undefined || value === null) return "";
	return String(value).trim();
}

function apiQty(value: unknown): number {
	if (typeof value === "number" && Number.isFinite(value)) return value;
	if (
		typeof value === "string" &&
		value.trim() &&
		Number.isFinite(Number(value))
	) {
		return Number(value);
	}
	return 0;
}

function viewFromKit(
	kitCode: string,
	row: Record<string, unknown>,
): InspectView {
	const rawParts = apiField(row, ["parts"]);
	if (!Array.isArray(rawParts)) {
		return { status: "fail", message: "BoM API response has no parts." };
	}
	const parts: Part[] = [];
	for (const item of rawParts) {
		if (!item || typeof item !== "object" || Array.isArray(item)) continue;
		const part = item as Record<string, unknown>;
		const ic = apiText(part, ["ic", "cc"]);
		if (!ic) continue;
		parts.push({
			ic,
			des: apiText(part, ["des", "description"]),
			qty: apiQty(apiField(part, ["qty", "quantity"])),
		});
	}
	const expected = apiField(row, ["expectedKitCount", "labelCount"]);
	return {
		status: "pass",
		message: "BoM loaded – 2 kit(s) on bench",
		kitCode,
		des: apiText(row, ["des", "description", "kitName"]),
		parts,
		kitsFound: 2,
		expectedKitCount:
			expected === undefined || expected === null
				? undefined
				: apiQty(expected),
	};
}

function kitsKeyedByIc(
	data: Record<string, unknown>,
): { kitCode: string; job: Record<string, unknown> }[] {
	// The BoM API uses the same shape as the sample file: { kitIC: { des, parts } }.
	const kits = [];
	for (const [key, value] of Object.entries(data)) {
		if (!value || typeof value !== "object" || Array.isArray(value)) continue;
		const job = value as Record<string, unknown>;
		if (!Array.isArray(apiField(job, ["parts"]))) continue;
		const kitCode = key.trim();
		if (kitCode) kits.push({ kitCode, job });
	}
	return kits;
}

function viewFromBomFile(bom: unknown, code: string): InspectView {
	if (!bom || typeof bom !== "object" || Array.isArray(bom)) {
		return { status: "fail", message: "Barcode not found in BoM" };
	}
	const job = (bom as Record<string, unknown>)[code];
	if (!job || typeof job !== "object" || Array.isArray(job)) {
		return { status: "fail", message: "Barcode not found in BoM" };
	}
	const row = job as Record<string, unknown>;
	const withParts = Array.isArray(row.parts) ? row : { ...row, parts: [] };
	return viewFromKit(code, withParts);
}

function viewFromApiData(data: unknown): InspectView {
	if (!data || typeof data !== "object" || Array.isArray(data)) {
		return { status: "fail", message: "BoM API response has no data object." };
	}
	const row = data as Record<string, unknown>;
	const directCode = apiText(row, ["kitIC", "kitIc", "kitCode", "kit", "ic"]);
	if (directCode && Array.isArray(apiField(row, ["parts"]))) {
		return viewFromKit(directCode, row);
	}
	const kits = kitsKeyedByIc(row);
	if (kits.length > 1) {
		return { status: "fail", message: "BoM API returned more than one kit." };
	}
	if (kits.length === 1) return viewFromKit(kits[0].kitCode, kits[0].job);
	if (directCode) {
		return { status: "fail", message: "BoM API response has no parts." };
	}
	return { status: "fail", message: "BoM API response has no kit IC." };
}

async function runApiLookup() {
	if (loading) return;
	loading = true;
	result = null;
	try {
		const body = await invoke("post_bom_api", {
			jobCardNumber: barcode.trim(),
		});
		const view = viewFromApiData(body);
		result = view;
		if (view.status === "pass") {
			storeBom(view);
			if (testMode) void chooseAndDetect();
		} else dropBom();
	} catch (err: unknown) {
		result = { status: "fail", message: invokeError(err, "Request failed") };
		dropBom();
	} finally {
		loading = false;
	}
}

async function runJsonLookup() {
	if (loading) return;
	loading = true;
	result = null;
	try {
		const bom = await invoke("load_bom_json");
		const view = viewFromBomFile(bom, barcode);
		result = view;
		if (view.status === "pass") {
			storeBom(view);
			if (testMode) void chooseAndDetect();
		} else dropBom();
	} catch (err: unknown) {
		result = { status: "fail", message: invokeError(err, "Request failed") };
		dropBom();
	} finally {
		loading = false;
	}
}

async function runInspect() {
	if (loading) return;
	await ensureBomSource();
	if (!pageAlive || loading) return;
	if (bomSource === "api") {
		await runApiLookup();
		return;
	}
	if (bomSource === "json") {
		await runJsonLookup();
		return;
	}
	loading = true;
	result = null;
	try {
		const res = await fetch("http://127.0.0.1:8008/inspect", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ barcode }),
		});
		if (!res.ok) throw new Error(await res.text());
		const body = (await res.json()) as InspectView;
		result = body;
		if (body.status === "pass") {
			storeBom(body);
			if (testMode) void chooseAndDetect();
		} else dropBom();
	} catch (err: unknown) {
		result = { status: "fail", message: invokeError(err, "Request failed") };
		dropBom();
	} finally {
		loading = false;
	}
}

async function decodeAndInspect() {
	if (loading) return;
	await ensureBomSource();
	await loadTestMode();
	if (!pageAlive) return;
	// Test mode never reads the camera. The typed code selects the kit, then
	// the page asks for an image file. SQL/JSON and API keep their own lookup.
	if (testMode) {
		await runInspect();
		return;
	}
	loading = true;
	try {
		const res = await fetch(
			`http://127.0.0.1:8008/camera/snapshot?index=${cameraIndex}`,
		);
		const body = (await res.json()) as WatchBody;
		if (!res.ok) throw new Error(body.message || "Decode failed");
		if (body.snapshot && body.image) {
			await showSnapshot(body);
			return;
		}
		if (body.barcode) {
			barcode = body.barcode;
			dropBom();
			result = {
				status: "fail",
				message: body.message || "Barcode not found in BoM",
			};
			return;
		}
		result = {
			status: "fail",
			message: body.message || "No code detected",
		};
	} catch (err: unknown) {
		result = { status: "fail", message: errorMessage(err, "Decode failed") };
	} finally {
		loading = false;
	}
}
</script>

<svelte:window on:keydown={onErpKey} />

<div class="relative flex h-dvh max-h-dvh flex-col overflow-hidden overscroll-none bg-neutral-950 text-neutral-100">
	<input
		bind:this={imagePicker}
		type="file"
		accept="image/*"
		class="absolute h-px w-px opacity-0"
		tabindex="-1"
		on:change={onImagePicked}
		on:cancel={onImageCancel}
	/>
	<header class="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">LS-Vision</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Operator</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<p
				class="inline-flex min-h-9 items-center gap-2 border px-2.5 text-xs font-semibold uppercase tracking-wide {snapshot ||
				(sidecarUp && camerasMissing)
					? 'border-amber-700/80 bg-amber-950 text-amber-200'
					: feedOn && sidecarUp && cameraFeedOn
						? 'border-neutral-600 bg-neutral-800 text-neutral-100'
						: 'border-neutral-700 bg-neutral-900 text-neutral-400'}"
			>
				<span
					class="h-2 w-2 shrink-0 {snapshot || (sidecarUp && camerasMissing)
						? 'bg-amber-400'
						: feedOn && sidecarUp && cameraFeedOn
							? 'bg-green-400'
							: 'bg-neutral-500'}"
					aria-hidden="true"
				></span>
				{snapshot
					? "Stream stopped"
					: sidecarUp && camerasMissing
						? "No cameras found"
						: feedOn && sidecarUp && cameraFeedOn
							? `Streaming · ${streamLeft}s`
							: "Stream paused"}
			</p>
			<a
				href="/settings"
				aria-label="Settings"
				class="inline-flex h-11 w-11 items-center justify-center border border-neutral-600 bg-neutral-800 text-neutral-100"
			>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					viewBox="0 0 24 24"
					class="h-5 w-5"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					aria-hidden="true"
				>
					<circle cx="12" cy="12" r="3" />
					<path
						d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9c.3.6.9 1.1 1.5 1.1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1.1z"
					/>
				</svg>
			</a>
			<button
				type="button"
				class="inline-flex min-h-11 min-w-44 items-center justify-center gap-2 border px-3 text-sm font-semibold disabled:opacity-100 {sidecarStarting
					? 'cursor-wait border-amber-300 bg-amber-400 text-neutral-950 disabled:cursor-wait'
					: sidecarUp
						? 'cursor-default border-green-700 bg-green-600 text-white disabled:cursor-default'
						: 'border-red-800 bg-red-600 text-white'}"
				disabled={sidecarStarting || sidecarUp}
				aria-busy={sidecarStarting}
				aria-label={sidecarStarting
					? "Starting vision sidecar"
					: sidecarUp
						? "Vision sidecar running"
						: "Start Vision Sidecar"}
				on:click={startSidecar}
			>
				{#if sidecarStarting}
					<span class="inline-flex items-end gap-1" aria-hidden="true">
						<span
							class="inline-block h-1.5 w-1.5 rounded-full bg-neutral-950 animate-bounce motion-reduce:animate-none"
						></span>
						<span
							class="inline-block h-1.5 w-1.5 rounded-full bg-neutral-950 animate-bounce motion-reduce:animate-none"
							style="animation-delay: 150ms"
						></span>
						<span
							class="inline-block h-1.5 w-1.5 rounded-full bg-neutral-950 animate-bounce motion-reduce:animate-none"
							style="animation-delay: 300ms"
						></span>
					</span>
					<span class="font-mono tabular-nums">{sidecarSeconds}s</span>
				{:else if sidecarUp}
					Vision sidecar running
				{:else}
					Start Vision Sidecar
				{/if}
			</button>
		</div>
	</header>

	<form
		class="flex shrink-0 flex-wrap items-end gap-2 border-b border-neutral-800 bg-neutral-900 px-4 py-2"
		on:submit|preventDefault={runInspect}
	>
		<label class="flex min-w-56 flex-1 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			{bomSource === "api" ? "Job card number" : "Barcode"}
			<input
				class="w-full border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-lg font-medium normal-case tracking-normal text-neutral-100"
				inputmode={bomSource === "api" ? "numeric" : undefined}
				bind:value={barcode}
			/>
		</label>
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Camera
			<select
				class="min-h-11 border border-neutral-600 bg-neutral-950 px-3 font-mono text-base normal-case tracking-normal text-neutral-100"
				bind:value={cameraIndex}
				on:change={saveCameraIndex}
			>
				{#if camerasMissing}
					<option value={cameraIndex}>No cameras found</option>
				{:else if cameraIndexes.length > 0}
					{#each cameraIndexes as index (index)}
						<option value={index}>{index}</option>
					{/each}
				{:else}
					<option value={cameraIndex}>{cameraIndex}</option>
				{/if}
			</select>
		</label>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
			on:click={decodeAndInspect}
			disabled={loading}
		>
			{testMode ? "Use typed code" : "Decode QR / Barcode"}
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
			on:click={toggleFeed}
		>
			{feedOn ? "Stop feed" : "Start feed"}
		</button>
		{#if admin}
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={beginSetRoi}
				disabled={savingRoi}
			>
				{drawing ? "Cancel" : "Set ROI"}
			</button>
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={clearDetectRoi}
				disabled={savingRoi || !detectRoi}
			>
				Clear ROI
			</button>
		{/if}
		{#if erpUrl}
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-amber-300 bg-amber-400 px-4 text-sm font-semibold text-neutral-950"
				on:click={openErp}
			>
				{erpName || "ERP"}
			</button>
		{/if}
	</form>

	<div class="grid min-h-0 flex-1 gap-3 overflow-hidden p-3 lg:grid-cols-[minmax(0,1.7fr)_minmax(22rem,28rem)]">
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
				{#if sidecarStarting && !snapshot}
					<div
						class="pointer-events-none absolute inset-0 z-20 flex flex-col items-center justify-center gap-3 text-neutral-100"
					>
						<span class="inline-flex items-end gap-1.5" aria-hidden="true">
							<span
								class="inline-block h-2 w-2 rounded-full bg-white animate-bounce motion-reduce:animate-none"
							></span>
							<span
								class="inline-block h-2 w-2 rounded-full bg-white animate-bounce motion-reduce:animate-none"
								style="animation-delay: 150ms"
							></span>
							<span
								class="inline-block h-2 w-2 rounded-full bg-white animate-bounce motion-reduce:animate-none"
								style="animation-delay: 300ms"
							></span>
						</span>
						<p class="font-mono text-sm tabular-nums">Starting sidecar · {sidecarSeconds}s</p>
					</div>
				{/if}
				{#if snapshot || (feedOn && cameraFeedOn)}
					<div
						class="absolute {zoom ? '' : 'inset-0'}"
						style={zoom
							? `left:${zoom.left}px;top:${zoom.top}px;width:${zoom.width}px;height:${zoom.height}px;`
							: ""}
					>
						{#if snapshot}
							<img
								bind:this={paneImage}
								src={snapshot.image}
								alt={snapshot.detected
									? `Detected parts for kit ${snapshot.kitCode}`
									: `Snapshot of kit ${snapshot.kitCode}`}
								class="h-full w-full {zoom ? 'object-fill' : 'object-contain'}"
								on:load={rememberPaneSize}
							/>
						{:else if cameraFeedOn}
							<img
								bind:this={paneImage}
								src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}&feed={streamEpoch}"
								alt="Camera feed"
								class="h-full w-full {zoom ? 'object-fill' : 'object-contain'}"
								on:load={rememberPaneSize}
								on:error={retryStream}
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
						{#if snapshot && paneWidth > 0 && paneHeight > 0 && detections.length > 0}
							{@const marks = detectionMarks(zoom)}
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
				{:else if feedOn && sidecarUp && !sidecarStarting}
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
						on:click={resumeLiveFeed}
					>
						{feedOn ? "Resume live feed" : "Back to feed"}
					</button>
				{/if}
				{#if drawing && snapshot}
					<div
						class="absolute inset-0 z-30 cursor-crosshair"
						on:pointerdown={onRoiPointerDown}
						on:pointermove={onRoiPointerMove}
						on:pointerup={onRoiPointerUp}
						on:pointercancel={onRoiPointerUp}
					></div>
					<p class="pointer-events-none absolute left-3 top-3 z-40 bg-neutral-950/80 px-3 py-2 text-sm text-amber-200">
						Drag a rectangle on the snapshot.
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

		<section class="flex min-h-0 min-w-0 flex-col overflow-hidden border border-neutral-700 bg-neutral-900">
			<h2 class="shrink-0 border-b border-neutral-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
				Result
			</h2>
			{#if result}
				<p
					class="shrink-0 px-4 py-4 text-4xl font-black uppercase leading-none tracking-widest {result.status ===
					'pass'
						? 'bg-green-500 text-neutral-950'
						: 'bg-red-600 text-white'}"
				>
					{result.status}
				</p>
				<div class="result-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
					{#if result.kitCode}
						<p class="font-mono text-2xl font-semibold tracking-tight">{result.kitCode}</p>
					{/if}
					{#if testMode && isLoadedBom(result)}
						<button
							type="button"
							class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white"
							on:click={chooseAndDetect}
						>
							Choose image
						</button>
						<p class="text-sm text-neutral-400">
							Test mode is on. Choose an image file to detect parts for this kit.
						</p>
					{/if}
					{#if result.des}
						<p class="text-base text-neutral-300">{result.des}</p>
					{/if}
					{#if result.message}
						<p class="text-base text-neutral-200">{result.message}</p>
					{/if}
					{#if result.parts?.length}
						{@const detectFailed =
							detectState === "done" && !!detectMessage && detections.length === 0}
						{@const showSeen =
							detectState !== "idle" && isLoadedBom(result) && !detectFailed}
						{@const counts = detectionCounts(detections)}
						<table class="w-full border-collapse text-left text-sm">
							<thead>
								<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
									<th class="py-2 pr-3 font-semibold">IC</th>
									<th class="py-2 pr-3 font-semibold">Description</th>
									<th class="py-2 text-right font-semibold">Qty</th>
									{#if showSeen}
										<th class="py-2 text-right font-semibold">Seen</th>
									{/if}
								</tr>
							</thead>
							<tbody>
								{#each result.parts as part, i (i)}
									<tr class="border-b border-neutral-800">
										<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
										<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
										<td class="py-2 text-right font-semibold">{part.qty}</td>
										{#if showSeen}
											<td
												class="py-2 text-right font-semibold {detectState === 'running'
													? 'text-neutral-500'
													: (counts[part.ic] ?? 0) === part.qty
														? 'text-green-400'
														: 'text-red-400'}"
											>
												{detectState === "running" ? "…" : (counts[part.ic] ?? 0)}
											</td>
										{/if}
									</tr>
								{/each}
							</tbody>
						</table>
						{#if showSeen && detectState === "done"}
							<p class="text-sm text-neutral-300">{matchSummary(result.parts, detections)}</p>
						{/if}
					{/if}
					{#if detectState === "running"}
						<p class="text-sm text-neutral-400">Detecting parts...</p>
					{/if}
					{#if detectState === "done"}
						{@const extras = partsOutsideBom(detections, bom?.parts ?? null)}
						{#if extras.length}
							<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								{bom ? "Not in BoM" : "Detected parts"}
							</h3>
							<table class="w-full border-collapse text-left text-sm">
								<thead>
									<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
										<th class="py-2 pr-3 font-semibold">IC</th>
										<th class="py-2 text-right font-semibold">Count</th>
									</tr>
								</thead>
								<tbody>
									{#each extras as row (row.ic)}
										<tr class="border-b border-neutral-800">
											<td class="py-2 pr-3 font-mono font-medium">{row.ic}</td>
											<td class="py-2 text-right font-semibold">{row.count}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{/if}
					{/if}
					{#if detectMessage}
						<p class="text-sm text-red-300">{detectMessage}</p>
					{/if}
					{#if result.info}
						<p class="text-sm text-neutral-400">{result.info}</p>
					{/if}
					{#if result.kitsFound != null || result.expectedKitCount != null}
						<p class="text-sm text-neutral-400">
							Info only: {result.kitsFound ?? "—"} kit(s) found, {result.expectedKitCount ?? "—"} expected
						</p>
					{/if}
				</div>
			{:else}
				<div class="px-4 py-8">
					<p class="text-lg font-medium text-neutral-300">No result yet</p>
					<p class="mt-1 text-sm text-neutral-500">
						{testMode
							? bomSource === "api"
								? "Type a job card number, then choose an image."
								: "Type a barcode, then choose an image."
							: bomSource === "api"
								? "A decoded barcode is used as the job card number."
								: "Decode a barcode or press Enter."}
					</p>
				</div>
			{/if}
		</section>
	</div>

	<footer class="flex shrink-0 items-center border-t border-neutral-800 bg-neutral-900 px-4 py-1.5">
		<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
			Model
			<span class="ml-2 font-mono text-xs font-medium normal-case tracking-normal text-neutral-300">
				{modelName}
			</span>
		</p>
	</footer>

	<button
		type="button"
		bind:this={erpCloseButton}
		class="absolute inset-y-0 left-0 z-50 flex w-12 flex-col items-center justify-center gap-3 border-r border-neutral-700 bg-neutral-900 text-yellow-300 transition-transform duration-300 ease-out motion-reduce:transition-none {erpOpen
			? 'pointer-events-auto translate-x-0'
			: 'pointer-events-none -translate-x-full'}"
		aria-label="Back to vision"
		aria-hidden={erpOpen ? "false" : "true"}
		tabindex={erpOpen ? 0 : -1}
		on:click={closeErp}
	>
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 24 24"
			class="h-6 w-6"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			aria-hidden="true"
		>
			<path d="M15 18l-6-6 6-6" />
		</svg>
		<span class="text-xs font-semibold uppercase tracking-widest [writing-mode:vertical-rl]">
			Back
		</span>
	</button>
	<div
		class="absolute inset-y-0 left-12 right-0 z-40 border-l border-neutral-800 bg-neutral-950 transition-transform duration-300 ease-out motion-reduce:transition-none {erpOpen
			? 'pointer-events-auto translate-x-0'
			: 'pointer-events-none translate-x-full'}"
		aria-hidden={erpOpen ? "false" : "true"}
	>
		{#if erpLoadedUrl}
			<iframe
				title={erpName || "ERP"}
				src={erpLoadedUrl}
				class="h-full w-full border-0 bg-white"
				on:error={onIframeError}
			></iframe>
		{/if}
		{#if iframeFailed}
			<div class="absolute inset-0 flex flex-col items-center justify-center gap-4 bg-neutral-950 px-8 text-center">
				<p class="text-lg font-medium text-neutral-100">Could not load this page in the panel.</p>
				<p class="max-w-xl break-all font-mono text-sm text-neutral-400">{erpUrl}</p>
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950"
					on:click={openErpInBrowser}
				>
					Open in system browser
				</button>
			</div>
		{:else if erpChecking && !erpLoadedUrl}
			<div class="absolute inset-0 flex items-center justify-center bg-neutral-950">
				<p class="text-lg text-neutral-300">Opening {erpName || "ERP"}...</p>
			</div>
		{/if}
	</div>
</div>

<style>
	.result-scroll {
		color-scheme: dark;
		scrollbar-color: #a3a3a3 #171717;
		scrollbar-width: auto;
	}

	.result-scroll::-webkit-scrollbar {
		width: 12px;
		height: 12px;
	}

	.result-scroll::-webkit-scrollbar-track {
		background: #171717;
	}

	.result-scroll::-webkit-scrollbar-thumb {
		background: #a3a3a3;
		border: 2px solid #171717;
	}

	.result-scroll::-webkit-scrollbar-thumb:hover {
		background: #d4d4d4;
	}

	.result-scroll::-webkit-scrollbar-corner {
		background: #171717;
	}
</style>
