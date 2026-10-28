// Catalogue overlay labels. Run with:
// node scripts/catalog-match.test.ts

import {
	type CatalogMatch,
	overlayLabel,
	readCatalogMatch,
} from "../src/lib/catalog-match.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const named: CatalogMatch = {
	recognized: true,
	label: "FAST-00124 (94%)",
	top: {
		ic: "FAST-00124",
		description: "Bolt",
		partType: "fastener",
		score: 0.94,
	},
	runnersUp: [],
};

check(
	"a recognised box shows the catalogue IC and percent",
	overlayLabel(named, "2BBMNM08") === "FAST-00124 (94%)",
);
check(
	"a low score shows No Match",
	overlayLabel(
		{
			recognized: false,
			label: "No Match",
			top: null,
			runnersUp: [],
		},
		"2BBMNM08",
	) === "No Match",
);
check(
	"a detect without a catalogue field keeps the class IC",
	overlayLabel(undefined, "2BBMNM08") === "2BBMNM08",
);

const parsed = readCatalogMatch({
	recognized: true,
	label: "FAST-00124 (94%)",
	top: {
		ic: "FAST-00124",
		description: "Bolt",
		partType: "fastener",
		score: 0.94,
	},
	runnersUp: [
		{
			ic: "FAST-00002",
			description: "Nut",
			partType: "fastener",
			score: 0.61,
		},
	],
});
check(
	"the detect payload keeps the top IC and one runner-up",
	parsed?.label === "FAST-00124 (94%)" &&
		parsed.top?.ic === "FAST-00124" &&
		parsed.runnersUp.length === 1 &&
		parsed.runnersUp[0]?.ic === "FAST-00002",
);
check(
	"an empty payload has no catalogue label",
	readCatalogMatch(null) === undefined,
);

if (failed > 0) {
	throw new Error(`${failed} catalogue match checks failed`);
}
console.log("catalogue match tests passed");
