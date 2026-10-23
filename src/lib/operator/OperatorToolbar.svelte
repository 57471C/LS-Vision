<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { useOperator } from "$lib/stores/operator";

const {
	beginSetBarcodeRoi,
	beginSetRoi,
	camerasMissing,
	clearBarcodeRoi,
	clearDetectRoi,
	decodeAndInspect,
	openErp,
	operator,
	runInspect,
	saveCameraIndex,
	setDetectSlot,
	toggleFeed,
} = useOperator();

function detectSlotChanged(event: Event) {
	const target = event.currentTarget;
	if (!(target instanceof HTMLSelectElement)) return;
	setDetectSlot(Number(target.value));
}

function cameraChanged(event: Event) {
	const target = event.currentTarget;
	if (!(target instanceof HTMLSelectElement)) return;
	void saveCameraIndex(Number(target.value));
}
</script>

<form
	class="flex shrink-0 flex-wrap items-end gap-2 border-b border-neutral-800 bg-neutral-900 px-4 py-2"
	on:submit|preventDefault={runInspect}
>
	<label class="flex min-w-56 flex-1 flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
		{$operator.bomSource === "api" ? "Job card number" : "Barcode"}
		<input
			class="w-full border border-neutral-600 bg-neutral-950 px-3 py-2 font-mono text-lg font-medium normal-case tracking-normal text-neutral-100"
			inputmode={$operator.bomSource === "api" ? "numeric" : undefined}
			bind:value={$operator.barcode}
		/>
	</label>
	{#if $operator.admin}
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Camera
			<select
				class="min-h-11 border border-neutral-600 bg-neutral-950 px-3 font-mono text-base normal-case tracking-normal text-neutral-100"
				bind:value={$operator.cameraIndex}
				on:change={cameraChanged}
			>
				{#if $camerasMissing}
					<option value={$operator.cameraIndex}>No cameras found</option>
				{:else if $operator.cameraIndexes.length > 0}
					{#each $operator.cameraIndexes as index (index)}
						<option value={index}>{index}</option>
					{/each}
				{:else}
					<option value={$operator.cameraIndex}>{$operator.cameraIndex}</option>
				{/if}
			</select>
		</label>
	{/if}
	<button
		type="button"
		class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
		on:click={decodeAndInspect}
		disabled={$operator.loading}
	>
		{$operator.testMode ? "Use typed code" : "Decode QR / Barcode"}
	</button>
	<button
		type="button"
		class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:cursor-not-allowed disabled:opacity-50"
		on:click={toggleFeed}
		disabled={$operator.feedBusy}
		aria-busy={$operator.feedBusy}
	>
		{$operator.feedBusy ? "Please wait" : $operator.feedOn ? "Stop feed" : "Start feed"}
	</button>
	{#if $operator.admin}
		<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Slot
			<select
				class="min-h-11 border border-neutral-600 bg-neutral-950 px-3 font-mono text-base normal-case tracking-normal text-neutral-100"
				value={String($operator.detectSlot || "")}
				on:change={detectSlotChanged}
			>
				<option value=""></option>
				<option value="1">1</option>
				<option value="2">2</option>
				<option value="3">3</option>
				<option value="4">4</option>
			</select>
		</label>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
			on:click={beginSetRoi}
			disabled={$operator.savingRoi}
		>
			{$operator.drawing ? "Cancel" : "Set ROI"}
		</button>
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
			on:click={clearDetectRoi}
			disabled={$operator.savingRoi ||
				$operator.detectSlot < 1 ||
				!$operator.detectRois[$operator.detectSlot - 1]}
		>
			Clear ROI
		</button>
		{#if !$operator.snapshot}
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={beginSetBarcodeRoi}
				disabled={$operator.savingBarcodeRoi}
			>
				{$operator.barcodeDrawing ? "Cancel" : "Set barcode ROI"}
			</button>
			<button
				type="button"
				class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
				on:click={clearBarcodeRoi}
				disabled={$operator.savingBarcodeRoi || !$operator.barcodeRoi}
			>
				Clear barcode ROI
			</button>
		{/if}
	{/if}
	{#if $operator.erpUrl}
		<button
			type="button"
			class="inline-flex min-h-11 items-center border border-amber-300 bg-amber-400 px-4 text-sm font-semibold text-neutral-950"
			on:click={openErp}
		>
			{$operator.erpName || "ERP"}
		</button>
	{/if}
</form>
