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
}

async function watchFeed() {
	if (!feedOn || watchInFlight || loading) return;
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

<div class="min-h-screen bg-neutral-100 text-neutral-950">
	<header class="border-b-4 border-neutral-950 bg-neutral-950 text-white">
		<div class="mx-auto flex max-w-[1600px] flex-wrap items-center justify-between gap-4 px-6 py-4">
			<div>
				<p class="text-sm font-bold uppercase tracking-widest text-neutral-300">Operator</p>
				<h1 class="text-4xl font-black tracking-tight">LS-Vision</h1>
			</div>
			<div class="flex flex-wrap items-center gap-3">
				<a
					href="/settings"
					aria-label="Settings"
					class="inline-flex h-14 w-14 items-center justify-center border-2 border-white bg-white text-neutral-950"
				>
					<svg
						xmlns="http://www.w3.org/2000/svg"
						viewBox="0 0 24 24"
						class="h-7 w-7"
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
					class="inline-flex min-h-14 items-center border-2 border-white bg-white px-5 text-lg font-bold text-neutral-950"
					on:click={startSidecar}
				>
					Start Vision Sidecar
				</button>
			</div>
		</div>
	</header>

	<main class="mx-auto flex max-w-[1600px] flex-col gap-6 px-6 py-6">
		<section class="flex flex-wrap items-end gap-4 border-4 border-neutral-950 bg-white p-4">
			<label class="flex min-w-64 flex-1 flex-col gap-2 text-sm font-black uppercase tracking-widest">
				Barcode
				<input
					class="w-full border-2 border-neutral-950 bg-white px-3 py-3 text-2xl font-semibold normal-case tracking-normal"
					bind:value={barcode}
				/>
			</label>
			<button
				type="button"
				class="inline-flex min-h-14 items-center border-2 border-neutral-950 bg-neutral-950 px-5 text-lg font-bold text-white disabled:border-neutral-400 disabled:bg-neutral-400 disabled:text-neutral-950"
				on:click={runInspect}
				disabled={loading}
			>
				{loading ? "Inspecting..." : "Inspect"}
			</button>
		</section>

		<div class="grid items-stretch gap-6 lg:grid-cols-2">
			<section class="flex min-w-0 flex-col gap-4">
				<div class="flex flex-wrap items-end justify-between gap-3">
					<h2 class="text-sm font-black uppercase tracking-widest">Camera</h2>
					<label class="flex items-center gap-3 text-sm font-black uppercase tracking-widest">
						Index
						<select
							class="min-h-14 border-2 border-neutral-950 bg-white px-3 text-lg font-semibold normal-case tracking-normal"
							bind:value={cameraIndex}
						>
							<option value={0}>0 (default)</option>
							<option value={1}>1</option>
						</select>
					</label>
				</div>
				<div class="border-4 border-neutral-950 bg-neutral-200">
					{#if feedOn}
						<img
							src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}"
							alt="Camera feed"
							class="h-[22rem] w-full bg-neutral-200 object-contain"
						/>
					{:else}
						<div
							class="flex h-[22rem] w-full items-center justify-center bg-neutral-200 text-2xl font-black"
						>
							Feed stopped
						</div>
					{/if}
				</div>
				<div class="flex flex-wrap items-center gap-3">
					<button
						type="button"
						class="inline-flex min-h-14 items-center border-2 border-neutral-950 bg-white px-5 text-lg font-bold"
						on:click={toggleFeed}
					>
						{feedOn ? "Stop feed" : "Start feed"}
					</button>
					<p
						class="inline-flex min-h-10 items-center gap-2 border-2 border-neutral-950 px-3 text-sm font-black uppercase tracking-wide {feedOn
							? 'bg-white'
							: 'bg-neutral-200'}"
					>
						<span
							class="h-3 w-3 shrink-0 {feedOn ? 'bg-green-700' : 'bg-neutral-950'}"
							aria-hidden="true"
						></span>
						{feedOn ? "Watching the feed · 8s" : "Watch paused"}
					</p>
				</div>
				<div class="flex flex-wrap items-center gap-3">
					<button
						type="button"
						class="inline-flex min-h-14 items-center border-2 border-neutral-950 bg-neutral-950 px-5 text-lg font-black text-white disabled:border-neutral-400 disabled:bg-neutral-400 disabled:text-neutral-950"
						on:click={decodeAndInspect}
						disabled={loading}
					>
						Decode QR / Barcode
					</button>
					<button
						type="button"
						class="inline-flex min-h-14 items-center border-2 border-neutral-950 bg-white px-5 text-lg font-bold disabled:border-neutral-400 disabled:bg-neutral-400"
						on:click={testSampleImage}
						disabled={loading}
					>
						Test sample image
					</button>
				</div>
			</section>

			<aside class="flex min-w-0 flex-col border-4 border-neutral-950 bg-white">
				<div
					class="border-b-4 border-neutral-950 px-5 py-4 {snapshot
						? 'bg-neutral-950 text-white'
						: 'bg-neutral-100 text-neutral-950'}"
				>
					<p class="text-sm font-black uppercase tracking-widest">
						{snapshot ? "Snapshot taken" : "Snapshot"}
					</p>
					{#if snapshot}
						<p class="mt-2 text-5xl font-black leading-none tracking-tight">{snapshot.kitCode}</p>
						<p class="mt-3 text-lg font-semibold">Taken {snapshot.takenAt}</p>
					{:else}
						<p class="mt-2 text-4xl font-black leading-none tracking-tight">No snapshot yet</p>
					{/if}
				</div>
				<div class="flex flex-1 flex-col p-4">
					{#if snapshot}
						<img
							src={snapshot.image}
							alt="Snapshot of kit {snapshot.kitCode}"
							class="h-[22rem] w-full border-4 border-neutral-950 bg-neutral-200 object-contain"
						/>
					{:else}
						<div
							class="flex h-[22rem] w-full flex-col items-center justify-center gap-3 border-4 border-dashed border-neutral-950 bg-neutral-100 px-6 text-center"
						>
							<p class="text-3xl font-black">Waiting for a BoM barcode</p>
							<p class="max-w-md text-lg leading-snug">
								A still appears here when the camera reads a code that is in the BoM.
							</p>
						</div>
					{/if}
				</div>
			</aside>
		</div>

		<section class="flex flex-col gap-3">
			<h2 class="text-sm font-black uppercase tracking-widest">Result</h2>
			{#if result}
				<div class="overflow-hidden border-4 border-neutral-950 bg-white">
					<p
						class="px-5 py-6 text-6xl font-black uppercase leading-none tracking-wide text-white {result.status ===
						'pass'
							? 'bg-green-700'
							: 'bg-red-700'}"
					>
						{result.status}
					</p>
					<div class="flex flex-col gap-4 p-5">
						{#if result.kitCode}
							<p class="text-4xl font-black tracking-tight">Kit {result.kitCode}</p>
						{/if}
						{#if result.des}
							<p class="text-xl text-neutral-800">{result.des}</p>
						{/if}
						{#if result.message}
							<p class="text-xl">{result.message}</p>
						{/if}
						{#if result.parts?.length}
							<table class="w-full border-collapse text-left text-lg">
								<thead>
									<tr class="border-b-2 border-neutral-950 text-sm uppercase tracking-wide">
										<th class="py-3 pr-3 font-bold">IC</th>
										<th class="py-3 pr-3 font-bold">Description</th>
										<th class="py-3 text-right font-bold">Qty</th>
									</tr>
								</thead>
								<tbody>
									{#each result.parts as part, i (i)}
										<tr class="border-b border-neutral-300">
											<td class="py-3 pr-3 font-mono font-semibold">{part.ic}</td>
											<td class="py-3 pr-3">{part.des}</td>
											<td class="py-3 text-right font-bold">{part.qty}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{/if}
						{#if result.image}
							<img
								src={result.image}
								alt="Annotated sample"
								class="w-full border-4 border-neutral-950 bg-neutral-200"
							/>
						{/if}
						{#if result.info}
							<p class="text-base text-neutral-800">{result.info}</p>
						{/if}
						{#if result.kitsFound != null || result.expectedKitCount != null}
							<p class="text-base text-neutral-800">
								Info only: {result.kitsFound ?? "—"} kit(s) found, {result.expectedKitCount ?? "—"} expected
							</p>
						{/if}
					</div>
				</div>
			{:else}
				<div class="border-4 border-neutral-950 bg-white px-5 py-8">
					<p class="text-3xl font-black">No result yet</p>
					<p class="mt-2 text-lg">Decode a barcode or press Inspect.</p>
				</div>
			{/if}
		</section>
	</main>
</div>
