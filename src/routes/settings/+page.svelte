<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { onDestroy, onMount } from "svelte";

const DEV_PASSWORD = "admin";
const UNLOCK_KEY = "ls-vision-settings-open";
const SECTION_KEY = "ls-vision-settings-section";

type BomSource = "json" | "mssql" | "api";
type AuthMode = "windows" | "sql";
type MssqlFields = { server: string; database: string; user: string };
type SettingsSection = "camera" | "capture" | "training" | "database";
type DetectRoi = { x: number; y: number; w: number; h: number };

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
let modelFiles: string[] = [];
let modelName = "";
let modelActive = "best.pt";
let modelLoaded = false;
let modelNotice = "";
let savingModel = false;
let cameraIndex = 0;
let cameraChoices: number[] = [0];
let cameraResolution = "";
let cameraFocus: number | null = null;
let cameraBrightness: number | null = null;
let cameraContrast: number | null = null;
let cameraNotice = "";
let cameraListNotice = "";
let previewNotice = "";
let savingCamera = false;
let killingSidecar = false;
let sidecarUp = false;
let sidecarNotice = "";
let sidecarTimer: ReturnType<typeof setInterval> | null = null;

const PREVIEW_LABEL = "camera-preview";
const PREVIEW_CHANNEL = "ls-vision-camera-preview";
const CAMERA_RESOLUTIONS = ["640x480", "1280x720", "1920x1080"];

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

async function loadSource() {
	notice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/bom-source");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		source = readSource(data.source);
		auth = data.mssql?.auth === "sql" ? "sql" : "windows";
		mssql = {
			server: data.mssql?.server ?? "",
			database: data.mssql?.database ?? "",
			user: auth === "windows" ? "" : (data.mssql?.user ?? ""),
		};
		mssqlPassword = auth === "windows" ? "" : (data.mssql?.password ?? "");
		readApi(data.api);
		await loadErp();
		await loadScan();
		await loadTestMode();
		await loadRoi();
		await loadModel();
	} catch (err: unknown) {
		notice = errorMessage(
			err,
			"Could not load the BoM source. Is the vision sidecar running?",
		);
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
	loaded?: unknown;
}) {
	modelFiles = readModelFiles(data.files);
	const active = typeof data.active === "string" ? data.active.trim() : "";
	modelActive = active || "best.pt";
	modelLoaded = data.loaded === true;
	const selected = typeof data.model === "string" ? data.model : "";
	if (modelFiles.includes(selected)) modelName = selected;
	else if (modelFiles.includes(modelActive)) modelName = modelActive;
	else modelName = modelFiles[0] ?? "";
}

async function loadModel() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/model");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		applyModel(data);
		modelNotice = typeof data.message === "string" ? data.message : "";
	} catch (err: unknown) {
		modelNotice = errorMessage(err, "Could not load the model list");
	}
}

async function saveModel() {
	savingModel = true;
	modelNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/model", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ filename: modelName }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Save failed");
		}
		applyModel(data);
		modelNotice =
			typeof data.message === "string" && data.message ? data.message : "Saved";
	} catch (err: unknown) {
		modelNotice = errorMessage(err, "Could not save the model");
	} finally {
		savingModel = false;
	}
}

async function loadScan() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/scan");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		const seconds = Number(data.seconds);
		if (Number.isInteger(seconds)) scanSeconds = seconds;
	} catch (err: unknown) {
		scanNotice = errorMessage(err, "Could not load the scan interval");
	}
}

async function loadTestMode() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/test-mode");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		testMode = data.enabled === true;
	} catch (err: unknown) {
		testNotice = errorMessage(err, "Could not load Test mode");
	}
}

async function loadRoi() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/detect-roi");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		fillRoi(asRoi(data.roi));
	} catch (err: unknown) {
		roiNotice = errorMessage(err, "Could not load the ROI");
	}
}

async function saveTestMode() {
	savingTest = true;
	testNotice = "";
	const enabled = testMode;
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/test-mode", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ enabled }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Save failed");
		}
		testMode = data.enabled === true;
		testNotice = data.message || "Saved";
	} catch (err: unknown) {
		testMode = !enabled;
		testNotice = errorMessage(err, "Could not save Test mode");
	} finally {
		savingTest = false;
	}
}

async function loadErp() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		erpName = typeof data.name === "string" ? data.name : "";
		erpUrl = typeof data.url === "string" ? data.url : "";
	} catch (err: unknown) {
		erpNotice = errorMessage(err, "Could not load the ERP button");
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
	watchSidecar();
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
	sidecarNotice = "";
	sessionStorage.removeItem(UNLOCK_KEY);
	stopSidecarWatch();
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
		const res = await fetch("http://127.0.0.1:8008/settings/scan", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ seconds }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Save failed");
		}
		if (Number.isInteger(Number(data.seconds)))
			scanSeconds = Number(data.seconds);
		scanNotice = data.message || "Saved";
	} catch (err: unknown) {
		scanNotice = errorMessage(err, "Could not save the scan interval");
	} finally {
		savingScan = false;
	}
}

