<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
// biome-ignore-all lint/correctness/noUnusedImports: Biome 2.4 does not treat Svelte template references as uses

import { partsOutsideBom } from "$lib/catalog-match";
import { panelsForSlots } from "$lib/detect-rois";
import {
	detectionCounts,
	inspectionVerdict,
	lineTally,
} from "$lib/operator/verdict";
import type { Detection, SlotDetections } from "$lib/stores/operator";
import { useOperator } from "$lib/stores/operator";

const { chooseAndDetect, chooseAndSaveReferences, operator } = useOperator();

type Part = { ic: string; des: string; qty: number };

type InspectView = {
	status: string;
	message?: string;
	kitCode?: string;
	jobCard?: string;
	des?: string;
	parts?: Part[];
	info?: string;
};

type BomView = {
	kitCode: string;
	des?: string;
	parts: Part[];
};

function isLoadedBom(view: InspectView | null, bom: BomView | null): boolean {
	return !!bom && !!view?.parts?.length && view.kitCode === bom.kitCode;
}

function tallyClass(status: "pass" | "short" | "extra"): string {
	if (status === "pass") return "text-green-400";
	if (status === "extra") return "text-amber-400";
	return "text-red-400";
}

function hideBenchNote(text: string | undefined): boolean {
	if (!text) return true;
	return text.includes("BoM loaded") || text.includes("kit(s) on bench");
}

function matchSummary(parts: Part[], items: Detection[]): string {
	const counts = detectionCounts(items);
	let matched = 0;
	for (const part of parts) {
		if ((counts[part.ic] ?? 0) >= part.qty) matched += 1;
	}
	const lines = parts.length === 1 ? "line" : "lines";
	return `${matched} of ${parts.length} BoM ${lines} match`;
}

$: counts = detectionCounts($operator.detections);
$: bomLoaded = isLoadedBom($operator.result, $operator.bom);
$: visibleSlots = panelsForSlots($operator.slotResults, bomLoaded);
$: splitSlots =
	visibleSlots.length > 0 &&
	($operator.slotResults.length > 1 ||
		$operator.slotResults.some((slot) => slot.slot !== 1));
$: detectFailed =
	$operator.detectState === "done" &&
	!!$operator.detectMessage &&
	$operator.detections.length === 0;
$: verdict = $operator.result
	? inspectionVerdict({
			status: $operator.result.status,
			bomLoaded,
			detectState: $operator.detectState,
			detectFailed,
			parts: $operator.result.parts ?? [],
			counts,
			detections: $operator.detections,
		})
	: "fail";

function slotVerdict(panel: SlotDetections): "pass" | "fail" | "incomplete" {
	const view = $operator.result;
	if (!view) return "fail";
	const slotCounts = detectionCounts(panel.detections);
	const failed =
		$operator.detectState === "done" &&
		!!$operator.detectMessage &&
		panel.detections.length === 0;
	return inspectionVerdict({
		status: view.status,
		bomLoaded,
		detectState: $operator.detectState,
		detectFailed: failed,
		parts: view.parts ?? [],
		counts: slotCounts,
		detections: panel.detections,
	});
}
</script>

