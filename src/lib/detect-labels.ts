// Boxes for the preview Detect label file.
// Coordinates stay in snapshot pixels. The Rust command normalises them.

export type LabelBox = [number, number, number, number];

function finite(value: unknown): value is number {
	return typeof value === "number" && Number.isFinite(value);
}

function boxFromRow(item: unknown): LabelBox | null {
	if (!item || typeof item !== "object") return null;
	const bbox = (item as { bbox?: unknown }).bbox;
	if (!Array.isArray(bbox) || bbox.length < 4) return null;
	const [rawX1, rawY1, rawX2, rawY2] = bbox;
	if (!finite(rawX1) || !finite(rawY1) || !finite(rawX2) || !finite(rawY2)) {
		return null;
	}
	const x1 = Math.min(rawX1, rawX2);
	const y1 = Math.min(rawY1, rawY2);
	const x2 = Math.max(rawX1, rawX2);
	const y2 = Math.max(rawY1, rawY2);
	if (x2 <= x1 || y2 <= y1) return null;
	return [x1, y1, x2, y2];
}

function boxesFromList(value: unknown): LabelBox[] {
	if (!Array.isArray(value)) return [];
	const boxes: LabelBox[] = [];
	for (const item of value) {
		const box = boxFromRow(item);
		if (box) boxes.push(box);
	}
	return boxes;
}

// A full frame is one object. Saved slots are an array of those objects.
// An idle detect is an empty array. Either way the order is the payload order.
export function boxesFromDetect(body: unknown): LabelBox[] {
	if (Array.isArray(body)) {
		const boxes: LabelBox[] = [];
		for (const entry of body) {
			if (!entry || typeof entry !== "object") continue;
			boxes.push(
				...boxesFromList((entry as { detections?: unknown }).detections),
			);
		}
		return boxes;
	}
	if (!body || typeof body !== "object") return [];
	return boxesFromList((body as { detections?: unknown }).detections);
}
