// Optional caliper result attached to a catalogue match.
// Settings > Capture stores the same object in vision/config.local.json.
// A missing object stays off. The overlay reads only a dimension mismatch.

export type MetrologyStatus = "pass" | "DIMENSION_MISMATCH";
export type ThresholdMode = "otsu" | "fixed";

export type MetrologyResult = {
	status: MetrologyStatus;
	lengthMm: number;
	diameterMm: number;
	expectedLengthMm?: number;
	expectedDiameterMm?: number;
	expectedLabel?: string;
	note?: string;
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
	};
}

// Empty unless this box failed the length or diameter check.
export function mismatchNote(value: unknown): string {
	const result = readMetrologyResult(value);
	if (!result) return "";
	if (result.status !== "DIMENSION_MISMATCH") return "";
	return result.note?.trim() ?? "";
}
