<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { invoke } from "@tauri-apps/api/core";
import { onDestroy, onMount } from "svelte";

const PREVIEW_CHANNEL = "ls-vision-camera-preview";
/** About 15 fps. The next request waits until this long after the previous one started. */
const FRAME_MS = 66;

type PreviewBody = {
	status?: string;
	image?: string;
	message?: string;
	width?: number;
	height?: number;
	format?: string;
	fps?: number | null;
};

let index = 0;
let message = "";
let failed = false;
let status = "";
let painted = false;
let canvasFps = "";
let canvasEl: HTMLCanvasElement | undefined;
let generation = 0;
const paintStamps: number[] = [];

function formatStatus(body: PreviewBody, canvasRate: string): string {
	const width = body.width;
	const height = body.height;
	const format = body.format?.trim() ?? "";
	const size =
		typeof width === "number" &&
		typeof height === "number" &&
		width > 0 &&
		height > 0
			? `${width}×${height}`
			: "";
	const capture =
		typeof body.fps === "number" && Number.isFinite(body.fps) && body.fps > 0
			? `capture ${body.fps.toFixed(1)} fps`
			: "";
	const canvas = canvasRate ? `canvas ${canvasRate}` : "";
	return [size, format, capture, canvas]
		.filter((part) => part.length > 0)
		.join(" · ");
}

function readIndex(value: unknown): number | null {
	const next = Number(value);
	if (!Number.isInteger(next) || next < 0 || next > 32) return null;
	return next;
}

function sleep(ms: number): Promise<void> {
	return new Promise((resolve) => {
		setTimeout(resolve, ms);
	});
}

function notePaint(now: number) {
	paintStamps.push(now);
	while (paintStamps.length > 0 && now - paintStamps[0] > 1000) {
		paintStamps.shift();
	}
	if (paintStamps.length < 2) {
		canvasFps = "";
		return;
	}
	const span = paintStamps[paintStamps.length - 1] - paintStamps[0];
	if (span < 200) {
		canvasFps = "";
		return;
	}
	canvasFps = `${(((paintStamps.length - 1) * 1000) / span).toFixed(1)} fps`;
}

async function drawFrame(url: string): Promise<boolean> {
	const bitmap = await createImageBitmap(await (await fetch(url)).blob());
	try {
		const node = canvasEl;
		if (!node) return false;
		if (node.width !== bitmap.width || node.height !== bitmap.height) {
			node.width = bitmap.width;
			node.height = bitmap.height;
		}
		const context = node.getContext("2d", { alpha: false });
		if (!context) return false;
		context.drawImage(bitmap, 0, 0);
		painted = true;
		return true;
	} finally {
		bitmap.close();
	}
}

function useIndex(next: number) {
	if (next === index) return;
	index = next;
	message = "";
	failed = false;
	status = "";
	painted = false;
	canvasFps = "";
	paintStamps.length = 0;
	void retitle(next);
	generation += 1;
	void runPreview(generation);
}

async function retitle(next: number) {
	if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
		return;
	try {
		const { getCurrentWindow } = await import("@tauri-apps/api/window");
		await getCurrentWindow().setTitle(`Camera ${next}`);
	} catch {
		// The page heading still shows the index when the window title cannot change.
	}
}

async function runPreview(gen: number) {
	while (gen === generation) {
		const started = performance.now();
		const current = index;
		try {
			const body = await invoke<PreviewBody>("capture_preview", {
				index: current,
			});
			if (gen !== generation || current !== index) return;
			if (body.status === "fail" || !body.image) {
				failed = true;
				status = "";
				message = body.message || "The camera did not open.";
				await sleep(400);
				continue;
			}
			const drew = await drawFrame(body.image);
			if (gen !== generation || current !== index) return;
			if (drew) notePaint(performance.now());
			status = formatStatus(body, canvasFps);
			message = "";
			failed = false;
		} catch (err: unknown) {
			if (gen !== generation || current !== index) return;
			failed = true;
			status = "";
			message =
				typeof err === "string" && err.trim()
					? err
					: "The camera did not open.";
			await sleep(400);
			continue;
		}
		const remain = FRAME_MS - (performance.now() - started);
		if (gen === generation && remain > 0) await sleep(remain);
	}
}

onMount(() => {
	const params = new URLSearchParams(window.location.search);
	const initial = readIndex(params.get("index"));
	if (initial !== null) index = initial;
	void retitle(index);
	void runPreview(generation);
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.onmessage = (event) => {
		const next = readIndex(event.data?.index);
		if (next !== null) useIndex(next);
	};
	return () => {
		generation += 1;
		channel.close();
	};
});

onDestroy(() => {
	generation += 1;
});
</script>

<svelte:head>
	<title>Camera {index}</title>
	<style>
		html {
			color-scheme: dark;
		}
	</style>
</svelte:head>

<div class="flex h-dvh flex-col overflow-hidden bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<h1 class="text-sm font-semibold tracking-tight">Camera {index}</h1>
		{#if status}
			<p class="min-w-0 truncate text-xs tabular-nums text-neutral-200">{status}</p>
		{:else}
			<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Preview</p>
		{/if}
	</header>
	<div class="relative min-h-0 flex-1 bg-black">
		<canvas
			bind:this={canvasEl}
			class="h-full w-full object-contain"
			class:invisible={!painted}
			aria-label="Camera {index} preview"
		></canvas>
		{#if failed || !painted}
			<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
				{message || "Opening camera…"}
			</p>
		{/if}
	</div>
</div>
