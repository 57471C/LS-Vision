<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { onDestroy, onMount, tick } from "svelte";
import ClassPicker from "$lib/ClassPicker.svelte";
import {
	activateCamera,
	assignActiveCamera,
	type CameraSettings,
	readDisplayFps,
	rememberActiveCamera,
	retainCameraSettings,
	settingsForCamera,
} from "$lib/camera-settings";
import {
	compactClearedSlot,
	type DetectRoi,
	parseDetectRoi,
	parseDetectRois,
	withSlotEnabled,
} from "$lib/detect-rois";
import {
	PART_LABEL_LIMIT,
	PART_RULE_LIMIT,
	type PartCondition,
	type PartRule,
	parsePartRules,
} from "$lib/part-composition";

const DEV_PASSWORD = "admin";
const DEFAULT_DESCRIPTION_PATH = "/VisionAppApi/ListInventoryDescription";
const UNLOCK_KEY = "ls-vision-settings-open";
const SECTION_KEY = "ls-vision-settings-section";
const DEFAULT_SQL_DRIVER = "ODBC Driver 18 for SQL Server";

type BomSource = "json" | "mssql" | "api";
type AuthMode = "windows" | "sql";
type MssqlFields = { server: string; database: string; user: string };
type SettingsSection = "camera" | "capture" | "training" | "parts" | "database";
type BarcodeRoi = { x: number; y: number; width: number; height: number };

const sectionOn =
	"flex w-full flex-col items-center gap-1 border border-blue-500 bg-neutral-800 px-1 py-2 text-[10px] font-semibold uppercase tracking-wide text-neutral-100";
const sectionOff =
	"flex w-full flex-col items-center gap-1 border border-transparent px-1 py-2 text-[10px] font-semibold uppercase tracking-wide text-neutral-500 hover:border-neutral-700 hover:bg-neutral-800 hover:text-neutral-100";

let unlocked = storedUnlocked();
let section: SettingsSection = storedSection();
let password = "";
let gateMessage = "";
let source: BomSource = "json";
let auth: AuthMode = "windows";
let mssql: MssqlFields = { server: "", database: "", user: "" };
let mssqlPassword = "";
let apiBaseUrl = "";
let apiPath = "";
let apiKey = "";
let apiDescriptionPath = DEFAULT_DESCRIPTION_PATH;
let resultsPath = "";
let notice = "";
let saving = false;
let erpName = "";
let erpUrl = "";
let erpNotice = "";
let savingErp = false;
let scanSeconds = 8;
let scanNotice = "";
let savingScan = false;
let testMode = false;
let skipResultSave = true;
let testNotice = "";
let savingTest = false;
let roi: DetectRoi | null = null;
let roiSlot = "";
let roiSlots: (DetectRoi | null)[] = [null, null, null, null];
let roiX: number | null = null;
let roiY: number | null = null;
let roiW: number | null = null;
let roiH: number | null = null;
let roiNotice = "";
let savingRoi = false;
let barcodeRoi: BarcodeRoi | null = null;
let barcodeX: number | null = null;
let barcodeY: number | null = null;
let barcodeW: number | null = null;
let barcodeH: number | null = null;
let barcodeNotice = "";
let savingBarcodeRoi = false;
let modelFiles: string[] = [];
let modelName = "";
let modelActive = "best.onnx";
let modelNotice = "";
let savingModel = false;
let updatingModel = false;
let updatingReferences = false;
let referenceNotice = "";
let referencePath = "";
type ReferenceRow = {
	ic: string;
	threshold: number | null;
	count: number;
};
let referencesOpen = false;
let referencesLoading = false;
let referencesNotice = "";
let referenceRows: ReferenceRow[] = [];
let referenceDrafts: Record<string, string> = {};
let referenceSort: "ic" | "threshold" = "ic";
let referenceSortDir: "asc" | "desc" = "asc";
let savingReferenceIc = "";
let deletingReferenceIc = "";
let referencesGeneration = 0;
let referencesClose: HTMLButtonElement | null = null;
const CONFIDENCE_MIN = 0.15;
const CONFIDENCE_MAX = 0.8;
const CONFIDENCE_DEFAULT = 0.45;
let confidence = CONFIDENCE_DEFAULT;
let confidenceText = "0.45";
let confidenceNotice = "";
let confidenceTimer: ReturnType<typeof setTimeout> | null = null;
let confidenceGeneration = 0;
let confidenceChain: Promise<void> = Promise.resolve();
const MATCH_MIN = 0.5;
const MATCH_MAX = 0.95;
const MATCH_DEFAULT = 0.7;
let matchThreshold = MATCH_DEFAULT;
let matchText = "0.70";
let matchNotice = "";
let matchTimer: ReturnType<typeof setTimeout> | null = null;
let matchGeneration = 0;
let matchChain: Promise<void> = Promise.resolve();
let inferenceProviders: string[] = ["cpu"];
let inferenceEp = "cpu";
let inferenceActive = "cpu";
let useOpenVino = false;
let openVinoActive = false;
let inferenceNotice = "";
let inferenceReady = false;
let savingInference = false;
let inferenceGeneration = 0;
let inferenceChain: Promise<void> = Promise.resolve();
let benchmarking = false;
let benchmarkText = "";
type CameraMode = {
	id: string;
	width: number;
	height: number;
	fps: number;
	format?: string;
};

let cameraIndex = 0;
let cameraChoices: number[] = [0];
let cameraModes: CameraMode[] = [];
let cameraModeDefault = "";
let cameraResolution = "";
const DISPLAY_FPS = [15, 20, 25, 30];
let cameraDisplayFps = 15;
let cameraFocus = 50;
let focusAuto = false;
let cameraBrightness = 50;
let cameraContrast = 50;
let savedCamera: {
	index: number;
	resolution: string;
	displayFps: number;
	focus: number | null;
	brightness: number;
	contrast: number;
} = {
	index: 0,
	resolution: "",
	displayFps: 15,
	focus: 50,
	brightness: 50,
	contrast: 50,
};
let focusText = "50";
let brightnessText = "50";
let contrastText = "50";
let cameraApplyTimer: ReturnType<typeof setTimeout> | null = null;
let cameraPushId = 0;
// Posts stay in order. A slow live update must not land after Reset or Cancel.
let cameraPostChain: Promise<void> = Promise.resolve();
// Last config used to fill the form. A camera change can paint before the file write.
let cameraFile: Record<string, unknown> | null = null;
// Bumped when the form changes. A finished file write paints only if it is still current.
let cameraFileVersion = 0;
let cameraNotice = "";
let cameraListNotice = "";
let previewNotice = "";
let previewChannel: BroadcastChannel | null = null;
let savingCamera = false;
let trainingDir = "";
let trainingNotice = "";
let savingTraining = false;
let configLoadId = 0;
type PartRuleDraft = PartRule & { id: number };
let partRules: PartRuleDraft[] = [];
let partNotice = "";
let savingParts = false;
let partLoadId = 0;
let partRuleSeq = 0;
let partClasses: string[] = [];
let partClassMessage = "Load a model to choose classes.";
let partClassesReady = false;
let partClassLoadId = 0;

const PREVIEW_LABEL = "camera-preview";
const PREVIEW_CHANNEL = "ls-vision-camera-preview";

function isModeId(value: string): boolean {
	return /^\d+x\d+@\d+(\.\d{1,3})?$/.test(value);
}

function modeLabel(mode: CameraMode): string {
	const fps = Number.isInteger(mode.fps)
		? String(mode.fps)
		: mode.fps.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
	return `${mode.width}×${mode.height} · ${fps} fps`;
}

function storedUnlocked(): boolean {
	if (typeof sessionStorage === "undefined") return false;
	return sessionStorage.getItem(UNLOCK_KEY) === "1";
}

function storedSection(): SettingsSection {
	if (typeof sessionStorage === "undefined") return "camera";
	const value = sessionStorage.getItem(SECTION_KEY);
	if (
		value === "camera" ||
		value === "capture" ||
		value === "training" ||
		value === "parts" ||
		value === "database"
	) {
		return value;
	}
	return "camera";
}

function selectSection(next: SettingsSection) {
	section = next;
	sessionStorage.setItem(SECTION_KEY, next);
	if (next === "camera") void loadCameraSettings();
	if (next === "training") void loadModel();
	if (next === "parts") {
		void loadPartRules();
		void loadPartClasses();
	}
}

function focusOnMount(node: HTMLInputElement) {
	node.focus();
	requestAnimationFrame(() => {
		if (node.isConnected) node.focus();
	});
}

function errorMessage(err: unknown, fallback: string): string {
	return err instanceof Error && err.message ? err.message : fallback;
}

function readSource(value: unknown): BomSource {
	if (value === "mssql" || value === "api" || value === "json") return value;
	return "json";
}

function readApi(value: unknown) {
	apiDescriptionPath = DEFAULT_DESCRIPTION_PATH;
	if (!value || typeof value !== "object") {
		resultsPath = "";
		return;
	}
	const api = value as {
		baseUrl?: unknown;
		path?: unknown;
		key?: unknown;
		descriptionPath?: unknown;
		resultsPath?: unknown;
	};
	if (typeof api.baseUrl === "string") apiBaseUrl = api.baseUrl;
	if (typeof api.path === "string") apiPath = api.path;
	if (typeof api.key === "string") apiKey = api.key;
	if ("descriptionPath" in api) {
		apiDescriptionPath =
			typeof api.descriptionPath === "string" ? api.descriptionPath : "";
	}
	resultsPath = typeof api.resultsPath === "string" ? api.resultsPath : "";
}

function descriptionPathError(value: string): string {
	const path = value.trim();
	if (!path) return "";
	if (path.length > 200 || /\s/.test(path) || path.includes("://")) {
		return "Enter a path such as /VisionAppApi/ListInventoryDescription.";
	}
	return "";
}

function resultsPathError(value: string): string {
	const path = value.trim();
	if (!path) return "";
	if (path.length > 200 || /\s/.test(path) || path.includes("://")) {
		return "Enter a path such as /VisionAppApi/SubmitResult.";
	}
	return "";
}

function apiBaseUrlError(value: string): string {
	const url = value.trim();
	if (!url) return "";
	try {
		const parsed = new URL(url);
		if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
			return "Base URL must start with http:// or https://";
		}
		if (!parsed.host) return "Base URL must start with http:// or https://";
	} catch {
		return "Base URL must start with http:// or https://";
	}
	return "";
}

function asRoi(value: unknown): DetectRoi | null {
	return parseDetectRoi(value);
}

function showRoiSlot() {
	const index = Number(roiSlot);
	if (!Number.isInteger(index) || index < 1 || index > 4) {
		fillRoi(null);
		return;
	}
	fillRoi(roiSlots[index - 1] ?? null);
}

function onRoiSlotChange(event: Event) {
	const target = event.currentTarget;
	if (!(target instanceof HTMLSelectElement)) return;
	roiSlot = target.value;
	roiNotice = "";
	showRoiSlot();
}

function fillRoi(value: DetectRoi | null) {
	roi = value;
	roiX = value ? value.x : null;
	roiY = value ? value.y : null;
	roiW = value ? value.w : null;
	roiH = value ? value.h : null;
}

function round4(value: number): number {
	return Math.round(value * 10000) / 10000;
}

function readRoiFields(): DetectRoi | null {
	if (roiX === null || roiY === null || roiW === null || roiH === null) {
		return null;
	}
	const next = {
		x: round4(roiX),
		y: round4(roiY),
		w: round4(roiW),
		h: round4(roiH),
		enabled: roi?.enabled !== false,
	};
	if (next.x < 0 || next.y < 0 || next.w < 0.01 || next.h < 0.01) return null;
	if (next.w > 1 || next.h > 1) return null;
	if (next.x + next.w > 1.0001 || next.y + next.h > 1.0001) return null;
	return next;
}

function asBarcodeRoi(value: unknown): BarcodeRoi | null {
	if (!value || typeof value !== "object") return null;
	const row = value as Record<string, unknown>;
	const x = Number(row.x);
	const y = Number(row.y);
	const width = Number(row.width);
	const height = Number(row.height);
	if (![x, y, width, height].every((part) => Number.isFinite(part)))
		return null;
	if (width <= 0 || height <= 0) return null;
	return { x, y, width, height };
}

function fillBarcodeRoi(value: BarcodeRoi | null) {
	barcodeRoi = value;
	barcodeX = value ? value.x : null;
	barcodeY = value ? value.y : null;
	barcodeW = value ? value.width : null;
	barcodeH = value ? value.height : null;
}

function readBarcodeFields(): BarcodeRoi | null {
	if (
		barcodeX === null ||
		barcodeY === null ||
		barcodeW === null ||
		barcodeH === null
	) {
		return null;
	}
	const next = {
		x: round4(barcodeX),
		y: round4(barcodeY),
		width: round4(barcodeW),
		height: round4(barcodeH),
	};
	if (next.x < 0 || next.y < 0 || next.width < 0.01 || next.height < 0.01)
		return null;
	if (next.width > 1 || next.height > 1) return null;
	if (next.x + next.width > 1.0001 || next.y + next.height > 1.0001)
		return null;
	return next;
}

type BomDocument = {
	source?: unknown;
	mssql?: {
		auth?: unknown;
		server?: unknown;
		database?: unknown;
		user?: unknown;
		password?: unknown;
		driver?: unknown;
	};
	api?: unknown;
	model?: unknown;
};

function stringField(value: unknown): string {
	return typeof value === "string" ? value : "";
}

function applyBomSource(data: BomDocument) {
	source = readSource(data.source);
	auth = data.mssql?.auth === "sql" ? "sql" : "windows";
	mssql = {
		server: stringField(data.mssql?.server),
		database: stringField(data.mssql?.database),
		user: auth === "windows" ? "" : stringField(data.mssql?.user),
	};
	mssqlPassword = auth === "windows" ? "" : stringField(data.mssql?.password);
	readApi(data.api);
}

async function invokeRust<T>(
	command: string,
	args?: Record<string, unknown>,
): Promise<T> {
	const { invoke } = await import("@tauri-apps/api/core");
	return invoke<T>(command, args);
}

async function loadDocument(): Promise<Record<string, unknown>> {
	return await invokeRust<Record<string, unknown>>("load_local_config");
}

async function storeDocument(
	config: Record<string, unknown>,
): Promise<Record<string, unknown>> {
	return await invokeRust<Record<string, unknown>>("save_local_config", {
		config,
	});
}

function objectField(value: unknown): Record<string, unknown> {
	if (!value || typeof value !== "object" || Array.isArray(value)) return {};
	return value as Record<string, unknown>;
}

