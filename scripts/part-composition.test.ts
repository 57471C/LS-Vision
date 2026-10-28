// Hinge-plate example and the rule engine. Run with:
// node scripts/part-composition.test.ts

import {
	detectionCounts,
	extraPartCount,
	hasUnexpected,
} from "../src/lib/operator/verdict.ts";
import {
	applyPartRules,
	type PartBox,
	type PartRule,
	parsePartRules,
} from "../src/lib/part-composition.ts";

const hingeRules: PartRule[] = [
	{
		parent: "Hinge_Plate",
		child: "P02_Logo",
		condition: "contains",
		result: "2QHP02",
	},
	{
		parent: "Hinge_Plate",
		child: "P02_Logo",
		condition: "no-child",
		result: "2QHP01",
	},
];

function box(ic: string, bbox: number[], confidence = 0.9): PartBox {
	return { ic, confidence, bbox };
}

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

function classes(items: readonly PartBox[]): string[] {
	return items.map((item) => item.ic);
}

const snapshot: PartBox[] = [
	box("Hinge_Plate", [0, 0, 100, 80], 0.91),
	box("P02_Logo", [10, 10, 30, 30], 0.88),
	box("Hinge_Plate", [200, 0, 300, 80], 0.77),
	box("2BBMNM08", [400, 400, 420, 430], 0.95),
];

const composed = applyPartRules(snapshot, hingeRules);
check(
	"plate with a logo becomes 2QHP02",
	composed.detections[0]?.ic === "2QHP02",
	composed.detections[0]?.ic,
);
check(
	"logo stays P02_Logo and is marked consumed",
	composed.detections[1]?.ic === "P02_Logo" &&
		composed.detections[1]?.consumed === true,
);
check(
	"the rewritten plate and the other boxes are not consumed",
	composed.detections[0]?.consumed !== true &&
		composed.detections[2]?.consumed !== true &&
		composed.detections[3]?.consumed !== true,
);
check(
	"plate without a logo becomes 2QHP01",
	composed.detections[2]?.ic === "2QHP01",
	composed.detections[2]?.ic,
);
check(
	"other parts stay unchanged",
	composed.detections[3]?.ic === "2BBMNM08" &&
		composed.detections[3]?.confidence === 0.95,
);
check(
	"rewritten box keeps its rectangle",
	composed.detections[0]?.bbox.join(",") === "0,0,100,80",
);
check(
	"both hinge rules are logged",
	composed.log.length === 2 &&
		composed.log[0]?.includes("contains P02_Logo -> 2QHP02") &&
		composed.log[1]?.includes("no child P02_Logo -> 2QHP01"),
	composed.log.join(" | "),
);
check(
	"source list is not rewritten",
	snapshot[0]?.ic === "Hinge_Plate" && snapshot[2]?.ic === "Hinge_Plate",
);

const reversed = applyPartRules(snapshot, [hingeRules[1], hingeRules[0]]);
check(
	"first match still yields 2QHP02 and 2QHP01 when no-child is listed first",
	classes(reversed.detections).join(",") === "2QHP02,P02_Logo,2QHP01,2BBMNM08",
	classes(reversed.detections).join(","),
);

const bothLogos = applyPartRules(
	[
		box("Hinge_Plate", [0, 0, 200, 100]),
		box("LogoA", [10, 10, 20, 20]),
		box("LogoB", [40, 40, 50, 50]),
		box("bolt", [300, 300, 320, 320]),
	],
	[
		{
			parent: "Hinge_Plate",
			child: "LogoA",
			condition: "contains",
			result: "IC_A",
		},
		{
			parent: "Hinge_Plate",
			child: "LogoB",
			condition: "contains",
			result: "IC_B",
		},
	],
);
check(
	"first matching rule wins when two children sit inside",
	classes(bothLogos.detections).join(",") === "IC_A,LogoA,LogoB,bolt",
	classes(bothLogos.detections).join(","),
);

const outside = applyPartRules(
	[box("Hinge_Plate", [0, 0, 100, 80]), box("P02_Logo", [150, 0, 170, 20])],
	hingeRules,
);
check(
	"a logo whose center is outside the plate is no child",
	classes(outside.detections).join(",") === "2QHP01,P02_Logo",
);
check(
	"a logo outside the plate is not consumed",
	outside.detections[1]?.consumed !== true,
);

