// Catalogue label drawn on a YOLO box. The BoM count still uses the class IC.

export type CatalogCandidate = {
	ic: string;
	description: string;
	partType: string;
	score: number;
};

export type CatalogMatch = {
	recognized: boolean;
	label: string;
	top: CatalogCandidate | null;
	runnersUp: CatalogCandidate[];
};

function readCandidate(value: unknown): CatalogCandidate | null {
	if (!value || typeof value !== "object") return null;
	const row = value as Record<string, unknown>;
	if (typeof row.ic !== "string" || !row.ic.trim()) return null;
	const score = typeof row.score === "number" ? row.score : Number.NaN;
	if (!Number.isFinite(score)) return null;
	return {
		ic: row.ic,
		description: typeof row.description === "string" ? row.description : "",
		partType: typeof row.partType === "string" ? row.partType : "",
		score,
	};
}

// Undefined when this detect did not return a catalogue label.
export function readCatalogMatch(value: unknown): CatalogMatch | undefined {
	if (!value || typeof value !== "object") return undefined;
	const row = value as Record<string, unknown>;
	const label = typeof row.label === "string" ? row.label.trim() : "";
	if (!label) return undefined;
	const runnersUp: CatalogCandidate[] = [];
	if (Array.isArray(row.runnersUp)) {
		for (const item of row.runnersUp) {
			const candidate = readCandidate(item);
			if (candidate) runnersUp.push(candidate);
		}
	}
	return {
		recognized: row.recognized === true,
		label,
		top: readCandidate(row.top),
		runnersUp,
	};
}

// A missing match keeps the class IC. "No Match" is the low-score label.
export function overlayLabel(
	match: CatalogMatch | undefined,
	fallbackIc: string,
): string {
	if (!match) return fallbackIc;
	return match.label;
}

// Item code for the Not in BoM row. The canvas draws this same code, plus its percent.
// A low score stays "No Match", which is what the box already shows.
export function resolvedItemIc(
	match: CatalogMatch | undefined,
	fallbackIc: string,
): string {
	if (match?.recognized) {
		const ic = match.top?.ic.trim() ?? "";
		if (ic) return ic;
	}
	if (match?.label) return match.label;
	return fallbackIc;
}

export type OutsideCount = {
	ic: string;
	count: number;
	des: string;
};

type OutsideBox = {
	ic: string;
	consumed?: boolean;
	referenceMatch?: boolean;
	catalogMatch?: CatalogMatch;
};

function catalogueDescription(item: OutsideBox): string {
	const match = item.catalogMatch;
	if (!match?.recognized) return "";
	return match.top?.description.trim() ?? "";
}

// Not in BoM rows, grouped by the catalogue IC. The description is the one
// stored on that catalogue part. `descriptions` fills a code the catalogue
// left blank. The BoM quantity check still uses the YOLO class on the box.
export function partsOutsideBom(
	items: readonly OutsideBox[],
	parts: readonly { ic: string }[] | null,
	descriptions: Readonly<Record<string, string>>,
): OutsideCount[] {
	const counts = new Map<string, number>();
	const named = new Map<string, string>();
	for (const item of items) {
		if (item.consumed === true || item.referenceMatch === false) continue;
		const ic = resolvedItemIc(item.catalogMatch, item.ic).trim();
		if (!ic) continue;
		counts.set(ic, (counts.get(ic) ?? 0) + 1);
		const fromCatalogue = catalogueDescription(item);
		if (fromCatalogue && !named.has(ic)) named.set(ic, fromCatalogue);
	}
	const known = new Set((parts ?? []).map((part) => part.ic));
	return [...counts.entries()]
		.filter(([ic]) => !known.has(ic))
		.sort((left, right) => left[0].localeCompare(right[0]))
		.map(([ic, count]) => ({
			ic,
			count,
			des: named.get(ic) || descriptions[ic]?.trim() || "",
		}));
}

// Codes that still need the description endpoint. A catalogue description,
// and the "No Match" label, are not posted.
export function inventoryCodesOutsideBom(
	rows: readonly { ic: string; des?: string }[],
): string[] {
	return rows
		.filter((row) => row.ic !== "No Match" && !(row.des ?? "").trim())
		.map((row) => row.ic);
}
