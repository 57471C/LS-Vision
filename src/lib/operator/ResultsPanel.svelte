<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { useOperator } from "$lib/stores/operator";

const { chooseAndDetect, operator } = useOperator();

type Part = { ic: string; des: string; qty: number };

type InspectView = {
	status: string;
	message?: string;
	kitCode?: string;
	jobCard?: string;
	des?: string;
	parts?: Part[];
	kitsFound?: number;
	expectedKitCount?: number;
	info?: string;
};

type BomView = {
	kitCode: string;
	des?: string;
	parts: Part[];
};

type Detection = {
	ic: string;
	confidence: number;
	bbox: number[];
};

type DetectedCount = {
	ic: string;
	count: number;
};

function detectionCounts(items: Detection[]): Record<string, number> {
	const counts: Record<string, number> = {};
	for (const item of items) {
		if (!item.ic) continue;
		counts[item.ic] = (counts[item.ic] ?? 0) + 1;
	}
	return counts;
}

function isLoadedBom(view: InspectView | null, bom: BomView | null): boolean {
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
</script>

<section class="flex min-h-0 min-w-0 flex-col overflow-hidden border border-neutral-700 bg-neutral-900">
	<h2 class="shrink-0 border-b border-neutral-800 px-3 py-1.5 text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
		Result
	</h2>
	{#if $operator.result}
		<p
			class="shrink-0 px-4 py-4 text-4xl font-black uppercase leading-none tracking-widest {$operator.result.status ===
			'pass'
				? 'bg-green-500 text-neutral-950'
				: 'bg-red-600 text-white'}"
		>
			{$operator.result.status}
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
			{#if $operator.result.des}
				<p class="text-base text-neutral-300">{$operator.result.des}</p>
			{/if}
			{#if $operator.result.message}
				<p class="text-base text-neutral-200">{$operator.result.message}</p>
			{/if}
			{#if $operator.result.parts?.length}
				{@const detectFailed =
					$operator.detectState === "done" && !!$operator.detectMessage && $operator.detections.length === 0}
				{@const showSeen =
					$operator.detectState !== "idle" && isLoadedBom($operator.result, $operator.bom) && !detectFailed}
				{@const counts = detectionCounts($operator.detections)}
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
						{#each $operator.result.parts as part, i (i)}
							<tr class="border-b border-neutral-800">
								<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
								<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
								<td class="py-2 text-right font-semibold">{part.qty}</td>
								{#if showSeen}
									<td
										class="py-2 text-right font-semibold {$operator.detectState === 'running'
											? 'text-neutral-500'
											: (counts[part.ic] ?? 0) === part.qty
												? 'text-green-400'
												: 'text-red-400'}"
									>
										{$operator.detectState === "running" ? "…" : (counts[part.ic] ?? 0)}
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
			{#if $operator.detectState === "running"}
				<p class="text-sm text-neutral-400">Detecting parts...</p>
			{/if}
			{#if $operator.detectState === "done"}
				{@const extras = partsOutsideBom($operator.detections, $operator.bom?.parts ?? null)}
				{#if extras.length}
					<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						{$operator.bom ? "Not in BoM" : "Detected parts"}
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
			{#if $operator.detectMessage}
				<p class="text-sm text-red-300">{$operator.detectMessage}</p>
			{/if}
			{#if $operator.result.info}
				<p class="text-sm text-neutral-400">{$operator.result.info}</p>
			{/if}
			{#if $operator.result.kitsFound != null || $operator.result.expectedKitCount != null}
				<p class="text-sm text-neutral-400">
					Info only: {$operator.result.kitsFound ?? "—"} kit(s) found, {$operator.result.expectedKitCount ?? "—"} expected
				</p>
			{/if}
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
