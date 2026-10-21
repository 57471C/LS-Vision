// Operator pass rule. A green PASS needs a loaded BoM and a full part count.
// Kit found / expected is not part of this check.

import type { Part } from "$lib/stores/operator";

export type InspectionVerdict = "pass" | "fail" | "incomplete";

export function detectionCounts(
	items: { ic: string }[],
): Record<string, number> {
	const counts: Record<string, number> = {};
	for (const item of items) {
		if (!item.ic) continue;
		counts[item.ic] = (counts[item.ic] ?? 0) + 1;
	}
	return counts;
}

// A line passes when the detected count meets or exceeds the BoM quantity.
export function quantitiesMeet(
	parts: Part[],
	counts: Record<string, number>,
): boolean {
	if (!parts.length) return false;
	return parts.every((part) => (counts[part.ic] ?? 0) >= part.qty);
}

// A class is unexpected only when a BoM is loaded and the class is not on it.
// Extra boxes of a listed class, including 2VDSWN02, are a quantity check.
export function hasUnexpected(
	items: { ic: string }[],
	parts: { ic: string }[],
): boolean {
	if (!parts.length) return false;
	const known = new Set(parts.map((part) => part.ic));
	return items.some((item) => item.ic !== "" && !known.has(item.ic));
}

export function inspectionVerdict(args: {
	status: string;
	bomLoaded: boolean;
	detectState: string;
	detectFailed: boolean;
	parts: Part[];
	counts: Record<string, number>;
	detections: { ic: string }[];
}): InspectionVerdict {
	if (args.status !== "pass" || !args.bomLoaded) return "fail";
	if (args.detectState !== "done") return "incomplete";
	if (
		args.detectFailed ||
		!quantitiesMeet(args.parts, args.counts) ||
		hasUnexpected(args.detections, args.parts)
	) {
		return "fail";
	}
	return "pass";
}
