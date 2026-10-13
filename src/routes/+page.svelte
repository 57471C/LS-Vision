<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";

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

let barcode = "JOB-12345";
let result: InspectView | null = null;
let loading = false;
let cameraIndex = 0;
let feedOn = true;

function toggleFeed() {
	feedOn = !feedOn;
}

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
	try {
		const res = await fetch(
			`http://127.0.0.1:8008/camera/decode?index=${cameraIndex}`,
		);
		const decoded = await res.json();

		if (decoded.status === "pass" && decoded.codes?.length) {
			barcode = decoded.codes[0].data;
			await runInspect();
		} else {
			result = {
				status: "fail",
				message: decoded.message || "No code detected",
			};
		}
	} catch (err: unknown) {
		result = { status: "fail", message: errorMessage(err, "Decode failed") };
	}
}
</script>

<div class="min-h-screen bg-neutral-100 text-neutral-950">
	<div class="mx-auto flex max-w-3xl flex-col gap-6 p-6">
		<div class="flex flex-wrap items-center justify-between gap-4">
			<h1 class="text-2xl font-black tracking-tight">LS-Vision</h1>
			<div class="flex items-center gap-3">
				<a
					href="/settings"
					aria-label="Settings"
					class="inline-flex h-12 w-12 items-center justify-center border-2 border-neutral-950 bg-white"
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
						<circle cx="12" cy="12" r="3" />
						<path
							d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9c.3.6.9 1.1 1.5 1.1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1.1z"
						/>
					</svg>
				</a>
				<button
					type="button"
					class="border-2 border-neutral-950 bg-white px-4 py-3 text-base font-bold"
					on:click={startSidecar}
				>
					Start Vision Sidecar
				</button>
			</div>
		</div>

		<div class="flex flex-wrap items-end gap-3">
			<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
				Barcode
				<input
					class="w-64 border-2 border-neutral-950 bg-white px-3 py-3 text-lg font-semibold normal-case tracking-normal"
					bind:value={barcode}
				/>
			</label>
			<button
				type="button"
				class="border-2 border-neutral-950 bg-neutral-950 px-5 py-3 text-lg font-bold text-white disabled:bg-neutral-400"
				on:click={runInspect}
				disabled={loading}
			>
				{loading ? "Inspecting..." : "Inspect"}
			</button>
		</div>

		<div class="flex flex-col gap-3">
			<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
				Camera
				<select
					class="w-48 border-2 border-neutral-950 bg-white px-3 py-3 text-lg font-semibold normal-case tracking-normal"
					bind:value={cameraIndex}
				>
					<option value={0}>0 (default)</option>
					<option value={1}>1</option>
				</select>
			</label>
			<div class="flex items-start gap-3">
				<div class="min-w-0 max-w-xl flex-1">
					{#if feedOn}
						<img
							src="http://127.0.0.1:8008/camera/stream?index={cameraIndex}"
							alt="camera"
							class="w-full border-4 border-neutral-950 bg-neutral-200"
						/>
					{:else}
						<div class="aspect-video w-full border-4 border-neutral-950 bg-neutral-200"></div>
					{/if}
				</div>
				<button
					type="button"
					class="shrink-0 border-2 border-neutral-950 bg-white px-4 py-3 text-base font-bold"
					on:click={toggleFeed}
				>
					{feedOn ? "Stop feed" : "Start feed"}
				</button>
			</div>
			<button
				type="button"
				class="w-fit border-4 border-neutral-950 bg-neutral-950 px-6 py-4 text-xl font-black text-white disabled:bg-neutral-400"
				on:click={decodeAndInspect}
				disabled={loading}
			>
				Decode QR / Barcode
			</button>
			<button
				type="button"
				class="w-fit border-2 border-neutral-950 bg-white px-5 py-3 text-lg font-bold disabled:bg-neutral-400"
				on:click={testSampleImage}
				disabled={loading}
			>
				Test sample image
			</button>
		</div>

		{#if result}
			<section class="overflow-hidden border-4 border-neutral-950 bg-white">
				<p
					class="px-5 py-6 text-6xl font-black uppercase leading-none tracking-wide text-white {result.status ===
					'pass'
						? 'bg-green-700'
						: 'bg-red-700'}"
				>
					{result.status}
				</p>
				<div class="flex flex-col gap-3 p-5">
					{#if result.kitCode}
						<p class="text-3xl font-black tracking-tight">Kit {result.kitCode}</p>
					{/if}
					{#if result.des}
  <p class="text-lg text-neutral-700">{result.des}</p>
{/if}
					{#if result.message}
						<p class="text-lg">{result.message}</p>
					{/if}
					{#if result.parts?.length}
						<table class="w-full border-collapse text-left text-lg">
							<thead>
								<tr class="border-b-2 border-neutral-950 text-sm uppercase tracking-wide">
									<th class="py-2 pr-3 font-bold">IC</th>
									<th class="py-2 pr-3 font-bold">Description</th>
									<th class="py-2 text-right font-bold">Qty</th>
								</tr>
							</thead>
							<tbody>
								{#each result.parts as part, i (i)}
									<tr class="border-b border-neutral-300">
										<td class="py-2 pr-3 font-mono font-semibold">{part.ic}</td>
										<td class="py-2 pr-3">{part.des}</td>
										<td class="py-2 text-right font-bold">{part.qty}</td>
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
						<p class="text-sm text-neutral-600">{result.info}</p>
					{/if}
					{#if result.kitsFound != null || result.expectedKitCount != null}
						<p class="text-sm text-neutral-600">
							Info only: {result.kitsFound ?? "—"} kit(s) found, {result.expectedKitCount ?? "—"} expected
						</p>
					{/if}
				</div>
			</section>
		{/if}
	</div>
</div>
