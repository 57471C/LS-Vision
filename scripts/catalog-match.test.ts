// Catalogue overlay labels. Run with:
// node scripts/catalog-match.test.ts

import {
	type CatalogCandidate,
	type CatalogMatch,
	dinoGuessLine,
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

const guesses: CatalogCandidate[] = [
	{ ic: "2ERGM22", description: "Hinge", partType: "hinge", score: 0.91 },
	{ ic: "6PBB833", description: "Bracket", partType: "bracket", score: 0.74 },
	{ ic: "4LRS006", description: "Rail", partType: "rail", score: 0.4 },
	{ ic: "EXTRA", description: "Extra", partType: "part", score: 0.1 },
];
check(
	"a rejected box lists the top three guesses",
	dinoGuessLine(undefined, guesses) === "2ERGM22 91% · 6PBB833 74% · 4LRS006 40%",
	dinoGuessLine(undefined, guesses),
);
check(
	"the catalogue match fills the guesses when the payload has none",
	dinoGuessLine(
		{
			recognized: true,
			label: "2ERGM22 (91%)",
			top: guesses[0],
			runnersUp: [guesses[1], guesses[2], guesses[3]],
		},
		undefined,
	) === "2ERGM22 91% · 6PBB833 74% · 4LRS006 40%",
);
check(
	"an empty catalogue has no guess line",
	dinoGuessLine(undefined, []) === "" && dinoGuessLine(undefined, null) === "",
);

if (failed > 0) {
	throw new Error(`${failed} catalogue match checks failed`);
}
console.log("catalogue match tests passed");
