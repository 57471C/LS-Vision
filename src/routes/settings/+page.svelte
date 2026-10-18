<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { onDestroy, onMount } from "svelte";
import {
	activateCamera,
	assignActiveCamera,
	type CameraSettings,
	readDisplayFps,
	rememberActiveCamera,
	retainCameraSettings,
	settingsForCamera,
} from "$lib/camera-settings";

const DEV_PASSWORD = "admin";
const UNLOCK_KEY = "ls-vision-settings-open";
const SECTION_KEY = "ls-vision-settings-section";
const DEFAULT_SQL_DRIVER = "ODBC Driver 18 for SQL Server";

type BomSource = "json" | "mssql" | "api";
type AuthMode = "windows" | "sql";
type MssqlFields = { server: string; database: string; user: string };
type SettingsSection = "camera" | "capture" | "training" | "database";
type DetectRoi = { x: number; y: number; w: number; h: number };
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
let testNotice = "";
let savingTest = false;
let roi: DetectRoi | null = null;
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
let inferenceProviders: string[] = ["cpu"];
let inferenceEp = "cpu";
let inferenceActive = "cpu";
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
let cameraBrightness = 50;
let cameraContrast = 50;
let savedCamera = {
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
let savingCamera = false;
let configLoadId = 0;

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
	if (!value || typeof value !== "object") return;
	const api = value as { baseUrl?: unknown; path?: unknown; key?: unknown };
	if (typeof api.baseUrl === "string") apiBaseUrl = api.baseUrl;
	if (typeof api.path === "string") apiPath = api.path;
	if (typeof api.key === "string") apiKey = api.key;
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
	if (!value || typeof value !== "object") return null;
	const row = value as Record<string, unknown>;
	const x = Number(row.x);
	const y = Number(row.y);
	const w = Number(row.w);
	const h = Number(row.h);
	if (![x, y, w, h].every((part) => Number.isFinite(part))) return null;
	return { x, y, w, h };
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
}

function inferenceLabel(id: string): string {
	if (id === "directml") return "DirectML";
	if (id === "cuda") return "CUDA";
	return "CPU";
}

function applyInference(data: {
	providers?: unknown;
	selected?: unknown;
	active?: unknown;
	message?: unknown;
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
		}>("inference_status");
		if (generation !== inferenceGeneration || savingInference) return;
		applyInference(data);
	} catch (err: unknown) {
		if (generation !== inferenceGeneration || savingInference) return;
		inferenceProviders = ["cpu"];
		inferenceEp = "cpu";
		inferenceActive = "cpu";
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
		modelNotice =
			typeof data.message === "string" && data.message ? data.message : "Saved";
	} catch (err: unknown) {
		modelNotice = invokeError(err, "Could not save the model");
	} finally {
		savingModel = false;
	}
}

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
	} catch (err: unknown) {
		testNotice = invokeError(err, "Could not load Test mode");
	}
}

async function loadRoi() {
	try {
		const data = await loadDocument();
		fillRoi(asRoi(data.detectRoi));
		fillBarcodeRoi(asBarcodeRoi(data.barcodeRoi));
	} catch (err: unknown) {
		roiNotice = invokeError(err, "Could not load the ROI");
	}
}

