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
};

type SnapshotView = {
	kitCode: string;
	image: string;
	takenAt: string;
	detected?: boolean;
};

type Detection = {
	// Part IC from the YOLO class name. Kit codes come from the barcode lookup.
	ic: string;
	confidence: number;
	bbox: number[];
};

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

let barcode = "JOB-12345";
let result: InspectView | null = null;
let bom: BomView | null = null;
let loading = false;
let cameraIndex = 0;
let feedOn = true;
let snapshot: SnapshotView | null = null;
let detections: Detection[] = [];
let detectMessage = "";
let detectState: "idle" | "running" | "done" = "idle";
let detectGeneration = 0;
let testMode = false;
let imagePicker: HTMLInputElement | null = null;
let imagePick: ((file: File | null) => void) | null = null;
let streamMs = DEFAULT_SCAN_SECONDS * 1000;
let streamLeft = DEFAULT_SCAN_SECONDS;
let streamTimer: ReturnType<typeof setInterval> | null = null;
let watchInFlight = false;
let pageAlive = true;
let sidecarUp = false;
let healthTimer: ReturnType<typeof setInterval> | null = null;
let erpName = "";
let erpUrl = "";
let erpOpen = false;
let iframeFailed = false;
let erpChecking = false;
let erpAttempt = 0;
let erpLoadedUrl = "";
let erpCloseButton: HTMLButtonElement | null = null;

function stopWatch() {
	if (streamTimer !== null) {
		clearInterval(streamTimer);
		streamTimer = null;
	}
}

function startWatch() {
	if (snapshot) return;
	stopWatch();
	streamLeft = streamMs / 1000;
	void watchFeed();
	streamTimer = setInterval(() => {
		if (!feedOn || snapshot) return;
		if (streamLeft <= 1) {
			streamLeft = streamMs / 1000;
			void watchFeed();
		} else {
			streamLeft -= 1;
		}
	}, 1000);
}

function toggleFeed() {
	feedOn = !feedOn;
	if (feedOn) startWatch();
	else stopWatch();
}

function resumeLiveFeed() {
	detectGeneration += 1;
	snapshot = null;
	detections = [];
	detectMessage = "";
	detectState = "idle";
	if (feedOn) startWatch();
}