function cleanErp(
	nameRaw: string,
	urlRaw: string,
): { name: string; url: string } | string {
	const name = nameRaw.trim().replace(/\s+/g, " ");
	const url = urlRaw.trim();
	if (name.length > 40) return "Button name must be 40 characters or fewer";
	if (url.length > 2000) return "URL is too long";
	if (!url) {
		if (name) return "URL is required";
		return { name: "", url: "" };
	}
	let parsed: URL;
	try {
		parsed = new URL(url);
	} catch {
		return "URL must start with http:// or https://";
	}
	if (
		(parsed.protocol !== "http:" && parsed.protocol !== "https:") ||
		!parsed.hostname
	) {
		return "URL must start with http:// or https://";
	}
	return { name: name || "ERP", url };
}

async function loadSource() {
	const id = ++configLoadId;
	notice = "";
	try {
		const data = await invokeRust<BomDocument>("load_local_config");
		if (id !== configLoadId) return;
		applyBomSource(data);
		await loadErp();
		await loadScan();
		await loadTestMode();
		await loadRoi();
		await loadModel();
		await loadPartRules();
		if (section === "parts") await loadPartClasses();
	} catch (err: unknown) {
		if (id !== configLoadId) return;
		notice = invokeError(err, "Could not load vision/config.local.json.");
	}
}

function readModelFiles(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter(
		(item): item is string => typeof item === "string" && item.length > 0,
	);
}

function applyModel(data: {
	model?: unknown;
	active?: unknown;
	files?: unknown;
}) {
	modelFiles = readModelFiles(data.files);
	const active = typeof data.active === "string" ? data.active.trim() : "";
	modelActive = active || "best.onnx";
	const selected = typeof data.model === "string" ? data.model : "";
	if (modelFiles.includes(selected)) modelName = selected;
	else if (modelFiles.includes(modelActive)) modelName = modelActive;
	else modelName = modelFiles[0] ?? "";
}

function matchFile(name: string, files: string[]): string {
	const cleaned = name.trim().toLowerCase();
	if (!cleaned) return "";
	return files.find((item) => item.toLowerCase() === cleaned) ?? "";
}

async function loadModelFromRust() {
	const files = await invokeRust<string[]>("list_vision_models");
	const config = await invokeRust<BomDocument>("load_local_config");
	const stored = stringField(config.model);
	const match = matchFile(stored, files);
	const fallback =
		matchFile("best.onnx", files) ||
		matchFile("best.pt", files) ||
		files.find((item) => item.toLowerCase().endsWith(".onnx")) ||
		"";
	applyModel({
		model: match,
		active: match || fallback || "best.onnx",
		files,
	});
	if (stored.trim() && !match) {
		modelNotice = fallback
			? `${stored.trim()} was not found. Using ${fallback}.`
			: `${stored.trim()} was not found. best.onnx was not found.`;
		return;
	}
	modelNotice = "";
}

async function loadModel() {
	try {
		await loadModelFromRust();
	} catch (err: unknown) {
		modelNotice = invokeError(err, "Could not load the model list");
	}
	await loadConfidence();
	await loadMatchThreshold();
}

function clampConfidence(value: number): number {
	if (!Number.isFinite(value)) return CONFIDENCE_DEFAULT;
	const clamped = Math.min(CONFIDENCE_MAX, Math.max(CONFIDENCE_MIN, value));
	return Math.round(clamped * 100) / 100;
}

function showConfidence(value: number) {
	confidence = clampConfidence(value);
	confidenceText = confidence.toFixed(2);
}

function confidenceFill(): number {
	return (
		((confidence - CONFIDENCE_MIN) / (CONFIDENCE_MAX - CONFIDENCE_MIN)) * 100
	);
}

function cancelConfidenceSave() {
	if (confidenceTimer === null) return;
	clearTimeout(confidenceTimer);
	confidenceTimer = null;
}

function queueConfidenceSave() {
	cancelConfidenceSave();
	confidenceTimer = setTimeout(() => {
		confidenceTimer = null;
		const run = confidenceChain.then(persistConfidence, persistConfidence);
		confidenceChain = run.then(
			() => undefined,
			() => undefined,
		);
	}, 200);
}

async function loadConfidence() {
	const generation = confidenceGeneration;
	try {
		const data = await loadDocument();
		if (generation !== confidenceGeneration) return;
		showConfidence(
			typeof data.confidence === "number"
				? data.confidence
				: CONFIDENCE_DEFAULT,
		);
	} catch (err: unknown) {
		if (generation !== confidenceGeneration) return;
		confidenceNotice = invokeError(
			err,
			"Could not load the confidence threshold",
		);
	}
}

async function persistConfidence() {
	const generation = ++confidenceGeneration;
	const next = confidence;
	confidenceNotice = "";
	try {
		const current = await loadDocument();
		if (generation !== confidenceGeneration || next !== confidence) return;
		current.confidence = next;
		await storeDocument(current);
		if (generation !== confidenceGeneration) return;
		confidenceNotice = `Saved. Confidence is ${next.toFixed(2)}.`;
	} catch (err: unknown) {
		if (generation !== confidenceGeneration) return;
		confidenceNotice = invokeError(
			err,
			"Could not save the confidence threshold",
		);
	}
}

function slideConfidence(raw: string) {
	showConfidence(Number(raw));
	queueConfidenceSave();
}

function stepConfidence(delta: number) {
	showConfidence(confidence + delta);
	queueConfidenceSave();
}

function previewConfidence(raw: string) {
	confidenceText = raw;
	cancelConfidenceSave();
}

function commitConfidence() {
	const parsed = Number(confidenceText);
	if (!Number.isFinite(parsed)) {
		confidenceText = confidence.toFixed(2);
		return;
	}
	showConfidence(parsed);
	queueConfidenceSave();
}

function clampMatch(value: number): number {
	if (!Number.isFinite(value)) return MATCH_DEFAULT;
	const clamped = Math.min(MATCH_MAX, Math.max(MATCH_MIN, value));
	return Math.round(clamped * 100) / 100;
}

function showMatch(value: number) {
	matchThreshold = clampMatch(value);
	matchText = matchThreshold.toFixed(2);
}

function matchFill(): number {
	return ((matchThreshold - MATCH_MIN) / (MATCH_MAX - MATCH_MIN)) * 100;
}

function cancelMatchSave() {
	if (matchTimer === null) return;
	clearTimeout(matchTimer);
	matchTimer = null;
}

function queueMatchSave() {
	cancelMatchSave();
	matchTimer = setTimeout(() => {
		matchTimer = null;
		const run = matchChain.then(persistMatch, persistMatch);
		matchChain = run.then(
			() => undefined,
			() => undefined,
		);
	}, 200);
}

async function loadMatchThreshold() {
	const generation = matchGeneration;
	try {
		const data = await loadDocument();
		if (generation !== matchGeneration) return;
		showMatch(
			typeof data.matchThreshold === "number"
				? data.matchThreshold
				: MATCH_DEFAULT,
		);
	} catch (err: unknown) {
		if (generation !== matchGeneration) return;
		matchNotice = invokeError(err, "Could not load the match threshold");
	}
}

async function persistMatch() {
	const generation = ++matchGeneration;
	const next = matchThreshold;
	matchNotice = "";
	try {
		const current = await loadDocument();
		if (generation !== matchGeneration || next !== matchThreshold) return;
		current.matchThreshold = next;
		await storeDocument(current);
		if (generation !== matchGeneration) return;
		matchNotice = `Saved. Match threshold is ${next.toFixed(2)}.`;
	} catch (err: unknown) {
		if (generation !== matchGeneration) return;
		matchNotice = invokeError(err, "Could not save the match threshold");
	}
}

function slideMatch(raw: string) {
	showMatch(Number(raw));
	queueMatchSave();
}

function stepMatch(delta: number) {
	showMatch(matchThreshold + delta);
	queueMatchSave();
}

function previewMatch(raw: string) {
	matchText = raw;
	cancelMatchSave();
}

function commitMatch() {
	const parsed = Number(matchText);
	if (!Number.isFinite(parsed)) {
		matchText = matchThreshold.toFixed(2);
		return;
	}
	showMatch(parsed);
	queueMatchSave();
}

function modelNoticeClass(text: string): string {
	if (
		text.startsWith("Updated ") ||
		text.startsWith("Saved.") ||
		text === "No file selected."
	) {
		return "text-neutral-200";
	}
	return "text-red-300";
}

function inferenceLabel(id: string): string {
	if (id === "directml") return "DirectML";
	if (id === "cuda") return "CUDA";
	if (id === "openvino") return "OpenVINO";
	return "CPU";
}

$: inferenceBackend = openVinoActive
	? "OpenVINO"
	: inferenceLabel(inferenceActive);

function applyInference(data: {
	providers?: unknown;
	selected?: unknown;
	active?: unknown;
	message?: unknown;
	useOpenVino?: unknown;
	openVinoActive?: unknown;
}) {
	const providers = Array.isArray(data.providers)
		? data.providers.filter(
				(item): item is string =>
					item === "cpu" || item === "directml" || item === "cuda",
			)
		: [];
	inferenceProviders = providers.length > 0 ? providers : ["cpu"];
	const selected = typeof data.selected === "string" ? data.selected : "";
	inferenceEp = inferenceProviders.includes(selected)
		? selected
		: inferenceProviders[0];
	const active = typeof data.active === "string" ? data.active : "";
	inferenceActive = inferenceProviders.includes(active) ? active : inferenceEp;
	useOpenVino = data.useOpenVino === true;
	openVinoActive = data.openVinoActive === true;
	inferenceNotice = typeof data.message === "string" ? data.message : "";
	inferenceReady = true;
}

async function loadInference() {
	const generation = inferenceGeneration;
	try {
		const data = await invokeRust<{
			providers?: unknown;
			selected?: unknown;
			active?: unknown;
			message?: unknown;
			useOpenVino?: unknown;
			openVinoActive?: unknown;
		}>("inference_status");
		if (generation !== inferenceGeneration || savingInference) return;
		applyInference(data);
	} catch (err: unknown) {
		if (generation !== inferenceGeneration || savingInference) return;
		inferenceProviders = ["cpu"];
		inferenceEp = "cpu";
		inferenceActive = "cpu";
		useOpenVino = false;
		openVinoActive = false;
		inferenceNotice = invokeError(err, "Could not list inference providers.");
		inferenceReady = true;
	}
}

function onInferenceChange() {
	const choice = inferenceEp;
	const generation = ++inferenceGeneration;
	inferenceChain = inferenceChain
		.catch(() => undefined)
		.then(() => persistInference(choice, generation));
}

async function persistInference(choice: string, generation: number) {
	savingInference = true;
	inferenceNotice = "Starting the session...";
	try {
		const data = await invokeRust<{
			providers?: unknown;
			selected?: unknown;
			active?: unknown;
			message?: unknown;
			useOpenVino?: unknown;
			openVinoActive?: unknown;
		}>("set_inference_ep", { provider: choice });
		if (generation !== inferenceGeneration) return;
		applyInference(data);
	} catch (err: unknown) {
		if (generation !== inferenceGeneration) return;
		const message = invokeError(err, "Could not save the inference provider.");
		try {
			const data = await invokeRust<{
				providers?: unknown;
				selected?: unknown;
				active?: unknown;
				message?: unknown;
				useOpenVino?: unknown;
				openVinoActive?: unknown;
			}>("inference_status");
			if (generation !== inferenceGeneration) return;
			applyInference(data);
		} catch {
			inferenceProviders = inferenceProviders.includes(choice)
				? inferenceProviders
				: ["cpu"];
		}
		inferenceNotice = message;
	} finally {
		if (generation === inferenceGeneration) savingInference = false;
	}
}

function onOpenVinoChange(event: Event) {
	const input = event.currentTarget;
	if (!(input instanceof HTMLInputElement)) return;
	const enabled = input.checked;
	useOpenVino = enabled;
	const generation = ++inferenceGeneration;
	inferenceChain = inferenceChain
		.catch(() => undefined)
		.then(() => persistOpenVino(enabled, generation));
}

async function persistOpenVino(enabled: boolean, generation: number) {
	savingInference = true;
	inferenceNotice = "Starting the session...";
	try {
		const data = await invokeRust<{
			providers?: unknown;
			selected?: unknown;
			active?: unknown;
			message?: unknown;
			useOpenVino?: unknown;
			openVinoActive?: unknown;
		}>("set_use_openvino", { enabled });
		if (generation !== inferenceGeneration) return;
		applyInference(data);
	} catch (err: unknown) {
		if (generation !== inferenceGeneration) return;
		const message = invokeError(err, "Could not save OpenVINO.");
		try {
			const data = await invokeRust<{
				providers?: unknown;
				selected?: unknown;
				active?: unknown;
				message?: unknown;
				useOpenVino?: unknown;
				openVinoActive?: unknown;
			}>("inference_status");
			if (generation !== inferenceGeneration) return;
			applyInference(data);
		} catch {
			useOpenVino = !enabled;
			openVinoActive = false;
		}
		inferenceNotice = message;
	} finally {
		if (generation === inferenceGeneration) savingInference = false;
	}
}

function formatMs(value: number): string {
	if (!Number.isFinite(value)) return "0.0";
	return value.toFixed(1);
}

function formatBenchmark(data: {
	provider?: unknown;
	average_ms?: unknown;
	min_ms?: unknown;
	max_ms?: unknown;
	width?: unknown;
	height?: unknown;
	image?: unknown;
}): string {
	const provider = typeof data.provider === "string" ? data.provider : "cpu";
	const average = typeof data.average_ms === "number" ? data.average_ms : 0;
	const min = typeof data.min_ms === "number" ? data.min_ms : 0;
	const max = typeof data.max_ms === "number" ? data.max_ms : 0;
	const width = typeof data.width === "number" ? data.width : 0;
	const height = typeof data.height === "number" ? data.height : 0;
	const image = typeof data.image === "string" ? data.image : "";
	const resolution = image
		? `${width}×${height} ${image}`
		: `${width}×${height}`;
	return [
		`Active EP: ${inferenceLabel(provider)}`,
		`Average: ${formatMs(average)} ms`,
		`Min / max: ${formatMs(min)} / ${formatMs(max)} ms`,
		`Image: ${resolution}`,
	].join("\n");
}

async function runBenchmark() {
	const generation = inferenceGeneration;
	benchmarking = true;
	benchmarkText = "Running 5 detects...";
	try {
		const data = await invokeRust<{
			provider?: unknown;
			average_ms?: unknown;
			min_ms?: unknown;
			max_ms?: unknown;
			width?: unknown;
			height?: unknown;
			image?: unknown;
		}>("benchmark_detect");
		if (generation !== inferenceGeneration) {
			benchmarkText = "";
			return;
		}
		benchmarkText = formatBenchmark(data);
		await loadInference();
	} catch (err: unknown) {
		if (generation !== inferenceGeneration) {
			benchmarkText = "";
			return;
		}
		benchmarkText = invokeError(err, "Benchmark failed.");
	} finally {
		benchmarking = false;
	}
}

