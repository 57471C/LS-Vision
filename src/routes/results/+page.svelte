<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
// biome-ignore-all lint/correctness/noUnusedImports: Biome 2.4 does not treat Svelte template references as uses

import { invoke } from "@tauri-apps/api/core";
import type ApexCharts from "apexcharts";
import type { ApexOptions } from "apexcharts";
import { onMount } from "svelte";
import {
	formatResultWhen,
	matchSummary,
	type ResultPart,
	type ResultRange,
	resultBuckets,
	type SavedResult,
} from "$lib/results";

const ADMIN_FLAG = "ls-vision-settings-open";
const RANGES: { id: ResultRange; label: string }[] = [
	{ id: "day", label: "Day" },
	{ id: "week", label: "Week" },
	{ id: "month", label: "Month" },
];

let admin = false;
let range: ResultRange = "day";
let rows: SavedResult[] = [];
let images: Record<number, string> = {};
let openId: number | null = null;
let notice = "";
let deletingId: number | null = null;
let chartEl: HTMLDivElement | null = null;
let chart: ApexCharts | null = null;

function readAdmin() {
	admin = sessionStorage.getItem(ADMIN_FLAG) === "1";
	if (!admin && deletingId !== null) deletingId = null;
}

function chartOptions(): ApexOptions {
	const buckets = resultBuckets(rows, range);
	return {
		chart: {
			type: "column",
			background: "transparent",
			toolbar: { show: false },
			fontFamily: "inherit",
			animations: { enabled: false },
		},
		theme: { mode: "dark" },
		series: [
			{ name: "Pass", data: buckets.pass },
			{ name: "Fail", data: buckets.fail },
		],
		colors: ["#22c55e", "#dc2626"],
		plotOptions: { bar: { horizontal: false, columnWidth: "55%" } },
		dataLabels: { enabled: false },
		xaxis: {
			categories: buckets.labels,
			labels: { style: { colors: "#a3a3a3" } },
		},
		yaxis: {
			labels: { style: { colors: "#a3a3a3" } },
			forceNiceScale: true,
			decimalsInFloat: 0,
		},
		grid: { borderColor: "#404040" },
		legend: { labels: { colors: "#e5e5e5" } },
		tooltip: { theme: "dark" },
	};
}

async function ensureChart() {
	if (!chartEl || chart) return;
	const apex = await import("apexcharts");
	const created = new apex.default(chartEl, chartOptions());
	chart = created;
	await created.render();
}

async function refreshChart() {
	await ensureChart();
	if (!chart) return;
	const options = chartOptions();
	await chart.updateOptions({
		series: options.series,
		xaxis: options.xaxis,
	});
}

async function loadImages(list: SavedResult[]) {
	await Promise.all(
		list.map(async (row) => {
			if (!row.hasImage || images[row.id]) return;
			try {
				const src = await invoke<string>("result_image", { id: row.id });
				if (!src) return;
				images = { ...images, [row.id]: src };
			} catch {
				// A missing file leaves the image cell empty.
			}
		}),
	);
}

async function load() {
	try {
		rows = await invoke<SavedResult[]>("list_results");
		notice = "";
		await loadImages(rows);
	} catch (err: unknown) {
		rows = [];
		const message = err instanceof Error ? err.message : String(err ?? "");
		notice = message.includes("invoke")
			? "Results are stored in the desktop app."
			: message || "Could not load results.";
	}
	try {
		await refreshChart();
	} catch {
		// The table still shows when the chart cannot start.
	}
}

function toggleRow(id: number) {
	openId = openId === id ? null : id;
}

async function removeRow(row: SavedResult) {
	if (!admin || deletingId !== null) return;
	if (!confirm("Delete this result and its image?")) return;
	deletingId = row.id;
	try {
		await invoke("delete_result", { id: row.id });
		rows = rows.filter((item) => item.id !== row.id);
		if (openId === row.id) openId = null;
		const next = { ...images };
		delete next[row.id];
		images = next;
		await refreshChart();
	} catch (err: unknown) {
		notice =
			err instanceof Error ? err.message : "Could not delete the result.";
	} finally {
		deletingId = null;
	}
}

