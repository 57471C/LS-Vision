// Catalogue-miss selection. Run with:
// node scripts/catalogue-capture.test.ts

import {
	catalogueMiss,
	readMatchThreshold,
} from "../src/lib/catalogue-capture.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

check("missing threshold stays 0.70", readMatchThreshold(undefined) === 0.7);
check("threshold clamps low", readMatchThreshold(0.1) === 0.5);
check("threshold clamps high", readMatchThreshold(2) === 0.95);

check(
	"unrecognized is selectable",
	catalogueMiss({ status: "unrecognized", score: 0.2 }, 0.7),
);
check(
	"a score under the threshold is selectable",
	catalogueMiss({ status: "wrong_part", score: 0.69 }, 0.7),
);
check(
	"a score at the threshold stays put",
	!catalogueMiss({ status: "wrong_part", score: 0.7 }, 0.7),
);
check(
	"matched_bom stays put",
	!catalogueMiss({ status: "matched_bom", score: 0.91 }, 0.7),
);
check("no two-pass result stays put", !catalogueMiss(undefined, 0.7));
check("an empty object stays put", !catalogueMiss({}, 0.7));

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
console.log("catalogue capture ok");