async function saveTestMode() {
	savingTest = true;
	testNotice = "";
	const enabled = testMode;
	try {
		const current = await loadDocument();
		current.testMode = enabled;
		const saved = await storeDocument(current);
		testMode = saved.testMode === true;
		testNotice = "Saved";
	} catch (err: unknown) {
		testMode = !enabled;
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

async function postRoi(next: DetectRoi | null) {
	savingRoi = true;
	roiNotice = "";
	try {
		const current = await loadDocument();
		current.detectRoi = next;
		await storeDocument(current);
		fillRoi(asRoi(next));
		roiNotice = "Saved";
	} catch (err: unknown) {
		roiNotice = invokeError(err, "Could not save the ROI");
	} finally {
		savingRoi = false;
	}
}

function saveRoi() {
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
	if (source === "api") {
		const problem = apiBaseUrlError(apiBaseUrl);
		if (problem) {
			notice = problem;
			return;
		}
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
					baseUrl: apiBaseUrl.trim(),
					path: apiPath.trim(),
					key: apiKey.trim(),
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
		focus: cameraFocus,
		brightness: cameraBrightness,
		contrast: cameraContrast,
	};
}

async function postCameraSettings(applyOnly = false, preset?: CameraSettings) {
	const body = preset ?? cameraSettingsBody();
	// The Rust camera stores these fields. It does not apply focus, brightness,
	// or contrast to the device, so a live slider change does not write config.
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
	const settings = {
		index,
		resolution: body.resolution,
		displayFps: body.displayFps,
		focus: body.focus,
		brightness: body.brightness,
		contrast: body.contrast,
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
		focus: cameraFocus,
		brightness: cameraBrightness,
		contrast: cameraContrast,
	};
}

function showCamera(next: {
	index: number;
	resolution: string;
	displayFps: number;
	focus: number;
	brightness: number;
	contrast: number;
}) {
	cameraIndex = next.index;
	cameraResolution = isModeId(next.resolution) ? next.resolution : "";
	cameraDisplayFps = readDisplayFps(next.displayFps);
	cameraFocus = clampLevel(next.focus);
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
	cameraFocus = levelValue(data.focus) ?? 50;
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

function postPreviewIndex(index: number) {
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.postMessage({
		index,
		displayFps: readDisplayFps(cameraDisplayFps),
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
	const url = `/camera-preview?index=${index}`;
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

onMount(() => {
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
});
</script>

<svelte:head>
	<style>
		html {
			color-scheme: dark;
		}
	</style>
</svelte:head>

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
					Inference {inferenceLabel(inferenceActive)}
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
				<div class="mx-auto flex max-w-2xl flex-col gap-4 p-4">
					{#if section === "camera"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight">
							Camera
						</h2>
						<form
							class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
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
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Decrease focus" on:click={() => stepLevel(cameraFocus, -1, (value) => (cameraFocus = value), (value) => (focusText = value))}>−</button>
										<input
											type="range"
											class="camera-slider min-w-0 flex-1"
											min="0"
											max="100"
											step="1"
											value={cameraFocus}
											style="--fill: {cameraFocus}%"
											aria-label="Focus"
											on:input={(event) => slideLevel(event.currentTarget.value, cameraFocus, (value) => (cameraFocus = value), (value) => (focusText = value))}
										/>
										<button type="button" class="inline-flex h-11 w-8 shrink-0 items-center justify-center text-lg text-neutral-300 hover:text-white" aria-label="Increase focus" on:click={() => stepLevel(cameraFocus, 1, (value) => (cameraFocus = value), (value) => (focusText = value))}>+</button>
										<input
											type="text"
											inputmode="numeric"
											maxlength="3"
											class="h-9 w-14 shrink-0 border border-neutral-600 bg-neutral-950 text-center font-mono text-sm text-neutral-100"
											aria-label="Focus value"
											value={focusText}
											on:input={(event) => previewDraft(event.currentTarget.value, (value) => (focusText = value), (value) => (cameraFocus = value))}
											on:blur={() => commitDraft(focusText, cameraFocus, (value) => (cameraFocus = value), (value) => (focusText = value))}
											on:keydown={(event) => {
												if (event.key !== "Enter") return;
												event.preventDefault();
												commitDraft(focusText, cameraFocus, (value) => (cameraFocus = value), (value) => (focusText = value));
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
									Resolution, focus, brightness, and contrast are sent to this camera. Reset returns the defaults. Cancel returns the last saved settings.
								</p>
							</fieldset>
							<div class="flex flex-wrap items-center gap-2">
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
								<p class="text-sm text-neutral-200">{cameraNotice}</p>
							{/if}
							{#if previewNotice}
								<p class="text-sm text-neutral-200">{previewNotice}</p>
							{/if}
						</form>
					{:else if section === "capture"}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight">
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
								<p class="text-sm text-neutral-400">
									{#if roi}
										Detection uses this region of the image.
									{:else}
										No region saved. Detection uses the full image.
									{/if}
									Hold a snapshot on the operator page and use Set ROI to draw it.
								</p>
								<div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Left
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={roiX}
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
											bind:value={roiY}
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
											bind:value={roiW}
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
											bind:value={roiH}
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
									disabled={savingRoi}
								>
									{savingRoi ? "Saving..." : "Save"}
								</button>
								<button
									type="button"
									class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
									on:click={clearRoi}
									disabled={savingRoi ||
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
								<div class="grid grid-cols-2 gap-3 sm:grid-cols-4">
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
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight">
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
										disabled={savingModel || modelFiles.length === 0}
									>
										{#each modelFiles as file (file)}
											<option value={file}>{file}</option>
										{/each}
									</select>
								</label>
								<p class="text-sm text-neutral-400">
									{#if modelFiles.length === 0}
										No .pt or .onnx files in vision/models. Detection uses best.onnx when that file is present, otherwise best.pt.
									{:else}
										Choose a .pt or .onnx file in vision/models. A missing file, or no selection, uses best.onnx when that file is present, otherwise best.pt.
									{/if}
								</p>
							</fieldset>
							<button
								type="submit"
								class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
								disabled={savingModel || modelFiles.length === 0 || !modelName}
							>
								{savingModel ? "Saving..." : "Save"}
							</button>
							{#if modelNotice}
								<p class="text-sm text-neutral-200">{modelNotice}</p>
							{/if}
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
									Active: {inferenceReady
										? inferenceLabel(inferenceActive)
										: "Checking..."}
								</p>
								<p class="text-sm text-neutral-400">
									CPU is always listed. DirectML and CUDA are listed only when this PC can run them.
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
					{:else}
						<h2 id="settings-section-heading" class="text-lg font-semibold tracking-tight">
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
									<p class="text-sm text-neutral-400">
										Inspect runs the queries in vision/bom_queries.sql.
									</p>
								</div>
							{:else if source === "api"}
								<div class="flex flex-col gap-3 border-t border-neutral-800 pt-4">
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
