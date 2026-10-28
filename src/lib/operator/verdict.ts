// Operator pass rule. A green PASS needs a loaded BoM and a full part count.
// Kit found / expected is not part of this check.

import type { Part } from "$lib/stores/operator";

export type InspectionVerdict = "pass" | "fail" | "incomplete";

type CountedDetection = {
	ic: string;
	consumed?: boolean;
	referenceMatch?: boolean;
	twoPass?: {
		status?: string;
		resolvedIc?: string | null;
	};
};

// A child box used by a matching part rule is not a BoM part.
// A two-pass box counts only when it matched the active BoM.
// Without that result, a box with saved references counts only when its cosine
// is above the threshold.
function countsTowardBom(item: CountedDetection): boolean {
	if (item.consumed === true) return false;
	if (item.twoPass) {
		return (
			item.twoPass.status === "matched_bom" && !!item.twoPass.resolvedIc?.trim()
		);
	}
	return item.ic !== "" && item.referenceMatch !== false;
}

function countedIc(item: CountedDetection): string {
	if (item.twoPass?.status === "matched_bom") {
		return item.twoPass.resolvedIc?.trim() ?? "";
	}
	return item.ic;
}

export type LineTally = "pass" | "short" | "extra";

// Exact quantity. Fewer boxes than the BoM line is short. More is extra.
export function lineTally(seen: number, expected: number): LineTally {
	if (seen < expected) return "short";
	if (seen > expected) return "extra";
	return "pass";
}

export function detectionCounts(
	items: CountedDetection[],
): Record<string, number> {
	const counts: Record<string, number> = {};
	for (const item of items) {
		if (!countsTowardBom(item)) continue;
		const ic = countedIc(item);
		if (!ic) continue;
		counts[ic] = (counts[ic] ?? 0) + 1;
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

// Units seen above the BoM quantity. A listed class never counts as a wrong part.
export function extraPartCount(
	parts: { ic: string; qty: number }[],
	counts: Record<string, number>,
): number {
	let extra = 0;
	for (const part of parts) {
		const over = (counts[part.ic] ?? 0) - part.qty;
		if (over > 0) extra += over;
	}
	return extra;
}

// A class is unexpected only when a BoM is loaded and the class is not on it.
// A two-pass wrong part or unrecognised box is unexpected. A BoM match is not.
// Extra boxes of a listed class, including 2VDSWN02, are a quantity check.
export function hasUnexpected(
	items: CountedDetection[],
	parts: { ic: string }[],
): boolean {
	if (!parts.length) return false;
	const known = new Set(parts.map((part) => part.ic));
	return items.some((item) => {
		if (item.consumed === true) return false;
		if (item.twoPass) {
			if (item.twoPass.status === "matched_bom") {
				const ic = item.twoPass.resolvedIc?.trim() ?? "";
				return ic === "" || !known.has(ic);
			}
			return (
				item.twoPass.status === "wrong_part" ||
				item.twoPass.status === "unrecognized"
			);
		}
		return countsTowardBom(item) && !known.has(item.ic);
	});
}

export function inspectionVerdict(args: {
	status: string;
	bomLoaded: boolean;
	detectState: string;
	detectFailed: boolean;
	parts: Part[];
	counts: Record<string, number>;
	detections: CountedDetection[];
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
