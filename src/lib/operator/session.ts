import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
	BarcodeFormat,
	BrowserMultiFormatReader,
	DecodeHintType,
} from "@zxing/library";
import { tick } from "svelte";
import { get, writable } from "svelte/store";
import { activateCamera, readDisplayFps } from "$lib/camera-settings";
import {
	compactClearedSlot,
	detectSlotNumber,
	panelsForSlots,
	parseDetectRoi,
	parseDetectRois,
	reindexSlotResults,
	withAllSlotsDisabled,
	withSlotEnabled,
} from "$lib/detect-rois";
import {
	appendConsoleLine,
	consoleStamp,
	isCameraOpenError,
} from "$lib/operator/console";
import { detectionCounts, inspectionVerdict } from "$lib/operator/verdict";
import { applyPartRules, parsePartRules } from "$lib/part-composition";
import {
	type BarcodeRoi,
	createOperatorViews,
	type Detection,
	type DetectRoi,
	type InspectView,
	initialOperator,
	type OperatorSession,
	type OperatorState,
	type OperatorStores,
	type Part,
	pixelBarcodeRoi,
} from "$lib/stores/operator";

// One visit to the operator page. Timers and DOM nodes stay off the reactive state.
export function createOperator(): OperatorStores {
	const operator = writable(initialOperator());
	const views = createOperatorViews(operator);
	const session = attachSession(
		operator,
		views.camerasMissing,
		views.cameraFeedOn,
	);
	return { operator, ...views, ...session };
}