const onEdge = applyPartRules(
	[box("Hinge_Plate", [0, 0, 100, 80]), box("P02_Logo", [90, 70, 110, 90])],
	hingeRules,
);
check(
	"a child center on the parent edge counts as inside",
	onEdge.detections[0]?.ic === "2QHP02",
);

const empty = applyPartRules(snapshot, []);
check(
	"an empty rule list leaves classes unchanged",
	classes(empty.detections).join(",") === classes(snapshot).join(",") &&
		empty.log.length === 0,
);

const missed = applyPartRules([box("bolt", [0, 0, 10, 10])], hingeRules);
check(
	"a detect with no parent logs that nothing matched",
	missed.detections[0]?.ic === "bolt" &&
		missed.log[0] === "Part rules: none matched",
);

const broken = applyPartRules(
	[box("Hinge_Plate", [0, 0, 0, 10]), box("P02_Logo", [0, 0, 4, 4])],
	hingeRules,
);
check(
	"a parent box with no area is left unchanged",
	broken.detections[0]?.ic === "Hinge_Plate",
);

const parsed = parsePartRules([
	{
		parent: " Hinge_Plate ",
		child: "P02_Logo",
		condition: "contains",
		result: "2QHP02",
	},
	{
		parent: "Hinge_Plate",
		child: "P02_Logo",
		condition: "no child",
		result: "2QHP01",
	},
	{ parent: "", child: "P02_Logo", condition: "contains", result: "X" },
	{
		parent: "Hinge_Plate",
		child: "P02_Logo",
		condition: "overlaps",
		result: "X",
	},
	"nope",
]);
check(
	"config rows parse in order and drop incomplete rules",
	parsed.length === 2 &&
		parsed[0]?.result === "2QHP02" &&
		parsed[0]?.parent === "Hinge_Plate" &&
		parsed[1]?.condition === "no-child",
);
check(
	"a missing list parses as no rules",
	parsePartRules(undefined).length === 0,
);

const chained = applyPartRules(
	[box("Hinge_Plate", [0, 0, 100, 80]), box("P02_Logo", [10, 10, 30, 30])],
	[
		hingeRules[0],
		{
			parent: "2QHP02",
			child: "P02_Logo",
			condition: "contains",
			result: "SHOULD_NOT",
		},
	],
);
check(
	"a result IC is not fed into a later rule",
	chained.detections[0]?.ic === "2QHP02",
);

const blankChild = parsePartRules([
	{
		parent: "Hinge_Plate",
		child: "",
		condition: "no-child",
		result: "2QHP01",
	},
	{
		parent: "Hinge_Plate",
		child: "",
		condition: "contains",
		result: "2QHP02",
	},
]);
check(
	"a blank child is kept for no child and dropped for contains",
	blankChild.length === 1 &&
		blankChild[0]?.condition === "no-child" &&
		blankChild[0]?.child === "",
);
const modelClasses = ["Hinge_Plate", "P02_Logo", "2BBMNM08", "bolt"];
const mixedRules: PartRule[] = [
	{
		parent: "Missing_Plate",
		child: "P02_Logo",
		condition: "contains",
		result: "2QHP99",
	},
	hingeRules[0],
	{
		parent: "Hinge_Plate",
		child: "Old_Logo",
		condition: "no-child",
		result: "SKIPPED_IC",
	},
];
const mixed = applyPartRules(snapshot, mixedRules, modelClasses);
check(
	"a missing parent is skipped and the valid rule still renames the plate",
	mixed.detections[0]?.ic === "2QHP02" &&
		mixed.detections[2]?.ic === "Hinge_Plate" &&
		!mixed.detections.some(
			(item) => item.ic === "SKIPPED_IC" || item.ic === "2QHP99",
		),
	mixed.detections.map((item) => item.ic).join(","),
);
check(
	"skipped rules are logged and counted once each",
	mixed.skipped === 2 &&
		mixed.log[0] ===
			"Part rule 1 skipped: parent Missing_Plate is not in the model class list" &&
		mixed.log[1] ===
			"Part rule 3 skipped: child Old_Logo is not in the model class list" &&
		mixed.log.some((line) => line.startsWith("Part rule 2 matched:")),
	mixed.log.join(" | "),
);
const bothMissing = applyPartRules(
	snapshot,
	[
		{
			parent: "Missing_Plate",
			child: "Old_Logo",
			condition: "contains",
			result: "2QHP99",
		},
	],
	modelClasses,
);
check(
	"a rule missing both classes is one skip and leaves the boxes unchanged",
	bothMissing.skipped === 1 &&
		bothMissing.detections[0]?.ic === "Hinge_Plate" &&
		bothMissing.log[0] ===
			"Part rule 1 skipped: parent Missing_Plate and child Old_Logo are not in the model class list" &&
		!bothMissing.log.some((line) => line === "Part rules: none matched"),
	bothMissing.log.join(" | "),
);
const blankOnModel = applyPartRules(
	[box("Hinge_Plate", [0, 0, 100, 80])],
	blankChild,
	modelClasses,
);
check(
	"a blank child is not treated as a missing class",
	blankOnModel.skipped === 0 && blankOnModel.detections[0]?.ic === "2QHP01",
	blankOnModel.log.join(" | "),
);
const noNames = applyPartRules(snapshot, hingeRules, []);
check(
	"a model with no class names skips every rule",
	noNames.skipped === 2 && noNames.detections[0]?.ic === "Hinge_Plate",
	noNames.log.join(" | "),
);