<section class="flex min-h-0 min-w-0 flex-col overflow-hidden border border-neutral-700 bg-neutral-900">
	<h2 class="shrink-0 border-b border-neutral-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
		Result
	</h2>
	{#if $operator.result && !splitSlots}
		<p
			class="shrink-0 px-4 py-4 text-4xl font-black uppercase leading-none tracking-widest {verdict ===
			'pass'
				? 'bg-green-500 text-neutral-950'
				: verdict === 'incomplete'
					? 'bg-amber-500 text-neutral-950'
					: 'bg-red-600 text-white'}"
		>
			{verdict}
		</p>
		<div class="result-scroll flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto p-4">
			{#if $operator.result.kitCode}
				<p class="font-mono text-2xl font-semibold tracking-tight">{$operator.result.kitCode}</p>
			{/if}
			{#if $operator.result.jobCard}
				<p class="font-mono text-xl font-semibold tracking-tight text-neutral-100">
					<span class="mr-2 text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Job card</span>{$operator.result.jobCard}
				</p>
			{/if}
			{#if $operator.testMode && isLoadedBom($operator.result, $operator.bom)}
				<div class="flex flex-wrap gap-2">
					<button
						type="button"
						class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white"
						on:click={chooseAndDetect}
					>
						Choose image
					</button>
					<button
						type="button"
						class="inline-flex min-h-11 w-fit items-center border border-neutral-500 bg-neutral-800 px-4 text-sm font-semibold text-white"
						on:click={chooseAndSaveReferences}
					>
						Save references
					</button>
				</div>
				<p class="text-sm text-neutral-400">
					Test mode is on. Choose an image file to detect parts for this kit. Save references
					stores each detected part under its IC and keeps it on this PC. A later image counts a
					part when its match is above the saved threshold.
				</p>
			{/if}
			{#if $operator.result.des}
				<p class="text-base text-neutral-300">{$operator.result.des}</p>
			{/if}
			{#if !hideBenchNote($operator.result.message)}
				<p class="text-base text-neutral-200">{$operator.result.message}</p>
			{/if}
			{#if $operator.result.parts?.length}
				{@const showSeen =
					$operator.detectState !== "idle" && isLoadedBom($operator.result, $operator.bom) && !detectFailed}
				<table class="w-full border-collapse text-left text-sm">
					<thead>
						<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
							<th class="py-2 pr-3 font-semibold">IC</th>
							<th class="py-2 pr-3 font-semibold">Description</th>
							<th class="py-2 text-right font-semibold">Qty</th>
							{#if showSeen}
								<th class="py-2 text-right font-semibold">Seen</th>
								<th class="py-2 text-right font-semibold">Check</th>
							{/if}
						</tr>
					</thead>
					<tbody>
						{#each $operator.result.parts as part, i (i)}
							{@const seen = counts[part.ic] ?? 0}
							{@const tally = lineTally(seen, part.qty)}
							<tr class="border-b border-neutral-800">
								<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
								<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
								<td class="py-2 text-right font-semibold">{part.qty}</td>
								{#if showSeen}
									<td
										class="py-2 text-right font-semibold {$operator.detectState === 'running'
											? 'text-neutral-500'
											: seen >= part.qty
												? 'text-green-400'
												: 'text-red-400'}"
									>
										{$operator.detectState === "running" ? "…" : seen}
									</td>
									<td
										class="py-2 text-right font-semibold uppercase {$operator.detectState === 'running'
											? 'text-neutral-500'
											: tallyClass(tally)}"
									>
										{$operator.detectState === "running" ? "…" : tally}
									</td>
								{/if}
							</tr>
						{/each}
					</tbody>
				</table>
				{#if showSeen && $operator.detectState === "done"}
					<p class="text-sm text-neutral-300">{matchSummary($operator.result.parts, $operator.detections)}</p>
				{/if}
			{/if}
			{#if verdict === "incomplete" && $operator.detectState !== "running"}
				<p class="text-sm text-neutral-400">Part counts are not confirmed yet.</p>
			{/if}
			{#if $operator.detectState === "running"}
				<p class="text-sm text-neutral-400">Detecting parts...</p>
			{/if}
			{#if $operator.detectState === "done"}
				{@const extras = partsOutsideBom(
					$operator.detections,
					$operator.bom?.parts ?? null,
					$operator.partDescriptions,
				)}
				{#if extras.length}
					{@const showDes = !!$operator.bom && extras.some((row) => row.des)}
					<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						{$operator.bom ? "Not in BoM" : "Detected parts"}
					</h3>
					<table class="w-full border-collapse text-left text-sm">
						<thead>
							<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
								<th class="py-2 pr-3 font-semibold">IC</th>
								{#if showDes}
									<th class="py-2 pr-3 font-semibold">Description</th>
								{/if}
								<th class="py-2 text-right font-semibold">Count</th>
							</tr>
						</thead>
						<tbody>
							{#each extras as row (`${row.ic}|${row.des}`)}
								<tr class="border-b border-neutral-800">
									<td class="py-2 pr-3 font-mono font-medium">{row.ic}</td>
									{#if showDes}
										<td class="py-2 pr-3 text-neutral-200">{row.des}</td>
									{/if}
									<td class="py-2 text-right font-semibold">{row.count}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				{/if}
			{/if}
			{#if $operator.detectMessage}
				<p class="text-sm text-red-300">{$operator.detectMessage}</p>
			{/if}
			{#if !hideBenchNote($operator.result.info)}
				<p class="text-sm text-neutral-400">{$operator.result.info}</p>
			{/if}
		</div>
	{:else if $operator.result}
		<div class="result-scroll flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-4">
			{#each visibleSlots as panel, panelIndex (panel.slot)}
				{@const slotCounts = detectionCounts(panel.detections)}
				{@const slotFailed =
					$operator.detectState === "done" &&
					!!$operator.detectMessage &&
					panel.detections.length === 0}
				{@const slotBanner = slotVerdict(panel)}
				{@const showSeen =
					$operator.detectState !== "idle" && bomLoaded && !slotFailed}
				<section class="flex flex-col gap-3 border border-neutral-800 bg-neutral-950 p-3">
					<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						Slot {panel.slot}
					</h3>
					<p
						class="px-4 py-4 text-4xl font-black uppercase leading-none tracking-widest {slotBanner ===
						'pass'
							? 'bg-green-500 text-neutral-950'
							: slotBanner === 'incomplete'
								? 'bg-amber-500 text-neutral-950'
								: 'bg-red-600 text-white'}"
					>
						{slotBanner}
					</p>
					{#if $operator.result.kitCode}
						<p class="font-mono text-2xl font-semibold tracking-tight">{$operator.result.kitCode}</p>
					{/if}
					{#if $operator.result.jobCard}
						<p class="font-mono text-xl font-semibold tracking-tight text-neutral-100">
							<span class="mr-2 text-[11px] font-semibold uppercase tracking-widest text-neutral-500">Job card</span>{$operator.result.jobCard}
						</p>
					{/if}
					{#if panelIndex === 0 && $operator.testMode && bomLoaded}
						<div class="flex flex-wrap gap-2">
							<button
								type="button"
								class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white"
								on:click={chooseAndDetect}
							>
								Choose image
							</button>
							<button
								type="button"
								class="inline-flex min-h-11 w-fit items-center border border-neutral-500 bg-neutral-800 px-4 text-sm font-semibold text-white"
								on:click={chooseAndSaveReferences}
							>
								Save references
							</button>
						</div>
						<p class="text-sm text-neutral-400">
							Test mode is on. Choose an image file to detect parts for this kit. Save references
							stores each detected part under its IC and keeps it on this PC. A later image counts a
					part when its match is above the saved threshold.
						</p>
					{/if}
					{#if $operator.result.des}
						<p class="text-base text-neutral-300">{$operator.result.des}</p>
					{/if}
					{#if !hideBenchNote($operator.result.message)}
						<p class="text-base text-neutral-200">{$operator.result.message}</p>
					{/if}
					{#if $operator.result.parts?.length}
						<table class="w-full border-collapse text-left text-sm">
							<thead>
								<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
									<th class="py-2 pr-3 font-semibold">IC</th>
									<th class="py-2 pr-3 font-semibold">Description</th>
									<th class="py-2 text-right font-semibold">Qty</th>
									{#if showSeen}
										<th class="py-2 text-right font-semibold">Seen</th>
										<th class="py-2 text-right font-semibold">Check</th>
									{/if}
								</tr>
							</thead>
							<tbody>
								{#each $operator.result.parts as part, i (i)}
									{@const seen = slotCounts[part.ic] ?? 0}
									{@const tally = lineTally(seen, part.qty)}
									<tr class="border-b border-neutral-800">
										<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
										<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
										<td class="py-2 text-right font-semibold">{part.qty}</td>
										{#if showSeen}
											<td
												class="py-2 text-right font-semibold {$operator.detectState === 'running'
													? 'text-neutral-500'
													: seen >= part.qty
														? 'text-green-400'
														: 'text-red-400'}"
											>
												{$operator.detectState === "running" ? "…" : seen}
											</td>
											<td
												class="py-2 text-right font-semibold uppercase {$operator.detectState === 'running'
													? 'text-neutral-500'
													: tallyClass(tally)}"
											>
												{$operator.detectState === "running" ? "…" : tally}
											</td>
										{/if}
									</tr>
								{/each}
							</tbody>
						</table>
						{#if showSeen && $operator.detectState === "done"}
							<p class="text-sm text-neutral-300">{matchSummary($operator.result.parts, panel.detections)}</p>
						{/if}
					{/if}
					{#if slotBanner === "incomplete" && $operator.detectState !== "running"}
						<p class="text-sm text-neutral-400">Part counts are not confirmed yet.</p>
					{/if}
					{#if $operator.detectState === "running"}
						<p class="text-sm text-neutral-400">Detecting parts...</p>
					{/if}
					{#if $operator.detectState === "done"}
						{@const extras = partsOutsideBom(
							panel.detections,
							$operator.bom?.parts ?? null,
							$operator.partDescriptions,
						)}
						{#if extras.length}
							{@const showDes = !!$operator.bom && extras.some((row) => row.des)}
							<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								{$operator.bom ? "Not in BoM" : "Detected parts"}
							</h3>
							<table class="w-full border-collapse text-left text-sm">
								<thead>
									<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
										<th class="py-2 pr-3 font-semibold">IC</th>
										{#if showDes}
											<th class="py-2 pr-3 font-semibold">Description</th>
										{/if}
										<th class="py-2 text-right font-semibold">Count</th>
									</tr>
								</thead>
								<tbody>
									{#each extras as row (`${row.ic}|${row.des}`)}
										<tr class="border-b border-neutral-800">
											<td class="py-2 pr-3 font-mono font-medium">{row.ic}</td>
											{#if showDes}
												<td class="py-2 pr-3 text-neutral-200">{row.des}</td>
											{/if}
											<td class="py-2 text-right font-semibold">{row.count}</td>
										</tr>
									{/each}
								</tbody>
							</table>
						{/if}
					{/if}
					{#if $operator.detectMessage}
						<p class="text-sm text-red-300">{$operator.detectMessage}</p>
					{/if}
					{#if !hideBenchNote($operator.result.info)}
						<p class="text-sm text-neutral-400">{$operator.result.info}</p>
					{/if}
				</section>
			{/each}
		</div>
	{:else}
		<div class="px-4 py-8">
			<p class="text-lg font-medium text-neutral-300">No result yet</p>
			<p class="mt-1 text-sm text-neutral-500">
				{$operator.testMode
					? $operator.bomSource === "api"
						? "Type a job card number, then choose an image."
						: "Type a barcode, then choose an image."
					: $operator.bomSource === "api"
						? "A decoded barcode is used as the job card number."
						: "Decode a barcode or press Enter."}
			</p>
		</div>
	{/if}
</section>

<style>
	.result-scroll {
		color-scheme: dark;
		scrollbar-color: #a3a3a3 #171717;
		scrollbar-width: auto;
	}

	.result-scroll::-webkit-scrollbar {
		width: 12px;
		height: 12px;
	}

	.result-scroll::-webkit-scrollbar-track {
		background: #171717;
	}

	.result-scroll::-webkit-scrollbar-thumb {
		background: #a3a3a3;
		border: 2px solid #171717;
	}

	.result-scroll::-webkit-scrollbar-thumb:hover {
		background: #d4d4d4;
	}

	.result-scroll::-webkit-scrollbar-corner {
		background: #171717;
	}
</style>
