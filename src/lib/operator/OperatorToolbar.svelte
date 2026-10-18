<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

type DetectRoi = {
	x: number;
	y: number;
	w: number;
	h: number;
};

type BarcodeRoi = {
	x: number;
	y: number;
	width: number;
	height: number;
};

export let barcode = "";
export let cameraIndex = 0;
export let bomSource: "json" | "mssql" | "api" = "json";
export let admin = false;
export let camerasMissing = false;
export let cameraIndexes: number[] = [];
export let loading = false;
export let testMode = false;
export let feedBusy = false;
export let feedOn = false;
export let drawing = false;
export let savingRoi = false;
export let detectRoi: DetectRoi | null = null;
export let snapshot: object | null = null;
export let barcodeDrawing = false;
export let savingBarcodeRoi = false;
export let barcodeRoi: BarcodeRoi | null = null;
export let erpUrl = "";
export let erpName = "";
export let onInspect: () => void | Promise<void> = () => {};
export let onCameraChange: (index: number) => void | Promise<void> = () => {};
export let onDecode: () => void | Promise<void> = () => {};
export let onToggleFeed: () => void = () => {};
export let onBeginSetRoi: () => void = () => {};
export let onClearDetectRoi: () => void = () => {};
export let onBeginSetBarcodeRoi: () => void = () => {};
export let onClearBarcodeRoi: () => void = () => {};
export let onOpenErp: () => void | Promise<void> = () => {};

function cameraChanged(event: Event) {
	const target = event.currentTarget;
	if (!(target instanceof HTMLSelectElement)) return;
	void onCameraChange(Number(target.value));
}
</script>

<form
	class="flex shrink-0 flex-wrap items-end gap-2 border-b border-neutral-800 bg-neutral-900 px-4 py-2"
	on:submit|preventDefault={onInspect}
>
	<label class="flex min-w-56 flex-1 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
		{bomSource === "api" ? "Job card number" : "Barcode"}
		<input
			class="w-full border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-lg font-medium normal-case tracking-normal text-neutral-100"
			inputmode={bomSource === "api" ? "numeric" : undefined}
			bind:value={barcode}
		/>
	</label>
	{#if admin}
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Camera
			<select
				class="min-h-11 border border-neutral-600 bg-neutral-950 px-3 font-mono text-base normal-case tracking-normal text-neutral-100"
				bind:value={cameraIndex}
				on:change={cameraChanged}
			>
				{#if camerasMissing}
					<option value={cameraIndex}>No cameras found</option>
				{:else if cameraIndexes.length > 0}
					{#each cameraIndexes as index (index)}
						<option value={index}>{index}</option>
					{/each}
				{:else}
					<option value={cameraIndex}>{cameraIndex}</option>
				{/if}
			</select>
		</label>
	{/if}
	<button
		type="button"
		class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
		on:click={onDecode}
		disabled={loading}
	>
		{testMode ? "Use typed code" : "Decode QR / Barcode"}
	</button>
	<button
		type="button"
		class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:cursor-not-allowed disabled:opacity-50"
		on:click={onToggleFeed}
		disabled={feedBusy}
		aria-busy={feedBusy}
	>
		{feedBusy ? "Please wait" : feedOn ? "Stop feed" : "Start feed"}
	</button>
	{#if admin}
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
			on:click={onBeginSetRoi}
			disabled={savingRoi}
		>
			{drawing ? "Cancel" : "Set ROI"}
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
			on:click={onClearDetectRoi}
			disabled={savingRoi || !detectRoi}
		>
			Clear ROI
		</button>
		{#if !snapshot}
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={onBeginSetBarcodeRoi}
				disabled={savingBarcodeRoi}
			>
				{barcodeDrawing ? "Cancel" : "Set barcode ROI"}
			</button>
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={onClearBarcodeRoi}
				disabled={savingBarcodeRoi || !barcodeRoi}
			>
				Clear barcode ROI
			</button>
		{/if}
	{/if}
	{#if erpUrl}
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-amber-300 bg-amber-400 px-4 text-sm font-semibold text-neutral-950"
			on:click={onOpenErp}
		>
			{erpName || "ERP"}
		</button>
	{/if}
</form>
