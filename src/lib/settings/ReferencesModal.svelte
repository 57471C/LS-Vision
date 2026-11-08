<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { createEventDispatcher, onMount, tick } from "svelte";
import {
	addedLabel,
	addedTitle,
	type CataloguePart,
	duplicateHashes,
	filterParts,
	primaryOrder,
	readCatalogueList,
	refsLabel,
	referencesMessage,
	reorderIndexes,
} from "./references-view";

export let epoch = 0;

const dispatch = createEventDispatcher<{ close: void }>();

type Selection = { ic: string; index: number };
type Confirm =
	| { kind: "part"; ic: string }
	| { kind: "images"; items: Selection[] };
type Menu = { ic: string; index: number; x: number; y: number };

let parts: CataloguePart[] = [];
let path = "";
let loading = true;
let notice = "";
let filterText = "";
let selected = new Set<string>();
let anchor: Selection | null = null;
let menu: Menu | null = null;
let confirming: Confirm | null = null;
let reassignItems: Selection[] | null = null;
let reassignText = "";
let busy = false;
let copiedIc = "";
let copyTimer: ReturnType<typeof setTimeout> | null = null;
let dragFrom: Selection | null = null;
let dragOverKey = "";
let generation = 0;
let seenEpoch = epoch;
let closeButton: HTMLButtonElement | null = null;
let filterInput: HTMLInputElement | null = null;
let confirmCancel: HTMLButtonElement | null = null;
let reassignInput: HTMLInputElement | null = null;

$: shown = filterParts(parts, filterText);
$: dupes = duplicateHashes(parts);
$: emptyFilter = referencesMessage(parts.length, shown.length, filterText);
$: if (epoch !== seenEpoch) {
	seenEpoch = epoch;
	void load();
}

onMount(async () => {
	await tick();
	closeButton?.focus();
	await load();
});

function selKey(ic: string, index: number): string {
	return `${ic}\t${index}`;
}

function parseKey(key: string): Selection | null {
	const split = key.lastIndexOf("\t");
	if (split <= 0) return null;
	const index = Number(key.slice(split + 1));
	if (!Number.isInteger(index)) return null;
	return { ic: key.slice(0, split), index };
}

function selectedItems(): Selection[] {
	const items: Selection[] = [];
	for (const key of selected) {
		const item = parseKey(key);
		if (item) items.push(item);
	}
	return items;
}

function invokeError(err: unknown, fallback: string): string {
	if (typeof err === "string" && err.trim()) return err;
	if (err instanceof Error && err.message.trim()) return err.message;
	return fallback;
}

async function invokeRust<T>(
	command: string,
	args?: Record<string, unknown>,
): Promise<T> {
	const { invoke } = await import("@tauri-apps/api/core");
	return invoke<T>(command, args);
}

function noticeClass(text: string): string {
	if (
		text.startsWith("Removed ") ||
		text.startsWith("Moved ") ||
		text.startsWith("Reordered")
	) {
		return "text-neutral-200";
	}
	return "text-red-300";
}

async function load() {
	const token = ++generation;
	loading = true;
	try {
		const data = await invokeRust<unknown>("list_catalogue");
		if (token !== generation) return;
		const list = readCatalogueList(data);
		if (
			!list ||
			list.rawCount < 0 ||
			(list.rawCount > 0 && list.parts.length === 0)
		) {
			parts = [];
			notice = "Could not load the references";
			return;
		}
		path = list.path;
		parts = list.parts;
		selected = new Set();
		anchor = null;
		notice = "";
	} catch (err: unknown) {
		if (token !== generation) return;
		parts = [];
		notice = invokeError(err, "Could not load the references");
	} finally {
		if (token === generation) loading = false;
	}
}

function close() {
	dispatch("close");
}

function onKey(event: KeyboardEvent) {
	if (event.key !== "Escape") return;
	if (confirming) {
		event.preventDefault();
		confirming = null;
		return;
	}
	if (reassignItems) {
		event.preventDefault();
		reassignItems = null;
		return;
	}
	if (menu) {
		event.preventDefault();
		menu = null;
		return;
	}
	if (filterText.trim() && document.activeElement === filterInput) {
		event.preventDefault();
		filterText = "";
		return;
	}
	event.preventDefault();
	close();
}