function partsOf(row: SavedResult): ResultPart[] {
	return Array.isArray(row.detail.parts) ? row.detail.parts : [];
}

onMount(() => {
	readAdmin();
	window.addEventListener("focus", readAdmin);
	void load();
	return () => {
		window.removeEventListener("focus", readAdmin);
		chart?.destroy();
		chart = null;
	};
});
</script>

<div class="results-root flex h-dvh max-h-dvh flex-col overflow-hidden bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex min-w-0 items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">Results</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Pass / Fail</p>
		</div>
		<a
			href="/"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
		>
			Operator
		</a>
	</header>

	<main class="results-scroll min-h-0 flex-1 overflow-y-auto">
		<div class="mx-auto flex w-full max-w-6xl flex-col gap-4 p-4">
			<section class="flex flex-col gap-3 border border-neutral-700 bg-neutral-900 p-4">
				<div class="flex flex-wrap items-center justify-between gap-3">
					<h2 class="text-lg font-semibold tracking-tight">Pass / Fail</h2>
					<div class="flex gap-2" role="group" aria-label="Result range">
						{#each RANGES as item (item.id)}
							<button
								type="button"
								class="inline-flex min-h-11 items-center border px-4 text-sm font-semibold {range === item.id
									? 'border-blue-500 bg-blue-600 text-white'
									: 'border-neutral-600 bg-neutral-800 text-neutral-100'}"
								aria-pressed={range === item.id}
								on:click={() => {
									range = item.id;
									void refreshChart();
								}}
							>
								{item.label}
							</button>
						{/each}
					</div>
				</div>
				<div bind:this={chartEl} class="min-h-72"></div>
			</section>

			{#if notice}
				<p class="text-sm text-neutral-200">{notice}</p>
			{/if}

			<section class="border border-neutral-700 bg-neutral-900">
				<table class="w-full border-collapse text-left text-sm">
					<thead>
						<tr class="border-b border-neutral-700 text-[11px] uppercase tracking-widest text-neutral-400">
							<th class="px-3 py-3 font-semibold">Result</th>
							<th class="px-3 py-3 font-semibold">Kit IC</th>
							<th class="px-3 py-3 font-semibold">Job Card</th>
							<th class="px-3 py-3 font-semibold">Description</th>
							<th class="px-3 py-3 font-semibold">Date/Time</th>
							<th class="px-3 py-3 font-semibold">Image</th>
							{#if admin}
								<th class="px-3 py-3 font-semibold"> </th>
							{/if}
						</tr>
					</thead>
					<tbody>
						{#if rows.length === 0}
							<tr>
								<td class="px-3 py-8 text-neutral-400" colspan={admin ? 7 : 6}>No results yet</td>
							</tr>
						{/if}
						{#each rows as row (row.id)}
							<tr
								class="cursor-pointer border-b border-neutral-800 hover:bg-neutral-800/60"
								aria-expanded={openId === row.id}
								on:click={() => toggleRow(row.id)}
							>
								<td class="px-3 py-3 font-black uppercase tracking-widest {row.passed ? 'text-green-400' : 'text-red-400'}">
									{row.passed ? "Pass" : "Fail"}
								</td>
								<td class="px-3 py-3 font-mono">{row.kitIc || "—"}</td>
								<td class="px-3 py-3 font-mono">{row.jobCard || "—"}</td>
								<td class="px-3 py-3 text-neutral-200">{row.description || "—"}</td>
								<td class="px-3 py-3 whitespace-nowrap">{formatResultWhen(row.createdAt)}</td>
								<td class="px-3 py-3">
									{#if images[row.id]}
										<img
											src={images[row.id]}
											alt=""
											class="h-12 w-16 border border-neutral-700 object-cover"
										/>
									{:else}
										<span class="text-neutral-500">—</span>
									{/if}
								</td>
								{#if admin}
									<td class="px-3 py-3 text-right">
										<button
											type="button"
											class="inline-flex min-h-11 items-center border border-red-700 bg-red-950 px-3 text-sm font-semibold text-red-200 disabled:opacity-50"
											disabled={deletingId === row.id}
											on:click|stopPropagation={() => removeRow(row)}
										>
											{deletingId === row.id ? "Deleting..." : "Delete"}
										</button>
									</td>
								{/if}
							</tr>
							{#if openId === row.id}
								<tr class="border-b border-neutral-800 bg-neutral-950">
									<td class="px-3 py-4" colspan={admin ? 7 : 6}>
										<div class="flex flex-col gap-3">
											<p class="font-mono text-lg font-semibold text-neutral-100">
												Job card {row.jobCard?.trim() || "—"}
											</p>
											{#if row.detail.message}
												<p class="text-base text-neutral-200">{row.detail.message}</p>
											{/if}
											{#if partsOf(row).length}
												<table class="w-full border-collapse text-left text-sm">
													<thead>
														<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
															<th class="py-2 pr-3 font-semibold">IC</th>
															<th class="py-2 pr-3 font-semibold">Description</th>
															<th class="py-2 text-right font-semibold">Qty</th>
															{#if partsOf(row).some((part) => part.seen !== null)}
																<th class="py-2 text-right font-semibold">Seen</th>
															{/if}
														</tr>
													</thead>
													<tbody>
														{#each partsOf(row) as part, index (index)}
															<tr class="border-b border-neutral-800">
																<td class="py-2 pr-3 font-mono font-medium">{part.ic}</td>
																<td class="py-2 pr-3 text-neutral-200">{part.des}</td>
																<td class="py-2 text-right font-semibold">{part.qty}</td>
																{#if partsOf(row).some((item) => item.seen !== null)}
																	<td
																		class="py-2 text-right font-semibold {part.seen !== null &&
																		part.seen >= part.qty
																			? 'text-green-400'
																			: 'text-red-400'}"
																	>
																		{part.seen ?? "—"}
																	</td>
																{/if}
															</tr>
														{/each}
													</tbody>
												</table>
												{#if matchSummary(partsOf(row))}
													<p class="text-sm text-neutral-300">{matchSummary(partsOf(row))}</p>
												{/if}
											{/if}
											{#if row.detail.extras?.length}
												<h3 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
													Not in BoM
												</h3>
												<table class="w-full border-collapse text-left text-sm">
													<thead>
														<tr class="border-b border-neutral-600 text-[11px] uppercase tracking-widest text-neutral-400">
															<th class="py-2 pr-3 font-semibold">IC</th>
															<th class="py-2 text-right font-semibold">Count</th>
														</tr>
													</thead>
													<tbody>
														{#each row.detail.extras as extra (extra.ic)}
															<tr class="border-b border-neutral-800">
																<td class="py-2 pr-3 font-mono font-medium">{extra.ic}</td>
																<td class="py-2 text-right font-semibold">{extra.count}</td>
															</tr>
														{/each}
													</tbody>
												</table>
											{/if}
											{#if row.detail.info}
												<p class="text-sm text-neutral-400">{row.detail.info}</p>
											{/if}
											{#if row.detail.kitsFound != null || row.detail.expectedKitCount != null}
												<p class="text-sm text-neutral-400">
													Info only: {row.detail.kitsFound ?? "—"} kit(s) found, {row.detail.expectedKitCount ?? "—"} expected
												</p>
											{/if}
										</div>
									</td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
			</section>
		</div>
	</main>
</div>

<style>
	.results-root {
		color-scheme: dark;
	}

	.results-scroll {
		color-scheme: dark;
		scrollbar-color: #a3a3a3 #171717;
		scrollbar-width: auto;
	}

	.results-scroll::-webkit-scrollbar {
		width: 12px;
		height: 12px;
	}

	.results-scroll::-webkit-scrollbar-track {
		background: #171717;
	}

	.results-scroll::-webkit-scrollbar-thumb {
		background: #a3a3a3;
		border: 2px solid #171717;
	}
</style>
