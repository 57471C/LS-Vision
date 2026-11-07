// Residual overlay categories. Run with:
// node scripts/residual.test.ts

import { readMotionDelayMs } from "../src/lib/operator/motion-delay.ts";
import {
	assignResiduals,
	type ResidualBox,
	residualStyle,
} from "../src/lib/operator/residual.ts";
import { detectionCounts } from "../src/lib/operator/verdict.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const parts = [
	{ ic: "2BB08C2HZ", des: "M8x20 8.8 HexBolt z", qty: 2 },
	{ ic: "2BBMNM08", des: "M8 Nut Nyloc z", qty: 1 },
];

function bolt(
	status: "matched_bom" | "wrong_part" | "unrecognized",
	ic: string | null,
	extra: Partial<ResidualBox> = {},
): ResidualBox {
	return {
		ic: "bolt",
		confidence: 0.9,
		twoPass: {
			status,
			resolvedIc: ic,
			description: ic === "2BB08C2HZ" ? "M8x20 hex bolt" : "M8x30 hex bolt",
			partType: "bolt",
			score: 0.91,
		},
		...extra,
	} as ResidualBox;
}

const boxes: ResidualBox[] = [
	bolt("matched_bom", "2BB08C2HZ"),
	bolt("matched_bom", "2BB08C2HZ", {
		metrology: {
			status: "DIMENSION_MISMATCH",
			lengthMm: 25.2,
			diameterMm: 8,
			expectedLabel: "M8x20",
			note: "Expected M8x20, measured 25.2 mm",
		},
	}),
	bolt("matched_bom", "2BB08C2HZ"),
	bolt("wrong_part", "2BB08C3HZ", {
		metrology: { status: "pass", lengthMm: 30, diameterMm: 8 },
	}),
	bolt("unrecognized", null, {
		twoPass: {
			status: "unrecognized",
			resolvedIc: null,
			description: null,
			partType: "bolt",
			score: 0.2,
		},
	}),
	{
		ic: "washer",
		referenceMatch: false,
	},
];

const tagged = assignResiduals(boxes, parts);
const kinds = tagged.map((item) => item.residual);

check(
	"the first two bolts fill the line and the third is surplus",
	kinds[0] === "matched" && kinds[1] === "near_miss" && kinds[2] === "surplus",
	kinds.join(","),
);
check(
	"a matched box keeps the item code and percent",
	residualStyle(tagged[0])?.label === "2BB08C2HZ - 91%" &&
		residualStyle(tagged[0])?.dashed === false &&
		residualStyle(tagged[0])?.color === "#16a34a",
);
check(
	"a near miss uses the metrology note",
	residualStyle(tagged[1])?.label === "Expected M8x20, measured 25.2 mm" &&
		residualStyle(tagged[1])?.color === "#dc2626" &&
		residualStyle(tagged[1])?.dashed === false,
);
check(
	"surplus is a dashed extra label",
	residualStyle(tagged[2])?.label === "EXTRA 2BB08C2HZ" &&
		residualStyle(tagged[2])?.dashed === true &&
		residualStyle(tagged[2])?.color === "#f59e0b",
);
check(
	"a wrong part with no dimensional match is rogue",
	kinds[3] === "rogue" && residualStyle(tagged[3])?.label === "2BB08C3HZ",
);
check(
	"an unrecognised box is unknown",
	kinds[4] === "rogue" &&
		residualStyle(tagged[4])?.label === "UNKNOWN" &&
		residualStyle(tagged[4])?.color === "#7c3aed",
);
check(
	"a missed embedding is rogue",
	kinds[5] === "rogue" && residualStyle(tagged[5])?.label === "UNKNOWN",
);

const sized = assignResiduals(
	[
		bolt("wrong_part", "FAST-9", {
			twoPass: {
				status: "wrong_part",
				resolvedIc: "FAST-9",
				description: "M8x20 hex bolt",
				partType: "bolt",
				score: 0.88,
			},
			metrology: { status: "pass", lengthMm: 20, diameterMm: 8 },
		}),
	],
	[{ ic: "2BB08C2HZ", des: "M8x20 8.8 HexBolt z", qty: 1 }],
);
check(
	"a wrong part that matches an open line's size stays matched",
	sized[0]?.residual === "matched" &&
		residualStyle(sized[0])?.label === "FAST-9 - 88%",
);

const before = detectionCounts(boxes);
const after = detectionCounts(tagged);
check(
	"residual does not change the seen count",
	JSON.stringify(before) === JSON.stringify(after) && before["2BB08C2HZ"] === 3,
);

const mismatchOnly = assignResiduals(
	[
		bolt("matched_bom", "2BBMNM08", {
			twoPass: {
				status: "matched_bom",
				resolvedIc: "2BBMNM08",
				description: "M8 nut",
				partType: "nut",
				score: 0.8,
			},
			metrology: { status: "DIMENSION_MISMATCH", lengthMm: 10, diameterMm: 8 },
		}),
	],
	parts,
);
check(
	"a short line with a dimension mismatch is a near miss",
	mismatchOnly[0]?.residual === "near_miss" &&
		(residualStyle(mismatchOnly[0])?.label ?? "").startsWith("WRONG LENGTH"),
);

const child = assignResiduals(
	[
		{
			ic: "bolt",
			consumed: true,
			twoPass: { status: "matched_bom", resolvedIc: "2BB08C2HZ", score: 1 },
		},
	],
	parts,
);
check("a consumed child has no residual", child[0]?.residual === undefined);

check("a missing delay is 400", readMotionDelayMs(undefined) === 400);
check("a blank delay is 400", readMotionDelayMs("") === 400);
check("text is 400", readMotionDelayMs("soon") === 400);
check("zero is kept", readMotionDelayMs(0) === 0);
check("a high delay clamps to 2000", readMotionDelayMs(2500) === 2000);
check("a negative delay clamps to 0", readMotionDelayMs(-5) === 0);
check("a fractional delay rounds", readMotionDelayMs(400.4) === 400);

if (failed > 0) {
	throw new Error(`${failed} residual checks failed`);
}
console.log("residual tests passed");