async function copyIc(ic: string) {
	try {
		await navigator.clipboard.writeText(ic);
		copiedIc = ic;
		if (copyTimer) clearTimeout(copyTimer);
		copyTimer = setTimeout(() => {
			if (copiedIc === ic) copiedIc = "";
		}, 1200);
	} catch {
		notice = "Could not copy the IC";
	}
}

function toggleOne(ic: string, index: number) {
	const key = selKey(ic, index);
	const next = new Set(selected);
	if (next.has(key)) next.delete(key);
	else next.add(key);
	selected = next;
	anchor = { ic, index };
}

function selectRange(ic: string, index: number) {
	const part = parts.find((item) => item.ic === ic);
	if (!part || !anchor || anchor.ic !== ic) {
		toggleOne(ic, index);
		return;
	}
	const indexes = part.crops.map((crop) => crop.index);
	const start = indexes.indexOf(anchor.index);
	const end = indexes.indexOf(index);
	if (start < 0 || end < 0) {
		toggleOne(ic, index);
		return;
	}
	const [from, to] = start < end ? [start, end] : [end, start];
	const next = new Set(selected);
	for (const cropIndex of indexes.slice(from, to + 1)) {
		next.add(selKey(ic, cropIndex));
	}
	selected = next;
}

function onCheck(event: MouseEvent, ic: string, index: number) {
	event.preventDefault();
	event.stopPropagation();
	if (event.shiftKey) selectRange(ic, index);
	else toggleOne(ic, index);
}

function onTileClick(event: MouseEvent, ic: string, index: number) {
	if (!event.shiftKey) return;
	event.preventDefault();
	selectRange(ic, index);
}

function openMenu(event: MouseEvent, ic: string, index: number) {
	event.preventDefault();
	const width = 200;
	const height = 140;
	menu = {
		ic,
		index,
		x: Math.max(8, Math.min(event.clientX, window.innerWidth - width - 8)),
		y: Math.max(8, Math.min(event.clientY, window.innerHeight - height - 8)),
	};
}

function menuTargets(): Selection[] {
	if (!menu) return [];
	const key = selKey(menu.ic, menu.index);
	if (selected.has(key) && selected.size > 1) return selectedItems();
	return [{ ic: menu.ic, index: menu.index }];
}

function groupItems(items: Selection[]): Map<string, number[]> {
	const groups = new Map<string, number[]>();
	for (const item of items) {
		const list = groups.get(item.ic) ?? [];
		list.push(item.index);
		groups.set(item.ic, list);
	}
	return groups;
}

async function focusConfirm() {
	await tick();
	confirmCancel?.focus();
}

function askDeletePart(ic: string) {
	if (busy) return;
	menu = null;
	confirming = { kind: "part", ic };
	void focusConfirm();
}

function askDeleteImages(items: Selection[]) {
	if (busy || items.length === 0) return;
	menu = null;
	confirming = { kind: "images", items };
	void focusConfirm();
}

function askReassign(items: Selection[]) {
	if (busy || items.length === 0) return;
	menu = null;
	reassignItems = items;
	reassignText = "";
	void tick().then(() => reassignInput?.focus());
}

async function confirmDelete() {
	if (!confirming || busy) return;
	const pending = confirming;
	confirming = null;
	busy = true;
	notice = "";
	try {
		if (pending.kind === "part") {
			await invokeRust("delete_catalogue_part", { ic: pending.ic });
			await load();
			if (!notice) notice = `Removed ${pending.ic}.`;
		} else {
			for (const [ic, indexes] of groupItems(pending.items)) {
				await invokeRust("delete_catalogue_crops", { ic, indexes });
			}
			const count = pending.items.length;
			await load();
			if (!notice) {
				notice = count === 1 ? "Removed 1 image." : `Removed ${count} images.`;
			}
		}
	} catch (err: unknown) {
		notice = invokeError(err, "Could not delete the part");
		await load();
	} finally {
		busy = false;
	}
}

