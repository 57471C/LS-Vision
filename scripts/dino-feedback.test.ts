// DINOv2 preview feedback. Run with:
// node scripts/dino-feedback.test.ts

import {
	clampDinoTopN,
	dinoDisagrees,
	dinoMatchChoice,
	dinoMatchLabel,
	dinoMatchLine,
	readDinoMatches,
} from "../src/lib/dino-feedback.ts";
import { detectionsFromBody } from "../src/lib/preview-detect.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

check("missing count stays at 3", clampDinoTopN(undefined) === 3);
check("a fraction stays at 3", clampDinoTopN(3.5) === 3);
check("zero clamps to 1", clampDinoTopN(0) === 1);
check("a large count clamps to 8", clampDinoTopN(40) === 8);
check("five is kept", clampDinoTopN(5) === 5);

const matches = readDinoMatches([
	{ ic: " 2ERGM22 ", description: " Hinge ", score: 0.812 },
	{ ic: "", score: 0.9 },
	{ ic: "ACH01", description: "Bracket", score: Number.NaN },
	{ ic: "M12-FC", score: 0.694 },
]);
check("blank and non-finite rows are dropped", matches.length === 2);
check("a description is trimmed", dinoMatchLabel(matches[0]) === "Hinge");
check(
	"a blank description keeps the item code",
	dinoMatchLabel(matches[1]) === "M12-FC",
);
check(
	"the line shows descriptions and two decimals",
	dinoMatchLine(matches) === "Hinge 0.81  M12-FC 0.69",
	dinoMatchLine(matches),
);
check(
	"a different top match is the correction signal",
	dinoDisagrees("BOLT", matches),
);
check(
	"the same top match is not highlighted",
	!dinoDisagrees("2ERGM22", matches),
);
check("an empty list does not disagree", !dinoDisagrees("BOLT", []));
check(
	"the choice shows the part type",
	dinoMatchChoice({
		ic: "2ERGM22",
		description: "M6 hex",
		partType: "cable_tie",
		score: 0.812,
	}) === "cable tie — M6 hex  0.81",
);
check(
	"the same part type is not highlighted",
	!dinoDisagrees("BOLT", [
		{ ic: "2ERGM22", description: "M6 hex", partType: "bolt", score: 0.8 },
	]),
);
check(
	"a different part type is highlighted",
	dinoDisagrees("BOLT", [
		{ ic: "2ERGM22", description: "M6 nut", partType: "nut", score: 0.8 },
	]),
);

const boxes = detectionsFromBody({
	detections: [
		{
			ic: "BOLT",
			confidence: 0.4,
			bbox: [1, 2, 8, 9],
			dinoMatches: [{ ic: "2ERGM22", description: " Hinge ", score: 0.81 }],
		},
	],
});
check(
	"the preview box keeps the catalogue matches",
	boxes[0]?.dinoMatches[0]?.ic === "2ERGM22" &&
		boxes[0]?.dinoMatches[0]?.description === "Hinge" &&
		boxes[0]?.dinoMatches[0]?.score === 0.81,
);
check(
	"a box without the field has no matches",
	detectionsFromBody({
		detections: [{ ic: "NUT", confidence: 0.2, bbox: [0, 0, 4, 4] }],
	})[0]?.dinoMatches.length === 0,
);

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
