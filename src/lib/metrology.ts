// Optional caliper result attached to a catalogue match.
// Settings > Capture stores the same object in vision/config.local.json.
// A missing object stays off. Spans are the measured sides, in the same
// pixels as the detection box. The overlay draws one line per span.

export type MetrologyStatus = "pass" | "DIMENSION_MISMATCH";
export type ThresholdMode = "otsu" | "fixed";

export type MetrologySpan = {
	kind: string;
	text: string;
	x1: number;
	y1: number;
	x2: number;
	y2: number;
};

export type MetrologyResult = {
	status: MetrologyStatus;
	lengthMm: number;
	diameterMm: number;
	expectedLengthMm?: number;
	expectedDiameterMm?: number;
	expectedLabel?: string;
	note?: string;
	spans?: MetrologySpan[];
};

export type OverlayBox = {
	x: number;
	y: number;
	w: number;
	h: number;
};

export type DimensionMark = {
	key: string;
	kind: string;
	text: string;
	x1: number;
	y1: number;
	x2: number;
	y2: number;
	head1: string;
	head2: string;
	labelX: number;
	labelY: number;
	labelW: number;
	labelH: number;
	font: number;
	padX: number;
	radius: number;
	stroke: string;
	labelFill: string;
	mismatch: boolean;
};

export type MetrologySettings = {
	enabled: boolean;
	marginPercent: number;
	thresholdMode: ThresholdMode;
	fixedThreshold: number;
	minContourAreaPx: number;
	lengthToleranceMm: number;
	diameterToleranceMm: number;
};

export const METROLOGY_DEFAULTS: MetrologySettings = {
	enabled: false,
	marginPercent: 5,
	thresholdMode: "otsu",
	fixedThreshold: 128,
	minContourAreaPx: 200,
	lengthToleranceMm: 1.5,
	diameterToleranceMm: 0.8,
};

function finite(value: unknown): number | null {
	const number = typeof value === "number" ? value : Number(value);
	return Number.isFinite(number) ? number : null;
}

function clamp(
	value: unknown,
	min: number,
	max: number,
	fallback: number,
): number {
	const number = finite(value);
	if (number === null) return fallback;
	return Math.min(max, Math.max(min, number));
}

function fixedThreshold(value: unknown): number {
	const number = finite(value);
	if (number === null) return METROLOGY_DEFAULTS.fixedThreshold;
	const rounded = Math.round(number);
	if (rounded < 0 || rounded > 255) return METROLOGY_DEFAULTS.fixedThreshold;
	return rounded;
}

function thresholdMode(value: unknown): ThresholdMode {
	return typeof value === "string" && value.trim().toLowerCase() === "fixed"
		? "fixed"
		: "otsu";
}

export function readMetrology(value: unknown): MetrologySettings {
	if (!value || typeof value !== "object" || Array.isArray(value)) {
		return { ...METROLOGY_DEFAULTS };
	}
	const row = value as Record<string, unknown>;
	return {
		enabled: row.enabled === true,
		marginPercent: clamp(
			row.marginPercent,
			0,
			100,
			METROLOGY_DEFAULTS.marginPercent,
		),
		thresholdMode: thresholdMode(row.thresholdMode),
		fixedThreshold: fixedThreshold(row.fixedThreshold),
		minContourAreaPx: clamp(
			row.minContourAreaPx,
			0,
			1_000_000,
			METROLOGY_DEFAULTS.minContourAreaPx,
		),
		lengthToleranceMm: clamp(
			row.lengthToleranceMm,
			0,
			1_000,
			METROLOGY_DEFAULTS.lengthToleranceMm,
		),
		diameterToleranceMm: clamp(
			row.diameterToleranceMm,
			0,
			1_000,
			METROLOGY_DEFAULTS.diameterToleranceMm,
		),
	};
}

export function metrologyDocument(
	settings: MetrologySettings,
): MetrologySettings {
	return readMetrology(settings);
}

function positive(value: unknown): number | undefined {
	const number = finite(value);
	if (number === null || number <= 0) return undefined;
	return number;
}