const blankApplied = applyPartRules(
	[
		box("Hinge_Plate", [0, 0, 100, 80]),
		box("Hinge_Plate", [200, 0, 280, 260]),
		box("bolt", [210, 210, 230, 230]),
	],
	blankChild,
);
check(
	"a blank no-child rule matches only a parent with nothing inside",
	blankApplied.detections.map((item) => item.ic).join(",") ===
		"2QHP01,Hinge_Plate,bolt",
	blankApplied.detections.map((item) => item.ic).join(","),
);

const split = applyPartRules(
	[
		box("Hinge_Plate", [0, 0, 100, 80]),
		box("P02_Logo", [10, 10, 30, 30]),
		box("P02_Logo", [150, 0, 170, 20]),
	],
	[hingeRules[0]],
);
const splitParts = [
	{ ic: "2QHP02", des: "plate", qty: 1 },
	{ ic: "P02_Logo", des: "logo", qty: 1 },
];
const splitCounts = detectionCounts(split.detections);
check(
	"only the logo inside the matched plate is consumed",
	split.detections[0]?.ic === "2QHP02" &&
		split.detections[1]?.consumed === true &&
		split.detections[2]?.consumed !== true,
);
check(
	"the outside logo still counts and the inside logo does not",
	splitCounts["2QHP02"] === 1 && splitCounts.P02_Logo === 1,
	JSON.stringify(splitCounts),
);
check(
	"a consumed logo is not an extra or an unexpected part",
	extraPartCount(splitParts, splitCounts) === 0 &&
		!hasUnexpected(split.detections, splitParts),
);

const twins = applyPartRules(
	[
		box("Hinge_Plate", [0, 0, 100, 80]),
		box("P02_Logo", [10, 10, 30, 30]),
		box("P02_Logo", [40, 40, 60, 60]),
	],
	[hingeRules[0]],
);
const twinCounts = detectionCounts(twins.detections);
check(
	"both logos inside the plate are consumed",
	twins.detections[1]?.consumed === true &&
		twins.detections[2]?.consumed === true,
);
check(
	"two consumed logos are left out of the BoM count",
	twinCounts.P02_Logo === undefined &&
		twinCounts["2QHP02"] === 1 &&
		extraPartCount(
			[
				{ ic: "2QHP02", qty: 1 },
				{ ic: "P02_Logo", qty: 1 },
			],
			twinCounts,
		) === 0 &&
		!hasUnexpected(twins.detections, [{ ic: "2QHP02" }]),
	JSON.stringify(twinCounts),
);

const labelled = applyPartRules(
	[
		{
			...box("Hinge_Plate", [0, 0, 100, 80]),
			catalogMatch: {
				recognized: true,
				label: "FAST-00124 (94%)",
				top: {
					ic: "FAST-00124",
					description: "Bolt",
					partType: "fastener",
					score: 0.94,
				},
				runnersUp: [],
			},
		},
		box("P02_Logo", [10, 10, 30, 30]),
	],
	[hingeRules[0]],
);
check(
	"a catalogue label stays on the box when a part rule renames it",
	labelled.detections[0]?.ic === "2QHP02" &&
		labelled.detections[0]?.catalogMatch?.label === "FAST-00124 (94%)",
);

if (failed > 0) {
	throw new Error(`${failed} part composition checks failed`);
}
console.log("part composition tests passed");