function holdStill(image: string, kitCode: string) {
	detectGeneration += 1;
	snapshot = {
		kitCode,
		image,
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
			annotated_image_base64?: unknown;
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
		const encoded = payload.annotated_image_base64;
		if (typeof encoded === "string" && encoded && snapshot) {
			snapshot = {
				...snapshot,
				image: detectedImageSrc(encoded),
				detected: true,
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
	barcode = kitCode;
	result = inspectView(body);
	storeBom(result);
	const generation = holdStill(body.image, kitCode);
	await runDetect(generation, { image: body.image });
}

async function watchFeed() {
	if (!feedOn || snapshot || watchInFlight || loading) return;
	watchInFlight = true;
	const index = cameraIndex;
	try {
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

async function checkSidecar() {
	if (!pageAlive) return;
	const wasUp = sidecarUp;
	try {
		const res = await fetch("http://127.0.0.1:8008/health");
		sidecarUp = res.ok;
	} catch {
		sidecarUp = false;
	}
	if (pageAlive && sidecarUp && !wasUp) {
		void loadErp();
		void loadScan();
		void loadTestMode();
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
	void checkSidecar();
	healthTimer = setInterval(() => {
		void checkSidecar();
	}, 3000);
	if (feedOn) startWatch();
});

onDestroy(() => {
	pageAlive = false;
	stopWatch();
	if (healthTimer !== null) {
		clearInterval(healthTimer);
		healthTimer = null;
	}
});

function errorMessage(err: unknown, fallback: string): string {
	return err instanceof Error && err.message ? err.message : fallback;
}

async function startSidecar() {
	try {
		const res = await invoke("start_vision_sidecar");
		console.log(res);
	} catch (err) {
		console.error(err);
	}
	void checkSidecar();
	setTimeout(() => {
		void checkSidecar();
	}, 1500);
}

async function runInspect() {
	if (loading) return;
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
		result = { status: "fail", message: errorMessage(err, "Request failed") };
		dropBom();
	} finally {
		loading = false;
	}
}

async function decodeAndInspect() {
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
				class="inline-flex min-h-9 items-center gap-2 border px-2.5 text-xs font-semibold uppercase tracking-wide {snapshot
					? 'border-amber-700/80 bg-amber-950 text-amber-200'
					: feedOn
						? 'border-neutral-600 bg-neutral-800 text-neutral-100'
						: 'border-neutral-700 bg-neutral-900 text-neutral-400'}"
			>
				<span
					class="h-2 w-2 shrink-0 {snapshot
						? 'bg-amber-400'
						: feedOn
							? 'bg-green-400'
							: 'bg-neutral-500'}"
					aria-hidden="true"
				></span>
				{snapshot ? "Stream stopped" : feedOn ? `Streaming · ${streamLeft}s` : "Stream paused"}
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
				class="inline-flex min-h-11 items-center border px-3 text-sm font-semibold text-white {sidecarUp
					? 'border-green-700 bg-green-600'
					: 'border-red-800 bg-red-600'}"
				on:click={startSidecar}
			>
				Start Vision Sidecar
			</button>
		</div>
	</header>

	<form
		class="flex shrink-0 flex-wrap items-end gap-2 border-b border-neutral-800 bg-neutral-900 px-4 py-2"
		on:submit|preventDefault={runInspect}
	>
		<label class="flex min-w-56 flex-1 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Barcode
			<input
				class="w-full border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-lg font-medium normal-case tracking-normal text-neutral-100"
				bind:value={barcode}
			/>
		</label>
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Camera
			<select
				class="min-h-11 border border-neutral-600 bg-neutral-950 px-3 font-mono text-base normal-case tracking-normal text-neutral-100"
				bind:value={cameraIndex}
			>
				<option value={0}>0</option>
				<option value={1}>1</option>
			</select>
		</label>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
			on:click={decodeAndInspect}
			disabled={loading}
		>
			Decode QR / Barcode
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
			on:click={toggleFeed}
		>
			{feedOn ? "Stop feed" : "Start feed"}
		</button>
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
				{#if snapshot}
					<p class="text-xs text-amber-200">Automatic stream is stopped</p>
				{/if}
			</div>
			<div class="relative min-h-0 flex-1 overflow-hidden">
				{#if snapshot}
					<img
						src={snapshot.image}
						alt={snapshot.detected
							? `Detected parts for kit ${snapshot.kitCode}`
							: `Snapshot of kit ${snapshot.kitCode}`}
						class="absolute inset-0 h-full w-full object-contain"
					/>
					<div class="absolute left-3 top-3 bg-neutral-950/80 px-3 py-2">
						<p class="font-mono text-3xl font-semibold leading-none text-white">{snapshot.kitCode}</p>
						<p class="mt-1 text-sm text-neutral-300">
							Taken {snapshot.takenAt}
						</p>
					</div>
					{#if detectState === "running"}
						<p class="absolute bottom-3 left-3 bg-neutral-950/80 px-3 py-2 text-sm font-semibold text-amber-200">
							Detecting parts...
						</p>
					{/if}
					<button
						type="button"
						class="absolute bottom-3 right-3 inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950"
						on:click={resumeLiveFeed}
					>
						{feedOn ? "Resume live feed" : "Back to feed"}
					</button>
				{:else if feedOn}
					<img
						src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}"
						alt="Camera feed"
						class="absolute inset-0 h-full w-full object-contain"
					/>
				{:else}
					<div class="absolute inset-0 flex items-center justify-center text-lg text-neutral-500">
						Feed stopped
					</div>
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
				<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
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
					<p class="mt-1 text-sm text-neutral-500">Decode a barcode or press Enter.</p>
				</div>
			{/if}
		</section>
	</div>

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