async function saveErp() {
	savingErp = true;
	erpNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ name: erpName, url: erpUrl }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || (await res.text()) || "Save failed");
		}
		erpName = typeof data.name === "string" ? data.name : erpName;
		erpUrl = typeof data.url === "string" ? data.url : erpUrl;
		erpNotice = data.message || "Saved";
	} catch (err: unknown) {
		erpNotice = errorMessage(err, "Could not save the ERP button");
	} finally {
		savingErp = false;
	}
}

async function postRoi(next: DetectRoi | null) {
	savingRoi = true;
	roiNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/detect-roi", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ roi: next }),
		});
		const data = (await res.json().catch(() => ({}))) as {
			status?: string;
			message?: string;
			roi?: unknown;
		};
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Could not save the ROI");
		}
		fillRoi(asRoi(data.roi));
		roiNotice = data.message || "Saved";
	} catch (err: unknown) {
		roiNotice = errorMessage(err, "Could not save the ROI");
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
		const res = await fetch("http://127.0.0.1:8008/settings/bom-source", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({
				source,
				server: mssql.server,
				database: mssql.database,
				auth,
				user: auth === "windows" ? "" : mssql.user,
				password: auth === "windows" ? "" : mssqlPassword,
				api: {
					baseUrl: apiBaseUrl,
					path: apiPath,
					key: apiKey,
				},
			}),
		});
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		if (data.status !== "pass") throw new Error(data.message || "Save failed");
		source = readSource(data.source);
		readApi(data.api);
		auth = data.mssql?.auth === "sql" ? "sql" : "windows";
		mssql.user = data.mssql?.user ?? "";
		mssqlPassword = data.mssql?.password ?? "";
		notice = data.message || "Saved";
	} catch (err: unknown) {
		notice = errorMessage(err, "Could not save the BoM source");
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

function applyCamera(data: {
	index?: unknown;
	resolution?: unknown;
	focus?: unknown;
	brightness?: unknown;
	contrast?: unknown;
}) {
	const index = Number(data.index);
	if (Number.isInteger(index) && index >= 0 && index <= 32) cameraIndex = index;
	const resolution = typeof data.resolution === "string" ? data.resolution : "";
	cameraResolution = CAMERA_RESOLUTIONS.includes(resolution) ? resolution : "";
	cameraFocus = levelValue(data.focus);
	cameraBrightness = levelValue(data.brightness);
	cameraContrast = levelValue(data.contrast);
	if (!cameraChoices.includes(cameraIndex)) {
		cameraChoices = [...cameraChoices, cameraIndex].sort(
			(left, right) => left - right,
		);
	}
}

function readLevel(value: number | null): number | null | undefined {
	if (value === null || Number.isNaN(value)) return null;
	if (!Number.isInteger(value) || value < 0 || value > 100) return undefined;
	return value;
}

async function loadCameraDevices() {
	cameraListNotice = "";
	try {
		const res = await fetch(
			`http://127.0.0.1:8008/cameras?selected=${cameraIndex}`,
		);
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		const selected = Number(cameraIndex);
		const found = cameraList(data.cameras);
		if (!found.includes(selected)) found.push(selected);
		found.sort((left, right) => left - right);
		cameraIndex = selected;
		cameraChoices = found;
	} catch (err: unknown) {
		cameraChoices = [cameraIndex];
		cameraListNotice = errorMessage(
			err,
			"Could not list cameras. Is the vision sidecar running?",
		);
	}
}

async function loadCameraSettings() {
	cameraNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/camera");
		if (!res.ok) throw new Error(await res.text());
		applyCamera(await res.json());
	} catch (err: unknown) {
		cameraNotice = errorMessage(err, "Could not load the camera settings");
	}
	await loadCameraDevices();
}

