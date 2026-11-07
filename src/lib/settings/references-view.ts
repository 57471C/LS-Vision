export type CatalogueCrop = {
	index: number;
	src: string;
	hash: string;
};

export type CatalogueSize = {
	key: string;
	label: string;
	value: string;
	toleranceMm: number | null;
};

export type CataloguePart = {
	ic: string;
	description: string;
	type: string;
	dimensions: string;
	thumbs: string[];
	crops: CatalogueCrop[];
	sizes: CatalogueSize[];
	addedUnix: number | null;
};

const DAY_MS = 86_400_000;

export function refsLabel(count: number): string {
	return count === 1 ? "1 ref" : `${count} refs`;
}

export function addedLabel(unix: number | null, nowMs: number): string {
	if (unix == null || !Number.isFinite(unix)) return "";
	const days = Math.floor((nowMs - unix * 1000) / DAY_MS);
	if (days <= 0) return "Added today";
	if (days === 1) return "Added 1 day ago";
	return `Added ${days} days ago`;
}

export function addedTitle(unix: number | null): string {
	if (unix == null || !Number.isFinite(unix)) return "";
	const date = new Date(unix * 1000);
	if (Number.isNaN(date.getTime())) return "";
	return date.toLocaleString();
}

export function filterParts<T extends { ic: string; description: string }>(
	parts: T[],
	query: string,
): T[] {
	const needle = query.trim().toLowerCase();
	if (!needle) return parts;
	return parts.filter(
		(part) =>
			part.ic.toLowerCase().includes(needle) ||
			part.description.toLowerCase().includes(needle),
	);
}

export function referencesMessage(
	partCount: number,
	shownCount: number,
	query: string,
): string {
	if (partCount === 0) return "";
	if (query.trim() && shownCount === 0) return "No matches";
	return "";
}

export function duplicateHashes(parts: { crops: { hash: string }[] }[]): Set<string> {
	const counts = new Map<string, number>();
	for (const part of parts) {
		for (const crop of part.crops) {
			if (!crop.hash) continue;
			counts.set(crop.hash, (counts.get(crop.hash) ?? 0) + 1);
		}
	}
	const dupes = new Set<string>();
	for (const [hash, count] of counts) {
		if (count > 1) dupes.add(hash);
	}
	return dupes;
}

export function reorderIndexes(
	indexes: number[],
	from: number,
	to: number,
): number[] | null {
	if (
		from === to ||
		from < 0 ||
		to < 0 ||
		from >= indexes.length ||
		to >= indexes.length
	) {
		return null;
	}
	const next = indexes.slice();
	const moved = next.splice(from, 1)[0];
	if (moved === undefined) return null;
	next.splice(to, 0, moved);
	return next;
}

export function primaryOrder(indexes: number[], index: number): number[] | null {
	const from = indexes.indexOf(index);
	if (from <= 0) return null;
	return reorderIndexes(indexes, from, 0);
}

export function readCatalogueList(value: unknown): {
	path: string;
	parts: CataloguePart[];
	rawCount: number;
} | null {
	if (!value || typeof value !== "object" || Array.isArray(value)) return null;
	const record = value as Record<string, unknown>;
	const path = typeof record.path === "string" ? record.path : "";
	const raw = Array.isArray(record.parts) ? record.parts : null;
	if (!raw) return { path, parts: [], rawCount: -1 };
	const parts: CataloguePart[] = [];
	for (const item of raw) {
		const part = readCataloguePart(item);
		if (part) parts.push(part);
	}
	return { path, parts, rawCount: raw.length };
}

function readCataloguePart(value: unknown): CataloguePart | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	const ic = typeof record.ic === "string" ? record.ic.trim() : "";
	if (!ic) return null;
	const description =
		typeof record.description === "string" ? record.description.trim() : "";
	const type = typeof record.type === "string" ? record.type.trim() : "";
	const dimensions =
		typeof record.dimensions === "string" ? record.dimensions.trim() : "";
	const thumbs = readThumbs(record.thumbs);
	const crops = readCrops(record.crops, thumbs);
	const sizes = Array.isArray(record.sizes)
		? record.sizes
				.map(readSize)
				.filter((item): item is CatalogueSize => item !== null)
		: [];
	const addedUnix =
		typeof record.addedUnix === "number" && Number.isFinite(record.addedUnix)
			? record.addedUnix
			: null;
	return {
		ic,
		description,
		type,
		dimensions,
		thumbs: crops.some((crop) => crop.src.startsWith("data:image/"))
			? crops
					.filter((crop) => crop.src.startsWith("data:image/"))
					.map((crop) => crop.src)
			: thumbs,
		crops,
		sizes,
		addedUnix,
	};
}

function readThumbs(value: unknown): string[] {
	if (!Array.isArray(value)) return [];
	return value.filter(
		(item): item is string =>
			typeof item === "string" && item.startsWith("data:image/"),
	);
}

function readCrops(value: unknown, thumbs: string[]): CatalogueCrop[] {
	if (Array.isArray(value)) {
		const crops: CatalogueCrop[] = [];
		for (let index = 0; index < value.length; index += 1) {
			const crop = readCrop(value[index], index);
			if (crop) crops.push(crop);
		}
		if (crops.length > 0 || value.length === 0) return crops;
	}
	return thumbs.map((src, index) => ({ index, src, hash: "" }));
}

function readCrop(value: unknown, fallbackIndex: number): CatalogueCrop | null {
	if (typeof value === "string" && value.startsWith("data:image/")) {
		return { index: fallbackIndex, src: value, hash: "" };
	}
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	const index =
		typeof record.index === "number" &&
		Number.isInteger(record.index) &&
		record.index >= 0
			? record.index
			: fallbackIndex;
	const src = typeof record.src === "string" ? record.src : "";
	const hash = typeof record.hash === "string" ? record.hash : "";
	if (src !== "" && !src.startsWith("data:image/")) return null;
	return { index, src, hash };
}

function readSize(value: unknown): CatalogueSize | null {
	if (!value || typeof value !== "object") return null;
	const record = value as Record<string, unknown>;
	const key = typeof record.key === "string" ? record.key.trim() : "";
	if (!key) return null;
	const label =
		typeof record.label === "string" && record.label.trim()
			? record.label.trim()
			: key;
	const raw = record.value;
	const text =
		typeof raw === "string" ? raw.trim() : typeof raw === "number" ? String(raw) : "";
	if (!text) return null;
	const tolerance = record.toleranceMm;
	const toleranceMm =
		typeof tolerance === "number" && Number.isFinite(tolerance) ? tolerance : null;
	return { key, label, value: text, toleranceMm };
}