function attachSession(
	operator: ReturnType<typeof writable<OperatorState>>,
	camerasMissing: OperatorStores["camerasMissing"],
	cameraFeedOn: OperatorStores["cameraFeedOn"],
): OperatorSession {
	const refs: OperatorSession["refs"] = {
		paneImage: null,
		paneFrame: null,
		liveCanvas: undefined,
		imagePicker: null,
		erpCloseButton: null,
	};
	// A field write publishes the same object, the same way a Svelte $store write does.
	const op: OperatorState = new Proxy({} as OperatorState, {
		get(_target, prop) {
			if (typeof prop !== "string") return undefined;
			return get(operator)[prop as keyof OperatorState];
		},
		set(_target, prop, value) {
			if (typeof prop !== "string") return false;
			const current = get(operator);
			(current as unknown as Record<string, unknown>)[prop] = value;
			operator.set(current);
			return true;
		},
	});

	type WatchBody = InspectView & {
		snapshot?: boolean;
		barcode?: string;
		image?: string;
		width?: number;
		height?: number;
		timestamp_ms?: number;
		jobCardNumber?: string;
	};

	let previewImage = "";
	let previewWidth = 0;
	let previewHeight = 0;
	let heldPreviewW = 0;
	let heldPreviewH = 0;
	let heldRawW = 0;
	let heldRawH = 0;
	let scaleToPreview = false;

	const DEFAULT_SCAN_SECONDS = 8;
	// Settings unlock is the admin flag for this tab. It is not stored in config.
	const ADMIN_FLAG = "ls-vision-settings-open";

	let bomSourceReady: Promise<void> | null = null;
	// Set when the saved camera is missing, so focus does not put that index back.
	let skippedSavedIndex: number | null = null;
	// Saves stay in order. A slower choice must not replace a newer one.
	let cameraSaveChain: Promise<void> = Promise.resolve();
	let cameraSaveGeneration = 0;
	let cameraSavePending = 0;
	let camerasPromise: Promise<boolean> | null = null;
	const FEED_DEBOUNCE_MS = 1520;
	let feedBusyTimer: ReturnType<typeof setTimeout> | null = null;
	let detectGeneration = 0;
	let roiDrag: { x: number; y: number } | null = null;
	let frameObserver: ResizeObserver | null = null;
	let imagePick: ((file: File | null) => void) | null = null;
	let streamMs = DEFAULT_SCAN_SECONDS * 1000;
	let streamTimer: ReturnType<typeof setInterval> | null = null;
	let liveGeneration = 0;
	let liveRunning = false;
	let displayFps = 15;
	let watchInFlight = false;
	let pageAlive = true;
	let consoleSeq = 1;
	// Preview retries about every 400ms. One line until that text changes.
	let cameraErrorNote = "";
	let modelNote = "";

	function writeConsole(text: string) {
		const line = {
			id: consoleSeq,
			time: consoleStamp(new Date()),
			text,
		};
		consoleSeq += 1;
		op.consoleLines = appendConsoleLine(op.consoleLines, line);
	}

	function clearConsole() {
		op.consoleLines = [];
		// The live loop can record the current open error on its next tick.
		cameraErrorNote = "";
	}

	function toggleConsole() {
		op.consoleOpen = !op.consoleOpen;
	}

	function dismissPartRuleWarning() {
		op.partRuleWarning = false;
	}

	function noteCameraOpenError(message: string) {
		const text = message.trim();
		if (!isCameraOpenError(text) || cameraErrorNote === text) return;
		cameraErrorNote = text;
		writeConsole(`Camera open error: ${text}`);
	}

	function noteCameraReady() {
		cameraErrorNote = "";
	}

	function noteModelLoaded(name: string) {
		const key = `loaded:${name}`;
		if (modelNote === key) return;
		modelNote = key;
		writeConsole(`Model loaded: ${name}`);
	}

	function noteModelFailed(message: string) {
		const text = message.trim() || "Model failed to load.";
		const key = `failed:${text}`;
		if (modelNote === key) return;
		modelNote = key;
		writeConsole(`Model failed: ${text}`);
	}

	function noteModelFromDetect(message: string) {
		if (
			message === "Model is missing." ||
			message === "Model failed to load." ||
			message.includes("is a .pt file")
		) {
			noteModelFailed(message);
			return;
		}
		if (!message && op.modelName.toLowerCase().endsWith(".onnx")) {
			noteModelLoaded(op.modelName);
		}
	}

	function stopWatch() {
		if (streamTimer !== null) {
			clearInterval(streamTimer);
			streamTimer = null;
		}
	}

	function startWatch() {
		if (op.snapshot || get(camerasMissing) || !op.feedOn) return;
		stopWatch();
		op.streamLeft = streamMs / 1000;
		void watchFeed();
		streamTimer = setInterval(() => {
			if (!op.feedOn || op.snapshot) return;
			if (op.streamLeft <= 1) {
				op.streamLeft = streamMs / 1000;
				void watchFeed();
			} else {
				op.streamLeft -= 1;
			}
		}, 1000);
	}

	function stopLive() {
		liveGeneration += 1;
		liveRunning = false;
		op.livePainted = false;
	}

	function showLiveFeed() {
		if (get(camerasMissing)) return;
		startLive();
		startWatch();
	}

	function releaseFeedButton() {
		feedBusyTimer = null;
		op.feedBusy = false;
	}

	function toggleFeed() {
		if (op.feedBusy) return;
		op.feedBusy = true;
		op.feedOn = !op.feedOn;
		if (op.feedOn) showLiveFeed();
		else {
			stopLive();
			stopWatch();
		}
		if (feedBusyTimer !== null) clearTimeout(feedBusyTimer);
		feedBusyTimer = setTimeout(() => {
			if (!pageAlive) return;
			releaseFeedButton();
		}, FEED_DEBOUNCE_MS);
	}

	function resumeLiveFeed() {
		cancelRoiDraw();
		cancelBarcodeDraw();
		detectGeneration += 1;
		op.snapshot = null;
		op.detections = [];
		op.slotResults = [];
		op.detectMessage = "";
		op.detectState = "idle";
		op.paneWidth = 0;
		op.paneHeight = 0;
		if (op.feedOn) showLiveFeed();
	}

	function holdStill(image: string, kitCode: string) {
		cancelRoiDraw();
		cancelBarcodeDraw();
		detectGeneration += 1;
		op.paneWidth = 0;
		op.paneHeight = 0;
		op.snapshot = {
			kitCode,
			image,
			source: image,
			takenAt: new Date().toLocaleTimeString(),
			detected: false,
		};
		stopWatch();
		stopLive();
		return detectGeneration;
	}

	function storeBom(view: InspectView) {
		// Only a passing barcode lookup loads a kit. Model class names do not.
		if (view.status !== "pass" || !view.kitCode || !view.parts?.length) return;
		op.bom = {
			kitCode: view.kitCode,
			des: view.des,
			parts: view.parts,
		};
	}

	function dropBom() {
		op.bom = null;
	}

	function asDetections(value: unknown): Detection[] {
		if (!Array.isArray(value)) return [];
		const found: Detection[] = [];
		for (const item of value) {
			if (!item || typeof item !== "object") continue;
			const row = item as Detection;
			if (typeof row.ic !== "string" || !row.ic) continue;
			const referenceMatch =
				row.referenceMatch === true
					? true
					: row.referenceMatch === false
						? false
						: undefined;
			found.push({
				ic: row.ic,
				confidence: typeof row.confidence === "number" ? row.confidence : 0,
				bbox: Array.isArray(row.bbox) ? row.bbox : [],
				referenceMatch,
			});
		}
		return found;
	}

	function modelClassList(value: unknown): string[] | null {
		if (!Array.isArray(value)) return null;
		const names: string[] = [];
		for (const item of value) {
			if (typeof item !== "string" || !item || names.includes(item)) continue;
			names.push(item);
		}
		return names;
	}

	// Rename parents before the counts, the overlay, and the saved result.
	// A consumed child stays on the list for the admin outline and is left out of the BoM count.
	// The model list from detect_image is left as the trained class names.
	// checked is false when this detect did not return a class list.
	async function composeDetections(
		items: Detection[],
		modelClasses: readonly string[] | null,
	): Promise<{ detections: Detection[]; skipped: number; checked: boolean }> {
		try {
			const config = await readConfig();
			const rules = parsePartRules(config.partRules);
			if (rules.length === 0) {
				return { detections: items, skipped: 0, checked: true };
			}
			const outcome = applyPartRules(items, rules, modelClasses);
			for (const line of outcome.log) writeConsole(line);
			return {
				detections: outcome.detections,
				skipped: outcome.skipped,
				checked: modelClasses !== null,
			};
		} catch (err: unknown) {
			writeConsole(
				`Part rules: ${invokeError(err, "Could not load part rules")}. Classes were left unchanged.`,
			);
			return { detections: items, skipped: 0, checked: false };
		}
	}

	function detectedImageSrc(encoded: string): string {
		const text = encoded.trim();
		if (text.startsWith("data:")) return text;
		return `data:image/jpeg;base64,${text.replace(/\s+/g, "")}`;
	}

	type DetectObject = {
		detections?: unknown;
		classes?: unknown;
		image_base64?: unknown;
		message?: unknown;
		slot?: unknown;
		dinov2?: unknown;
	};

	function noteDinov2(value: unknown) {
		if (!Array.isArray(value)) return;
		for (const line of value) {
			if (typeof line === "string" && line.trim()) writeConsole(line);
		}
	}

	async function requestDetections(
		image?: string,
		captureReferences = false,
	): Promise<unknown> {
		// .onnx runs in Rust. A live detect uses the latest raw frame. Test mode
		// passes the chosen file. A .pt file is not detected. Saving references
		// stores each surviving box under its IC and does not change the result.
		if (op.modelName.toLowerCase().endsWith(".onnx")) {
			return await invoke("detect_image", {
				image: image && image.trim() ? image : null,
				captureReferences,
			});
		}
		return {
			detections: [],
			message: `${op.modelName} is a .pt file. Choose an .onnx model to detect parts.`,
		};
	}

	function isDetectObject(value: unknown): value is DetectObject {
		return !!value && typeof value === "object" && !Array.isArray(value);
	}

	function noteRawSize(frame: WatchBody) {
		if (typeof frame.width === "number" && frame.width > 0) heldRawW = frame.width;
		if (typeof frame.height === "number" && frame.height > 0)
			heldRawH = frame.height;
	}

	// Boxes come back in full-frame pixels. The held still is the preview JPEG.
	function fitDetections(items: Detection[]): Detection[] {
		if (
			!scaleToPreview ||
			heldRawW < 1 ||
			heldRawH < 1 ||
			heldPreviewW < 1 ||
			heldPreviewH < 1
		) {
			return items;
		}
		if (heldRawW === heldPreviewW && heldRawH === heldPreviewH) return items;
		const sx = heldPreviewW / heldRawW;
		const sy = heldPreviewH / heldRawH;
		return items.map((item) => {
			if (item.bbox.length < 4) return item;
			const [x1, y1, x2, y2] = item.bbox;
			return {
				...item,
				bbox: [x1 * sx, y1 * sy, x2 * sx, y2 * sy],
			};
		});
	}

	function holdPreviewStill(kitCode: string): number | null {
		if (!previewImage) return null;
		scaleToPreview = true;
		heldPreviewW = previewWidth;
		heldPreviewH = previewHeight;
		return holdStill(previewImage, kitCode);
	}

	async function recordResult(
		image: string | null,
		includeDetection: boolean,
		descriptions: Record<string, string> = {},
		slot = 1,
		detectionsOverride?: Detection[],
		noteSlot = false,
	) {
		if (op.testMode) return;
		const view = op.result;
		if (!view || (view.status !== "pass" && view.status !== "fail")) return;
		const detections = includeDetection
			? (detectionsOverride ?? op.detections).slice()
			: [];
		const storedSlot = detectSlotNumber(slot) ?? 1;
		const detectMessage = includeDetection ? op.detectMessage : "";
		try {
			const config = await readConfig();
			if (config.testMode === true) return;
			if (config.skipResultSave !== false) return;
			const detectFailed =
				includeDetection && !!detectMessage && detections.length === 0;
			const counts =
				includeDetection && !detectFailed ? detectionCounts(detections) : null;
			const parts = view.parts ?? [];
			const bomLoaded =
				!!op.bom && parts.length > 0 && op.bom.kitCode === (view.kitCode ?? "");
			const passed = includeDetection
				? inspectionVerdict({
						status: view.status,
						bomLoaded,
						detectState: "done",
						detectFailed,
						parts,
						counts: counts ?? {},
						detections: includeDetection && !detectFailed ? detections : [],
					}) === "pass"
				: view.status === "pass";
			const known = new Set(parts.map((part) => part.ic));
			const extras =
				counts === null
					? []
					: Object.entries(counts)
							.filter(([ic]) => !known.has(ic))
							.sort((left, right) => left[0].localeCompare(right[0]))
							.map(([ic, count]) => {
								const des = descriptions[ic]?.trim() ?? "";
								return des ? { ic, count, des } : { ic, count };
							});
			const detail = {
				message: view.message ?? "",
				info: view.info ?? "",
				kitsFound: view.kitsFound ?? null,
				expectedKitCount: view.expectedKitCount ?? null,
				parts: parts.map((part) => ({
					ic: part.ic,
					des: part.des,
					qty: part.qty,
					seen: counts ? (counts[part.ic] ?? 0) : null,
				})),
				extras,
			};
			const outcome = await invoke<{
				saved: boolean;
				createdAt?: string;
				imagePath?: string;
			}>("save_result", {
				passed,
				kitIc: view.kitCode ?? "",
				jobCard: view.jobCard ?? "",
				description: view.des ?? "",
				imageBase64: image,
				detail,
				slot: storedSlot,
			});
			if (!outcome.saved) return;
			const job = view.jobCard?.trim();
			writeConsole(
				`Result saved: ${passed ? "pass" : "fail"} ${view.kitCode?.trim() || "(no kit)"}${job ? ` job ${job}` : ""}${noteSlot ? ` slot ${storedSlot}` : ""}`,
			);
			const resultsPath =
				typeof config.api?.resultsPath === "string"
					? config.api.resultsPath.trim()
					: "";
			if (!resultsPath) return;
			try {
				await invoke("post_inspection_result", {
					jobCard: view.jobCard ?? "",
					timestamp: outcome.createdAt ?? "",
					passed,
					kitIc: view.kitCode ?? "",
					description: view.des ?? "",
					imagePath: outcome.imagePath ?? "",
					detail,
					slot: storedSlot,
				});
			} catch (postErr: unknown) {
				writeConsole(
					`Result post failed: ${invokeError(postErr, "Could not post the result")}. The result is saved on this PC.`,
				);
			}
		} catch (err: unknown) {
			writeConsole(
				`Result save failed: ${invokeError(err, "Could not save the result")}`,
			);
		}
	}

	function applyLookup(view: InspectView) {
		op.result = view;
		if (view.status === "pass") {
			storeBom(view);
			if (op.testMode && op.bom) {
				void chooseAndDetect();
				return;
			}
		} else {
			dropBom();
		}
		void recordResult(null, false);
	}

	function unexpectedIcs(): string[] {
		const parts = op.bom?.parts ?? [];
		if (!op.bom || parts.length === 0) return [];
		const known = new Set(parts.map((part) => part.ic));
		return Object.keys(detectionCounts(op.detections))
			.filter((ic) => !known.has(ic))
			.sort((left, right) => left.localeCompare(right));
	}

	function descriptionMap(body: { items?: unknown }): Record<string, string> {
		if (!Array.isArray(body.items)) return {};
		const names: Record<string, string> = {};
		for (const item of body.items) {
			if (!item || typeof item !== "object" || Array.isArray(item)) continue;
			const row = item as Record<string, unknown>;
			const ic = typeof row.ic === "string" ? row.ic.trim() : "";
			const des = typeof row.des === "string" ? row.des.trim() : "";
			if (ic && des && !names[ic]) names[ic] = des;
		}
		return names;
	}

	async function loadPartDescriptions(
		generation: number,
		icList: string[],
	): Promise<Record<string, string>> {
		if (!icList.length || generation !== detectGeneration || !pageAlive)
			return {};
		try {
			const body = await invoke<{ items?: unknown }>(
				"post_inventory_descriptions",
				{ icList },
			);
			if (generation !== detectGeneration || !pageAlive) return {};
			return descriptionMap(body);
		} catch (err: unknown) {
			if (generation !== detectGeneration || !pageAlive) return {};
			writeConsole(
				`Inventory descriptions: ${invokeError(err, "lookup failed")}. The list shows the IC only.`,
			);
			return {};
		}
	}

	async function finishDetection(
		generation: number,
		viewAtStart: InspectView | null,
		image: string | null,
		icList: string[],
	) {
		const descriptions = await loadPartDescriptions(generation, icList);
		if (generation !== detectGeneration || !pageAlive) return;
		op.partDescriptions = descriptions;
		if (op.result !== viewAtStart) return;
		const parts = viewAtStart?.parts ?? [];
		const bomLoaded =
			!!op.bom && parts.length > 0 && op.bom.kitCode === (viewAtStart?.kitCode ?? "");
		const panels = panelsForSlots(op.slotResults, bomLoaded);
		if (panels.length > 0) {
			for (const panel of panels) {
				await recordResult(
					image,
					true,
					descriptions,
					panel.slot,
					panel.detections,
					true,
				);
			}
			return;
		}
		await recordResult(image, true, descriptions);
	}

	function noteDetectMessage(message: string) {
		op.detectMessage = message;
		if (message === "Model is missing." || message === "Model failed to load.") {
			op.modelNotice = message;
		} else if (
			!message &&
			(op.modelNotice === "Model is missing." ||
				op.modelNotice === "Model failed to load.")
		) {
			op.modelNotice = "";
		}
		noteModelFromDetect(message);
	}

	function applyDetectedStill(encoded: unknown) {
		if (!op.snapshot) return;
		op.snapshot = {
			...op.snapshot,
			image:
				typeof encoded === "string" && encoded
					? detectedImageSrc(encoded)
					: op.snapshot.image,
			detected: op.detections.length > 0,
		};
	}

	async function runDetect(
		generation: number,
		image?: string,
		keepPanels = false,
		captureReferences = false,
	) {
		if (generation !== detectGeneration) return;
		const viewAtStart = op.result;
		op.detectState = "running";
		if (!keepPanels) {
			op.detections = [];
			op.slotResults = [];
		}
		op.partDescriptions = {};
		op.detectMessage = "";
		try {
			const payload = await requestDetections(image, captureReferences);
			if (generation !== detectGeneration || !pageAlive) return;
			if (Array.isArray(payload)) {
				const parsed: { slot: number; detections: Detection[] }[] = [];
				const flat: Detection[] = [];
				let skipped = 0;
				let checked = false;
				for (const entry of payload) {
					if (!isDetectObject(entry)) continue;
					noteDinov2(entry.dinov2);
					const slot = detectSlotNumber(entry.slot);
					if (slot === null) continue;
					const composed = await composeDetections(
						fitDetections(asDetections(entry.detections)),
						modelClassList(entry.classes),
					);
					if (generation !== detectGeneration || !pageAlive) return;
					parsed.push({ slot, detections: composed.detections });
					flat.push(...composed.detections);
					if (composed.checked) {
						checked = true;
						skipped += composed.skipped;
					}
				}
				// Slot 1 on its own keeps the single result panel.
				if (parsed.length === 1 && parsed[0].slot === 1) {
					op.slotResults = [];
					op.detections = parsed[0].detections;
				} else {
					op.slotResults = parsed;
					op.detections = flat;
				}
				if (checked) op.partRuleWarning = skipped > 0;
				noteDetectMessage("");
				applyDetectedStill(null);
			} else {
				if (!isDetectObject(payload)) throw new Error("Detect failed");
				noteDinov2(payload.dinov2);
				const composed = await composeDetections(
					fitDetections(asDetections(payload.detections)),
					modelClassList(payload.classes),
				);
				if (generation !== detectGeneration || !pageAlive) return;
				op.slotResults = [];
				op.detections = composed.detections;
				if (composed.checked) op.partRuleWarning = composed.skipped > 0;
				noteDetectMessage(
					typeof payload.message === "string" ? payload.message : "",
				);
				applyDetectedStill(payload.image_base64);
			}
			op.detectState = "done";
			const outside = unexpectedIcs();
			if (op.result === viewAtStart) {
				void finishDetection(
					generation,
					viewAtStart,
					op.snapshot?.image ?? null,
					outside,
				);
			}
		} catch (err: unknown) {
			if (generation !== detectGeneration || !pageAlive) return;
			op.slotResults = [];
			op.detectMessage = errorMessage(err, "Detect failed");
			noteModelFromDetect(op.detectMessage);
			op.detectState = "done";
			if (op.result === viewAtStart) {
				void recordResult(op.snapshot?.image ?? null, true);
			}
		}
	}

	type RustDecode = {
		status?: string;
		barcode?: string;
		type?: string;
		message?: string;
	};

	function decodedText(body: RustDecode): string {
		if (body.status !== "pass" || typeof body.barcode !== "string") return "";
		return body.barcode;
	}

	function stripOneTrailingSlash(text: string): string {
		return text.endsWith("/") ? text.slice(0, -1) : text;
	}

	type QrBarcodeDetector = {
		detect: (source: ImageBitmap) => Promise<{ rawValue?: string }[]>;
	};

	function qrBarcodeDetector(): {
		new (options?: { formats?: string[] }): QrBarcodeDetector;
		getSupportedFormats?: () => Promise<string[]>;
	} | null {
		const host = globalThis as {
			BarcodeDetector?: {
				new (options?: { formats?: string[] }): QrBarcodeDetector;
				getSupportedFormats?: () => Promise<string[]>;
			};
		};
		const Detector = host.BarcodeDetector;
		return typeof Detector === "function" ? Detector : null;
	}

	function asBarcodeRoi(value: unknown): BarcodeRoi | null {
		if (!value || typeof value !== "object") return null;
		const row = value as BarcodeRoi;
		const nums = [row.x, row.y, row.width, row.height];
		if (!nums.every((n) => typeof n === "number" && Number.isFinite(n)))
			return null;
		if (row.width <= 0 || row.height <= 0) return null;
		return { x: row.x, y: row.y, width: row.width, height: row.height };
	}
	async function cropBarcodeImage(image: string): Promise<string> {
		if (!op.barcodeRoi) return image;
		let source: ImageBitmap | null = null;
		try {
			source = await createImageBitmap(await (await fetch(image)).blob());
			const box = pixelBarcodeRoi(op.barcodeRoi, source.width, source.height);
			if (!box) return image;
			const canvas = document.createElement("canvas");
			canvas.width = box.width;
			canvas.height = box.height;
			const context = canvas.getContext("2d");
			if (!context) return image;
			context.drawImage(
				source,
				box.x,
				box.y,
				box.width,
				box.height,
				0,
				0,
				box.width,
				box.height,
			);
			console.info(
				`barcode roi applied x=${box.x} y=${box.y} width=${box.width} height=${box.height}`,
			);
			return canvas.toDataURL("image/jpeg", 0.8);
		} catch {
			return image;
		} finally {
			source?.close();
		}
	}

	async function readNative(image: string): Promise<string> {
		const Detector = qrBarcodeDetector();
		if (!Detector) return "";
		let bitmap: ImageBitmap | null = null;
		try {
			if (Detector.getSupportedFormats) {
				const formats = await Detector.getSupportedFormats();
				if (!formats.includes("qr_code")) return "";
			}
			const detector = new Detector({ formats: ["qr_code"] });
			bitmap = await createImageBitmap(await (await fetch(image)).blob());
			const found = await detector.detect(bitmap);
			const text = found.find((item) => item.rawValue)?.rawValue ?? "";
			if (!text) return "";
			return stripOneTrailingSlash(text);
		} catch {
			return "";
		} finally {
			bitmap?.close();
		}
	}

	function zxingReader(): BrowserMultiFormatReader {
		const hints = new Map();
		hints.set(DecodeHintType.POSSIBLE_FORMATS, [BarcodeFormat.QR_CODE]);
		hints.set(DecodeHintType.TRY_HARDER, true);
		return new BrowserMultiFormatReader(hints);
	}

	async function readZxing(image: string): Promise<string> {
		const reader = zxingReader();
		try {
			const found = await reader.decodeFromImageUrl(image);
			const text = found.getText();
			if (!text) return "";
			return stripOneTrailingSlash(text);
		} catch {
			return "";
		} finally {
			reader.reset();
		}
	}

	async function decodeCameraImage(preview: string): Promise<RustDecode> {
		if (preview) {
			const frame = await cropBarcodeImage(preview);
			const fromNative = await readNative(frame);
			if (fromNative) {
				op.barcodeDecoder = "native";
				console.info("barcode detector native");
				writeConsole(`Barcode decoded: ${fromNative} (JS)`);
				return { status: "pass", barcode: fromNative };
			}
			const fromZxing = await readZxing(frame);
			if (fromZxing) {
				op.barcodeDecoder = "js";
				console.info("barcode detector zxing");
				writeConsole(`Barcode decoded: ${fromZxing} (JS)`);
				return { status: "pass", barcode: fromZxing };
			}
		}
		const decoded = await invoke<RustDecode>("decode_barcode");
		const code = decodedText(decoded);
		if (code) {
			op.barcodeDecoder = "rust";
			console.info("barcode detector rust");
			writeConsole(`Barcode decoded: ${code} (backend)`);
		} else {
			op.barcodeDecoder = "";
		}
		return decoded;
	}

	async function readFrame(index: number): Promise<WatchBody> {
		try {
			const body = await invoke<WatchBody>("capture_snapshot", { index });
			if (body.status === "fail") {
				const message = body.message || "No frame";
				noteCameraOpenError(body.message || "Camera did not open.");
				throw new Error(message);
			}
			return body;
		} catch (err: unknown) {
			if (err instanceof Error && cameraErrorNote === err.message) throw err;
			const message = invokeError(err, "Camera did not open.");
			noteCameraOpenError(message);
			if (err instanceof Error) throw err;
			throw new Error(message);
		}
	}

	function sleep(ms: number): Promise<void> {
		return new Promise((resolve) => {
			setTimeout(resolve, ms);
		});
	}

	function yieldToPaint(): Promise<void> {
		return new Promise((resolve) => {
			requestAnimationFrame(() => resolve());
		});
	}

	function frameSurface(): {
		node: HTMLElement;
		width: number;
		height: number;
	} | null {
		if (op.snapshot) {
			const image = refs.paneImage;
			if (!image || image.naturalWidth < 1 || image.naturalHeight < 1)
				return null;
			return {
				node: image,
				width: image.naturalWidth,
				height: image.naturalHeight,
			};
		}
		const canvas = refs.liveCanvas;
		if (!canvas || canvas.width < 1 || canvas.height < 1) return null;
		return { node: canvas, width: canvas.width, height: canvas.height };
	}

	function rememberLiveSize() {
		const canvas = refs.liveCanvas;
		if (!canvas || op.snapshot) return;
		if (canvas.width === op.paneWidth && canvas.height === op.paneHeight)
			return;
		op.paneWidth = canvas.width;
		op.paneHeight = canvas.height;
		measureFrame();
	}

	async function drawLiveFrame(url: string): Promise<boolean> {
		const bitmap = await createImageBitmap(await (await fetch(url)).blob());
		try {
			const node = refs.liveCanvas;
			if (!node) return false;
			if (node.width !== bitmap.width || node.height !== bitmap.height) {
				node.width = bitmap.width;
				node.height = bitmap.height;
			}
			const context = node.getContext("2d", { alpha: false });
			if (!context) return false;
			context.drawImage(bitmap, 0, 0);
			previewImage = url;
			previewWidth = bitmap.width;
			previewHeight = bitmap.height;
			op.livePainted = true;
			return true;
		} finally {
			bitmap.close();
		}
	}

	// The live view keeps calling capture_preview while a still is taken.
	// capture_snapshot reads a separate still handle, and detection does not
	// stop this loop.
	async function runLive(gen: number) {
		try {
			while (gen === liveGeneration && pageAlive) {
				if (!op.feedOn || !get(cameraFeedOn)) return;
				const started = performance.now();
				const index = Number(op.cameraIndex);
				try {
					const body = await invoke<WatchBody>("capture_preview", { index });
					if (gen !== liveGeneration || !pageAlive) return;
					if (!op.feedOn || index !== Number(op.cameraIndex)) continue;
					if (body.status === "fail" || !body.image) {
						const message = body.message || "Camera did not open.";
						op.liveMessage = message;
						noteCameraOpenError(message);
						await sleep(400);
						continue;
					}
					noteCameraReady();
					if (!refs.liveCanvas) await tick();
					const drew = await drawLiveFrame(body.image);
					if (gen !== liveGeneration || !pageAlive) {
						op.livePainted = false;
						return;
					}
					if (!drew) {
						await sleep(30);
						continue;
					}
					op.liveMessage = "";
					rememberLiveSize();
				} catch (err: unknown) {
					if (gen !== liveGeneration || !pageAlive) return;
					const message = invokeError(err, "Camera did not open.");
					op.liveMessage = message;
					noteCameraOpenError(message);
					await sleep(400);
					continue;
				}
				const remain = 1000 / displayFps - (performance.now() - started);
				if (gen === liveGeneration && remain > 0) await sleep(remain);
			}
		} finally {
			if (liveGeneration === gen) liveRunning = false;
		}
	}

	function startLive() {
		if (!op.feedOn || !get(cameraFeedOn)) return;
		if (liveRunning) return;
		const gen = liveGeneration;
		liveRunning = true;
		void runLive(gen);
	}

	function bomLabel(code: string): string {
		return code.trim() ? code : "(empty)";
	}

	async function traceBom(
		code: string,
		run: () => Promise<InspectView>,
	): Promise<InspectView> {
		const label = bomLabel(code);
		writeConsole(`BoM lookup started: ${label}`);
		try {
			const view = await run();
			if (view.status === "pass") {
				writeConsole(`BoM lookup succeeded: ${label}`);
			} else {
				const message = view.message?.trim() || "Lookup failed";
				writeConsole(`BoM lookup failed: ${label}: ${message}`);
			}
			return view;
		} catch (err: unknown) {
			const message = invokeError(err, "Request failed");
			writeConsole(`BoM lookup failed: ${label}: ${message}`);
			throw err;
		}
	}

	async function lookupCode(code: string): Promise<InspectView> {
		// API sends the trimmed job card. JSON and MSSQL send the field as typed.
		const sent = op.bomSource === "api" ? code.trim() : code;
		const view = await traceBom(sent, async () => {
			if (op.bomSource === "api") {
				const body = await invoke("post_bom_api", {
					jobCardNumber: sent,
				});
				return viewFromApiData(body);
			}
			if (op.bomSource === "json") {
				const file = await invoke("load_bom_json");
				return viewFromBomFile(file, sent);
			}
			return await invoke<InspectView>("inspect_bom", { barcode: sent });
		});
		return withJobCard(view, sent);
	}

	function stillWatches(index: number): boolean {
		return (
			pageAlive &&
			!op.loading &&
			op.feedOn &&
			!op.snapshot &&
			!op.testMode &&
			index === op.cameraIndex
		);
	}

	async function watchFeed() {
		if (!op.feedOn || op.snapshot || watchInFlight || op.loading) return;
		// Wait for the camera list so the first open uses the saved index.
		if (!get(cameraFeedOn)) return;
		watchInFlight = true;
		const index = op.cameraIndex;
		try {
			// Test mode uses the typed code and does not read the camera.
			// Every other source tries native, then JS, then Rust.
			await ensureBomSource();
			await loadTestMode();
			if (
				!pageAlive ||
				!op.feedOn ||
				op.snapshot ||
				op.loading ||
				index !== op.cameraIndex
			) {
				return;
			}
			if (op.testMode) return;
			const frame = await readFrame(index);
			if (!stillWatches(index) || frame.status === "fail") return;
			noteRawSize(frame);
			await yieldToPaint();
			const decoded = await decodeCameraImage(previewImage);
			const code = decodedText(decoded);
			if (!stillWatches(index) || !code) return;
			const view = await lookupCode(code);
			if (!stillWatches(index) || view.status !== "pass") return;
			const kitCode = view.kitCode || code;
			op.barcode = op.bomSource === "api" ? code : kitCode;
			op.result = view;
			storeBom(view);
			await yieldToPaint();
			const generation = holdPreviewStill(kitCode);
			if (generation === null) {
				detectGeneration += 1;
				void runDetect(detectGeneration);
			} else {
				void runDetect(generation);
			}
		} catch {
			// A missed poll leaves the last still and the last BoM result.
		} finally {
			watchInFlight = false;
		}
	}

	type LocalConfig = {
		source?: unknown;
		scanSeconds?: unknown;
		testMode?: unknown;
		skipResultSave?: unknown;
		detectRoi?: unknown;
		detectRois?: unknown;
		partRules?: unknown;
		api?: { descriptionPath?: unknown; resultsPath?: unknown } | null;
		camera?: { index?: unknown; displayFps?: unknown } | null;
		erp?: { name?: unknown; url?: unknown } | null;
	};

	async function readConfig(): Promise<LocalConfig> {
		return await invoke<LocalConfig>("load_local_config");
	}

	async function loadScan() {
		try {
			const data = await readConfig();
			const seconds = Number(data.scanSeconds);
			if (!Number.isInteger(seconds) || seconds < 1 || seconds > 60) return;
			streamMs = seconds * 1000;
			if (op.feedOn && !op.snapshot) op.streamLeft = seconds;
			else if (op.streamLeft > seconds) op.streamLeft = seconds;
		} catch {
			// A missed read keeps the current interval.
		}
	}

	async function loadErp() {
		try {
			const data = await readConfig();
			const erp =
				data.erp && typeof data.erp === "object"
					? data.erp
					: { name: "", url: "" };
			const nextName = typeof erp.name === "string" ? erp.name.trim() : "";
			const nextUrl = typeof erp.url === "string" ? erp.url.trim() : "";
			if (nextUrl !== op.erpUrl) {
				op.erpOpen = false;
				op.iframeFailed = false;
				op.erpChecking = false;
				op.erpLoadedUrl = "";
			}
			op.erpName = nextName;
			op.erpUrl = nextUrl;
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
		if (op.camerasReady) return;
		op.cameraLookupFailed = true;
	}

	async function loadCamerasOnce(): Promise<boolean> {
		const selected = Number(op.cameraIndex);
		try {
			const listed = await invoke<unknown>("list_cameras");
			if (!pageAlive) return false;
			const found = cameraList(listed);
			const current = Number(op.cameraIndex);
			// They picked another index while the probe was running. Keep it.
			if (
				current !== selected &&
				Number.isInteger(current) &&
				current >= 0 &&
				!found.includes(current)
			) {
				op.cameraIndexes = cameraList([...found, current]);
				op.camerasReady = true;
				op.cameraLookupFailed = false;
				return true;
			}
			op.cameraIndexes = found;
			op.camerasReady = true;
			op.cameraLookupFailed = false;
			if (
				current === selected &&
				found.length > 0 &&
				!found.includes(selected)
			) {
				skippedSavedIndex = selected;
				op.cameraIndex = found[0];
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
		if (!pageAlive || op.snapshot || !op.feedOn) return;
		if (get(camerasMissing)) {
			stopLive();
			stopWatch();
			return;
		}
		if (!get(cameraFeedOn)) return;
		showLiveFeed();
	}

	async function boot() {
		if (!pageAlive) return;
		void invoke("inference_status").catch(() => undefined);
		void loadErp();
		void loadScan();
		void loadTestMode();
		void loadDetectRoi();
		void loadBarcodeRoi();
		void loadBomSource();
		void loadModel();
		await loadCamera();
		if (pageAlive) await connectCameras();
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
				op.bomSource = data.source;
			}
		} catch {
			// A missed read keeps the current label.
		}
	}

	async function loadTestMode() {
		try {
			const data = await readConfig();
			op.testMode = data.testMode === true;
		} catch {
			// A missed read keeps the current mode.
		}
	}

	function asRoi(value: unknown): DetectRoi | null {
		return parseDetectRoi(value);
	}

	function applyDetectRois(slots: (DetectRoi | null)[]) {
		const next = slots.length === 4 ? slots : parseDetectRois(slots, null);
		op.detectRois = next;
		op.detectRoi = next[0];
	}

	async function loadCamera() {
		const pending = cameraSavePending;
		try {
			const data = await readConfig();
			// Focus can read the file while a choice is still being saved.
			if (
				!pageAlive ||
				pending !== cameraSavePending ||
				cameraSavePending !== 0
			)
				return;
			const camera =
				data.camera && typeof data.camera === "object" ? data.camera : {};
			displayFps = readDisplayFps(camera.displayFps);
			const index = Number(camera.index);
			if (!Number.isInteger(index) || index < 0 || index > 32) return;
			if (index === skippedSavedIndex) return;
			op.cameraIndex = index;
		} catch {
			// A missed read keeps the current camera.
		}
	}

	async function applySavedCamera() {
		const previous = Number(op.cameraIndex);
		await loadCamera();
		if (!pageAlive) return;
		if (Number(op.cameraIndex) !== previous) {
			await loadCameras();
			if (op.feedOn && !op.snapshot) showLiveFeed();
		}
	}

	async function saveCameraIndex(next: number) {
		skippedSavedIndex = null;
		const index = Number(next);
		op.cameraIndex = index;
		if (!Number.isInteger(index) || index < 0 || index > 32) return;
		const generation = ++cameraSaveGeneration;
		cameraSavePending += 1;
		const run = cameraSaveChain.then(async () => {
			try {
				if (!pageAlive || generation !== cameraSaveGeneration) return;
				const config =
					await invoke<Record<string, unknown>>("load_local_config");
				if (!pageAlive || generation !== cameraSaveGeneration) return;
				const chosen = activateCamera(config, index);
				displayFps = chosen.displayFps;
				await invoke("save_local_config", { config });
				if (!pageAlive || generation !== cameraSaveGeneration) return;
				if (op.feedOn && !op.snapshot) showLiveFeed();
			} catch {
				// The live view still uses the picked index. The next launch reads the last save.
			} finally {
				cameraSavePending -= 1;
			}
		});
		cameraSaveChain = run.then(
			() => undefined,
			() => undefined,
		);
		await run;
	}

	function readAdminFlag() {
		op.admin = sessionStorage.getItem(ADMIN_FLAG) === "1";
		if (!op.admin) {
			op.modelMenuOpen = false;
			op.consoleOpen = false;
		}
		void loadBarcodeRoi();
		bomSourceReady = loadBomSource();
		void loadTestMode();
		void loadModel();
		void applySavedCamera();
	}

	function matchModelName(name: string, files: string[]): string {
		const cleaned = name.trim().toLowerCase();
		if (!cleaned) return "";
		return files.find((item) => item.toLowerCase() === cleaned) ?? "";
	}

	function preferredModel(files: string[]): string {
		return (
			matchModelName("best.onnx", files) ||
			matchModelName("best.pt", files) ||
			files.find((item) => item.toLowerCase().endsWith(".onnx")) ||
			files[0] ||
			"best.onnx"
		);
	}

	function readModelFiles(value: unknown): string[] {
		if (!Array.isArray(value)) return [];
		return value.filter(
			(item): item is string => typeof item === "string" && item.length > 0,
		);
	}

	function closeModelMenu() {
		if (op.savingModel) return;
		op.modelMenuOpen = false;
	}

	async function loadModel() {
		try {
			const listed = await invoke<unknown>("list_vision_models");
			const config = await invoke<{ model?: unknown }>("load_local_config");
			const files = readModelFiles(listed);
			op.modelFiles = files;
			const stored = typeof config.model === "string" ? config.model : "";
			const match = matchModelName(stored, files);
			op.modelName = match || preferredModel(files);
			const present = files.some(
				(item) => item.toLowerCase() === op.modelName.toLowerCase(),
			);
			if (!present) {
				op.modelNotice = "Model is missing.";
				noteModelFailed("Model is missing.");
			} else if (op.modelName.toLowerCase().endsWith(".pt")) {
				if (op.modelNotice === "Model is missing.") op.modelNotice = "";
				noteModelFailed(
					`${op.modelName} is a .pt file. Choose an .onnx model to detect parts.`,
				);
			} else if (op.modelNotice === "Model is missing.") {
				op.modelNotice = "";
			}
		} catch {
			// A missed read keeps the last model name.
		}
	}

	async function toggleModelMenu() {
		if (!op.admin || op.savingModel) return;
		if (op.modelMenuOpen) {
			op.modelMenuOpen = false;
			return;
		}
		op.modelMenuOpen = true;
		op.modelNotice = "";
		await loadModel();
	}

	async function chooseModel(filename: string) {
		if (!op.admin || op.savingModel) return;
		if (filename.toLowerCase() === op.modelName.toLowerCase()) {
			op.modelMenuOpen = false;
			return;
		}
		op.savingModel = true;
		op.modelNotice = "";
		try {
			const saved = await invoke<{
				active?: unknown;
				files?: unknown;
			}>("set_active_model", { filename });
			const files = readModelFiles(saved.files);
			if (files.length > 0) op.modelFiles = files;
			const active =
				typeof saved.active === "string" && saved.active.trim()
					? saved.active.trim()
					: filename;
			op.modelName = active;
			op.modelMenuOpen = false;
			const notice = active.toLowerCase().endsWith(".onnx")
				? `Saved. Active model is ${active}.`
				: `${active} is a .pt file. Choose an .onnx model to detect parts.`;
			op.modelNotice = notice;
			if (!active.toLowerCase().endsWith(".onnx")) noteModelFailed(notice);
		} catch (err: unknown) {
			const message = invokeError(err, "Could not switch the model");
			op.modelNotice = message;
			noteModelFailed(message);
		} finally {
			op.savingModel = false;
		}
	}

	function rememberPaneSize(image?: HTMLImageElement | null) {
		// The snapshot load can finish before the child bind reaches this page.
		if (image) refs.paneImage = image;
		const still = image ?? refs.paneImage;
		op.paneWidth = still?.naturalWidth ?? 0;
		op.paneHeight = still?.naturalHeight ?? 0;
		measureFrame();
	}

	function measureFrame() {
		op.frameWidth = refs.paneFrame?.clientWidth ?? 0;
		op.frameHeight = refs.paneFrame?.clientHeight ?? 0;
	}

	function showDraft(box: DetectRoi) {
		const surface = frameSurface();
		const frame = refs.paneFrame;
		if (!surface || !frame || (box.w <= 0 && box.h <= 0)) {
			op.draftMark = null;
			return;
		}
		const nw = surface.width;
		const nh = surface.height;
		if (nw < 1 || nh < 1) {
			op.draftMark = null;
			return;
		}
		const rect = surface.node.getBoundingClientRect();
		const frameRect = frame.getBoundingClientRect();
		if (rect.width < 1 || rect.height < 1) {
			op.draftMark = null;
			return;
		}
		const scale = Math.min(rect.width / nw, rect.height / nh);
		const drawnW = nw * scale;
		const drawnH = nh * scale;
		const originX = rect.left + (rect.width - drawnW) / 2 - frameRect.left;
		const originY = rect.top + (rect.height - drawnH) / 2 - frameRect.top;
		op.draftMark = {
			left: originX + box.x * drawnW,
			top: originY + box.y * drawnH,
			width: Math.max(2, box.w * drawnW),
			height: Math.max(2, box.h * drawnH),
		};
	}

	function cancelRoiDraw() {
		op.drawing = false;
		op.draftMark = null;
		roiDrag = null;
	}

	function cancelBarcodeDraw() {
		op.barcodeDrawing = false;
		op.draftMark = null;
		roiDrag = null;
	}

	function setDetectSlot(next: number) {
		op.detectSlot = detectSlotNumber(next) ?? 0;
	}

	function beginSetRoi() {
		if (!op.admin) return;
		if (op.barcodeDrawing) cancelBarcodeDraw();
		if (op.drawing) {
			cancelRoiDraw();
			op.roiMessage = "";
			return;
		}
		if (detectSlotNumber(op.detectSlot) === null) {
			op.roiMessage = "Choose a slot from 1 to 4.";
			return;
		}
		if (!op.snapshot) {
			op.roiMessage = "Hold a snapshot, then draw the region.";
			return;
		}
		op.roiMessage = "";
		op.drawing = true;
		op.draftMark = null;
		roiDrag = null;
	}

	function pointInImage(event: PointerEvent): { x: number; y: number } | null {
		const surface = frameSurface();
		if (!surface) return null;
		const rect = surface.node.getBoundingClientRect();
		const nw = surface.width;
		const nh = surface.height;
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
			enabled: true,
		};
	}

	function onRoiPointerDown(event: PointerEvent) {
		if (!op.drawing && !op.barcodeDrawing) return;
		const point = pointInImage(event);
		if (!point) return;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
		roiDrag = point;
		op.draftMark = null;
	}

	function onRoiPointerMove(event: PointerEvent) {
		if (!roiDrag) return;
		const point = pointInImage(event);
		if (!point) return;
		const box = rectFromPoints(roiDrag, point);
		showDraft(box);
	}

	function onRoiPointerUp(event: PointerEvent) {
		if (!roiDrag) return;
		const start = roiDrag;
		const point = pointInImage(event) ?? start;
		roiDrag = null;
		const roi = rectFromPoints(start, point);
		if (roi.w < 0.01 || roi.h < 0.01) {
			op.draftMark = null;
			op.roiMessage = "Drag a larger rectangle.";
			return;
		}
		showDraft(roi);
		if (op.barcodeDrawing) {
			void saveBarcodeRoi({
				x: roi.x,
				y: roi.y,
				width: roi.w,
				height: roi.h,
			});
			return;
		}
		void saveDetectRoi(roi);
	}

	async function loadDetectRoi() {
		try {
			const data = await readConfig();
			applyDetectRois(parseDetectRois(data.detectRois, data.detectRoi));
		} catch {
			// A missed read keeps the current region.
		}
	}

	function roundRoi(roi: DetectRoi): DetectRoi {
		const round = (value: number) => Math.round(value * 10000) / 10000;
		return {
			x: round(roi.x),
			y: round(roi.y),
			w: round(roi.w),
			h: round(roi.h),
			enabled: roi.enabled !== false,
		};
	}

	async function saveDetectRoi(roi: DetectRoi | null) {
		if (!op.admin) return;
		const slot = detectSlotNumber(op.detectSlot);
		if (slot === null) {
			op.roiMessage = "Choose a slot from 1 to 4.";
			op.draftMark = null;
			return;
		}
		op.savingRoi = true;
		op.roiMessage = "";
		try {
			const config = await invoke<Record<string, unknown>>("load_local_config");
			const slots = parseDetectRois(config.detectRois, config.detectRoi);
			const previous = slots[slot - 1];
			const next = roi
				? { ...roundRoi(roi), enabled: previous?.enabled ?? true }
				: null;
			const stored =
				next === null
					? compactClearedSlot(slots, slot)
					: slots.map((item, index) => (index === slot - 1 ? next : item));
			config.detectRois = stored;
			config.detectRoi = stored[0];
			await invoke("save_local_config", { config });
			applyDetectRois(stored.map((item) => asRoi(item)));
			if (next === null) applyClearedSlotResults(slot);
			op.draftMark = null;
			op.drawing = false;
			roiDrag = null;
			op.roiMessage = next ? `Saved slot ${slot}.` : `Cleared slot ${slot}.`;
			rerunDetect(next === null);
		} catch (err: unknown) {
			op.roiMessage = errorMessage(err, "Could not save the ROI");
		} finally {
			op.savingRoi = false;
		}
	}

	function clearDetectRoi() {
		if (!op.admin || op.savingRoi) return;
		if (detectSlotNumber(op.detectSlot) === null) return;
		cancelRoiDraw();
		void saveDetectRoi(null);
	}

	let detectRoiWrite: Promise<void> = Promise.resolve();

	function queueDetectRoiWrite(slots: (DetectRoi | null)[]) {
		const snapshot = slots.map((item) => (item ? { ...item } : null));
		const run = async () => {
			try {
				const config = await invoke<Record<string, unknown>>("load_local_config");
				config.detectRois = snapshot;
				config.detectRoi = snapshot[0];
				await invoke("save_local_config", { config });
			} catch (err: unknown) {
				op.roiMessage = errorMessage(err, "Could not save the slot");
				await loadDetectRoi();
			}
		};
		detectRoiWrite = detectRoiWrite.then(run, run);
	}

	function toggleDetectSlot(slot: number) {
		if (op.admin) return;
		const index = slot - 1;
		const roi = op.detectRois[index];
		if (!roi) return;
		const stored = withSlotEnabled(op.detectRois, slot, !roi.enabled);
		applyDetectRois(stored);
		queueDetectRoiWrite(stored);
	}

	function disableAllDetectSlots() {
		if (op.admin) return;
		if (!op.detectRois.some((roi) => roi?.enabled)) return;
		const stored = withAllSlotsDisabled(op.detectRois);
		applyDetectRois(stored);
		queueDetectRoiWrite(stored);
	}

	async function loadBarcodeRoi() {
		try {
			const config = await invoke<{ barcodeRoi?: unknown }>(
				"load_local_config",
			);
			op.barcodeRoi = asBarcodeRoi(config.barcodeRoi);
		} catch {
			// A missed read keeps the current barcode region.
		}
	}

	function roundBarcodeRoi(roi: BarcodeRoi): BarcodeRoi {
		const round = (value: number) => Math.round(value * 10000) / 10000;
		return {
			x: round(roi.x),
			y: round(roi.y),
			width: round(roi.width),
			height: round(roi.height),
		};
	}

	async function saveBarcodeRoi(roi: BarcodeRoi | null) {
		if (!op.admin) return;
		op.savingBarcodeRoi = true;
		op.roiMessage = "";
		try {
			const config = await invoke<Record<string, unknown>>("load_local_config");
			config.barcodeRoi = roi ? roundBarcodeRoi(roi) : null;
			await invoke("save_local_config", { config });
			op.barcodeRoi = asBarcodeRoi(config.barcodeRoi);
			op.draftMark = null;
			op.barcodeDrawing = false;
			roiDrag = null;
			op.roiMessage = op.barcodeRoi
				? "Saved. Barcode decode uses this region."
				: "Saved. Barcode decode uses the full frame.";
		} catch (err: unknown) {
			op.roiMessage = invokeError(err, "Could not save the barcode ROI");
		} finally {
			op.savingBarcodeRoi = false;
		}
	}

	function beginSetBarcodeRoi() {
		if (!op.admin) return;
		if (op.barcodeDrawing) {
			cancelBarcodeDraw();
			op.roiMessage = "";
			return;
		}
		if (op.snapshot) {
			op.roiMessage = "Resume the live feed, then draw the barcode region.";
			return;
		}
		if (!get(cameraFeedOn)) {
			op.roiMessage = "Start the camera, then draw the barcode region.";
			return;
		}
		cancelRoiDraw();
		op.roiMessage = "";
		op.barcodeDrawing = true;
		op.draftMark = null;
		roiDrag = null;
	}

	function clearBarcodeRoi() {
		if (!op.admin || op.savingBarcodeRoi) return;
		cancelBarcodeDraw();
		void saveBarcodeRoi(null);
	}

	function applyClearedSlotResults(clearedSlot: number) {
		if (op.slotResults.length === 0) {
			if (!op.detectRois.some((roi) => roi !== null)) op.detections = [];
			return;
		}
		const remaining = reindexSlotResults(op.slotResults, clearedSlot);
		if (remaining.length === 0) {
			op.slotResults = [];
			op.detections = [];
			return;
		}
		if (remaining.length === 1) {
			op.slotResults = [];
			op.detections = remaining[0].detections;
			return;
		}
		op.slotResults = remaining;
		op.detections = remaining.flatMap((panel) => panel.detections);
	}

	function rerunDetect(keepPanels = false) {
		if (!op.snapshot?.source || !pageAlive) return;
		detectGeneration += 1;
		const generation = detectGeneration;
		void runDetect(generation, undefined, keepPanels);
	}

	function chooseImageFile(): Promise<File | null> {
		const input = refs.imagePicker;
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

	async function chooseImageAndDetect(captureReferences: boolean) {
		if (!op.bom || !op.testMode) return;
		const kitCode = op.bom.kitCode;
		const file = await chooseImageFile();
		if (!file || !pageAlive || !op.bom || op.bom.kitCode !== kitCode) return;
		try {
			const dataUrl = await readImageFile(file);
			if (!pageAlive || !op.bom || op.bom.kitCode !== kitCode) return;
			// Boxes stay in this file's pixels. The camera frame is not the photo.
			scaleToPreview = false;
			const generation = holdStill(dataUrl, kitCode);
			await runDetect(generation, dataUrl, false, captureReferences);
		} catch (err: unknown) {
			if (!pageAlive) return;
			op.detectMessage = errorMessage(err, "Could not read the image");
			op.detectState = "done";
		}
	}

	function chooseAndDetect() {
		return chooseImageAndDetect(false);
	}

	function chooseAndSaveReferences() {
		return chooseImageAndDetect(true);
	}

	async function openErp() {
		if (!op.erpUrl) return;
		op.erpOpen = true;
		await tick();
		refs.erpCloseButton?.focus();
		// The ERP page stays loaded behind the vision view. Opening it again
		// must not navigate the iframe, or a menu selection is lost.
		if (op.erpLoadedUrl === op.erpUrl && !op.iframeFailed) return;
		op.iframeFailed = false;
		op.erpChecking = false;
		op.erpLoadedUrl = op.erpUrl;
	}

	function closeErp() {
		op.erpOpen = false;
	}

	function onErpKey(event: KeyboardEvent) {
		if (event.key === "Escape" && op.erpOpen) closeErp();
		else if (event.key === "Escape" && op.drawing) cancelRoiDraw();
		else if (event.key === "Escape" && op.modelMenuOpen) closeModelMenu();
		else if (event.key === "Escape" && op.barcodeDrawing) cancelBarcodeDraw();
		else if (!op.admin && !op.erpOpen) onSlotKey(event);
	}

	function typingTarget(target: EventTarget | null): boolean {
		if (!(target instanceof HTMLElement)) return false;
		const tag = target.tagName;
		return (
			tag === "INPUT" ||
			tag === "TEXTAREA" ||
			tag === "SELECT" ||
			target.isContentEditable
		);
	}

	function onSlotKey(event: KeyboardEvent) {
		if (event.repeat || event.ctrlKey || event.metaKey || event.altKey) return;
		if (typingTarget(event.target)) return;
		if (event.key === "`") {
			event.preventDefault();
			disableAllDetectSlots();
			return;
		}
		if (event.key < "1" || event.key > "4") return;
		const slot = Number(event.key);
		if (!op.detectRois[slot - 1]) return;
		event.preventDefault();
		toggleDetectSlot(slot);
	}

	function onIframeError() {
		op.iframeFailed = true;
	}

	async function openErpInBrowser() {
		if (!op.erpUrl) return;
		try {
			await openUrl(op.erpUrl);
		} catch {
			window.open(op.erpUrl, "_blank", "noopener,noreferrer");
		}
	}

	function start() {
		readAdminFlag();
		window.addEventListener("focus", readAdminFlag);
		measureFrame();
		if (refs.paneFrame) {
			frameObserver = new ResizeObserver(() => measureFrame());
			frameObserver.observe(refs.paneFrame);
		}
		void boot();
	}

	function stop() {
		pageAlive = false;
		if (feedBusyTimer !== null) {
			clearTimeout(feedBusyTimer);
			feedBusyTimer = null;
		}
		window.removeEventListener("focus", readAdminFlag);
		frameObserver?.disconnect();
		frameObserver = null;
		stopWatch();
		stopLive();
	}

	function errorMessage(err: unknown, fallback: string): string {
		return err instanceof Error && err.message ? err.message : fallback;
	}

	function invokeError(err: unknown, fallback: string): string {
		if (typeof err === "string" && err.trim()) return err;
		return errorMessage(err, fallback);
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

	function jobCardText(row: Record<string, unknown>): string {
		return apiText(row, [
			"jobCard",
			"jobCardNumber",
			"job_card",
			"job_card_number",
			"jobCardNo",
			"jc",
		]).slice(0, 80);
	}

	function normalizeJobCard(raw: string): string {
		const text = raw.trim();
		if (!text) return "";
		if (/^\d+$/.test(text)) {
			const digits = text.replace(/^0+(?=\d)/, "");
			return digits.length <= 18 ? digits : "";
		}
		return text.slice(0, 80);
	}

	function jobCardFromLabel(raw: string): string {
		const text = stripOneTrailingSlash(raw.trim());
		if (!text) return "";
		if (text.startsWith("{")) {
			try {
				const parsed: unknown = JSON.parse(text);
				if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
					const found = jobCardText(parsed as Record<string, unknown>);
					if (found) return normalizeJobCard(found);
				}
			} catch {
				// The barcode is not a JSON object.
			}
		}
		const labelled = text.match(
			/(?:^|[^a-z0-9])(?:job\s*card|jobcard|job|jc)\s*[:=#-]?\s*(\d{1,18})(?!\d)/i,
		);
		if (labelled?.[1]) return normalizeJobCard(labelled[1]);
		const compact = text.replace(/\s+/g, "");
		if (/^\d{1,18}$/.test(compact)) return normalizeJobCard(compact);
		return "";
	}

	function withJobCard(view: InspectView, scanned: string): InspectView {
		// The scanned label wins. The BoM field is used when the label has no job card.
		const jobCard = jobCardFromLabel(scanned) || (view.jobCard ?? "").trim();
		if (!jobCard || jobCard === view.jobCard) return view;
		return { ...view, jobCard };
	}

	function keepBomJobCard(
		view: InspectView,
		row: Record<string, unknown>,
	): InspectView {
		if (view.jobCard?.trim()) return view;
		const jobCard = jobCardText(row);
		if (!jobCard) return view;
		return { ...view, jobCard };
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
		const jobCard = jobCardText(row);
		return {
			status: "pass",
			message: "BoM loaded – 2 kit(s) on bench",
			kitCode,
			...(jobCard ? { jobCard } : {}),
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
			return {
				status: "fail",
				message: "BoM API response has no data object.",
			};
		}
		const row = data as Record<string, unknown>;
		const directCode = apiText(row, ["kitIC", "kitIc", "kitCode", "kit", "ic"]);
		if (directCode && Array.isArray(apiField(row, ["parts"]))) {
			return keepBomJobCard(viewFromKit(directCode, row), row);
		}
		const kits = kitsKeyedByIc(row);
		if (kits.length > 1) {
			return keepBomJobCard(
				{ status: "fail", message: "BoM API returned more than one kit." },
				row,
			);
		}
		if (kits.length === 1) {
			return keepBomJobCard(viewFromKit(kits[0].kitCode, kits[0].job), row);
		}
		if (directCode) {
			return keepBomJobCard(
				{ status: "fail", message: "BoM API response has no parts." },
				row,
			);
		}
		return keepBomJobCard(
			{ status: "fail", message: "BoM API response has no kit IC." },
			row,
		);
	}

	async function runApiLookup() {
		if (op.loading) return;
		op.loading = true;
		op.result = null;
		try {
			applyLookup(await lookupCode(op.barcode));
		} catch (err: unknown) {
			op.result = {
				status: "fail",
				message: invokeError(err, "Request failed"),
			};
			dropBom();
			void recordResult(null, false);
		} finally {
			op.loading = false;
		}
	}

	async function runJsonLookup() {
		if (op.loading) return;
		op.loading = true;
		op.result = null;
		try {
			applyLookup(await lookupCode(op.barcode));
		} catch (err: unknown) {
			op.result = {
				status: "fail",
				message: invokeError(err, "Request failed"),
			};
			dropBom();
			void recordResult(null, false);
		} finally {
			op.loading = false;
		}
	}

	async function runInspect() {
		if (op.loading) return;
		await ensureBomSource();
		if (!pageAlive || op.loading) return;
		if (op.bomSource === "api") {
			await runApiLookup();
			return;
		}
		if (op.bomSource === "json") {
			await runJsonLookup();
			return;
		}
		op.loading = true;
		op.result = null;
		try {
			applyLookup(await lookupCode(op.barcode));
		} catch (err: unknown) {
			op.result = {
				status: "fail",
				message: invokeError(err, "Request failed"),
			};
			dropBom();
			void recordResult(null, false);
		} finally {
			op.loading = false;
		}
	}

	async function decodeAndInspect() {
		if (op.loading) return;
		await ensureBomSource();
		await loadTestMode();
		if (!pageAlive) return;
		// Test mode never reads the camera. The typed code selects the kit, then
		// the page asks for an image file. SQL/JSON and API keep their own lookup.
		if (op.testMode) {
			await runInspect();
			return;
		}
		op.loading = true;
		try {
			const frame = await readFrame(op.cameraIndex);
			if (!pageAlive) return;
			noteRawSize(frame);
			await yieldToPaint();
			const decoded = await decodeCameraImage(previewImage);
			if (!pageAlive) return;
			const code = decodedText(decoded);
			if (!code) {
				op.result = {
					status: "fail",
					message: decoded.message || "No code detected",
				};
				void recordResult(previewImage || null, false);
				return;
			}
			const view = await lookupCode(code);
			if (!pageAlive) return;
			op.result = view;
			if (view.status === "pass") {
				const kitCode = view.kitCode || code;
				op.barcode = op.bomSource === "api" ? code : kitCode;
				storeBom(view);
				await yieldToPaint();
				const generation = holdPreviewStill(kitCode);
				if (generation === null) {
					detectGeneration += 1;
					void runDetect(detectGeneration);
				} else {
					void runDetect(generation);
				}
			} else {
				op.barcode = code;
				dropBom();
				void recordResult(previewImage || null, false);
			}
		} catch (err: unknown) {
			op.result = {
				status: "fail",
				message: invokeError(err, "Decode failed"),
			};
			void recordResult(null, false);
		} finally {
			op.loading = false;
		}
	}

	const session: OperatorSession = {
		refs,
		start,
		stop,
		runInspect,
		saveCameraIndex,
		decodeAndInspect,
		toggleFeed,
		beginSetRoi,
		clearDetectRoi,
		setDetectSlot,
		toggleDetectSlot,
		beginSetBarcodeRoi,
		clearBarcodeRoi,
		openErp,
		rememberPaneSize,
		resumeLiveFeed,
		onRoiPointerDown,
		onRoiPointerMove,
		onRoiPointerUp,
		chooseAndDetect,
		chooseAndSaveReferences,
		onImagePicked,
		onImageCancel,
		toggleModelMenu,
		closeModelMenu,
		chooseModel,
		closeErp,
		onErpKey,
		onIframeError,
		openErpInBrowser,
		toggleConsole,
		clearConsole,
		dismissPartRuleWarning,
	};
	return session;
}
