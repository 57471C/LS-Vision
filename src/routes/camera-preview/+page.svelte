<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { onMount } from "svelte";

const PREVIEW_CHANNEL = "ls-vision-camera-preview";

let index = 0;
let epoch = 0;
let failed = false;
let ignoreStreamErrorUntil = 0;

function readIndex(value: unknown): number | null {
	const next = Number(value);
	if (!Number.isInteger(next) || next < 0 || next > 32) return null;
	return next;
}

function useIndex(next: number) {
	if (next === index) return;
	index = next;
	epoch += 1;
	failed = false;
	ignoreStreamErrorUntil = Date.now() + 1000;
	void retitle(next);
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

function onFrameError() {
	if (Date.now() < ignoreStreamErrorUntil) return;
	failed = true;
}

function onFrameLoad() {
	failed = false;
}

onMount(() => {
	const params = new URLSearchParams(window.location.search);
	const initial = readIndex(params.get("index"));
	if (initial !== null) index = initial;
	void retitle(index);
	const channel = new BroadcastChannel(PREVIEW_CHANNEL);
	channel.onmessage = (event) => {
		const next = readIndex(event.data?.index);
		if (next !== null) useIndex(next);
	};
	return () => channel.close();
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
		<p class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Preview</p>
	</header>
	<div class="relative min-h-0 flex-1 bg-black">
		<img
			class="h-full w-full object-contain"
			alt="Camera {index} preview"
			src="http://127.0.0.1:8008/camera/stream?index={index}&feed={epoch}"
			on:error={onFrameError}
			on:load={onFrameLoad}
		/>
		{#if failed}
			<p class="absolute inset-0 flex items-center justify-center px-6 text-center text-sm text-neutral-300">
				The camera stream is not available. Start the vision sidecar and check this camera index.
			</p>
		{/if}
	</div>
</div>