async function saveModel() {
	savingModel = true;
	modelNotice = "";
	try {
		const data = await invokeRust<{
			model?: unknown;
			active?: unknown;
			files?: unknown;
			message?: unknown;
		}>("set_active_model", { filename: modelName });
		applyModel(data);
		void loadPartClasses();
		modelNotice =
			typeof data.message === "string" && data.message ? data.message : "Saved";
	} catch (err: unknown) {
		modelNotice = invokeError(err, "Could not save the model");
	} finally {
		savingModel = false;
	}
}

async function updateModel() {
	updatingModel = true;
	modelNotice = "";
	try {
		const data = await invokeRust<{
			model?: unknown;
			active?: unknown;
			files?: unknown;
			message?: unknown;
		}>("update_model", { filename: modelName });
		const message =
			typeof data.message === "string" && data.message
				? data.message
				: "Updated";
		if (message !== "No file selected.") {
			applyModel(data);
			void loadPartClasses();
		}
		modelNotice = message;
	} catch (err: unknown) {
		modelNotice = invokeError(err, "Could not update the model");
	} finally {
		updatingModel = false;
	}
}

function referenceNoticeClass(text: string): string {
	if (text.startsWith("References loaded ") || text === "No file selected.") {
		return "text-neutral-200";
	}
	return "text-red-300";
}

async function updateReferences() {
	updatingReferences = true;
	referenceNotice = "";
	try {
		const data = await invokeRust<{
			classes?: unknown;
			message?: unknown;
			path?: unknown;
		}>("update_references");
		if (!data || typeof data !== "object") {
			referenceNotice = "Could not update the references";
			return;
		}
		if (typeof data.path === "string" && data.path) referencePath = data.path;
		referenceNotice =
			typeof data.message === "string" && data.message
				? data.message
				: "References loaded.";
	} catch (err: unknown) {
		referenceNotice = invokeError(err, "Could not update the references");
	} finally {
		updatingReferences = false;
	}
}

function readReferenceRow(value: unknown): ReferenceRow | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	const ic = typeof record.ic === "string" ? record.ic.trim() : "";
	if (!ic) return null;
	const count =
		typeof record.count === "number" && Number.isFinite(record.count)
			? record.count
			: 0;
	const threshold =
		typeof record.threshold === "number" && Number.isFinite(record.threshold)
			? record.threshold
			: null;
	return { ic, threshold, count };
}

function readReferenceRows(value: unknown): ReferenceRow[] {
	if (!Array.isArray(value)) return [];
	const rows: ReferenceRow[] = [];
	for (const item of value) {
		const row = readReferenceRow(item);
		if (row) rows.push(row);
	}
	return rows;
}

function readReferenceList(
	value: unknown,
): { path: string; rows: ReferenceRow[]; rawCount: number } | null {
	if (!value || typeof value !== "object" || Array.isArray(value)) return null;
	const record = value as Record<string, unknown>;
	const path = typeof record.path === "string" ? record.path : "";
	const rawCount = Array.isArray(record.rows) ? record.rows.length : -1;
	return { path, rows: readReferenceRows(record.rows), rawCount };
}

function effectiveReferenceThreshold(row: ReferenceRow): number {
	return row.threshold ?? matchThreshold;
}

function sortReferenceRows(
	rows: ReferenceRow[],
	key: "ic" | "threshold",
	dir: "asc" | "desc",
	fallback: number,
): ReferenceRow[] {
	const copy = [...rows];
	copy.sort((left, right) => {
		let order = 0;
		if (key === "ic") {
			order = left.ic.localeCompare(right.ic);
		} else {
			order = (left.threshold ?? fallback) - (right.threshold ?? fallback);
			if (order === 0) order = left.ic.localeCompare(right.ic);
		}
		return dir === "asc" ? order : -order;
	});
	return copy;
}

function showReferenceRows(rows: ReferenceRow[]) {
	referenceRows = rows;
	const drafts: Record<string, string> = {};
	for (const row of rows) {
		drafts[row.ic] = effectiveReferenceThreshold(row).toFixed(2);
	}
	referenceDrafts = drafts;
}

function referencesNoticeClass(text: string): string {
	if (text.startsWith("Saved.") || text.startsWith("Removed "))
		return "text-neutral-200";
	return "text-red-300";
}

async function openReferences() {
	referencesOpen = true;
	referencesNotice = "";
	referencesLoading = true;
	const generation = ++referencesGeneration;
	try {
		const data = await invokeRust<unknown>("list_references");
		if (generation !== referencesGeneration || !referencesOpen) return;
		const list = readReferenceList(data);
		if (!list) {
			referenceRows = [];
			referenceDrafts = {};
			referencesNotice = "Could not load the references";
			return;
		}
		if (list.path) referencePath = list.path;
		if (list.rawCount < 0 || (list.rawCount > 0 && list.rows.length === 0)) {
			referenceRows = [];
			referenceDrafts = {};
			referencesNotice = "Could not load the references";
			return;
		}
		showReferenceRows(list.rows);
	} catch (err: unknown) {
		if (generation !== referencesGeneration || !referencesOpen) return;
		referenceRows = [];
		referenceDrafts = {};
		referencesNotice = invokeError(err, "Could not load the references");
	} finally {
		if (generation === referencesGeneration) referencesLoading = false;
	}
	if (referencesOpen && generation === referencesGeneration) {
		await tick();
		referencesClose?.focus();
	}
}

function closeReferences() {
	referencesOpen = false;
	referencesGeneration += 1;
	referencesLoading = false;
}

function onReferencesKey(event: KeyboardEvent) {
	if (!referencesOpen || event.key !== "Escape") return;
	event.preventDefault();
	closeReferences();
}

function sortReferencesBy(key: "ic" | "threshold") {
	if (referenceSort === key) {
		referenceSortDir = referenceSortDir === "asc" ? "desc" : "asc";
		return;
	}
	referenceSort = key;
	referenceSortDir = "asc";
}

function setReferenceDraft(ic: string, value: string) {
	referenceDrafts = { ...referenceDrafts, [ic]: value };
}

function showReferenceDraft(
	ic: string,
	shown: string,
	field?: HTMLInputElement,
) {
	setReferenceDraft(ic, shown);
	if (field) field.value = shown;
}

async function commitReferenceThreshold(ic: string, field?: HTMLInputElement) {
	const row = referenceRows.find((item) => item.ic === ic);
	if (!row || savingReferenceIc || deletingReferenceIc) return;
	const parsed = Number(referenceDrafts[ic] ?? "");
	if (!Number.isFinite(parsed)) {
		showReferenceDraft(ic, effectiveReferenceThreshold(row).toFixed(2), field);
		referencesNotice = "Enter a threshold from 0.50 to 0.95.";
		return;
	}
	const next = Math.min(
		MATCH_MAX,
		Math.max(MATCH_MIN, Math.round(parsed * 100) / 100),
	);
	const current = effectiveReferenceThreshold(row);
	if (Math.abs(next - current) < 0.001) {
		showReferenceDraft(ic, current.toFixed(2), field);
		return;
	}
	savingReferenceIc = ic;
	referencesNotice = "";
	try {
		const data = await invokeRust<unknown>("set_reference_threshold", {
			ic,
			threshold: next,
		});
		const saved = readReferenceRow(data);
		if (!saved) {
			showReferenceDraft(ic, current.toFixed(2), field);
			referencesNotice = "Could not save the threshold.";
			return;
		}
		referenceRows = referenceRows.map((item) =>
			item.ic === ic ? saved : item,
		);
		showReferenceDraft(saved.ic, (saved.threshold ?? next).toFixed(2), field);
		referencesNotice = `Saved. ${saved.ic} threshold is ${(saved.threshold ?? next).toFixed(2)}.`;
	} catch (err: unknown) {
		showReferenceDraft(ic, current.toFixed(2), field);
		referencesNotice = invokeError(err, "Could not save the threshold");
	} finally {
		savingReferenceIc = "";
	}
}

async function deleteReferenceRow(ic: string) {
	if (savingReferenceIc || deletingReferenceIc) return;
	deletingReferenceIc = ic;
	referencesNotice = "";
	try {
		await invokeRust("delete_reference", { ic });
		referenceRows = referenceRows.filter((item) => item.ic !== ic);
		const drafts = { ...referenceDrafts };
		delete drafts[ic];
		referenceDrafts = drafts;
		referencesNotice = `Removed ${ic}.`;
	} catch (err: unknown) {
		referencesNotice = invokeError(err, "Could not delete the reference");
	} finally {
		deletingReferenceIc = "";
	}
}

$: sortedReferenceRows = sortReferenceRows(
	referenceRows,
	referenceSort,
	referenceSortDir,
	matchThreshold,
);

async function loadScan() {
	try {
		const data = await loadDocument();
		const seconds = Number(data.scanSeconds);
		if (Number.isInteger(seconds)) scanSeconds = seconds;
	} catch (err: unknown) {
		scanNotice = invokeError(err, "Could not load the scan interval");
	}
}

async function loadTestMode() {
	try {
		const data = await loadDocument();
		testMode = data.testMode === true;
		skipResultSave = data.skipResultSave !== false;
	} catch (err: unknown) {
		testNotice = invokeError(err, "Could not load Test mode");
	}
}

async function loadRoi() {
	try {
		const data = await loadDocument();
		roiSlots = parseDetectRois(data.detectRois, data.detectRoi);
		showRoiSlot();
		fillBarcodeRoi(asBarcodeRoi(data.barcodeRoi));
	} catch (err: unknown) {
		roiNotice = invokeError(err, "Could not load the ROI");
	}
}

async function saveTestMode() {
	savingTest = true;
	testNotice = "";
	const enabled = testMode;
	const skipSave = skipResultSave;
	try {
		const current = await loadDocument();
		current.testMode = enabled;
		current.skipResultSave = skipSave;
		const saved = await storeDocument(current);
		testMode = saved.testMode === true;
		skipResultSave = saved.skipResultSave !== false;
		testNotice = "Saved";
	} catch (err: unknown) {
		testMode = !enabled;
		skipResultSave = !skipSave;
		testNotice = errorMessage(err, "Could not save Test mode");
	} finally {
		savingTest = false;
	}
}

async function loadErp() {
	try {
		const data = await loadDocument();
		const erp = objectField(data.erp);
		erpName = typeof erp.name === "string" ? erp.name : "";
		erpUrl = typeof erp.url === "string" ? erp.url : "";
	} catch (err: unknown) {
		erpNotice = invokeError(err, "Could not load the ERP button");
	}
}

function unlock() {
	if (password !== DEV_PASSWORD) {
		gateMessage = "Wrong password";
		return;
	}
	password = "";
	gateMessage = "";
	unlocked = true;
	sessionStorage.setItem(UNLOCK_KEY, "1");
	postPreviewAdmin();
	loadSource();
	void loadInference();
	if (section === "camera") void loadCameraSettings();
}

function onAuthChange() {
	if (auth === "windows") {
		mssql.user = "";
		mssqlPassword = "";
	}
}

function lock() {
	unlocked = false;
	password = "";
	gateMessage = "";
	mssqlPassword = "";
	sessionStorage.removeItem(UNLOCK_KEY);
	postPreviewAdmin();
}

async function saveScan() {
	savingScan = true;
	scanNotice = "";
	const seconds = Number(scanSeconds);
	if (!Number.isInteger(seconds) || seconds < 1 || seconds > 60) {
		scanNotice = "Enter a whole number of seconds from 1 to 60.";
		savingScan = false;
		return;
	}
	try {
		const current = await loadDocument();
		current.scanSeconds = seconds;
		const saved = await storeDocument(current);
		const stored = Number(saved.scanSeconds);
		if (Number.isInteger(stored)) scanSeconds = stored;
		scanNotice = "Saved";
	} catch (err: unknown) {
		scanNotice = invokeError(err, "Could not save the scan interval");
	} finally {
		savingScan = false;
	}
}

async function saveErp() {
	savingErp = true;
	erpNotice = "";
	const cleaned = cleanErp(erpName, erpUrl);
	if (typeof cleaned === "string") {
		erpNotice = cleaned;
		savingErp = false;
		return;
	}
	try {
		const current = await loadDocument();
		current.erp = cleaned;
		await storeDocument(current);
		erpName = cleaned.name;
		erpUrl = cleaned.url;
		erpNotice = "Saved";
	} catch (err: unknown) {
		erpNotice = invokeError(err, "Could not save the ERP button");
	} finally {
		savingErp = false;
	}
}

let roiWrite: Promise<void> = Promise.resolve();

function enqueueRoi(task: () => Promise<void>): Promise<void> {
	const job = roiWrite.then(task, task);
	roiWrite = job.then(
		() => undefined,
		() => undefined,
	);
	return job;
}

async function postRoi(next: DetectRoi | null) {
	const index = Number(roiSlot);
	if (!Number.isInteger(index) || index < 1 || index > 4) {
		roiNotice = "Choose a slot.";
		return;
	}
	savingRoi = true;
	roiNotice = "";
	try {
		await enqueueRoi(async () => {
			const current = await loadDocument();
			const slots = parseDetectRois(current.detectRois, current.detectRoi);
			const stored =
				next === null
					? compactClearedSlot(slots, index)
					: slots.map((item, itemIndex) =>
							itemIndex === index - 1
								? { ...next, enabled: item?.enabled ?? true }
								: item,
						);
			current.detectRois = stored;
			current.detectRoi = stored[0];
			await storeDocument(current);
			roiSlots = stored;
			if (next === null) showRoiSlot();
			else fillRoi(asRoi(stored[index - 1]));
			roiNotice = "Saved";
		});
	} catch (err: unknown) {
		roiNotice = invokeError(err, "Could not save the ROI");
	} finally {
		savingRoi = false;
	}
}

function toggleSavedSlot(slot: number) {
	const current = roiSlots[slot - 1];
	if (!current) return;
	const enabled = !current.enabled;
	roiSlots = withSlotEnabled(roiSlots, slot, enabled);
	if (roi && Number(roiSlot) === slot) roi = { ...roi, enabled };
	void enqueueRoi(async () => {
		const document = await loadDocument();
		const slots = withSlotEnabled(
			parseDetectRois(document.detectRois, document.detectRoi),
			slot,
			enabled,
		);
		document.detectRois = slots;
		document.detectRoi = slots[0];
		await storeDocument(document);
		roiSlots = slots;
		if (Number(roiSlot) === slot) {
			const saved = slots[slot - 1];
			if (saved && roi) roi = { ...roi, enabled: saved.enabled };
		}
		roiNotice = "Saved";
	}).catch((err: unknown) => {
		roiNotice = invokeError(err, "Could not save the ROI");
		void loadRoi();
	});
}

