// Metrology settings and the mismatch note. Run with:
// node scripts/metrology.test.ts

import {
	METROLOGY_DEFAULTS,
	metrologyDocument,
	mismatchNote,
	readMetrology,
	readMetrologyResult,
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

if (failed > 0) {
	throw new Error(`${failed} metrology checks failed`);
}
console.log("metrology tests passed");
