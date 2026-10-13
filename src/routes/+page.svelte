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

const WATCH_MS = 8000;

let barcode = "JOB-12345";
let result: InspectView | null = null;
let loading = false;
let cameraIndex = 0;
let feedOn = true;
let snapshot: SnapshotView | null = null;
let watchTimer: ReturnType<typeof setInterval> | null = null;
let watchInFlight = false;
let pageAlive = true;

function stopWatch() {
	if (watchTimer !== null) {
		clearInterval(watchTimer);
		watchTimer = null;
	}
}

function startWatch() {
	if (snapshot) return;
	stopWatch();
	void watchFeed();
	watchTimer = setInterval(() => {
		void watchFeed();
	}, WATCH_MS);
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

onMount(() => {
	if (feedOn) startWatch();
});

onDestroy(() => {
	pageAlive = false;
	stopWatch();
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
}

async function runInspect() {
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

<div class="flex min-h-screen flex-col bg-slate-950 text-slate-100">
	<header class="flex flex-wrap items-center justify-between gap-3 border-b border-slate-800 bg-slate-900 px-4 py-2">
		<div class="flex items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">LS-Vision</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-slate-500">Operator</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<p
				class="inline-flex min-h-9 items-center gap-2 border px-2.5 text-xs font-semibold uppercase tracking-wide {snapshot
					? 'border-amber-700/80 bg-amber-950 text-amber-200'
					: feedOn
						? 'border-slate-600 bg-slate-800 text-slate-100'
						: 'border-slate-700 bg-slate-900 text-slate-400'}"
			>
				<span
					class="h-2 w-2 shrink-0 {snapshot
						? 'bg-amber-400'
						: feedOn
							? 'bg-green-400'
							: 'bg-slate-500'}"
					aria-hidden="true"
				></span>
				{snapshot ? "Watch stopped" : feedOn ? "Watching · 8s" : "Watch paused"}
			</p>
			<a
				href="/settings"
				aria-label="Settings"
				class="inline-flex h-11 w-11 items-center justify-center border border-slate-600 bg-slate-800 text-slate-100"
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
				class="inline-flex min-h-11 items-center border border-slate-600 bg-slate-800 px-3 text-sm font-semibold text-slate-100"
				on:click={startSidecar}
			>
				Start Vision Sidecar
			</button>
		</div>
	</header>

	<div class="flex flex-wrap items-end gap-2 border-b border-slate-800 bg-slate-900 px-4 py-2">
		<label class="flex min-w-56 flex-1 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-slate-400">
			Barcode
			<input
				class="w-full border border-slate-600 bg-slate-950 px-3 py-2 font-mono text-lg font-medium normal-case tracking-normal text-slate-100"
				bind:value={barcode}
			/>
		</label>
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-slate-400">
			Camera
			<select
				class="min-h-11 border border-slate-600 bg-slate-950 px-3 font-mono text-base normal-case tracking-normal text-slate-100"
				bind:value={cameraIndex}
			>
				<option value={0}>0</option>
				<option value={1}>1</option>
			</select>
		</label>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-slate-200 bg-slate-100 px-4 text-sm font-semibold text-slate-950 disabled:border-slate-700 disabled:bg-slate-800 disabled:text-slate-500"
			on:click={runInspect}
			disabled={loading}
		>
			{loading ? "Inspecting..." : "Inspect"}
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-slate-200 bg-slate-100 px-4 text-sm font-semibold text-slate-950 disabled:border-slate-700 disabled:bg-slate-800 disabled:text-slate-500"
			on:click={decodeAndInspect}
			disabled={loading}
		>
			Decode QR / Barcode
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-slate-600 bg-slate-800 px-4 text-sm font-semibold text-slate-100 disabled:text-slate-500"
			on:click={testSampleImage}
			disabled={loading}
		>
			Test sample image
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-slate-600 bg-slate-800 px-4 text-sm font-semibold text-slate-100"
			on:click={toggleFeed}
		>
			{feedOn ? "Stop feed" : "Start feed"}
		</button>
	</div>

	<div class="grid min-h-0 flex-1 gap-3 p-3 lg:grid-cols-[minmax(0,1.7fr)_minmax(22rem,28rem)]">
		<section class="flex min-h-[28rem] min-w-0 flex-col border border-slate-700 bg-black">
			<div class="flex items-center justify-between border-b border-slate-800 px-3 py-1.5">
				<h2 class="text-[11px] font-semibold uppercase tracking-widest text-slate-500">
					{snapshot ? "Snapshot" : "Camera"}
				</h2>
				{#if snapshot}
					<p class="text-xs text-amber-200">Automatic watch is stopped</p>
				{/if}
			</div>
			<div class="relative min-h-0 flex-1">
				{#if snapshot}
					<img
						src={snapshot.image}
						alt="Snapshot of kit {snapshot.kitCode}"
						class="h-full max-h-[calc(100vh-11rem)] w-full object-contain"
					/>
					<div
						class="absolute inset-x-0 bottom-0 flex flex-wrap items-end justify-between gap-3 bg-gradient-to-t from-slate-950 via-slate-950/85 to-transparent px-4 pt-16 pb-3"
					>
						<div>
							<p class="font-mono text-3xl font-semibold leading-none text-white">{snapshot.kitCode}</p>
							<p class="mt-1 text-sm text-slate-300">Taken {snapshot.takenAt}</p>
						</div>
						<button
							type="button"
							class="inline-flex min-h-11 items-center border border-slate-200 bg-slate-100 px-4 text-sm font-semibold text-slate-950"
							on:click={resumeLiveFeed}
						>
							Resume live feed
						</button>
					</div>
				{:else if feedOn}
					<img
						src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}"
						alt="Camera feed"
						class="h-full max-h-[calc(100vh-11rem)] w-full object-contain"
					/>
				{:else}
					<div class="flex h-full min-h-[24rem] items-center justify-center text-lg text-slate-500">
						Feed stopped
					</div>
				{/if}
			</div>
		</section>

		<section class="flex min-w-0 flex-col border border-slate-700 bg-slate-900">
			<h2 class="border-b border-slate-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-widest text-slate-500">
				Result
			</h2>
			{#if result}
				<p
					class="px-4 py-5 text-5xl font-black uppercase leading-none tracking-widest {result.status ===
					'pass'
						? 'bg-green-500 text-slate-950'
						: 'bg-red-600 text-white'}"
				>
					{result.status}
				</p>
				<div class="flex flex-col gap-3 p-4">
					{#if result.kitCode}
						<p class="font-mono text-2xl font-semibold tracking-tight">{result.kitCode}</p>
					{/if}
					{#if result.des}
						<p class="text-base text-slate-300">{result.des}</p>
					{/if}
					{#if result.message}
						<p class="text-base text-slate-200">{result.message}</p>
					{/if}
					{#if result.parts?.length}
						<table class="w-full border-collapse text-left text-sm">
							<thead>
								<tr class="border-b border-slate-600 text-[11px] uppercase tracking-widest text-slate-400">
									<th class="py-2 pr-3 font-semibold">IC</th>
									<th class="py-2 pr-3 font-semibold">Description</th>
									<th class="py-2 text-right font-semibold">Qty</th>
								</tr>
							</thead>
							<tbody>
								{#each result.parts as part, i (i)}
									<tr class="border-b border-slate-800">
										<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
										<td class="py-2 pr-3 text-slate-200">{part.des}</td>
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
							class="w-full border border-slate-700 bg-black"
						/>
					{/if}
					{#if result.info}
						<p class="text-sm text-slate-400">{result.info}</p>
					{/if}
					{#if result.kitsFound != null || result.expectedKitCount != null}
						<p class="text-sm text-slate-400">
							Info only: {result.kitsFound ?? "—"} kit(s) found, {result.expectedKitCount ?? "—"} expected
						</p>
					{/if}
				</div>
			{:else}
				<div class="px-4 py-8">
					<p class="text-lg font-medium text-slate-300">No result yet</p>
					<p class="mt-1 text-sm text-slate-500">Decode a barcode or press Inspect.</p>
				</div>
			{/if}
		</section>
	</div>
</div>
