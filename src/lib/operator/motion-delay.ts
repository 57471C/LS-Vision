// Operator pause after a barcode or a typed job card is accepted, before detect_image.
// Settings > Capture stores motionDelayMs. A missing value is 400.
// The barcode read and the frozen frame do not use this number.

export const DEFAULT_MOTION_DELAY_MS = 400;
export const MAX_MOTION_DELAY_MS = 2000;

export function readMotionDelayMs(value: unknown): number {
	if (value === undefined || value === null || value === "") {
		return DEFAULT_MOTION_DELAY_MS;
	}
	const number = typeof value === "number" ? value : Number(value);
	if (!Number.isFinite(number)) return DEFAULT_MOTION_DELAY_MS;
	const rounded = Math.round(number);
	return Math.min(MAX_MOTION_DELAY_MS, Math.max(0, rounded));
}
