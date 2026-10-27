// Admin preview marquee. A drag large enough to see is a crop. A short press
// on a box still selects that box for the catalogue.

export type MarqueeMark = {
	index: number;
	embed: boolean;
	x: number;
	y: number;
	w: number;
	h: number;
};

export type MarqueeGesture =
	| { type: "marquee" }
	| { type: "select"; index: number }
	| { type: "ignore" };

/** The topmost box under the point. Later entries are painted above earlier ones. */
export function topMarkAt<T extends MarqueeMark>(
	marks: readonly T[],
	x: number,
	y: number,
): T | null {
	for (let i = marks.length - 1; i >= 0; i -= 1) {
		const mark = marks[i];
		if (
			x >= mark.x &&
			x <= mark.x + mark.w &&
			y >= mark.y &&
			y <= mark.y + mark.h
		) {
			return mark;
		}
	}
	return null;
}

/** A box in both directions is a crop. A short press selects an embeddable box. */
export function marqueeGesture(
	width: number,
	height: number,
	mark: MarqueeMark | null,
	minimum: number,
): MarqueeGesture {
	if (
		Number.isFinite(width) &&
		Number.isFinite(height) &&
		width >= minimum &&
		height >= minimum
	) {
		return { type: "marquee" };
	}
	if (mark?.embed) return { type: "select", index: mark.index };
	return { type: "ignore" };
}