async function confirmReassign() {
	if (!reassignItems || busy) return;
	const target = reassignText.trim();
	if (!target) {
		notice = "Enter an IC.";
		return;
	}
	const items = reassignItems.filter((item) => item.ic !== target);
	if (items.length === 0) {
		notice = "Choose a different IC.";
		return;
	}
	reassignItems = null;
	busy = true;
	notice = "";
	try {
		for (const [ic, indexes] of groupItems(items)) {
			await invokeRust("reassign_catalogue_crops", { ic, indexes, target });
		}
		await load();
		if (!notice) notice = `Moved ${items.length} to ${target}.`;
	} catch (err: unknown) {
		notice = invokeError(err, "Could not reassign the image");
		await load();
	} finally {
		busy = false;
	}
}

async function runReorder(ic: string, order: number[]) {
	busy = true;
	notice = "";
	try {
		await invokeRust("reorder_catalogue_crops", { ic, order });
		await load();
		if (!notice) notice = "Reordered.";
	} catch (err: unknown) {
		notice = invokeError(err, "Could not reorder the images");
	} finally {
		busy = false;
	}
}

async function setPrimary(ic: string, index: number) {
	const part = parts.find((item) => item.ic === ic);
	if (!part) return;
	const order = primaryOrder(
		part.crops.map((crop) => crop.index),
		index,
	);
	menu = null;
	if (!order) return;
	await runReorder(ic, order);
}

function onDragStart(event: DragEvent, ic: string, index: number) {
	dragFrom = { ic, index };
	event.dataTransfer?.setData("text/plain", selKey(ic, index));
	if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
}

function onDragOver(event: DragEvent, ic: string, index: number) {
	if (!dragFrom || dragFrom.ic !== ic) return;
	event.preventDefault();
	dragOverKey = selKey(ic, index);
	if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
}

async function onDrop(event: DragEvent, part: CataloguePart, index: number) {
	event.preventDefault();
	dragOverKey = "";
	const from = dragFrom;
	dragFrom = null;
	if (!from || from.ic !== part.ic || busy) return;
	const indexes = part.crops.map((crop) => crop.index);
	const order = reorderIndexes(
		indexes,
		indexes.indexOf(from.index),
		indexes.indexOf(index),
	);
	if (!order) return;
	await runReorder(part.ic, order);
}

async function saveTolerance(ic: string, key: string, raw: string) {
	const text = raw.trim();
	let mm: number | null = null;
	if (text !== "") {
		const value = Number(text);
		if (!Number.isFinite(value) || value < 0 || value > 1000) {
			notice = "Enter a tolerance from 0 to 1000 mm.";
			return;
		}
		mm = value;
	}
	try {
		await invokeRust("set_catalogue_tolerance", { ic, key, mm });
		parts = parts.map((part) =>
			part.ic === ic
				? {
						...part,
						sizes: part.sizes.map((size) =>
							size.key === key ? { ...size, toleranceMm: mm } : size,
						),
					}
				: part,
		);
		if (notice.startsWith("Enter a tolerance")) notice = "";
	} catch (err: unknown) {
		notice = invokeError(err, "Could not save the tolerance");
	}
}

function confirmText(): string {
	if (!confirming) return "";
	if (confirming.kind === "part") {
		return `Delete ${confirming.ic} and all its reference images?`;
	}
	if (confirming.items.length === 1) return "Delete this image?";
	return `Delete ${confirming.items.length} reference images?`;
}

function menuCanPrimary(): boolean {
	const targets = menuTargets();
	if (targets.length !== 1 || !menu) return false;
	return menu.index !== 0;
}
</script>

<svelte:window on:keydown={onKey} />

<div
	class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
	on:click={close}
