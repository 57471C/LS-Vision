// Hinge-plate example and the rule engine. Run with:
// node scripts/part-composition.test.ts

import {
	applyPartRules,
	parsePartRules,
	type PartBox,
	type PartRule,
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

function box(
	ic: string,
	bbox: number[],
	confidence = 0.9,
): PartBox {
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
	"logo stays P02_Logo",
	composed.detections[1]?.ic === "P02_Logo",
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
	classes(reversed.detections).join(",") ===
		"2QHP02,P02_Logo,2QHP01,2BBMNM08",
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
	[
		box("Hinge_Plate", [0, 0, 100, 80]),
		box("P02_Logo", [150, 0, 170, 20]),
	],
	hingeRules,
);
check(
	"a logo whose center is outside the plate is no child",
	classes(outside.detections).join(",") === "2QHP01,P02_Logo",
);

const onEdge = applyPartRules(
	[
		box("Hinge_Plate", [0, 0, 100, 80]),
		box("P02_Logo", [90, 70, 110, 90]),
	],
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

const missed = applyPartRules(
	[box("bolt", [0, 0, 10, 10])],
	hingeRules,
);
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
	{ parent: "Hinge_Plate", child: "P02_Logo", condition: "overlaps", result: "X" },
	"nope",
]);
check(
	"config rows parse in order and drop incomplete rules",
	parsed.length === 2 &&
		parsed[0]?.result === "2QHP02" &&
		parsed[0]?.parent === "Hinge_Plate" &&
		parsed[1]?.condition === "no-child",
);
check("a missing list parses as no rules", parsePartRules(undefined).length === 0);

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

if (failed > 0) {
	throw new Error(`${failed} part composition checks failed`);
}
console.log("part composition tests passed");
