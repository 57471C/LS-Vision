// Not in BoM rows use the catalogue IC drawn on the box. Run with:
// node scripts/outside-bom.test.ts

import {
	type CatalogMatch,
	inventoryCodesOutsideBom,
	partsOutsideBom,
} from "../src/lib/catalog-match.ts";
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

function match(ic: string, description: string): CatalogMatch {
	return {
		recognized: true,
		label: `${ic} (94%)`,
		top: {
			ic,
			description,
			partType: "part",
			score: 0.94,
		},
		runnersUp: [],
	};
}

function box(ic: string, catalogMatch?: CatalogMatch) {
	return { ic, catalogMatch };
}

const detections = [
	box("2", match("2ERGM22", "Hinge")),
	box("6", match("2ERGM22", "Hinge")),
	box("3", match("4LRS006", "Rail")),
	box("5", match("FAST-9", "")),
	box("9", match("2CNM", "Nut")),
	box("8", match("2CNM", "Nut")),
	box("4", {
		recognized: false,
		label: "No Match",
		top: {
			ic: "LOW",
			description: "Below threshold",
			partType: "part",
			score: 0.2,
		},
		runnersUp: [],
	}),
	{ ic: "2", catalogMatch: match("2ERGM22", "Hinge"), consumed: true },
	{
		ic: "9",
		catalogMatch: match("2CNM", "Nut"),
		referenceMatch: false as const,
	},
];

const rows = partsOutsideBom(detections, [{ ic: "2CNM" }], {
	"2ERGM22": "End hinge plate",
	"4LRS006": "Long rail",
	"FAST-9": "From the database",
});

check(
	"rows group by the catalogue IC instead of the class index",
	rows.map((row) => row.ic).join(",") === "2ERGM22,4LRS006,FAST-9,No Match",
	rows.map((row) => row.ic).join(","),
);
check(
	"two boxes of one catalogue IC count as two",
	rows.find((row) => row.ic === "2ERGM22")?.count === 2,
);
check(
	"a single catalogue IC keeps a count of one",
	rows.find((row) => row.ic === "4LRS006")?.count === 1,
);
check(
	"the catalogue description is shown and is not replaced by the endpoint",
	rows.find((row) => row.ic === "2ERGM22")?.des === "Hinge" &&
		rows.find((row) => row.ic === "4LRS006")?.des === "Rail",
);
check(
	"a blank catalogue description uses the endpoint text",
	rows.find((row) => row.ic === "FAST-9")?.des === "From the database",
);
check(
	"a catalogue IC that is on the BoM is left out",
	rows.every((row) => row.ic !== "2CNM"),
);
check(
	"an unrecognised box is grouped as No Match",
	rows.find((row) => row.ic === "No Match")?.count === 1 &&
		rows.find((row) => row.ic === "No Match")?.des === "",
);
check(
	"only a code with no catalogue description is sent to the endpoint",
	inventoryCodesOutsideBom(
		partsOutsideBom(detections, [{ ic: "2CNM" }], {}),
	).join(",") === "FAST-9",
);
check(
	"the BoM count still uses the YOLO class",
	detectionCounts(detections)["2"] === 1 &&
		detectionCounts(detections)["6"] === 1 &&
		detectionCounts(detections)["3"] === 1 &&
		detectionCounts(detections)["2ERGM22"] === undefined,
);

const plain = partsOutsideBom(
	[{ ic: "2BBMNM08" }, { ic: "2BBMNM08" }],
	null,
	{},
);
check(
	"a box without a catalogue match keeps its class IC",
	plain.length === 1 && plain[0]?.ic === "2BBMNM08" && plain[0]?.count === 2,
);

if (failed > 0) {
	throw new Error(`${failed} outside-bom checks failed`);
}
console.log("outside bom tests passed");