async function saveCamera() {
	const focus = readLevel(cameraFocus);
	const brightness = readLevel(cameraBrightness);
	const contrast = readLevel(cameraContrast);
	if (
		focus === undefined ||
		brightness === undefined ||
		contrast === undefined
	) {
		cameraNotice =
			"Focus, brightness, and contrast are whole numbers from 0 to 100, or blank.";
		return;
	}
	savingCamera = true;
	cameraNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/camera", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({
				index: Number(cameraIndex),
				resolution: cameraResolution,
				focus,
				brightness,
				contrast,
			}),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Save failed");
		}
		applyCamera(data);
		cameraIndex = Number(cameraIndex);
		cameraNotice =
			typeof data.message === "string" && data.message ? data.message : "Saved";
	} catch (err: unknown) {
		cameraNotice = errorMessage(err, "Could not save the camera");
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

async function refreshSidecar() {
	if (killingSidecar) return;
	try {
		const res = await fetch("http://127.0.0.1:8008/health");
		sidecarUp = res.ok;
	} catch {
		sidecarUp = false;
	}
}

function watchSidecar() {
	void refreshSidecar();
	if (sidecarTimer !== null) return;
	sidecarTimer = setInterval(() => {
		void refreshSidecar();
	}, 2000);
}

function stopSidecarWatch() {
	if (sidecarTimer !== null) {
		clearInterval(sidecarTimer);
		sidecarTimer = null;
	}
	sidecarUp = false;
}

async function killSidecar() {
	if (killingSidecar || !sidecarUp) return;
	killingSidecar = true;
	sidecarNotice = "";
	let stopped = false;
	try {
		try {
			const res = await fetch("http://127.0.0.1:8008/shutdown", {
				method: "POST",
			});
			if (res.ok) stopped = true;
		} catch {
			// The process can close the socket before the response finishes.
			stopped = true;
		}
		if (inTauri()) {
			try {
				const { invoke } = await import("@tauri-apps/api/core");
				const message = await invoke<string>("kill_vision_sidecar");
				if (
					typeof message === "string" &&
					message.toLowerCase().includes("stopped")
				) {
					stopped = true;
				}
			} catch (err: unknown) {
				if (!stopped) {
					sidecarNotice = invokeError(err, "Could not stop the vision sidecar");
				}
			}
		}
		sidecarUp = true;
		for (let attempt = 0; attempt < 8; attempt += 1) {
			await new Promise((resolve) => setTimeout(resolve, 200));
			try {
				const res = await fetch("http://127.0.0.1:8008/health");
				sidecarUp = res.ok;
			} catch {
				sidecarUp = false;
			}
			if (!sidecarUp) break;
		}
		if (!sidecarUp) sidecarNotice = "Stopped vision sidecar.";
		else if (!sidecarNotice)
			sidecarNotice = "Could not stop the vision sidecar.";
	} finally {
		killingSidecar = false;
	}
}

function postPreviewIndex(index: number) {
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.postMessage({ index });
	channel.close();
}

function onCameraIndexChange() {
	cameraIndex = Number(cameraIndex);
	postPreviewIndex(cameraIndex);
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
	if (!unlocked) return;
	void loadSource();
	if (section === "camera") void loadCameraSettings();
	watchSidecar();
});

onDestroy(() => {
	stopSidecarWatch();
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
		<div class="flex items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">Settings</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Admin</p>
		</div>
		<div class="flex items-center gap-2">
			{#if unlocked}
				{#if sidecarUp || killingSidecar}
					<button
						type="button"
						class="inline-flex min-h-11 items-center border border-red-800 bg-red-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
						on:click={killSidecar}
						disabled={killingSidecar}
					>
						{killingSidecar ? "Stopping..." : "Kill Vision Sidecar"}
					</button>
				{/if}
				{#if sidecarNotice}
					<p class="max-w-64 text-sm text-neutral-200">{sidecarNotice}</p>
				{/if}
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
									The operator page opens this camera. A camera that is unplugged at launch is tried again next time.
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
									>
										<option value="">Not set</option>
										<option value="640x480">640×480</option>
										<option value="1280x720">1280×720</option>
										<option value="1920x1080">1920×1080</option>
									</select>
								</label>
								<div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Focus
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={cameraFocus}
											min="0"
											max="100"
											step="1"
											inputmode="numeric"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Brightness
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={cameraBrightness}
											min="0"
											max="100"
											step="1"
											inputmode="numeric"
										/>
									</label>
									<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
										Contrast
										<input
											type="number"
											class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
											bind:value={cameraContrast}
											min="0"
											max="100"
											step="1"
											inputmode="numeric"
										/>
									</label>
								</div>
								<p class="text-sm text-neutral-400">
									Resolution, focus, brightness, and contrast are saved with the camera. They are not sent to the device yet. Leave a number blank to clear it.
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
									ROI
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
									Current model
									<span class="font-mono text-neutral-100">{modelActive}</span>
									{#if modelLoaded}
										<span class="text-neutral-400">loaded</span>
									{:else}
										<span class="text-amber-200">not loaded</span>
									{/if}
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
										No .pt or .onnx files in vision/models. Detection uses best.pt when that file is present.
									{:else}
										Choose a .pt or .onnx file in vision/models. A missing file, or no selection, uses best.pt.
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
</style>
