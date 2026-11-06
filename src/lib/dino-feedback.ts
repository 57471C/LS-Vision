// Top catalogue matches shown under a preview detection.
// The operator barcode path does not read this.

export const DINO_TOP_MIN = 1;
export const DINO_TOP_MAX = 8;
export const DINO_TOP_DEFAULT = 3;

export type DinoMatch = {
	ic: string;
	description: string;
	partType: string;
	score: number;
};

// Missing or invalid values stay at the default. Whole numbers clamp to 1–8.
export function clampDinoTopN(value: unknown): number {
	const number = typeof value === "number" ? value : Number.NaN;
	if (!Number.isInteger(number)) return DINO_TOP_DEFAULT;
	return Math.min(DINO_TOP_MAX, Math.max(DINO_TOP_MIN, number));
}

export function readDinoMatches(value: unknown): DinoMatch[] {
	if (!Array.isArray(value)) return [];
	const matches: DinoMatch[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const row = item as {
			ic?: unknown;
			description?: unknown;
			partType?: unknown;
			score?: unknown;
		};
		const ic = typeof row.ic === "string" ? row.ic.trim() : "";
		if (!ic) continue;
		if (typeof row.score !== "number" || !Number.isFinite(row.score)) continue;
		const description = typeof row.description === "string" ? row.description.trim() : "";
		const partType = typeof row.partType === "string" ? row.partType.trim() : "";
		matches.push({ ic, description, partType, score: row.score });
	}
	return matches;
}

function partWords(value: string): string {
	return value.trim().replace(/[_-]+/g, " ").replace(/\s+/g, " ");
}

// The catalogue description. A blank description keeps the item code.
export function dinoMatchLabel(match: DinoMatch): string {
	const description = match.description.trim();
	return description.length > 0 ? description : match.ic;
}

// `Hinge 0.81  Bracket 0.74  Rail 0.69`
export function dinoMatchLine(matches: readonly DinoMatch[]): string {
	return matches.map((item) => `${dinoMatchLabel(item)} ${item.score.toFixed(2)}`).join("  ");
}

// Dropdown label: `bolt — M6 hex  0.81`.
export function dinoMatchChoice(match: DinoMatch): string {
	const type = partWords(match.partType);
	const name = dinoMatchLabel(match);
	const score = match.score.toFixed(2);
	if (!type || type.toLowerCase() === name.toLowerCase()) return `${name}  ${score}`;
	return `${type} — ${name}  ${score}`;
}

function partKey(value: string): string {
	return partWords(value).toLowerCase();
}

// True when the closest catalogue type is not the detector class.
// A match with no type falls back to comparing item codes.
export function dinoDisagrees(
	classIc: string,
	matches: readonly DinoMatch[],
): boolean {
	const top = matches[0];
	if (!top) return false;
	const kind = partKey(top.partType);
	const label = partKey(classIc);
	if (kind && label) return kind !== label;
	const ic = top.ic.trim();
	const raw = classIc.trim();
	return ic.length > 0 && raw.length > 0 && ic !== raw;
}