export function readMetrologyResult(
	value: unknown,
): MetrologyResult | undefined {
	if (!value || typeof value !== "object" || Array.isArray(value))
		return undefined;
	const row = value as Record<string, unknown>;
	const status =
		row.status === "pass" || row.status === "DIMENSION_MISMATCH"
			? row.status
			: null;
	const lengthMm = finite(row.lengthMm);
	const diameterMm = finite(row.diameterMm);
	if (!status || lengthMm === null || diameterMm === null) return undefined;
	const expectedLengthMm = positive(row.expectedLengthMm);
	const expectedDiameterMm = positive(row.expectedDiameterMm);
	const expectedLabel =
		typeof row.expectedLabel === "string" ? row.expectedLabel : undefined;
	const note = typeof row.note === "string" ? row.note : undefined;
	return {
		status,
		lengthMm,
		diameterMm,
		expectedLengthMm,
		expectedDiameterMm,
		expectedLabel,
		note,
		spans: readSpans(row.spans),
	};
}

function readSpan(value: unknown): MetrologySpan | null {
	if (!value || typeof value !== "object" || Array.isArray(value)) return null;
	const row = value as Record<string, unknown>;
	const kind = typeof row.kind === "string" ? row.kind.trim() : "";
	const text = typeof row.text === "string" ? row.text.trim() : "";
	const x1 = finite(row.x1);
	const y1 = finite(row.y1);
	const x2 = finite(row.x2);
	const y2 = finite(row.y2);
	if (
		!kind ||
		!text ||
		x1 === null ||
		y1 === null ||
		x2 === null ||
		y2 === null
	) {
		return null;
	}
	return { kind, text, x1, y1, x2, y2 };
}

function readSpans(value: unknown): MetrologySpan[] {
	if (!Array.isArray(value)) return [];
	const spans: MetrologySpan[] = [];
	for (const item of value) {
		const span = readSpan(item);
		if (span) spans.push(span);
	}
	return spans;
}

export function metrologySpans(value: unknown): MetrologySpan[] {
	return readMetrologyResult(value)?.spans ?? [];
}

export function scaleMetrology(
	result: MetrologyResult | undefined,
	sx: number,
	sy: number,
): MetrologyResult | undefined {
	if (!result?.spans || result.spans.length === 0) return result;
	if (!Number.isFinite(sx) || !Number.isFinite(sy)) return result;
	return {
		...result,
		spans: result.spans.map((span) => ({
			...span,
			x1: span.x1 * sx,
			y1: span.y1 * sy,
			x2: span.x2 * sx,
			y2: span.y2 * sy,
		})),
	};
}

// Empty unless this box failed the length or diameter check.
export function mismatchNote(value: unknown): string {
	const result = readMetrologyResult(value);
	if (!result) return "";
	if (result.status !== "DIMENSION_MISMATCH") return "";
	return result.note?.trim() ?? "";
}

const PASS_STROKE = "#f5f5f5";
const MISMATCH_STROKE = "#f59e0b";
const LABEL_FILL = "#0a0a0a";

function boxAt(cx: number, cy: number, w: number, h: number): OverlayBox {
	return { x: cx - w / 2, y: cy - h / 2, w, h };
}

function overlaps(a: OverlayBox, b: OverlayBox): boolean {
	return (
		a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
	);
}

function containsBox(inner: OverlayBox, outer: OverlayBox): boolean {
	return (
		inner.x >= outer.x &&
		inner.y >= outer.y &&
		inner.x + inner.w <= outer.x + outer.w &&
		inner.y + inner.h <= outer.y + outer.h
	);
}

function clampBox(box: OverlayBox, bounds: OverlayBox): OverlayBox {
	if (box.w >= bounds.w || box.h >= bounds.h) return box;
	return {
		x: Math.min(Math.max(bounds.x, box.x), bounds.x + bounds.w - box.w),
		y: Math.min(Math.max(bounds.y, box.y), bounds.y + bounds.h - box.h),
		w: box.w,
		h: box.h,
	};
}

function arrowPoints(
	tipX: number,
	tipY: number,
	ux: number,
	uy: number,
	nx: number,
	ny: number,
	length: number,
	half: number,
): string {
	const bx = tipX - ux * length;
	const by = tipY - uy * length;
	const lx = bx + nx * half;
	const ly = by + ny * half;
	const rx = bx - nx * half;
	const ry = by - ny * half;
	return `${tipX},${tipY} ${lx},${ly} ${rx},${ry}`;
}

