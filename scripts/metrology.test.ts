// Metrology settings and the mismatch note. Run with:
// node scripts/metrology.test.ts

import {
	layoutDimensions,
	METROLOGY_DEFAULTS,
	metrologyDocument,
	mismatchNote,
	readMetrology,
	readMetrologyResult,
	scaleMetrology,
} from "../src/lib/metrology.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const missing = readMetrology(undefined);
check("missing settings stay off", missing.enabled === false);
check(
	"default margin is 5",
	missing.marginPercent === METROLOGY_DEFAULTS.marginPercent,
);
check("default mode is otsu", missing.thresholdMode === "otsu");
check("default fixed threshold is 128", missing.fixedThreshold === 128);
check("default min area is 200", missing.minContourAreaPx === 200);
check("default length tolerance is 1.5", missing.lengthToleranceMm === 1.5);
check("default diameter tolerance is 0.8", missing.diameterToleranceMm === 0.8);

const clamped = readMetrology({
	enabled: true,
	marginPercent: 250,
	thresholdMode: "Fixed",
	fixedThreshold: 400,
	minContourAreaPx: -4,
	lengthToleranceMm: 2.25,
	diameterToleranceMm: 0.8,
});
check("enabled stays on", clamped.enabled);
check("margin clamps to 100", clamped.marginPercent === 100);
check("fixed mode is accepted", clamped.thresholdMode === "fixed");
check(
	"fixed threshold outside 0-255 falls back to 128",
	clamped.fixedThreshold === 128,
);
check("negative min area clamps to 0", clamped.minContourAreaPx === 0);
check("length tolerance is kept", clamped.lengthToleranceMm === 2.25);

const saved = metrologyDocument(clamped);
check(
	"saved document matches the clamped settings",
	saved.fixedThreshold === 128 && saved.enabled,
);

const mismatch = {
	status: "DIMENSION_MISMATCH",
	lengthMm: 25.2,
	diameterMm: 6,
	expectedLengthMm: 40,
	expectedDiameterMm: 6,
	expectedLabel: "M6x40",
	note: "Expected M6x40, measured 25.2 mm",
};
check(
	"mismatch note is the expected sentence",
	mismatchNote(mismatch) === "Expected M6x40, measured 25.2 mm",
);
check(
	"a pass has no note",
	mismatchNote({
		status: "pass",
		lengthMm: 40,
		diameterMm: 6,
		note: "ignored",
	}) === "",
);
check("an unrecognized box has no note", mismatchNote(undefined) === "");
check(
	"a result without numbers is dropped",
	readMetrologyResult({ status: "DIMENSION_MISMATCH" }) === undefined,
);

const spanned = readMetrologyResult({
	status: "pass",
	lengthMm: 25.2,
	diameterMm: 6.1,
	spans: [
		{ kind: "length", text: "25.2 mm", x1: 10, y1: 50, x2: 110, y2: 50 },
		{ kind: "diameter", text: "Ø 6.1 mm", x1: 60, y1: 30, x2: 60, y2: 70 },
		{ kind: "", text: "no", x1: 0, y1: 0, x2: 1, y2: 1 },
	],
});
check("a measured span is kept", spanned?.spans?.length === 2);
check("a blank span is dropped", spanned?.spans?.[0]?.text === "25.2 mm");

const scaled = scaleMetrology(spanned, 0.5, 2);
check(
	"spans scale with the preview",
	scaled?.spans?.[0]?.x2 === 55 && scaled?.spans?.[0]?.y2 === 100,
);

const part = { x: 10, y: 30, w: 100, h: 40 };
const classLabel = { x: 10, y: 8, w: 80, h: 16 };
const drawn = layoutDimensions(
	spanned?.spans ?? [],
	false,
	[classLabel],
	{ x: 0, y: 0, w: 220, h: 140 },
	1,
);
check("both measured sides are drawn", drawn.length === 2);
check(
	"a pass line stays neutral",
	drawn.every((mark) => mark.stroke === "#f5f5f5"),
);
check(
	"labels show the measured value",
	drawn.some((mark) => mark.text === "25.2 mm") &&
		drawn.some((mark) => mark.text === "Ø 6.1 mm"),
);
check(
	"arrowheads sit on both ends",
	drawn.every(
		(mark) =>
			mark.head1.startsWith(`${mark.x1},${mark.y1}`) &&
			mark.head2.startsWith(`${mark.x2},${mark.y2}`),
	),
);

function hits(
	a: { x: number; y: number; w: number; h: number },
	b: typeof part,
) {
	return (
		a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
	);
}

check(
	"labels stay off the part and the class text",
	drawn.every((mark) => {
		const label = {
			x: mark.labelX,
			y: mark.labelY,
			w: mark.labelW,
			h: mark.labelH,
		};
		return !hits(label, part) && !hits(label, classLabel);
	}),
);

const mismatchLines = layoutDimensions(
	spanned?.spans ?? [],
	true,
	[classLabel],
	{ x: 0, y: 0, w: 220, h: 140 },
	1,
);
check(
	"a mismatch tints the line",
	mismatchLines.every(
		(mark) => mark.stroke === "#f59e0b" && mark.labelFill === "#0a0a0a",
	),
);
check(
	"no spans draw nothing",
	layoutDimensions([], false, [], null, 1).length === 0,
);

if (failed > 0) {
	throw new Error(`${failed} metrology checks failed`);
}
console.log("metrology tests passed");
