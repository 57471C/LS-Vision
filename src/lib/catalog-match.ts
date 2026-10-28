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
