// A preview box the catalogue did not recognise. The operator can assign an IC.

const DEFAULT_MATCH_THRESHOLD = 0.7;
const MIN_MATCH_THRESHOLD = 0.5;
const MAX_MATCH_THRESHOLD = 0.95;

export function readMatchThreshold(value: unknown): number {
	if (typeof value !== "number" || !Number.isFinite(value)) {
		return DEFAULT_MATCH_THRESHOLD;
	}
	return Math.min(MAX_MATCH_THRESHOLD, Math.max(MIN_MATCH_THRESHOLD, value));
}

// `unrecognized`, or any two-pass score under the saved match threshold.
export function catalogueMiss(twoPass: unknown, threshold: number): boolean {
	if (!twoPass || typeof twoPass !== "object") return false;
	const row = twoPass as { status?: unknown; score?: unknown };
	if (row.status === "unrecognized") return true;
	const cutoff = readMatchThreshold(threshold);
	return (
		typeof row.score === "number" &&
		Number.isFinite(row.score) &&
		row.score < cutoff
	);
}