function slotSquareClass(box: DetectRoi | null): string {
	if (!box) return "border-neutral-700 bg-neutral-800 text-neutral-500";
	if (box.enabled) return "border-green-400 bg-green-500 text-neutral-950";
	return "border-red-500 bg-red-600 text-white";
}

function roiFieldsEmpty(): boolean {
	return roiX === null && roiY === null && roiW === null && roiH === null;
}

function saveRoi() {
	if (!roiSlot) {
		roiNotice = "Choose a slot.";
		return;
	}
	if (roiFieldsEmpty()) {
		void postRoi(null);
		return;
	}
	const next = readRoiFields();
	if (!next) {
		roiNotice =
			"Enter left, top, width, and height as fractions from 0 to 1. The region must be at least 0.01 by 0.01 and stay inside the image.";
		return;
	}
	void postRoi(next);
}

function clearRoi() {
	if (
		roi === null &&
		roiX === null &&
		roiY === null &&
		roiW === null &&
		roiH === null
	) {
		return;
	}
	if (roi === null) {
		fillRoi(null);
		roiNotice = "";
		return;
	}
	void postRoi(null);
}

async function postBarcodeRoi(next: BarcodeRoi | null) {
	savingBarcodeRoi = true;
	barcodeNotice = "";
	try {
		const current = await loadDocument();
		current.barcodeRoi = next;
		await storeDocument(current);
		fillBarcodeRoi(asBarcodeRoi(next));
		barcodeNotice = "Saved";
	} catch (err: unknown) {
		barcodeNotice = invokeError(err, "Could not save the barcode ROI");
	} finally {
		savingBarcodeRoi = false;
	}
}

function saveBarcodeRoi() {
	const next = readBarcodeFields();
	if (!next) {
		barcodeNotice =
			"Enter left, top, width, and height as fractions from 0 to 1. The region must be at least 0.01 by 0.01 and stay inside the image.";
		return;
	}
	void postBarcodeRoi(next);
}

function clearBarcodeRoi() {
	if (
		barcodeRoi === null &&
		barcodeX === null &&
		barcodeY === null &&
		barcodeW === null &&
		barcodeH === null
	) {
		return;
	}
	if (barcodeRoi === null) {
		fillBarcodeRoi(null);
		barcodeNotice = "";
		return;
	}
	void postBarcodeRoi(null);
}

async function saveSource() {
	if (auth === "windows") {
		mssql.user = "";
		mssqlPassword = "";
	} else if (!mssql.user.trim() || !mssqlPassword.trim()) {
		notice =
			"Username and password are required for SQL Server Authentication.";
		return;
	}
	const resultsProblem = resultsPathError(resultsPath);
	if (resultsProblem) {
		notice = resultsProblem;
		return;
	}
	if (source === "api") {
		const problem = apiBaseUrlError(apiBaseUrl);
		if (problem) {
			notice = problem;
			return;
		}
	}
	const endpointProblem = descriptionPathError(apiDescriptionPath);
	if (endpointProblem) {
		notice = endpointProblem;
		return;
	}
	saving = true;
	notice = "";
	try {
		const current =
			await invokeRust<Record<string, unknown>>("load_local_config");
		const mssqlCurrent =
			current.mssql &&
			typeof current.mssql === "object" &&
			!Array.isArray(current.mssql)
				? (current.mssql as Record<string, unknown>)
				: {};
		const driver = stringField(mssqlCurrent.driver).trim();
		const apiCurrent =
			current.api &&
			typeof current.api === "object" &&
			!Array.isArray(current.api)
				? (current.api as Record<string, unknown>)
				: {};
		const saved = await invokeRust<BomDocument>("save_local_config", {
			config: {
				...current,
				source,
				mssql: {
					...mssqlCurrent,
					driver: driver || DEFAULT_SQL_DRIVER,
					server: mssql.server.trim(),
					database: mssql.database.trim(),
					auth,
					user: auth === "windows" ? "" : mssql.user.trim(),
					password: auth === "windows" ? "" : mssqlPassword,
				},
				api: {
					...apiCurrent,
					baseUrl: apiBaseUrl.trim(),
					path: apiPath.trim(),
					key: apiKey.trim(),
					descriptionPath: apiDescriptionPath.trim(),
					resultsPath: resultsPath.trim(),
				},
			},
		});
		applyBomSource(saved);
		if (source === "mssql") {
			notice = "Saved. Inspect queries MSSQL using vision/bom_queries.sql.";
		} else if (source === "api") {
			notice = "Saved. Look up posts the job card number to the API.";
		} else {
			notice = "Saved. Inspect uses vision/sample-bom.json.";
		}
	} catch (err: unknown) {
		notice = invokeError(err, "Could not save vision/config.local.json.");
	} finally {
		saving = false;
	}
}

function blankPartRule(): PartRuleDraft {
	partRuleSeq += 1;
	return {
		id: partRuleSeq,
		parent: "",
		child: "",
		condition: "contains",
		result: "",
	};
}

function showPartRules(value: unknown) {
	partRules = parsePartRules(value).map((rule) => {
		partRuleSeq += 1;
		return { id: partRuleSeq, ...rule };
	});
}

function partRuleProblem(drafts: readonly PartRuleDraft[]): string {
	const filled = drafts.filter(
		(rule) => rule.parent.trim() || rule.child.trim() || rule.result.trim(),
	);
	if (filled.length > PART_RULE_LIMIT) {
		return `Save at most ${PART_RULE_LIMIT} rules.`;
	}
	for (const [index, rule] of drafts.entries()) {
		const parent = rule.parent.trim();
		const child = rule.child.trim();
		const result = rule.result.trim();
		if (!parent && !child && !result) continue;
		const noChild = rule.condition === "no-child";
		if (!parent || !result || (!noChild && !child)) {
			return noChild
				? `Rule ${index + 1} needs a parent class and a result IC.`
				: `Rule ${index + 1} needs a parent class, a child class, and a result IC.`;
		}
		if (
			parent.length > PART_LABEL_LIMIT ||
			child.length > PART_LABEL_LIMIT ||
			result.length > PART_LABEL_LIMIT
		) {
			return `Rule ${index + 1} must be ${PART_LABEL_LIMIT} characters or fewer.`;
		}
	}
	return "";
}

function partRulesFromDrafts(drafts: readonly PartRuleDraft[]): PartRule[] {
	const rules: PartRule[] = [];
	for (const rule of drafts) {
		const parent = rule.parent.trim();
		const child = rule.child.trim();
		const result = rule.result.trim();
		if (!parent && !child && !result) continue;
		const condition: PartCondition =
			rule.condition === "no-child" ? "no-child" : "contains";
		if (!parent || !result || (condition === "contains" && !child)) continue;
		rules.push({ parent, child, condition, result });
	}
	return rules;
}

async function loadPartClasses() {
	const id = ++partClassLoadId;
	try {
		const data = await invokeRust<{
			model?: unknown;
			classes?: unknown;
			message?: unknown;
		}>("model_class_names");
		if (id !== partClassLoadId) return;
		partClasses = Array.isArray(data.classes)
			? data.classes.filter(
					(item): item is string => typeof item === "string" && item.length > 0,
				)
			: [];
		const message = typeof data.message === "string" ? data.message.trim() : "";
		partClassMessage = partClasses.length
			? ""
			: message || "Load a model to choose classes.";
		partClassesReady = true;
	} catch {
		if (id !== partClassLoadId) return;
		partClasses = [];
		partClassMessage = "Load a model to choose classes.";
		partClassesReady = true;
	}
}

async function loadPartRules() {
	const epoch = ++partLoadId;
	try {
		const data = await loadDocument();
		if (epoch !== partLoadId || savingParts) return;
		showPartRules(data.partRules);
	} catch (err: unknown) {
		if (epoch !== partLoadId) return;
		partNotice = invokeError(err, "Could not load part rules");
	}
}

function addPartRule() {
	if (partRules.length >= PART_RULE_LIMIT) {
		partNotice = `Save at most ${PART_RULE_LIMIT} rules.`;
		return;
	}
	partNotice = "";
	partRules = [...partRules, blankPartRule()];
}

function removePartRule(id: number) {
	partNotice = "";
	partRules = partRules.filter((rule) => rule.id !== id);
}

function movePartRule(id: number, delta: number) {
	const index = partRules.findIndex((rule) => rule.id === id);
	const target = index + delta;
	if (index < 0 || target < 0 || target >= partRules.length) return;
	const next = partRules.slice();
	const [row] = next.splice(index, 1);
	next.splice(target, 0, row);
	partRules = next;
}

async function savePartRules() {
	const problem = partRuleProblem(partRules);
	if (problem) {
		partNotice = problem;
		return;
	}
	const rules = partRulesFromDrafts(partRules);
	savingParts = true;
	partLoadId += 1;
	partNotice = "";
	try {
		const current = await loadDocument();
		current.partRules = rules;
		const saved = await storeDocument(current);
		showPartRules(saved.partRules);
		partNotice = rules.length
			? `Saved. ${rules.length} part ${rules.length === 1 ? "rule" : "rules"}.`
			: "Saved. No part rules, so classes stay as the model named them.";
	} catch (err: unknown) {
		partNotice = invokeError(err, "Could not save part rules");
	} finally {
		partLoadId += 1;
		savingParts = false;
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

function levelValue(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value)) return null;
	if (value < 0 || value > 100) return null;
	return value;
}

function clampLevel(value: number): number {
	const next = Math.round(Number(value));
	if (!Number.isInteger(next)) return 50;
	return Math.min(100, Math.max(0, next));
}

function nudgeLevel(value: number, delta: number): number {
	return clampLevel(value + delta);
}

function rememberLevels() {
	focusText = String(cameraFocus);
	brightnessText = String(cameraBrightness);
	contrastText = String(cameraContrast);
}

function wholeLevel(raw: string): number | null {
	const text = raw.trim();
	if (!/^\d{1,3}$/.test(text)) return null;
	return Number(text);
}

function cancelCameraApply() {
	if (cameraApplyTimer !== null) {
		clearTimeout(cameraApplyTimer);
		cameraApplyTimer = null;
	}
	cameraPushId += 1;
}

function scheduleCameraApply() {
	cameraPushId += 1;
	const id = cameraPushId;
	if (cameraApplyTimer !== null) clearTimeout(cameraApplyTimer);
	cameraApplyTimer = setTimeout(() => {
		cameraApplyTimer = null;
		void pushCameraSettings(id);
	}, 200);
}

function previewDraft(
	raw: string,
	setText: (value: string) => void,
	assign: (value: number) => void,
) {
	setText(raw);
	const parsed = wholeLevel(raw);
	if (parsed === null || parsed > 100) {
		cancelCameraApply();
		return;
	}
	assign(parsed);
	scheduleCameraApply();
}

function commitDraft(
	raw: string,
	current: number,
	assign: (value: number) => void,
	setText: (value: string) => void,
) {
	const parsed = wholeLevel(raw);
	if (parsed === null) {
		setText(String(current));
		return;
	}
	const next = clampLevel(parsed);
	assign(next);
	setText(String(next));
	if (next !== current) scheduleCameraApply();
}

function stepLevel(
	current: number,
	delta: number,
	assign: (value: number) => void,
	setText: (value: string) => void,
) {
	const next = nudgeLevel(current, delta);
	assign(next);
	setText(String(next));
	if (next !== current) scheduleCameraApply();
}

function slideLevel(
	raw: string,
	current: number,
	assign: (value: number) => void,
	setText: (value: string) => void,
) {
	const next = clampLevel(Number(raw));
	assign(next);
	setText(String(next));
	if (next !== current) scheduleCameraApply();
}

function cameraSettingsBody() {
	cameraFocus = clampLevel(cameraFocus);
	cameraBrightness = clampLevel(cameraBrightness);
	cameraContrast = clampLevel(cameraContrast);
	return {
		index: Number(cameraIndex),
		resolution: cameraResolution,
		displayFps: readDisplayFps(cameraDisplayFps),
		focus: focusAuto ? null : cameraFocus,
		brightness: cameraBrightness,
		contrast: cameraContrast,
	};
}

async function postCameraSettings(
	applyOnly = false,
	preset?: Omit<CameraSettings, "scale">,
) {
	const body = preset ?? cameraSettingsBody();
	// This form stores the fields. The preview Focus slider is what sets the
	// camera. Brightness and contrast are not applied, so a live change here
	// does not write config.
	if (applyOnly) return { ...body, status: "pass", message: "" };
	const index = Number(body.index);
	if (!Number.isInteger(index) || index < 0 || index > 32) {
		throw new Error("Camera index must be from 0 to 32.");
	}
	if (body.resolution !== "" && !isModeId(body.resolution)) {
		throw new Error("Choose a resolution from the list.");
	}
	if (
		cameraModes.length > 0 &&
		body.resolution !== "" &&
		!cameraModes.some((mode) => mode.id === body.resolution)
	) {
		throw new Error("Choose a resolution from the list.");
	}
	const version = cameraFileVersion;
	const current = await loadDocument();
	const existing = settingsForCamera(objectField(current.camera), index);
	const settings = {
		index,
		resolution: body.resolution,
		displayFps: body.displayFps,
		focus: body.focus,
		brightness: body.brightness,
		contrast: body.contrast,
		scale: existing.scale,
	};
	// Save belongs to the camera on the form when Save was clicked. A later
	// camera change must not move that record onto the newly selected camera.
	if (Number(cameraIndex) === index) assignActiveCamera(current, settings);
	else {
		rememberActiveCamera(current);
		retainCameraSettings(current, settings);
	}
	await storeDocument(current);
	if (cameraFileVersion === version) cameraFile = current;
	return { ...settings, status: "pass", message: "Saved" };
}

function queueCameraPost(work: () => Promise<void>): Promise<void> {
	const run = cameraPostChain.then(work, work);
	cameraPostChain = run.then(
		() => undefined,
		() => undefined,
	);
	return run;
}

function pushCameraSettings(id: number) {
	void queueCameraPost(async () => {
		if (id !== cameraPushId) return;
		try {
			await postCameraSettings(true);
		} catch (err: unknown) {
			if (id !== cameraPushId) return;
			cameraNotice = errorMessage(err, "Could not apply the camera settings");
		}
	});
}

