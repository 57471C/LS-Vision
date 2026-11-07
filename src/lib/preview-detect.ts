// Preview Detect payload. A full frame is one object. Saved slots are an array
// of those objects. Indices follow the payload so a reference save still
// matches the held embeddings.

export type PreviewDinoMatch = {
	ic: string;
	description: string;
	partType: string;
	score: number;
};

export type PreviewBox = {
	ic: string;
	confidence: number;
	bbox: [number, number, number, number];
	embed: boolean;
	twoPass: unknown;
	metrology: unknown;
	dinoMatches: PreviewDinoMatch[];
	slot: number | null;
	index: number;
};

// Same shape as dino-feedback.readDinoMatches. This file stays free of
// imports so the node payload test can load it directly.
function readDinoMatches(value: unknown): PreviewDinoMatch[] {
	if (!Array.isArray(value)) return [];
	const matches: PreviewDinoMatch[] = [];
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
		const description =
			typeof row.description === "string" ? row.description.trim() : "";
		const partType =
			typeof row.partType === "string" ? row.partType.trim() : "";
		matches.push({ ic, description, partType, score: row.score });
	}
	return matches;
}

function finite(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value);
}

function slotNumber(value: unknown): number | null {
	if (typeof value !== "number" || !Number.isInteger(value)) return null;
	if (value < 1 || value > 4) return null;
	return value;
}

function boxFrom(
	item: unknown,
	slot: number | null,
	index: number,
): PreviewBox | null {
	if (!item || typeof item !== "object") return null;
	const row = item as {
		ic?: unknown;
		confidence?: unknown;
		bbox?: unknown;
		embed?: unknown;
		twoPass?: unknown;
		metrology?: unknown;
		dinoMatches?: unknown;
	};
	const bbox = Array.isArray(row.bbox) ? row.bbox : [];
	if (bbox.length < 4) return null;
	const [rawX1, rawY1, rawX2, rawY2] = bbox;
	if (![rawX1, rawY1, rawX2, rawY2].every(finite)) return null;
	const x1 = Math.min(rawX1, rawX2);
	const y1 = Math.min(rawY1, rawY2);
	const x2 = Math.max(rawX1, rawX2);
	const y2 = Math.max(rawY1, rawY2);
	if (x2 <= x1 || y2 <= y1) return null;
	return {
		ic: typeof row.ic === "string" ? row.ic : "",
		confidence: typeof row.confidence === "number" ? row.confidence : 0,
		bbox: [x1, y1, x2, y2],
		embed: row.embed === true,
		twoPass: row.twoPass,
		metrology: row.metrology,
		dinoMatches: readDinoMatches(row.dinoMatches),
		slot,
		index,
	};
}

function boxesFromEntry(
	entry: unknown,
	slot: number | null,
	nextIndex: { value: number },
): PreviewBox[] {
	if (!entry || typeof entry !== "object") return [];
	const detections = (entry as { detections?: unknown }).detections;
	if (!Array.isArray(detections)) return [];
	const boxes: PreviewBox[] = [];
	for (const item of detections) {
		const index = nextIndex.value;
		nextIndex.value += 1;
		const box = boxFrom(item, slot, index);
		if (box) boxes.push(box);
	}
	return boxes;
}

// Failure is one object with `message`. Slots are an array and have no message.
export function detectFailure(body: unknown): string {
	if (!body || typeof body !== "object" || Array.isArray(body)) return "";
	const message = (body as { message?: unknown }).message;
	return typeof message === "string" ? message.trim() : "";
}

export function detectionsFromBody(body: unknown): PreviewBox[] {
	const nextIndex = { value: 0 };
	if (Array.isArray(body)) {
		const boxes: PreviewBox[] = [];
		for (const entry of body) {
			if (!entry || typeof entry !== "object") continue;
			const slot = slotNumber((entry as { slot?: unknown }).slot);
			boxes.push(...boxesFromEntry(entry, slot, nextIndex));
		}
		return boxes;
	}
	if (!body || typeof body !== "object") return [];
	return boxesFromEntry(body, null, nextIndex);
}

export function multiSlot(boxes: readonly PreviewBox[]): boolean {
	const slots = new Set<number>();
	for (const box of boxes) {
		if (box.slot !== null) slots.add(box.slot);
	}
	return slots.size > 1;
}