function layoutSpan(
	span: MetrologySpan,
	index: number,
	spans: readonly MetrologySpan[],
	mismatch: boolean,
	avoids: readonly OverlayBox[],
	bounds: OverlayBox | null,
	unit: number,
): DimensionMark | null {
	const dx = span.x2 - span.x1;
	const dy = span.y2 - span.y1;
	const length = Math.hypot(dx, dy);
	if (!(length > 1)) return null;
	const ux = dx / length;
	const uy = dy / length;
	const nx = -uy;
	const ny = ux;
	const font = 10 * unit;
	const padX = 4 * unit;
	const padY = 2 * unit;
	const gap = 6 * unit;
	const labelH = font + padY * 2;
	const labelW = Math.ceil(span.text.length * font * 0.62) + padX * 2;
	const other = spans.find((_, itemIndex) => itemIndex !== index);
	const otherHalf = other
		? Math.hypot(other.x2 - other.x1, other.y2 - other.y1) / 2
		: 0;
	const halfNormal = Math.abs(nx) * labelW * 0.5 + Math.abs(ny) * labelH * 0.5;
	const halfAlong = Math.abs(ux) * labelW * 0.5 + Math.abs(uy) * labelH * 0.5;
	const offset = otherHalf + gap + halfNormal;
	const arrowLen = Math.min(7 * unit, length * 0.22);
	const arrowHalf = arrowLen * 0.45;
	const endOffset = arrowLen + gap + halfAlong;
	const mx = (span.x1 + span.x2) / 2;
	const my = (span.y1 + span.y2) / 2;
	const candidates = [
		{ cx: mx + nx * offset, cy: my + ny * offset, bias: 0 },
		{ cx: mx - nx * offset, cy: my - ny * offset, bias: 0 },
		{ cx: span.x2 + ux * endOffset, cy: span.y2 + uy * endOffset, bias: 1 },
		{ cx: span.x1 - ux * endOffset, cy: span.y1 - uy * endOffset, bias: 1 },
	];
	let best: OverlayBox | null = null;
	let bestScore = Number.POSITIVE_INFINITY;
	const anchor = avoids[0] ?? null;
	for (const candidate of candidates) {
		const raw = boxAt(candidate.cx, candidate.cy, labelW, labelH);
		const placed = bounds ? clampBox(raw, bounds) : raw;
		let score = candidate.bias;
		if (bounds && !containsBox(raw, bounds)) score += 20;
		for (const avoid of avoids) {
			if (overlaps(placed, avoid)) score += 10;
		}
		if (anchor) {
			score -=
				Math.hypot(
					candidate.cx - (anchor.x + anchor.w / 2),
					candidate.cy - (anchor.y + anchor.h / 2),
				) * 0.01;
		}
		if (score < bestScore) {
			bestScore = score;
			best = placed;
		}
	}
	if (!best) return null;
	return {
		key: `${span.kind}:${index}`,
		kind: span.kind,
		text: span.text,
		x1: span.x1,
		y1: span.y1,
		x2: span.x2,
		y2: span.y2,
		head1: arrowPoints(span.x1, span.y1, -ux, -uy, nx, ny, arrowLen, arrowHalf),
		head2: arrowPoints(span.x2, span.y2, ux, uy, nx, ny, arrowLen, arrowHalf),
		labelX: best.x,
		labelY: best.y,
		labelW,
		labelH,
		font,
		padX,
		radius: 3 * unit,
		stroke: mismatch ? MISMATCH_STROKE : PASS_STROKE,
		labelFill: LABEL_FILL,
		mismatch,
	};
}

// One thin dimension per measured side. The label sits off the part and away
// from the class label. An empty list draws nothing.
export function layoutDimensions(
	spans: readonly MetrologySpan[],
	mismatch: boolean,
	avoids: readonly OverlayBox[],
	bounds: OverlayBox | null,
	unit: number,
): DimensionMark[] {
	const safeUnit = Number.isFinite(unit) && unit > 0 ? unit : 1;
	const marks: DimensionMark[] = [];
	const taken: OverlayBox[] = [];
	for (const [index, span] of spans.entries()) {
		const mark = layoutSpan(
			span,
			index,
			spans,
			mismatch,
			[...avoids, ...taken],
			bounds,
			safeUnit,
		);
		if (!mark) continue;
		taken.push({
			x: mark.labelX,
			y: mark.labelY,
			w: mark.labelW,
			h: mark.labelH,
		});
		marks.push(mark);
	}
	return marks;
}