function rememberSaved() {
	savedCamera = {
		index: Number(cameraIndex),
		resolution: cameraResolution,
		displayFps: readDisplayFps(cameraDisplayFps),
		focus: focusAuto ? null : cameraFocus,
		brightness: cameraBrightness,
		contrast: cameraContrast,
	};
}

function setCameraFocus(value: number) {
	focusAuto = false;
	cameraFocus = value;
}

function showCamera(next: {
	index: number;
	resolution: string;
	displayFps: number;
	focus: number | null;
	brightness: number;
	contrast: number;
}) {
	cameraIndex = next.index;
	cameraResolution = isModeId(next.resolution) ? next.resolution : "";
	cameraDisplayFps = readDisplayFps(next.displayFps);
	focusAuto = next.focus === null;
	cameraFocus = next.focus === null ? 50 : clampLevel(next.focus);
	cameraBrightness = clampLevel(next.brightness);
	cameraContrast = clampLevel(next.contrast);
	rememberLevels();
	if (!cameraChoices.includes(cameraIndex)) {
		cameraChoices = [...cameraChoices, cameraIndex].sort(
			(left, right) => left - right,
		);
	}
}

async function applyShownCamera(notice: string) {
	cancelCameraApply();
	postPreviewIndex(Number(cameraIndex));
	const id = ++cameraPushId;
	await queueCameraPost(async () => {
		if (id !== cameraPushId) return;
		try {
			await postCameraSettings(true);
			if (id !== cameraPushId) return;
			cameraNotice = notice;
		} catch (err: unknown) {
			if (id !== cameraPushId) return;
			cameraNotice = errorMessage(err, "Could not apply the camera settings");
		}
	});
}

function resetCamera() {
	showCamera({
		index: Number(cameraIndex),
		resolution: cameraModeDefault,
		displayFps: 15,
		focus: 50,
		brightness: 50,
		contrast: 50,
	});
	void persistResolution(cameraResolution);
	void persistDisplayFps(cameraDisplayFps);
	void applyShownCamera("Reset to defaults.");
}

function cancelCameraEdits() {
	showCamera(savedCamera);
	void applyShownCamera("Restored the last saved settings.");
}

function applyCamera(data: {
	index?: unknown;
	resolution?: unknown;
	displayFps?: unknown;
	focus?: unknown;
	brightness?: unknown;
	contrast?: unknown;
}) {
	const index = Number(data.index);
	if (Number.isInteger(index) && index >= 0 && index <= 32) cameraIndex = index;
	const resolution = typeof data.resolution === "string" ? data.resolution : "";
	cameraResolution = isModeId(resolution) ? resolution : "";
	cameraDisplayFps = readDisplayFps(data.displayFps);
	const parsedFocus = levelValue(data.focus);
	focusAuto = parsedFocus === null;
	cameraFocus = parsedFocus ?? 50;
	cameraBrightness = levelValue(data.brightness) ?? 50;
	cameraContrast = levelValue(data.contrast) ?? 50;
	rememberLevels();
	if (!cameraChoices.includes(cameraIndex)) {
		cameraChoices = [...cameraChoices, cameraIndex].sort(
			(left, right) => left - right,
		);
	}
}

async function loadCameraDevices() {
	cameraListNotice = "";
	try {
		const listed = await invokeRust<unknown>("list_cameras");
		const selected = Number(cameraIndex);
		const found = cameraList(listed);
		if (!found.includes(selected)) found.push(selected);
		found.sort((left, right) => left - right);
		cameraIndex = selected;
		cameraChoices = found;
	} catch (err: unknown) {
		cameraChoices = [cameraIndex];
		cameraListNotice = invokeError(err, "Could not list cameras.");
	}
}

async function loadCameraSettings() {
	cameraNotice = "";
	const version = cameraFileVersion;
	try {
		const config = await loadDocument();
		if (cameraFileVersion === version) {
			cameraFile = config;
			const camera = objectField(config.camera);
			const index = Number(camera.index);
			const selected =
				Number.isInteger(index) && index >= 0 && index <= 32 ? index : 0;
			showCamera(settingsForCamera(camera, selected));
			rememberSaved();
		}
	} catch (err: unknown) {
		if (cameraFileVersion === version) {
			cameraNotice = invokeError(err, "Could not load the camera settings");
		}
	}
	await loadCameraDevices();
	if (cameraFileVersion !== version) return;
	await loadCameraModes();
	if (cameraFileVersion !== version) return;
	await loadTrainingFolder();
}

async function loadTrainingFolder() {
	trainingNotice = "";
	try {
		const config = await loadDocument();
		const saved =
			typeof config.trainingDir === "string" ? config.trainingDir.trim() : "";
		if (saved) {
			trainingDir = saved;
			return;
		}
		trainingDir = await invokeRust<string>("default_training_folder");
	} catch (err: unknown) {
		if (!trainingDir.trim()) trainingDir = "Documents\\ls-vision\\training";
		trainingNotice = invokeError(err, "Could not load the training folder");
	}
}

async function browseTrainingFolder() {
	trainingNotice = "";
	try {
		const picked = await invokeRust<string>("pick_training_folder");
		if (typeof picked === "string" && picked.trim())
			trainingDir = picked.trim();
	} catch (err: unknown) {
		trainingNotice = invokeError(err, "Could not choose a folder");
	}
}

async function saveTrainingFolder() {
	if (savingTraining) return;
	savingTraining = true;
	trainingNotice = "";
	try {
		const checked = await invokeRust<string>("check_training_folder", {
			path: trainingDir.trim(),
		});
		await queueCameraPost(async () => {
			const config = await loadDocument();
			config.trainingDir = checked;
			cameraFile = await storeDocument(config);
			trainingDir = checked;
			trainingNotice = "Saved.";
		});
	} catch (err: unknown) {
		trainingNotice = invokeError(err, "Could not save the training folder");
	} finally {
		savingTraining = false;
	}
}

function copyCameraFile(): Record<string, unknown> | null {
	if (!cameraFile) return null;
	return JSON.parse(JSON.stringify(cameraFile)) as Record<string, unknown>;
}

function cameraRecord(
	document: Record<string, unknown>,
	index: number,
	patch: Partial<{
		resolution: string;
		displayFps: number;
	}>,
): CameraSettings {
	rememberActiveCamera(document);
	const record = settingsForCamera(objectField(document.camera), index);
	if (patch.resolution !== undefined) {
		record.resolution = isModeId(patch.resolution) ? patch.resolution : "";
	}
	if (patch.displayFps !== undefined) {
		record.displayFps = readDisplayFps(patch.displayFps);
	}
	return record;
}

function stageCameraPatch(
	index: number,
	patch: Partial<{
		resolution: string;
		displayFps: number;
	}>,
) {
	const draft = copyCameraFile();
	if (!draft) return;
	const record = cameraRecord(draft, index, patch);
	if (Number(cameraIndex) === index) assignActiveCamera(draft, record);
	else retainCameraSettings(draft, record);
	cameraFile = draft;
	cameraFileVersion += 1;
}

async function persistCamera(
	index: number,
	patch: Partial<{
		resolution: string;
		displayFps: number;
	}>,
) {
	stageCameraPatch(index, patch);
	const version = cameraFileVersion;
	await queueCameraPost(async () => {
		const current = await loadDocument();
		const record = cameraRecord(current, index, patch);
		// The dropdown may already show another camera. Keep the patch on this
		// camera, and leave the selected camera's top-level fields alone.
		const selected = Number(cameraIndex) === index;
		if (selected) assignActiveCamera(current, record);
		else retainCameraSettings(current, record);
		await storeDocument(current);
		if (cameraFileVersion !== version) return;
		cameraFile = current;
		if (!selected) return;
		savedCamera = {
			...savedCamera,
			index,
			resolution: record.resolution,
			displayFps: record.displayFps,
		};
	});
}

async function persistResolution(resolution: string) {
	await persistCamera(Number(cameraIndex), { resolution });
}

async function persistDisplayFps(displayFps: number) {
	await persistCamera(Number(cameraIndex), { displayFps });
}

function onDisplayFpsChange() {
	cameraDisplayFps = readDisplayFps(cameraDisplayFps);
	postPreviewIndex(Number(cameraIndex));
	void persistDisplayFps(cameraDisplayFps);
}

async function loadCameraModes() {
	const index = Number(cameraIndex);
	try {
		const body = await invokeRust<{
			modes?: CameraMode[];
			default?: string;
		}>("list_camera_modes", { index });
		if (index !== Number(cameraIndex)) return;
		const modes = Array.isArray(body.modes)
			? body.modes.filter(
					(mode) =>
						typeof mode?.id === "string" &&
						isModeId(mode.id) &&
						typeof mode.width === "number" &&
						typeof mode.height === "number" &&
						typeof mode.fps === "number",
				)
			: [];
		cameraModes = modes;
		cameraModeDefault =
			typeof body.default === "string" && isModeId(body.default)
				? body.default
				: (modes[0]?.id ?? "");
		if (
			cameraModeDefault &&
			!modes.some((mode) => mode.id === cameraResolution)
		) {
			cameraResolution = cameraModeDefault;
			await persistResolution(cameraResolution);
		}
	} catch (err: unknown) {
		if (index !== Number(cameraIndex)) return;
		cameraListNotice = invokeError(err, "Could not list camera modes.");
	}
}

async function saveCamera() {
	if (cameraApplyTimer !== null) {
		clearTimeout(cameraApplyTimer);
		cameraApplyTimer = null;
	}
	rememberLevels();
	const id = ++cameraPushId;
	const body = cameraSettingsBody();
	savingCamera = true;
	cameraNotice = "";
	try {
		await queueCameraPost(async () => {
			if (id !== cameraPushId) return;
			const data = await postCameraSettings(false, body);
			if (id !== cameraPushId || Number(cameraIndex) !== body.index) return;
			applyCamera(data);
			rememberSaved();
			cameraIndex = Number(cameraIndex);
			cameraNotice =
				typeof data.message === "string" && data.message
					? data.message
					: "Saved";
		});
	} catch (err: unknown) {
		if (id === cameraPushId) {
			cameraNotice = errorMessage(err, "Could not save the camera");
		}
	} finally {
		savingCamera = false;
	}
}