>
	<div
		class="relative flex max-h-[85vh] w-full max-w-5xl flex-col border border-neutral-700 bg-neutral-900 p-4 text-neutral-100"
		role="dialog"
		aria-modal="true"
		aria-labelledby="references-title"
		on:click|stopPropagation
	>
		<button
			type="button"
			bind:this={closeButton}
			class="absolute right-3 top-3 inline-flex h-9 w-9 items-center justify-center border border-red-700 bg-red-600 text-lg font-bold leading-none text-white"
			aria-label="Close"
			on:click={close}
		>
			×
		</button>
		<h2 id="references-title" class="pr-12 text-lg font-semibold tracking-tight">
			DINOv2 references
		</h2>
		<p class="mt-1 text-sm text-neutral-400">
			Imported parts. Delete asks before it removes a part and its images.
		</p>
		{#if path}
			<p class="mt-2 break-all pr-12 font-mono text-xs text-neutral-500">{path}</p>
		{/if}
		<label class="mt-3 flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
			Filter
			<input
				bind:this={filterInput}
				bind:value={filterText}
				class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
				autocomplete="off"
				placeholder="IC or description"
			/>
		</label>
		{#if notice}
			<p class="mt-3 text-sm {noticeClass(notice)}">{notice}</p>
		{/if}
		{#if selected.size > 0}
			<div class="mt-3 flex flex-wrap items-center gap-2">
				<span class="text-sm text-neutral-300">{selected.size} selected</span>
				<button
					type="button"
					class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-3 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
					on:click={() => askDeleteImages(selectedItems())}
					disabled={busy}
				>
					Delete
				</button>
				<button
					type="button"
					class="inline-flex min-h-9 items-center border border-neutral-600 bg-neutral-800 px-3 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
					on:click={() => askReassign(selectedItems())}
					disabled={busy}
				>
					Reassign
				</button>
				<button
					type="button"
					class="inline-flex min-h-9 items-center border border-neutral-700 px-3 text-sm font-semibold text-neutral-300"
					on:click={() => {
						selected = new Set();
						anchor = null;
					}}
					disabled={busy}
				>
					Clear
				</button>
			</div>
		{/if}
		<div class="mt-3 min-h-0 flex-1 overflow-auto">
			{#if loading && parts.length === 0}
				<p class="text-sm text-neutral-200">Loading...</p>
			{:else if parts.length === 0}
				{#if !notice || notice.startsWith("Removed ")}
					<p class="text-sm text-neutral-200">No references loaded.</p>
				{/if}
			{:else if emptyFilter}
				<p class="text-sm text-neutral-200">{emptyFilter}</p>
			{:else}
				<ul class="flex flex-col">
					{#each shown as part (part.ic)}
						<li class="border-b border-neutral-800 py-3">
							<div class="flex items-start gap-3">
								<div class="min-w-0 flex-1">
									<div class="flex flex-wrap items-center gap-2">
										<button
											type="button"
											class="font-mono text-sm text-neutral-100 underline decoration-neutral-600 underline-offset-2"
											title="Copy IC"
											aria-label="Copy {part.ic}"
											on:click={() => copyIc(part.ic)}
										>
											{part.ic}
										</button>
										<span
											class="border border-neutral-700 bg-neutral-950 px-1.5 py-0.5 text-[11px] font-semibold uppercase tracking-wide text-neutral-300"
										>
											{refsLabel(part.crops.length)}
										</span>
										{#if copiedIc === part.ic}
											<span class="text-xs text-neutral-400">Copied</span>
										{/if}
									</div>
									{#if part.addedUnix != null}
										<p class="text-xs text-neutral-500" title={addedTitle(part.addedUnix)}>
											{addedLabel(part.addedUnix, Date.now())}
										</p>
									{/if}
									{#if part.description || part.dimensions}
										<p class="text-sm text-neutral-200">
											{#if part.description}{part.description}{/if}
											{#if part.dimensions}
												<span class="font-medium text-neutral-100"
													>{part.description ? " " : ""}{part.dimensions}</span
												>
											{/if}
										</p>
									{/if}
									{#if part.type}
										<p class="text-sm text-neutral-400">Type {part.type}</p>
									{/if}
									{#if part.sizes.length > 0}
										<div class="mt-2 flex flex-wrap gap-x-4 gap-y-2">
											{#each part.sizes as size (size.key)}
												<label class="flex items-center gap-2 text-sm text-neutral-300">
													<span>{size.label} {size.value}</span>
													<input
														class="w-20 border border-neutral-600 bg-neutral-950 px-2 py-1 text-sm text-neutral-100"
														type="number"
														min="0"
														max="1000"
														step="0.1"
														inputmode="decimal"
														aria-label="Tolerance for {part.ic} {size.label}"
														value={size.toleranceMm ?? ""}
														disabled={busy}
														on:change={(event) => {
															const target = event.currentTarget;
															if (!(target instanceof HTMLInputElement)) return;
															saveTolerance(part.ic, size.key, target.value);
														}}
													/>
													<span class="text-xs text-neutral-500">± mm</span>
												</label>
											{/each}
										</div>
									{/if}
								</div>
								<button
									type="button"
									class="inline-flex min-h-9 shrink-0 items-center border border-neutral-600 bg-neutral-800 px-3 text-sm font-semibold text-neutral-100 disabled:text-neutral-500"
									aria-label="Delete {part.ic}"
									on:click={() => askDeletePart(part.ic)}
									disabled={busy}
								>
									Delete
								</button>
							</div>
							{#if part.crops.length === 0}
								<p class="mt-2 text-sm text-neutral-500">No crop</p>
							{:else}
								<div class="mt-2 flex flex-wrap gap-2">
									{#each part.crops as crop (`${part.ic}-${crop.index}`)}
										<div
											class="relative h-24 w-24 shrink-0 border bg-neutral-950 {dupes.has(crop.hash)
												? 'border-orange-500'
												: 'border-neutral-800'} {selected.has(selKey(part.ic, crop.index))
												? 'outline outline-2 outline-offset-1 outline-neutral-200'
												: ''} {dragOverKey === selKey(part.ic, crop.index)
												? 'outline outline-2 outline-neutral-400'
												: ''}"
											on:click={(event) => onTileClick(event, part.ic, crop.index)}
											on:contextmenu={(event) => openMenu(event, part.ic, crop.index)}
											on:dragover={(event) => onDragOver(event, part.ic, crop.index)}
											on:dragleave={() => {
												if (dragOverKey === selKey(part.ic, crop.index)) dragOverKey = "";
											}}
											on:drop={(event) => onDrop(event, part, crop.index)}
										>
											{#if crop.src}
												<img
													src={crop.src}
													alt="{part.ic} crop {crop.index + 1}"
													class="h-full w-full cursor-grab object-contain"
													draggable="true"
													on:dragstart={(event) => onDragStart(event, part.ic, crop.index)}
													on:dragend={() => {
														dragFrom = null;
														dragOverKey = "";
													}}
												/>
											{:else}
												<div
													class="flex h-full w-full cursor-grab items-center justify-center text-[10px] font-semibold uppercase tracking-wide text-neutral-500"
													draggable="true"
													on:dragstart={(event) => onDragStart(event, part.ic, crop.index)}
													on:dragend={() => {
														dragFrom = null;
														dragOverKey = "";
													}}
												>
													No crop
												</div>
											{/if}
											<input
												type="checkbox"
												class="absolute left-1 top-1 h-4 w-4 accent-neutral-200"
												checked={selected.has(selKey(part.ic, crop.index))}
												aria-label="Select {part.ic} image {crop.index + 1}"
												on:click={(event) => onCheck(event, part.ic, crop.index)}
											/>
											{#if crop.index === 0}
												<span
													class="pointer-events-none absolute right-1 top-1 bg-neutral-950 px-1 text-[10px] font-semibold uppercase tracking-wide text-neutral-300"
												>
													Primary
												</span>
											{/if}
											{#if dupes.has(crop.hash)}
												<span
													class="pointer-events-none absolute bottom-1 left-1 bg-orange-950 px-1 text-[10px] font-semibold uppercase tracking-wide text-orange-200"
												>
													Duplicate
												</span>
											{/if}
										</div>
									{/each}
								</div>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	</div>
</div>

{#if menu}
	<button
		type="button"
		class="fixed inset-0 z-[60] cursor-default bg-transparent"
		aria-label="Close menu"
		on:click={() => (menu = null)}
	></button>
	<div
		class="fixed z-[61] w-48 border border-neutral-700 bg-neutral-900 py-1 text-sm text-neutral-100 shadow-lg"
		style="left: {menu.x}px; top: {menu.y}px"
		role="menu"
	>
		{#if menuCanPrimary()}
			<button
				type="button"
				class="block w-full px-3 py-2 text-left hover:bg-neutral-800"
				role="menuitem"
				on:click={() => setPrimary(menu?.ic ?? "", menu?.index ?? 0)}
			>
				Set as primary
			</button>
		{/if}
		<button
			type="button"
			class="block w-full px-3 py-2 text-left hover:bg-neutral-800"
			role="menuitem"
			on:click={() => askReassign(menuTargets())}
		>
			Reassign IC
		</button>
		<button
			type="button"
			class="block w-full px-3 py-2 text-left text-red-200 hover:bg-neutral-800"
			role="menuitem"
			on:click={() => askDeleteImages(menuTargets())}
		>
			{menuTargets().length > 1 ? `Delete ${menuTargets().length} images` : "Delete this image"}
		</button>
	</div>
{/if}

{#if confirming}
	<div
		class="fixed inset-0 z-[70] flex items-center justify-center bg-black/70 p-4"
		on:click={() => {
			if (!busy) confirming = null;
		}}
	>
		<div
			class="flex w-full max-w-lg flex-col border border-neutral-700 bg-neutral-900 p-4"
			role="dialog"
			aria-modal="true"
			aria-labelledby="delete-reference-title"
			on:click|stopPropagation
		>
			<h2 id="delete-reference-title" class="text-lg font-semibold tracking-tight">
				Delete
			</h2>
			<p class="mt-3 text-sm text-neutral-200">{confirmText()}</p>
			<div class="mt-4 flex justify-end gap-2">
				<button
					type="button"
					bind:this={confirmCancel}
					class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
					on:click={() => (confirming = null)}
					disabled={busy}
				>
					Cancel
				</button>
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-red-800 bg-red-950 px-4 text-sm font-semibold text-red-100"
					on:click={confirmDelete}
					disabled={busy}
				>
					Delete
				</button>
			</div>
		</div>
	</div>
{/if}

{#if reassignItems}
	<div
		class="fixed inset-0 z-[70] flex items-center justify-center bg-black/70 p-4"
		on:click={() => {
			if (!busy) reassignItems = null;
		}}
	>
		<div
			class="flex w-full max-w-lg flex-col border border-neutral-700 bg-neutral-900 p-4"
			role="dialog"
			aria-modal="true"
			aria-labelledby="reassign-reference-title"
			on:click|stopPropagation
		>
			<h2 id="reassign-reference-title" class="text-lg font-semibold tracking-tight">
				Reassign IC
			</h2>
			<p class="mt-3 text-sm text-neutral-200">
				Move {reassignItems.length === 1 ? "this image" : `${reassignItems.length} images`} to an item code. A new code is added when it is not in the catalogue.
			</p>
			<label class="mt-3 flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
				IC
				<input
					bind:this={reassignInput}
					bind:value={reassignText}
					list="reference-ics"
					class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
					autocomplete="off"
				/>
			</label>
			<datalist id="reference-ics">
				{#each parts as part (part.ic)}
					<option value={part.ic}></option>
				{/each}
			</datalist>
			<div class="mt-4 flex justify-end gap-2">
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
					on:click={() => (reassignItems = null)}
					disabled={busy}
				>
					Cancel
				</button>
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
					on:click={confirmReassign}
					disabled={busy || reassignText.trim() === ""}
				>
					Reassign
				</button>
			</div>
		</div>
	</div>
{/if}
