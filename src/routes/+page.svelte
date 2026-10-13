<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";
import { onDestroy, onMount } from "svelte";

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
};

const STREAM_MS = 8000;

let barcode = "JOB-12345";
let result: InspectView | null = null;
let loading = false;
let cameraIndex = 0;
let feedOn = true;
let snapshot: SnapshotView | null = null;
let streamLeft = STREAM_MS / 1000;
let streamTimer: ReturnType<typeof setInterval> | null = null;
let watchInFlight = false;
let pageAlive = true;
let sidecarUp = false;
let healthTimer: ReturnType<typeof setInterval> | null = null;

function stopWatch() {
	if (streamTimer !== null) {
		clearInterval(streamTimer);
		streamTimer = null;
	}
}

function startWatch() {
	if (snapshot) return;
	stopWatch();
	streamLeft = STREAM_MS / 1000;
	void watchFeed();
	streamTimer = setInterval(() => {
		if (!feedOn || snapshot) return;
		if (streamLeft <= 1) {
			streamLeft = STREAM_MS / 1000;
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
	snapshot = null;
	if (feedOn) startWatch();
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

function showSnapshot(body: WatchBody) {
	if (!body.image) return;
	const kitCode = body.kitCode || body.barcode || barcode;
	barcode = kitCode;
	snapshot = {
		kitCode,
		image: body.image,
		takenAt: new Date().toLocaleTimeString(),
	};
	result = inspectView(body);
	stopWatch();
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
		showSnapshot(body);
	} catch {
		// A missed poll leaves the last still and the last BoM result.
	} finally {
		watchInFlight = false;
	}
}

async function checkSidecar() {
	if (!pageAlive) return;
	try {
		const res = await fetch("http://127.0.0.1:8008/health");
		sidecarUp = res.ok;
	} catch {
		sidecarUp = false;
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
		result = await res.json();
	} catch (err: unknown) {
		result = { status: "fail", message: errorMessage(err, "Request failed") };
	} finally {
		loading = false;
	}
}

async function testSampleImage() {
	loading = true;
	result = null;
	try {
		const res = await fetch("http://127.0.0.1:8008/inspect/sample", {
			method: "POST",
		});
		if (!res.ok) throw new Error(await res.text());
		result = await res.json();
	} catch (err: unknown) {
		result = {
			status: "fail",
			message: errorMessage(err, "Sample test failed"),
		};
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
			showSnapshot(body);
			return;
		}
		if (body.barcode) {
			barcode = body.barcode;
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

<div class="flex h-dvh max-h-dvh flex-col overflow-hidden overscroll-none bg-neutral-950 text-neutral-100">
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
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
			on:click={testSampleImage}
			disabled={loading}
		>
			Test sample image
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
			on:click={toggleFeed}
		>
			{feedOn ? "Stop feed" : "Start feed"}
		</button>
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
						alt="Snapshot of kit {snapshot.kitCode}"
						class="absolute inset-0 h-full w-full object-contain"
					/>
					<div class="absolute left-3 top-3 bg-neutral-950/80 px-3 py-2">
						<p class="font-mono text-3xl font-semibold leading-none text-white">{snapshot.kitCode}</p>
						<p class="mt-1 text-sm text-neutral-300">Taken {snapshot.takenAt}</p>
					</div>
					<button
						type="button"
						class="absolute bottom-3 right-3 inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950"
						on:click={resumeLiveFeed}
					>
						Resume live feed
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
					{#if result.des}
						<p class="text-base text-neutral-300">{result.des}</p>
					{/if}
					{#if result.message}
						<p class="text-base text-neutral-200">{result.message}</p>
					{/if}
					{#if result.parts?.length}
						<table class="w-full border-collapse text-left text-sm">
							<thead>
								<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
									<th class="py-2 pr-3 font-semibold">IC</th>
									<th class="py-2 pr-3 font-semibold">Description</th>
									<th class="py-2 text-right font-semibold">Qty</th>
								</tr>
							</thead>
							<tbody>
								{#each result.parts as part, i (i)}
									<tr class="border-b border-neutral-800">
										<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
										<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
										<td class="py-2 text-right font-semibold">{part.qty}</td>
									</tr>
								{/each}
							</tbody>
						</table>
					{/if}
					{#if result.image}
						<img
							src={result.image}
							alt="Annotated sample"
							class="w-full border border-neutral-700 bg-black"
						/>
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
</div>
