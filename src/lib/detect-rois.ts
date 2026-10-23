// Up to four part-detection regions. Each value is a fraction of the frame.
// Barcode ROI is a separate single region and is not stored here.

export type DetectRoi = {
	x: number;
	y: number;
	w: number;
	h: number;
	// A new box starts on. A missing flag on an older file is also on.
	enabled: boolean;
};

export const DETECT_SLOT_COUNT = 4;

export const SLOT_COLORS = [
	{ stroke: "#fbbf24", bright: "#fde68a", fill: "rgba(251,191,36,0.22)" },
	{ stroke: "#22d3ee", bright: "#a5f3fc", fill: "rgba(34,211,238,0.22)" },
	{ stroke: "#a3e635", bright: "#d9f99d", fill: "rgba(163,230,53,0.22)" },
	{ stroke: "#e879f9", bright: "#f5d0fe", fill: "rgba(232,121,249,0.22)" },
] as const;

export function slotColor(slot: number): (typeof SLOT_COLORS)[number] {
	const index = slot - 1;
	if (index < 0 || index >= SLOT_COLORS.length) return SLOT_COLORS[0];
	return SLOT_COLORS[index];
}

export function emptyDetectRois(): (DetectRoi | null)[] {
	return [null, null, null, null];
}

export function parseDetectRoi(value: unknown): DetectRoi | null {
	if (!value || typeof value !== "object") return null;
	const row = value as Record<string, unknown>;
	const x = typeof row.x === "number" ? row.x : Number.NaN;
	const y = typeof row.y === "number" ? row.y : Number.NaN;
	const w = typeof row.w === "number" ? row.w : Number.NaN;
	const h = typeof row.h === "number" ? row.h : Number.NaN;
	if (![x, y, w, h].every((part) => Number.isFinite(part))) return null;
	if (w < 0.01 || h < 0.01) return null;
	if (x < 0 || y < 0 || w > 1 || h > 1) return null;
	if (x + w > 1.0001 || y + h > 1.0001) return null;
	const enabled = typeof row.enabled === "boolean" ? row.enabled : true;
	return { x, y, w, h, enabled };
}

// `detectRois` wins when it is an array, including an array of blanks.
// Older files only have `detectRoi`, which is slot 1.
export function parseDetectRois(
	detectRois: unknown,
	detectRoi: unknown,
): (DetectRoi | null)[] {
	const slots = emptyDetectRois();
	if (Array.isArray(detectRois)) {
		const count = Math.min(DETECT_SLOT_COUNT, detectRois.length);
		for (let index = 0; index < count; index += 1) {
			slots[index] = parseDetectRoi(detectRois[index]);
		}
		return slots;
	}
	slots[0] = parseDetectRoi(detectRoi);
	return slots;
}

// One saved region still zooms the preview. Two or more stay on the full frame.
export function soleDetectRoi(
	slots: readonly (DetectRoi | null)[],
): DetectRoi | null {
	let found: DetectRoi | null = null;
	for (const roi of slots) {
		if (!roi) continue;
		if (found) return null;
		found = roi;
	}
	return found;
}

export function detectSlotNumber(value: unknown): number | null {
	const slot = typeof value === "number" ? value : Number.NaN;
	if (!Number.isInteger(slot) || slot < 1 || slot > DETECT_SLOT_COUNT)
		return null;
	return slot;
}

// Drop the cleared slot and pack every remaining region to the front.
// The list stays four long. Saving a rectangle does not use this.
export function compactClearedSlot(
	slots: readonly (DetectRoi | null)[],
	clearedSlot: number,
): (DetectRoi | null)[] {
	const next = emptyDetectRois();
	let write = 0;
	const count = Math.min(DETECT_SLOT_COUNT, slots.length);
	for (let index = 0; index < count; index += 1) {
		if (index === clearedSlot - 1) continue;
		const roi = slots[index];
		if (!roi) continue;
		next[write] = roi;
		write += 1;
	}
	return next;
}

// Flip one saved slot. A slot with no box is left unchanged.
export function withSlotEnabled(
	slots: readonly (DetectRoi | null)[],
	slot: number,
	enabled: boolean,
): (DetectRoi | null)[] {
	return slots.map((roi, index) =>
		index === slot - 1 && roi ? { ...roi, enabled } : roi,
	);
}

// Every saved box turns off. Empty slots stay empty.
export function withAllSlotsDisabled(
	slots: readonly (DetectRoi | null)[],
): (DetectRoi | null)[] {
	return slots.map((roi) => (roi ? { ...roi, enabled: false } : roi));
}

// Remaining panels keep their order and take slots 1, 2, 3, ...
export function reindexSlotResults<T extends { slot: number }>(
	panels: readonly T[],
	clearedSlot: number,
): T[] {
	return panels
		.filter((panel) => panel.slot !== clearedSlot)
		.slice()
		.sort((a, b) => a.slot - b.slot)
		.map((panel, index) => ({ ...panel, slot: index + 1 }));
}

export function panelsForSlots<T extends { detections: readonly unknown[] }>(
	slots: readonly T[],
	bomLoaded: boolean,
): T[] {
	return slots.filter((slot) => slot.detections.length > 0 || bomLoaded);
}

// One slot keeps the detection list as it is. Two or more slots are drawn
// together, and each box keeps the slot it came from.
export function overlayDetections<T>(
	detections: readonly T[],
	slots: readonly { slot: number; detections: readonly T[] }[],
): (T & { slot?: number })[] {
	if (slots.length < 2) return detections as (T & { slot?: number })[];
	const combined: (T & { slot?: number })[] = [];
	for (const panel of slots) {
		for (const item of panel.detections) {
			combined.push({ ...item, slot: panel.slot });
		}
	}
	return combined;
}