function inTauri(): boolean {
	return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function invokeError(err: unknown, fallback: string): string {
	if (typeof err === "string" && err.trim()) return err;
	return errorMessage(err, fallback);
}

function patchCameraFocus(
	document: Record<string, unknown>,
	nextIndex: number,
	nextFocus: number | null,
) {
	const camera = objectField(document.camera);
	const byIndex = objectField(camera.byIndex);
	const key = String(nextIndex);
	if (Object.hasOwn(byIndex, key)) {
		const record = objectField(byIndex[key]);
		record.focus = nextFocus;
		byIndex[key] = record;
	} else if (Number(camera.index) === nextIndex) {
		byIndex[key] = {
			resolution:
				typeof camera.resolution === "string" ? camera.resolution : "",
			displayFps: camera.displayFps,
			focus: nextFocus,
			brightness: camera.brightness,
			contrast: camera.contrast,
		};
	} else {
		byIndex[key] = { focus: nextFocus };
	}
	if (Number(camera.index) === nextIndex) camera.focus = nextFocus;
	camera.byIndex = byIndex;
	document.camera = camera;
}

function postPreviewAdmin() {
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.postMessage({ admin: unlocked });
	channel.close();
}

function onPreviewMessage(event: MessageEvent) {
	onPreviewFocus(event);
	const data = event.data;
	if (!data || typeof data !== "object" || data.ready !== true) return;
	postPreviewAdmin();
}

function onPreviewFocus(event: MessageEvent) {
	const data = event.data;
	if (!data || typeof data !== "object" || !("focus" in data)) return;
	const payload = data as { index?: unknown; focus?: unknown };
	const nextIndex = Number(payload.index);
	if (!Number.isInteger(nextIndex) || nextIndex !== Number(cameraIndex)) return;
	const cleared = payload.focus === null;
	const nextFocus = Number(payload.focus);
	if (
		!cleared &&
		(!Number.isInteger(nextFocus) || nextFocus < 0 || nextFocus > 100)
	)
		return;
	focusAuto = cleared;
	cameraFocus = cleared ? 50 : nextFocus;
	focusText = String(cameraFocus);
	savedCamera = { ...savedCamera, focus: cleared ? null : nextFocus };
	const draft = copyCameraFile();
	if (draft) {
		patchCameraFocus(draft, nextIndex, cleared ? null : nextFocus);
		cameraFile = draft;
	}
	cameraFileVersion += 1;
}

function postPreviewIndex(index: number) {
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.postMessage({
		index,
		displayFps: readDisplayFps(cameraDisplayFps),
		admin: unlocked,
	});
	channel.close();
}

function onCameraIndexChange() {
	cameraIndex = Number(cameraIndex);
	const next = cameraIndex;
	cameraFileVersion += 1;
	const version = cameraFileVersion;
	const draft = copyCameraFile();
	if (draft) {
		const record = activateCamera(draft, next);
		cameraFile = draft;
		cameraModes = [];
		cameraModeDefault = "";
		showCamera(record);
		rememberSaved();
	}
	postPreviewIndex(next);
	void switchToCamera(next, version);
}

async function switchToCamera(next: number, version: number) {
	try {
		await queueCameraPost(async () => {
			if (Number(cameraIndex) !== next) return;
			const current = await loadDocument();
			if (Number(cameraIndex) !== next) return;
			const record = activateCamera(current, next);
			await storeDocument(current);
			if (Number(cameraIndex) !== next || cameraFileVersion !== version) return;
			cameraFile = current;
			showCamera(record);
			rememberSaved();
		});
	} catch (err: unknown) {
		if (Number(cameraIndex) === next) {
			cameraNotice = errorMessage(err, "Could not load the camera settings");
		}
		return;
	}
	if (Number(cameraIndex) !== next) return;
	postPreviewIndex(next);
	await loadCameraModes();
}

async function openPreview() {
	previewNotice = "";
	const index = Number(cameraIndex);
	if (!Number.isInteger(index) || index < 0) {
		previewNotice = "Choose a camera first.";
		return;
	}
	const url = `/camera-preview?index=${index}${unlocked ? "&admin=1" : ""}`;
	if (!inTauri()) {
		const popup = window.open(url, PREVIEW_LABEL, "popup,width=800,height=600");
		if (!popup) {
			previewNotice = "The browser blocked the preview window.";
			return;
		}
		previewNotice =
			"Preview opened in a browser window. Always on top is available in the desktop app.";
		return;
	}
	try {
		const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
		const existing = await WebviewWindow.getByLabel(PREVIEW_LABEL);
		if (existing) {
			await existing.setAlwaysOnTop(true);
			await existing.setFocus();
			postPreviewIndex(index);
			return;
		}
		const preview = new WebviewWindow(PREVIEW_LABEL, {
			url,
			title: `Camera ${index}`,
			width: 800,
			height: 600,
			alwaysOnTop: true,
			resizable: true,
			focus: true,
			center: true,
			devtools: true,
		});
		preview.once("tauri://created", () => {
			postPreviewIndex(index);
		});
		preview.once("tauri://error", (event) => {
			const payload = event.payload;
			previewNotice =
				typeof payload === "string" && payload
					? payload
					: "Could not open the camera preview.";
		});
	} catch (err: unknown) {
		previewNotice = errorMessage(err, "Could not open the camera preview.");
	}
}

function onWindowFocus() {
	if (!unlocked || section !== "parts") return;
	void loadPartClasses();
}

onMount(() => {
	previewChannel = new BroadcastChannel(PREVIEW_CHANNEL);
	previewChannel.onmessage = onPreviewMessage;
	window.addEventListener("focus", onWindowFocus);
	void loadInference();
	if (!unlocked) {
		void loadModel();
		return;
	}
	void loadSource();
	if (section === "camera") void loadCameraSettings();
});

onDestroy(() => {
	cancelCameraApply();
	cancelConfidenceSave();
	window.removeEventListener("focus", onWindowFocus);
	previewChannel?.close();
	previewChannel = null;
});
</script>

<svelte:head>
	<style>
		html {
			color-scheme: dark;
		}
	</style>
</svelte:head>

<svelte:window on:keydown={onReferencesKey} />

<div class="settings-root flex h-dvh max-h-dvh flex-col overflow-hidden overscroll-none bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex min-w-0 items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">Settings</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Admin</p>
			{#if unlocked && inferenceReady}
				<p
					class="truncate text-xs font-medium uppercase tracking-widest text-neutral-300"
					title={inferenceNotice || undefined}
				>
					Inference {inferenceBackend}
				</p>
			{/if}
		</div>
		<div class="flex items-center gap-2">
			{#if unlocked}
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
					on:click={lock}
				>
					Lock
				</button>
			{/if}
			<a
				href="/"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
			>
				Operator
			</a>
		</div>
	</header>

	{#if !unlocked}
		<main class="settings-scroll min-h-0 flex-1 overflow-y-auto">
			<div class="mx-auto flex max-w-2xl flex-col gap-4 p-4">
				<section class="flex flex-col gap-3 border border-neutral-700 bg-neutral-900 p-4">
					<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						Password
						<input
							id="settings-password"
							use:focusOnMount
							type="password"
							autocomplete="off"
							autofocus
							class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
							bind:value={password}
							on:keydown={(event) => event.key === "Enter" && unlock()}
						/>
					</label>
					<button
						type="button"
						class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white"
						on:click={unlock}
					>
						Unlock
					</button>
					{#if gateMessage}
						<p class="text-sm font-semibold text-red-400">{gateMessage}</p>
					{/if}
				</section>
			</div>
		</main>
	{:else}
		<div class="flex min-h-0 flex-1">
			<nav
				class="settings-scroll flex w-24 shrink-0 flex-col gap-2 overflow-y-auto border-r border-neutral-800 bg-neutral-900 p-2"
				aria-label="Settings sections"
			>
				<a href="/results" class={sectionOff}>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 24 24"
						class="h-5 w-5"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						aria-hidden="true"
					>
						<path d="M4 19V10" />
						<path d="M10 19V5" />
						<path d="M16 19v-6" />
						<path d="M22 19H2" />
					</svg>
					Results
				</a>
				<button
					type="button"
					class={section === "camera" ? sectionOn : sectionOff}
					aria-current={section === "camera" ? "page" : undefined}
					on:click={() => selectSection("camera")}
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
						<path
							d="M14.5 4h-5L7 7H4a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3l-2.5-3z"
						/>
						<circle cx="12" cy="13" r="3" />
					</svg>
					Camera
				</button>
				<button
					type="button"
					class={section === "capture" ? sectionOn : sectionOff}
					aria-current={section === "capture" ? "page" : undefined}
					on:click={() => selectSection("capture")}
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
						<path d="M4 8V5a1 1 0 0 1 1-1h3" />
						<path d="M16 4h3a1 1 0 0 1 1 1v3" />
						<path d="M20 16v3a1 1 0 0 1-1 1h-3" />
						<path d="M8 20H5a1 1 0 0 1-1-1v-3" />
					</svg>
					Capture
				</button>
				<button
					type="button"
					class={section === "training" ? sectionOn : sectionOff}
					aria-current={section === "training" ? "page" : undefined}
					on:click={() => selectSection("training")}
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
						<path
							d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z"
						/>
					</svg>
					Training
				</button>
				<button
					type="button"
					class={section === "parts" ? sectionOn : sectionOff}
					aria-current={section === "parts" ? "page" : undefined}
					on:click={() => selectSection("parts")}
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
						<rect x="3" y="3" width="12" height="12" />
						<rect x="9" y="9" width="12" height="12" />
					</svg>
					Parts
				</button>
				<button
					type="button"
					class={section === "database" ? sectionOn : sectionOff}
					aria-current={section === "database" ? "page" : undefined}
					on:click={() => selectSection("database")}
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
						<ellipse cx="12" cy="5" rx="9" ry="3" />
						<path d="M3 5v6c0 1.7 4 3 9 3s9-1.3 9-3V5" />
						<path d="M3 11v6c0 1.7 4 3 9 3s9-1.3 9-3v-6" />
					</svg>
					Database
				</button>
			</nav>

			<main
				class="settings-scroll min-h-0 flex-1 overflow-y-auto"
				aria-labelledby="settings-section-heading"
			>
				<div class="mx-auto grid w-full max-w-2xl grid-cols-1 items-start gap-4 p-4 xl:max-w-6xl xl:grid-cols-2">
					{#if section === "camera"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight xl:col-span-2">
							Camera
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4 xl:col-span-2 xl:grid xl:grid-cols-2 xl:items-start"
							on:submit|preventDefault={saveCamera}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Primary camera
								</legend>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Camera
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-base font-medium normal-case tracking-normal text-neutral-100"
										bind:value={cameraIndex}
										on:change={onCameraIndexChange}
									>
										{#each cameraChoices as index (index)}
											<option value={index}>{index}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									The operator page opens this camera. A camera that is unplugged at launch is tried again next time. Resolution, display rate, and levels are kept for each camera.
								</p>
								{#if cameraListNotice}
									<p class="text-sm text-neutral-200">{cameraListNotice}</p>
								{/if}
							</fieldset>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Image
								</legend>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Resolution
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
										bind:value={cameraResolution}
										on:change={() => {
											void persistResolution(cameraResolution);
										}}
									>
										{#if cameraResolution && !cameraModes.some((mode) => mode.id === cameraResolution)}
											<option value={cameraResolution}>{cameraResolution}</option>
										{/if}
										{#each cameraModes as mode (mode.id)}
											<option value={mode.id}>{modeLabel(mode)}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									The preview reopens in this mode. NV12 is used when the camera lists it.
								</p>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Display FPS
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
										bind:value={cameraDisplayFps}
										on:change={onDisplayFpsChange}
									>
										{#each DISPLAY_FPS as rate (rate)}
											<option value={rate}>{rate}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									The preview canvas aims for this rate. The next frame is requested after the previous one is drawn.
								</p>
								<div class="divide-y divide-neutral-800 border-y border-neutral-800">
									<div class="flex items-center gap-3 py-1">
										<span class="flex w-36 shrink-0 items-center gap-2 text-sm font-medium normal-case tracking-normal text-neutral-100">
											<svg viewBox="0 0 24 24" class="h-5 w-5 shrink-0" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
												<circle cx="12" cy="12" r="3" />
												<circle cx="12" cy="12" r="7.5" />
											</svg>
											Focus
										</span>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Decrease focus" on:click={() => stepLevel(cameraFocus, -1, setCameraFocus, (value) => (focusText = value))}>−</button>
										<input
											type="range"
											class="camera-slider min-w-0 flex-1"
											min="0"
											max="100"
											step="1"
											value={cameraFocus}
											style="--fill: {cameraFocus}%"
											aria-label="Focus"
											on:input={(event) => slideLevel(event.currentTarget.value, cameraFocus, setCameraFocus, (value) => (focusText = value))}
										/>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Increase focus" on:click={() => stepLevel(cameraFocus, 1, setCameraFocus, (value) => (focusText = value))}>+</button>
										<input
											type="text"
											inputmode="numeric"
											maxlength="3"
											class="h-9 w-14 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
											aria-label="Focus value"
											value={focusText}
											on:input={(event) => previewDraft(event.currentTarget.value, (value) => (focusText = value), setCameraFocus)}
											on:blur={() => commitDraft(focusText, cameraFocus, setCameraFocus, (value) => (focusText = value))}
											on:keydown={(event) => {
												if (event.key !== "Enter") return;
												event.preventDefault();
												commitDraft(focusText, cameraFocus, setCameraFocus, (value) => (focusText = value));
											}}
										/>
									</div>
									<div class="flex items-center gap-3 py-1">
										<span class="flex w-36 shrink-0 items-center gap-2 text-sm font-medium normal-case tracking-normal text-neutral-100">
											<svg viewBox="0 0 24 24" class="h-5 w-5 shrink-0" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true">
												<circle cx="12" cy="12" r="3.5" />
												<path d="M12 3v2.5M12 18.5V21M4.9 4.9l1.8 1.8M17.3 17.3l1.8 1.8M3 12h2.5M18.5 12H21M4.9 19.1l1.8-1.8M17.3 6.7l1.8-1.8" />
											</svg>
											Brightness
										</span>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Decrease brightness" on:click={() => stepLevel(cameraBrightness, -1, (value) => (cameraBrightness = value), (value) => (brightnessText = value))}>−</button>
										<input
											type="range"
											class="camera-slider min-w-0 flex-1"
											min="0"
											max="100"
											step="1"
											value={cameraBrightness}
											style="--fill: {cameraBrightness}%"
											aria-label="Brightness"
											on:input={(event) => slideLevel(event.currentTarget.value, cameraBrightness, (value) => (cameraBrightness = value), (value) => (brightnessText = value))}
										/>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Increase brightness" on:click={() => stepLevel(cameraBrightness, 1, (value) => (cameraBrightness = value), (value) => (brightnessText = value))}>+</button>
										<input
											type="text"
											inputmode="numeric"
											maxlength="3"
											class="h-9 w-14 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
											aria-label="Brightness value"
											value={brightnessText}
											on:input={(event) => previewDraft(event.currentTarget.value, (value) => (brightnessText = value), (value) => (cameraBrightness = value))}
											on:blur={() => commitDraft(brightnessText, cameraBrightness, (value) => (cameraBrightness = value), (value) => (brightnessText = value))}
											on:keydown={(event) => {
												if (event.key !== "Enter") return;
												event.preventDefault();
												commitDraft(brightnessText, cameraBrightness, (value) => (cameraBrightness = value), (value) => (brightnessText = value));
											}}
										/>
									</div>
									<div class="flex items-center gap-3 py-1">
										<span class="flex w-36 shrink-0 items-center gap-2 text-sm font-medium normal-case tracking-normal text-neutral-100">
											<svg viewBox="0 0 24 24" class="h-5 w-5 shrink-0" aria-hidden="true">
												<circle cx="12" cy="12" r="8" fill="none" stroke="currentColor" stroke-width="2" />
												<path d="M12 4a8 8 0 0 0 0 16z" fill="currentColor" />
											</svg>
											Contrast
										</span>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Decrease contrast" on:click={() => stepLevel(cameraContrast, -1, (value) => (cameraContrast = value), (value) => (contrastText = value))}>−</button>
										<input
											type="range"
											class="camera-slider min-w-0 flex-1"
											min="0"
											max="100"
											step="1"
											value={cameraContrast}
											style="--fill: {cameraContrast}%"
											aria-label="Contrast"
											on:input={(event) => slideLevel(event.currentTarget.value, cameraContrast, (value) => (cameraContrast = value), (value) => (contrastText = value))}
										/>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Increase contrast" on:click={() => stepLevel(cameraContrast, 1, (value) => (cameraContrast = value), (value) => (contrastText = value))}>+</button>
										<input
											type="text"
											inputmode="numeric"
											maxlength="3"
											class="h-9 w-14 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
											aria-label="Contrast value"
											value={contrastText}
											on:input={(event) => previewDraft(event.currentTarget.value, (value) => (contrastText = value), (value) => (cameraContrast = value))}
											on:blur={() => commitDraft(contrastText, cameraContrast, (value) => (cameraContrast = value), (value) => (contrastText = value))}
											on:keydown={(event) => {
												if (event.key !== "Enter") return;
												event.preventDefault();
												commitDraft(contrastText, cameraContrast, (value) => (cameraContrast = value), (value) => (contrastText = value));
											}}
										/>
									</div>
								</div>
								<p class="text-sm text-neutral-400">
									Resolution is sent to this camera. The preview Focus slider sets focus and saves it here. Auto on the preview clears it. Brightness and contrast are saved with the camera. Reset returns the defaults. Cancel returns the last saved settings.
								</p>
							</fieldset>
							<div class="flex flex-wrap items-center gap-2 xl:col-span-2">
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingCamera}
								>
									{savingCamera ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={resetCamera}
									disabled={savingCamera}
								>
									Reset
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={cancelCameraEdits}
									disabled={savingCamera}
								>
									Cancel
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
									on:click={openPreview}
								>
									Preview
								</button>
							</div>
							{#if cameraNotice}
								<p class="text-sm text-neutral-200 xl:col-span-2">{cameraNotice}</p>
							{/if}
							{#if previewNotice}
								<p class="text-sm text-neutral-200 xl:col-span-2">{previewNotice}</p>
							{/if}
						</form>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4 xl:col-span-2"
							on:submit|preventDefault={saveTrainingFolder}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Training image folder
								</legend>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Folder
									<input
										type="text"
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-sm font-medium normal-case tracking-normal text-neutral-100"
										bind:value={trainingDir}
										disabled={savingTraining}
										spellcheck="false"
										autocomplete="off"
									/>
								</label>
								<p class="text-sm text-neutral-400">
									Preview snapshots are saved in a dd-mm-yyyy folder here. The default is Documents/ls-vision/training. Save stores this folder.
								</p>
							</fieldset>
							<div class="flex flex-wrap items-center gap-2">
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingTraining}
								>
									{savingTraining ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={browseTrainingFolder}
									disabled={savingTraining}
								>
									Browse
								</button>
							</div>
							{#if trainingNotice}
								<p class="text-sm text-neutral-200">{trainingNotice}</p>
							{/if}
						</form>
					{:else if section === "capture"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight xl:col-span-2">
							Capture
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							on:submit|preventDefault={saveTestMode}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Test mode
								</legend>
								<label class="flex items-center gap-3 text-sm font-medium text-neutral-100">
									<input
										type="checkbox"
										class="h-5 w-5 accent-blue-500"
										bind:checked={testMode}
										disabled={savingTest}
									/>
									Use an image file after a kit IC loads
								</label>
								<p class="text-sm text-neutral-400">
									When this is on, entering a kit IC and loading its BoM asks for an image file. Detection runs on that file. When it is off, the operator page keeps the camera and barcode flow.
								</p>
								<label class="flex items-center gap-3 text-sm font-medium text-neutral-100">
									<input
										type="checkbox"
										class="h-5 w-5 accent-blue-500"
										bind:checked={skipResultSave}
										disabled={savingTest}
									/>
									Do not save Pass / Fail results.
								</label>
								<p class="text-sm text-neutral-400">
									When this is on, a pass or fail is not written and no snapshot is saved. Turn it off to keep the result and its image.
								</p>
							</fieldset>
							<button
								type="submit"
								class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
								disabled={savingTest}
							>
								{savingTest ? "Saving..." : "Save"}
							</button>
							{#if testNotice}
								<p class="text-sm text-neutral-200">{testNotice}</p>
							{/if}
						</form>

						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							method="dialog"
							novalidate
							on:submit|preventDefault={saveScan}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Barcode scan
								</legend>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Interval (seconds)
									<input
										type="number"
										class="w-28 border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
										bind:value={scanSeconds}
										min="1"
										max="60"
										step="1"
										required
									/>
								</label>
								<p class="text-sm text-neutral-400">
									How often the live feed checks for a barcode. The default is 8 seconds.
								</p>
							</fieldset>
							<button
								type="submit"
								class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
								disabled={savingScan}
							>
								{savingScan ? "Saving..." : "Save"}
							</button>
							{#if scanNotice}
								<p class="text-sm text-neutral-200">{scanNotice}</p>
							{/if}
						</form>

						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							method="dialog"
							novalidate
							on:submit|preventDefault={saveRoi}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Part detection
								</legend>
								<div class="flex items-center gap-1" role="group" aria-label="Detect slots">
									{#each [1, 2, 3, 4] as slot (slot)}
										{@const box = roiSlots[slot - 1]}
										{#if box}
											<button
												type="button"
												class="inline-flex h-9 w-9 cursor-pointer items-center justify-center border text-sm font-semibold {slotSquareClass(box)}"
												aria-pressed={box.enabled}
												aria-label="Slot {slot}"
												on:click={() => toggleSavedSlot(slot)}
											>
												{slot}
											</button>
										{:else}
											<span
												class="inline-flex h-9 w-9 cursor-default items-center justify-center border border-neutral-700 bg-neutral-800 text-sm font-semibold text-neutral-500"
												aria-hidden="true"
											>
												{slot}
											</span>
										{/if}
									{/each}
								</div>
								<p class="text-sm text-neutral-400">
									Grey has no box. Green is on. Red is off. A new box starts on.
								</p>
								<label class="flex w-28 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Slot
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-base font-medium normal-case tracking-normal text-neutral-100"
										value={roiSlot}
										on:change={onRoiSlotChange}
									>
										<option value=""></option>
										<option value="1">1</option>
										<option value="2">2</option>
										<option value="3">3</option>
										<option value="4">4</option>
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									{#if !roiSlot}
										No slot selected. With no slots saved, detection uses the full image.
									{:else if roi}
										Slot {roiSlot} uses this region of the image.
									{:else}
										Slot {roiSlot} is unset.
									{/if}
									Hold a snapshot on the operator page and use Set ROI to draw it.
								</p>
								<div class="grid grid-cols-2 gap-3 sm:grid-cols-4 xl:grid-cols-2">
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Left
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={roiX}
											min="0"
											max="1"
											step="any"
											disabled={!roiSlot}
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Top
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={roiY}
											min="0"
											max="1"
											step="any"
											disabled={!roiSlot}
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Width
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={roiW}
											min="0"
											max="1"
											step="any"
											disabled={!roiSlot}
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Height
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={roiH}
											min="0"
											max="1"
											step="any"
											disabled={!roiSlot}
										/>
									</label>
								</div>
							</fieldset>
							<div class="flex flex-wrap items-center gap-2">
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingRoi || !roiSlot}
								>
									{savingRoi ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={clearRoi}
									disabled={savingRoi ||
										!roiSlot ||
										(roi === null &&
											roiX === null &&
											roiY === null &&
											roiW === null &&
											roiH === null)}
								>
									Clear ROI
								</button>
							</div>
							{#if roiNotice}
								<p class="text-sm text-neutral-200">{roiNotice}</p>
							{/if}
						</form>

						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							method="dialog"
							novalidate
							on:submit|preventDefault={saveBarcodeRoi}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Barcode
								</legend>
								<p class="text-sm text-neutral-400">
									{#if barcodeRoi}
										Barcode decode uses this region of the image.
									{:else}
										No region saved. Barcode decode uses the full frame.
									{/if}
									On the operator page, use Set barcode ROI to draw it on the live feed.
								</p>
								<div class="grid grid-cols-2 gap-3 sm:grid-cols-4 xl:grid-cols-2">
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Left
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={barcodeX}
											min="0"
											max="1"
											step="any"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Top
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={barcodeY}
											min="0"
											max="1"
											step="any"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Width
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={barcodeW}
											min="0"
											max="1"
											step="any"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Height
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={barcodeH}
											min="0"
											max="1"
											step="any"
										/>
									</label>
								</div>
							</fieldset>
							<div class="flex flex-wrap items-center gap-2">
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingBarcodeRoi}
								>
									{savingBarcodeRoi ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={clearBarcodeRoi}
									disabled={savingBarcodeRoi ||
										(barcodeRoi === null &&
											barcodeX === null &&
											barcodeY === null &&
											barcodeW === null &&
											barcodeH === null)}
								>
									Clear barcode ROI
								</button>
							</div>
							{#if barcodeNotice}
								<p class="text-sm text-neutral-200">{barcodeNotice}</p>
							{/if}
						</form>
					{:else if section === "training"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight xl:col-span-2">
							Training
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							on:submit|preventDefault={saveModel}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Model
								</legend>
								<p class="text-sm text-neutral-200">
									Chosen file
									<span class="font-mono text-neutral-100">{modelActive}</span>.
								</p>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									File
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-base font-medium normal-case tracking-normal text-neutral-100 disabled:text-neutral-500"
										bind:value={modelName}
										disabled={savingModel || updatingModel || modelFiles.length === 0}
									>
										{#each modelFiles as file (file)}
											<option value={file}>{file}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									Models are stored in the app data models folder. Update model copies an .onnx file over the selected .onnx name, or over best.onnx. The next detection loads that file.
								</p>
								<div class="flex items-center gap-3">
									<span class="w-28 shrink-0 text-sm font-medium normal-case tracking-normal text-neutral-100">
										Confidence
									</span>
									<button
										type="button"
										class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white"
										aria-label="Decrease confidence"
										on:click={() => stepConfidence(-0.01)}
									>−</button>
									<input
										type="range"
										class="camera-slider min-w-0 flex-1"
										min={CONFIDENCE_MIN}
										max={CONFIDENCE_MAX}
										step="0.01"
										value={confidence}
										style="--fill: {confidenceFill()}%"
										aria-label="Confidence threshold"
										on:input={(event) => slideConfidence(event.currentTarget.value)}
									/>
									<button
										type="button"
										class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white"
										aria-label="Increase confidence"
										on:click={() => stepConfidence(0.01)}
									>+</button>
									<input
										type="text"
										inputmode="decimal"
										class="h-9 w-16 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
										aria-label="Confidence value"
										value={confidenceText}
										on:input={(event) => previewConfidence(event.currentTarget.value)}
										on:blur={commitConfidence}
										on:keydown={(event) => {
											if (event.key !== "Enter") return;
											event.preventDefault();
											commitConfidence();
										}}
									/>
								</div>
								<p class="text-sm text-neutral-200">
									Current value <span class="font-mono text-neutral-100">{confidence.toFixed(2)}</span>
								</p>
								<p class="text-sm text-neutral-400">
									Boxes at or below this score are dropped. Range 0.15 to 0.80. The next detection uses the saved value.
								</p>
								{#if confidenceNotice}
									<p class="text-sm text-neutral-200">{confidenceNotice}</p>
								{/if}
								<div class="flex items-center gap-3">
									<span class="w-28 shrink-0 text-sm font-medium normal-case tracking-normal text-neutral-100">
										Match
									</span>
									<button
										type="button"
										class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white"
										aria-label="Decrease match threshold"
										on:click={() => stepMatch(-0.01)}
									>−</button>
									<input
										type="range"
										class="camera-slider min-w-0 flex-1"
										min={MATCH_MIN}
										max={MATCH_MAX}
										step="0.01"
										value={matchThreshold}
										style="--fill: {matchFill()}%"
										aria-label="Match threshold"
										on:input={(event) => slideMatch(event.currentTarget.value)}
									/>
									<button
										type="button"
										class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white"
										aria-label="Increase match threshold"
										on:click={() => stepMatch(0.01)}
									>+</button>
									<input
										type="text"
										inputmode="decimal"
										class="h-9 w-16 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
										aria-label="Match threshold value"
										value={matchText}
										on:input={(event) => previewMatch(event.currentTarget.value)}
										on:blur={commitMatch}
										on:keydown={(event) => {
											if (event.key !== "Enter") return;
											event.preventDefault();
											commitMatch();
										}}
									/>
								</div>
								<p class="text-sm text-neutral-200">
									Current value <span class="font-mono text-neutral-100">{matchThreshold.toFixed(2)}</span>
								</p>
								<p class="text-sm text-neutral-400">
									A part with saved references counts toward the BoM only when its best match is above this score. View references can set a different score for one IC. Range 0.50 to 0.95. A part with no reference still counts.
								</p>
								{#if matchNotice}
									<p class="text-sm text-neutral-200">{matchNotice}</p>
								{/if}
							</fieldset>
							<div class="flex flex-wrap gap-2">
								<button
									type="submit"
									class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingModel ||
										updatingModel ||
										modelFiles.length === 0 ||
										!modelName}
								>
									{savingModel ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 w-fit items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={updateModel}
									disabled={savingModel || updatingModel}
								>
									{updatingModel ? "Updating..." : "Update model"}
								</button>
							</div>
							{#if modelNotice}
								<p class="break-all text-sm {modelNoticeClass(modelNotice)}">{modelNotice}</p>
							{/if}
							<div class="flex flex-col items-start gap-2">
								<button
									type="button"
									class="inline-flex min-h-11 w-fit items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={updateReferences}
									disabled={updatingReferences}
								>
									{updatingReferences ? "Updating..." : "Update references"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 w-fit items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={openReferences}
									disabled={updatingReferences}
								>
									View references
								</button>
								<p class="text-sm text-neutral-400">
									Replaces the saved DINOv2 references. The next detection uses the loaded embeddings.
								</p>
								{#if referenceNotice}
									<p class="text-sm {referenceNoticeClass(referenceNotice)}">{referenceNotice}</p>
								{/if}
								{#if referencePath}
									<p class="break-all font-mono text-xs text-neutral-500">{referencePath}</p>
								{/if}
							</div>
						</form>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							on:submit|preventDefault={() => undefined}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Inference
								</legend>
								<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Inference EP
									<select
										class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100 disabled:text-neutral-500"
										bind:value={inferenceEp}
										on:change={onInferenceChange}
										disabled={savingInference}
									>
										{#each inferenceProviders as provider (provider)}
											<option value={provider}>{inferenceLabel(provider)}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-200">
									Active: {inferenceReady ? inferenceBackend : "Checking..."}
								</p>
								<p class="text-sm text-neutral-400">
									CPU is always listed. DirectML and CUDA are listed only when this PC can run them.
								</p>
								<label class="flex items-center gap-3 text-sm font-medium text-neutral-100">
									<input
										type="checkbox"
										class="h-5 w-5 accent-blue-500"
										checked={useOpenVino}
										on:change={onOpenVinoChange}
										disabled={savingInference || !inferenceReady}
									/>
									Use OpenVINO (Arc)
								</label>
								<p class="text-sm text-neutral-400">
									Loads the same ONNX files through the OpenVINO plugin on Arc. If the plugin is missing or does not start, detection keeps the Inference EP above and the status line says so.
								</p>
								<button
									type="button"
									class="inline-flex min-h-11 w-fit items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={runBenchmark}
									disabled={benchmarking || savingInference || !inferenceReady}
								>
									{benchmarking ? "Benchmarking..." : "Benchmark detect"}
								</button>
								{#if benchmarkText}
									<pre
										class="select-all whitespace-pre-wrap font-mono text-sm leading-6 text-neutral-100"
										title="Click to select"
									>{benchmarkText}</pre>
								{/if}
							</fieldset>
							{#if inferenceNotice}
								<p class="text-sm text-neutral-200">{inferenceNotice}</p>
							{/if}
						</form>
					{:else if section === "parts"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight xl:col-span-2">
							Part rules
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4 xl:col-span-2"
							on:submit|preventDefault={savePartRules}
						>
							<p class="text-sm text-neutral-400">
								After detection, a parent box is renamed to the result IC when its rule matches. Parent and child are chosen from the active model's class list. Contains matches when the child box center lies inside the parent, including the edge. No child matches when that center is absent. A blank child on no child means any class inside the parent. Rules run from top to bottom and the first match wins. Child boxes keep the class the model gave them. An empty list leaves every class unchanged.
							</p>
							{#if !partClassesReady}
								<p class="text-sm text-neutral-400">Loading classes...</p>
							{:else if partClasses.length === 0}
								<p class="text-sm text-neutral-400">{partClassMessage}</p>
							{/if}
							<div class="overflow-x-auto">
								<table class="w-full min-w-[44rem] border-collapse text-left">
									<thead>
										<tr class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											<th class="px-2 py-2" scope="col">Parent class</th>
											<th class="px-2 py-2" scope="col">Child class</th>
											<th class="px-2 py-2" scope="col">Condition</th>
											<th class="px-2 py-2" scope="col">Result IC</th>
											<th class="px-2 py-2" scope="col">Actions</th>
										</tr>
									</thead>
									<tbody>
										{#each partRules as rule, index (rule.id)}
											<tr class="border-t border-neutral-800">
												<td class="px-2 py-2">
													<ClassPicker
														bind:value={rule.parent}
														classes={partClasses}
														label={`Parent class for rule ${index + 1}`}
														disabled={savingParts ||
															!partClassesReady ||
															partClasses.length === 0}
													/>
												</td>
												<td class="px-2 py-2">
													<ClassPicker
														bind:value={rule.child}
														classes={partClasses}
														label={`Child class for rule ${index + 1}`}
														allowEmpty={rule.condition === "no-child"}
														disabled={savingParts ||
															!partClassesReady ||
															partClasses.length === 0}
													/>
												</td>
												<td class="px-2 py-2">
													<select
														class="w-full border border-neutral-600 bg-neutral-950 px-2 py-2 text-sm text-neutral-100"
														bind:value={rule.condition}
														aria-label={`Condition for rule ${index + 1}`}
													>
														<option value="contains">contains</option>
														<option value="no-child">no child</option>
													</select>
												</td>
												<td class="px-2 py-2">
													<input
														class="w-full min-w-28 border border-neutral-600 bg-neutral-950 px-2 py-2 font-mono text-sm text-neutral-100"
														bind:value={rule.result}
														aria-label={`Result IC for rule ${index + 1}`}
														maxlength={PART_LABEL_LIMIT}
														autocomplete="off"
													/>
												</td>
												<td class="px-2 py-2">
													<div class="flex flex-wrap gap-1">
														<button
															type="button"
															class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-2 text-sm font-semibold text-neutral-100 disabled:border-neutral-800 disabled:text-neutral-600"
															on:click={() => movePartRule(rule.id, -1)}
															disabled={index === 0 || savingParts}
														>
															Up
														</button>
														<button
															type="button"
															class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-2 text-sm font-semibold text-neutral-100 disabled:border-neutral-800 disabled:text-neutral-600"
															on:click={() => movePartRule(rule.id, 1)}
															disabled={index === partRules.length - 1 || savingParts}
														>
															Down
														</button>
														<button
															type="button"
															class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-2 text-sm font-semibold text-neutral-100 disabled:border-neutral-800 disabled:text-neutral-600"
															on:click={() => removePartRule(rule.id)}
															disabled={savingParts}
														>
															Remove
														</button>
													</div>
												</td>
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
							{#if partRules.length === 0}
								<p class="text-sm text-neutral-400">
									No rules. Detected classes stay as the model named them.
								</p>
							{/if}
							<div class="flex flex-wrap items-center gap-2">
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:text-neutral-500"
									on:click={addPartRule}
									disabled={savingParts || partRules.length >= PART_RULE_LIMIT}
								>
									Add rule
								</button>
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={savingParts}
								>
									{savingParts ? "Saving..." : "Save"}
								</button>
							</div>
							{#if partNotice}
								<p class="text-sm text-neutral-200">{partNotice}</p>
							{/if}
						</form>
					{:else}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight xl:col-span-2">
							Database
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							on:submit|preventDefault={saveSource}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									BoM source
								</legend>
								<label class="flex items-center gap-3 text-base font-medium">
									<input
										class="h-4 w-4 accent-blue-500"
										type="radio"
										name="bom-source"
										value="mssql"
										bind:group={source}
									/>
									SQL
									<span class="text-sm font-normal text-neutral-400">vision/bom_queries.sql</span>
								</label>
								<label class="flex items-center gap-3 text-base font-medium">
									<input
										class="h-4 w-4 accent-blue-500"
										type="radio"
										name="bom-source"
										value="json"
										bind:group={source}
									/>
									JSON
									<span class="text-sm font-normal text-neutral-400">vision/sample-bom.json</span>
								</label>
								<label class="flex items-center gap-3 text-base font-medium">
									<input
										class="h-4 w-4 accent-blue-500"
										type="radio"
										name="bom-source"
										value="api"
										bind:group={source}
									/>
									API
								</label>
							</fieldset>

							{#if source === "mssql"}
								<div class="flex flex-col gap-3 border-t border-neutral-800 pt-4">
									<fieldset class="flex flex-col gap-2">
										<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											Authentication
										</legend>
										<label class="flex items-center gap-3 text-base font-medium">
											<input
												class="h-4 w-4 accent-blue-500"
												type="radio"
												name="mssql-auth"
												value="windows"
												bind:group={auth}
												on:change={onAuthChange}
											/>
											Windows Authentication
										</label>
										<label class="flex items-center gap-3 text-base font-medium">
											<input
												class="h-4 w-4 accent-blue-500"
												type="radio"
												name="mssql-auth"
												value="sql"
												bind:group={auth}
												on:change={onAuthChange}
											/>
											SQL Server Authentication
										</label>
									</fieldset>
									<div class="grid grid-cols-1 gap-3 xl:grid-cols-2">
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											Server
											<input
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
												bind:value={mssql.server}
												autocomplete="off"
											/>
										</label>
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											Database
											<input
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
												bind:value={mssql.database}
												autocomplete="off"
											/>
										</label>
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											<span>
												Username
												{#if auth === "sql"}
													<span class="font-normal normal-case tracking-normal text-red-400">Required</span>
												{/if}
											</span>
											<input
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100 disabled:bg-neutral-800 disabled:text-neutral-500"
												bind:value={mssql.user}
												autocomplete="off"
												disabled={auth === "windows"}
												required={auth === "sql"}
											/>
										</label>
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											<span>
												Password
												{#if auth === "sql"}
													<span class="font-normal normal-case tracking-normal text-red-400">Required</span>
												{/if}
											</span>
											<input
												type="password"
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100 disabled:bg-neutral-800 disabled:text-neutral-500"
												bind:value={mssqlPassword}
												autocomplete="new-password"
												disabled={auth === "windows"}
												required={auth === "sql"}
											/>
										</label>
									</div>
									<p class="text-sm text-neutral-400">
										Inspect runs the queries in vision/bom_queries.sql.
									</p>
								</div>
							{:else if source === "api"}
								<div class="flex flex-col gap-3 border-t border-neutral-800 pt-4">
									<div class="grid grid-cols-1 gap-3 xl:grid-cols-2">
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											Base URL
											<input
												type="url"
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
												bind:value={apiBaseUrl}
												autocomplete="off"
												placeholder="https://"
											/>
										</label>
										<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
											Endpoint path
											<input
												class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
												bind:value={apiPath}
												autocomplete="off"
												placeholder="/bom"
											/>
										</label>
									</div>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										API key
										<input
											type="password"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={apiKey}
											autocomplete="new-password"
										/>
									</label>
									<p class="text-sm text-neutral-400">
										Stored in vision/config.local.json. Look up posts the job card number to this API.
									</p>
								</div>
							{/if}

							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								Description endpoint
								<input
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
									bind:value={apiDescriptionPath}
									autocomplete="off"
									placeholder="/VisionAppApi/ListInventoryDescription"
								/>
							</label>
							<p class="text-sm text-neutral-400">
								Optional. Unexpected codes are posted to this path on the API base URL, with the API key. The description is shown on the Not in BoM list. Leave blank to show the code only.
							</p>

							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								Results endpoint
								<input
									id="results-endpoint"
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
									bind:value={resultsPath}
									autocomplete="off"
									placeholder="/VisionAppApi/SubmitResult"
								/>
							</label>
							<p class="text-sm text-neutral-400">
								Optional. Each saved inspection is posted here with the API base URL and API key. Leave blank to keep the result on this PC only.
							</p>

							<div class="flex flex-wrap items-center gap-2">
								<button
									type="submit"
									class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									disabled={saving}
								>
									{saving ? "Saving..." : "Save"}
								</button>
							</div>
							{#if notice}
								<p class="text-sm text-neutral-200">{notice}</p>
							{/if}
						</form>

						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
							on:submit|preventDefault={saveErp}
						>
							<fieldset class="flex flex-col gap-3">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									ERP
								</legend>
								<div class="grid grid-cols-1 gap-3 xl:grid-cols-2">
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Button name
										<input
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={erpName}
											maxlength="40"
											autocomplete="off"
											placeholder="ERP"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										URL
										<input
											type="url"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={erpUrl}
											autocomplete="off"
											placeholder="https://"
										/>
									</label>
								</div>
								<p class="text-sm text-neutral-400">
									The operator page shows a button with this name. It slides the URL over the vision view. Leave the URL empty to hide the button.
								</p>
							</fieldset>
							<button
								type="submit"
								class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
								disabled={savingErp}
							>
								{savingErp ? "Saving..." : "Save"}
							</button>
							{#if erpNotice}
								<p class="text-sm text-neutral-200">{erpNotice}</p>
							{/if}
						</form>
					{/if}
				</div>
			</main>
		</div>
	{/if}
</div>

{#if referencesOpen}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
		on:click={closeReferences}
	>
		<div
			class="flex max-h-[80vh] w-full max-w-3xl flex-col border border-neutral-700 bg-neutral-900 p-4"
			role="dialog"
			aria-modal="true"
			aria-labelledby="references-title"
			on:click|stopPropagation
		>
			<h2 id="references-title" class="text-lg font-semibold tracking-tight">DINOv2 references</h2>
			<p class="mt-2 text-sm text-neutral-400">
				Each IC uses its saved threshold. An IC without one uses the Training match score,
				currently <span class="font-mono text-neutral-100">{matchThreshold.toFixed(2)}</span>.
				The next detection uses a change without a restart.
			</p>
			{#if referencePath}
				<p class="mt-2 break-all font-mono text-xs text-neutral-500">{referencePath}</p>
			{/if}
			{#if referencesNotice}
				<p class="mt-3 text-sm {referencesNoticeClass(referencesNotice)}">{referencesNotice}</p>
			{/if}
			<div class="mt-4 min-h-0 flex-1 overflow-auto">
				{#if referencesLoading}
					<p class="text-sm text-neutral-200">Loading...</p>
				{:else if referenceRows.length === 0}
					{#if !referencesNotice || referencesNotice.startsWith("Removed ")}
						<p class="text-sm text-neutral-200">No references loaded.</p>
					{/if}
				{:else}
					<table class="w-full border-collapse text-left text-sm">
						<thead class="sticky top-0 bg-neutral-900">
							<tr class="border-b border-neutral-700 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								<th
									class="px-2 py-2"
									aria-sort={referenceSort === "ic"
										? referenceSortDir === "asc"
											? "ascending"
											: "descending"
										: "none"}
								>
									<button
										type="button"
										class="inline-flex items-center gap-1 uppercase tracking-widest"
										on:click={() => sortReferencesBy("ic")}
									>
										IC
										{#if referenceSort === "ic"}
											{referenceSortDir === "asc" ? "↑" : "↓"}
										{/if}
									</button>
								</th>
								<th
									class="px-2 py-2"
									aria-sort={referenceSort === "threshold"
										? referenceSortDir === "asc"
											? "ascending"
											: "descending"
										: "none"}
								>
									<button
										type="button"
										class="inline-flex items-center gap-1 uppercase tracking-widest"
										on:click={() => sortReferencesBy("threshold")}
									>
										Threshold
										{#if referenceSort === "threshold"}
											{referenceSortDir === "asc" ? "↑" : "↓"}
										{/if}
									</button>
								</th>
								<th class="px-2 py-2 text-right">Embeddings</th>
								<th class="px-2 py-2"><span class="sr-only">Delete</span></th>
							</tr>
						</thead>
						<tbody>
							{#each sortedReferenceRows as row (row.ic)}
								<tr class="border-b border-neutral-800">
									<td class="px-2 py-2 font-mono text-neutral-100">{row.ic}</td>
									<td class="px-2 py-2">
										<input
											type="text"
											inputmode="decimal"
											class="h-9 w-20 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100 disabled:text-neutral-500"
											aria-label="Threshold for {row.ic}"
											value={referenceDrafts[row.ic] ?? effectiveReferenceThreshold(row).toFixed(2)}
											disabled={savingReferenceIc === row.ic || deletingReferenceIc === row.ic}
											on:input={(event) => setReferenceDraft(row.ic, event.currentTarget.value)}
											on:blur={(event) =>
												commitReferenceThreshold(row.ic, event.currentTarget)}
											on:keydown={(event) => {
												if (event.key !== "Enter") return;
												event.preventDefault();
												commitReferenceThreshold(row.ic, event.currentTarget);
											}}
										/>
									</td>
									<td class="px-2 py-2 text-right font-mono text-neutral-100">{row.count}</td>
									<td class="px-2 py-2 text-right">
										<button
											type="button"
											class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-3 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:text-neutral-500"
											on:click={() => deleteReferenceRow(row.ic)}
											disabled={deletingReferenceIc === row.ic || savingReferenceIc === row.ic}
										>
											{deletingReferenceIc === row.ic ? "Deleting..." : "Delete"}
										</button>
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			</div>
			<div class="mt-4 flex justify-end">
				<button
					type="button"
					bind:this={referencesClose}
					class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
					on:click={closeReferences}
				>
					Close
				</button>
			</div>
		</div>
	</div>
{/if}

<style>
	.settings-root {
		color-scheme: dark;
	}

	.settings-scroll {
		color-scheme: dark;
		scrollbar-color: #a3a3a3 #171717;
		scrollbar-width: auto;
	}

	.settings-scroll::-webkit-scrollbar {
		width: 12px;
		height: 12px;
	}

	.settings-scroll::-webkit-scrollbar-track {
		background: #171717;
	}

	.settings-scroll::-webkit-scrollbar-thumb {
		background: #a3a3a3;
		border: 2px solid #171717;
	}

	.settings-scroll::-webkit-scrollbar-thumb:hover {
		background: #d4d4d4;
	}

	.settings-scroll::-webkit-scrollbar-corner {
		background: #171717;
	}

	.camera-slider {
		-webkit-appearance: none;
		appearance: none;
		height: 18px;
		background: transparent;
		cursor: pointer;
	}

	.camera-slider::-webkit-slider-runnable-track {
		height: 4px;
		border-radius: 999px;
		background: linear-gradient(to right, #3b82f6 var(--fill), #d4d4d4 var(--fill));
	}

	.camera-slider::-webkit-slider-thumb {
		-webkit-appearance: none;
		appearance: none;
		width: 16px;
		height: 16px;
		margin-top: -6px;
		border: 2px solid #f5f5f5;
		border-radius: 999px;
		background: #3b82f6;
	}

	.camera-slider:focus-visible::-webkit-slider-thumb {
		outline: 2px solid #93c5fd;
		outline-offset: 2px;
	}

	.camera-slider::-moz-range-track {
		height: 4px;
		border: none;
		border-radius: 999px;
		background: #d4d4d4;
	}

	.camera-slider::-moz-range-progress {
		height: 4px;
		border-radius: 999px;
		background: #3b82f6;
	}

	.camera-slider::-moz-range-thumb {
		width: 16px;
		height: 16px;
		border: 2px solid #f5f5f5;
		border-radius: 999px;
		background: #3b82f6;
	}
</style>
