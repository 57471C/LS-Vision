// Two-pass badges, seen counts, and Not in BoM groups. Run with:
// node scripts/two-pass.test.ts

import {
	badgeLabel,
	badgeStyle,
	type CatalogCandidate,
	inventoryCodesOutsideBom,
	partsOutsideBom,
	readTwoPass,
	type TwoPassMatch,
} from "../src/lib/catalog-match.ts";
import { detectionCounts, hasUnexpected } from "../src/lib/operator/verdict.ts";
import { applyPartRules } from "../src/lib/part-composition.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

function candidate(ic: string, score: number): CatalogCandidate {
	return { ic, description: ic, partType: "part", score };
}

function pass(
	status: TwoPassMatch["status"],
	extras: Partial<TwoPassMatch> = {},
): TwoPassMatch {
	return {
		boundingBox: [1, 2, 3, 4],
		status,
		resolvedIc: status === "unrecognized" ? null : "6PBB833",
		description:
			status === "matched_bom"
				? "Bracket"
				: status === "wrong_part"
					? "Hinge"
					: null,
		partType: status === "unrecognized" ? "Washer" : "bracket",
		score: 0.94,
		inBom: status === "matched_bom",
		...extras,
	};
}

const matched = pass("matched_bom");
const foreign = pass("wrong_part", {
	resolvedIc: "2ERGM22",
	description: "Hinge",
	score: 0.91,
});
const unknown = pass("unrecognized", {
	resolvedIc: null,
	description: null,
	partType: "Washer",
	score: 0.22,
	inBom: false,
});

check(
	"a BoM match badge shows the IC and percent",
	badgeLabel(matched) === "6PBB833 - 94%",
	badgeLabel(matched),
);
check(
	"a foreign part badge names that IC",
	badgeLabel(foreign) === "WRONG PART: 2ERGM22 (91%)",
	badgeLabel(foreign),
);
check(
	"an unknown object badge uses the catalogue part type",
	badgeLabel(unknown) === "UNKNOWN Washer (22%)",
	badgeLabel(unknown),
);
check(
	"badges use green, orange, and a yellow dashed border",
	badgeStyle(matched).color === "#16a34a" &&
		badgeStyle(matched).dashed === false &&
		badgeStyle(foreign).color === "#ea580c" &&
		badgeStyle(unknown).color === "#eab308" &&
		badgeStyle(unknown).dashed === true &&
		badgeStyle(unknown).labelInk === "#171717",
);

const hidden = readTwoPass({
	boundingBox: [0, 0, 10, 10],
	status: "matched_bom",
	resolvedIc: "6PBB833",
	description: "Bracket",
	partType: "bracket",
	score: 0.94,
	inBom: true,
});
const shown = readTwoPass({
	boundingBox: [0, 0, 10, 10],
	status: "wrong_part",
	resolvedIc: "2ERGM22",
	description: "Hinge",
	partType: "hinge",
	score: 0.91,
	inBom: false,
	candidates: [candidate("2ERGM22", 0.91), candidate("6PBB833", 0.12)],
});
check(
	"candidates are omitted unless the payload includes them",
	hidden?.candidates === undefined && shown?.candidates?.length === 2,
);
check(
	"a payload without a two-pass object is left unchanged",
	readTwoPass(null) === undefined,
);

const boxes = [
	{ ic: "2", twoPass: matched },
	{ ic: "2", twoPass: matched },
	{ ic: "6", twoPass: foreign },
	{ ic: "3", twoPass: unknown },
	{ ic: "3", twoPass: unknown },
	{
		ic: "9",
		twoPass: pass("wrong_part", {
			resolvedIc: "2ERGM22",
			description: "Other hinge",
			score: 0.8,
		}),
	},
	{ ic: "4", twoPass: matched, consumed: true },
	{
		ic: "8",
		twoPass: pass("matched_bom", { resolvedIc: "6PBB833", score: 0.99 }),
		referenceMatch: false as const,
	},
];
const counts = detectionCounts(boxes);
check(
	"QTY SEEN counts only matched BoM item codes",
	counts["6PBB833"] === 3 &&
		counts["2"] === undefined &&
		counts["2ERGM22"] === undefined,
	JSON.stringify(counts),
);
check(
	"a wrong part and an unknown object do not count",
	counts["2ERGM22"] === undefined && counts.Washer === undefined,
);

const parts = [{ ic: "6PBB833" }];
check(
	"a BoM match is not unexpected",
	!hasUnexpected([{ ic: "2", twoPass: matched }], parts),
);
check(
	"a foreign part fails the inspection",
	hasUnexpected([{ ic: "6", twoPass: foreign }], parts),
);
check(
	"an unknown object fails the inspection",
	hasUnexpected([{ ic: "3", twoPass: unknown }], parts),
);

const rows = partsOutsideBom(boxes, parts, { "2ERGM22": "From the database" });
check(
	"wrong parts group by catalogue IC and description",
	rows.map((row) => `${row.ic}:${row.des}:${row.count}`).join("|") ===
		"2ERGM22:Hinge:1|2ERGM22:Other hinge:1|Unknown Washer::2",
	rows.map((row) => `${row.ic}:${row.des}:${row.count}`).join("|"),
);
check(
	"a matched BoM line is left out of Not in BoM",
	rows.every((row) => row.ic !== "6PBB833"),
);
check(
	"an unknown part type is not posted",
	rows.every((row) => !row.ic.startsWith("Unknown") || row.des === ""),
);

const posted = inventoryCodesOutsideBom(
	partsOutsideBom(
		[
			{ ic: "1", twoPass: foreign },
			{ ic: "2", twoPass: unknown },
			{
				ic: "3",
				twoPass: pass("wrong_part", {
					resolvedIc: "FAST-9",
					description: "",
					score: 0.8,
				}),
			},
		],
		parts,
		{},
	),
);
check(
	"only a blank catalogue description is sent to the endpoint",
	posted.join(",") === "FAST-9",
	posted.join(","),
);

const renamed = applyPartRules(
	[
		{
			ic: "Hinge_Plate",
			confidence: 0.9,
			bbox: [0, 0, 100, 80],
			twoPass: matched,
		},
		{
			ic: "P02_Logo",
			confidence: 0.9,
			bbox: [10, 10, 30, 30],
		},
	],
	[
		{
			parent: "Hinge_Plate",
			child: "P02_Logo",
			condition: "contains",
			result: "2QHP02",
		},
	],
);
check(
	"a part rule keeps the two-pass result on the renamed box",
	renamed.detections[0]?.ic === "2QHP02" &&
		renamed.detections[0]?.twoPass?.status === "matched_bom" &&
		renamed.detections[1]?.consumed === true,
);

if (failed > 0) {
	throw new Error(`${failed} two-pass checks failed`);
}
console.log("two-pass tests passed");
