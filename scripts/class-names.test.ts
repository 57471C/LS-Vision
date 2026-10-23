// Class dropdown filter. Run with:
// node scripts/class-names.test.ts

import { classChoices, filterClassNames } from "../src/lib/class-names.ts";

const classes = ["Hinge_Plate", "P02_Logo", "2BBMNM08", "bolt"];
let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

check(
	"hinge filters to Hinge_Plate",
	filterClassNames(classes, "hinge").join(",") === "Hinge_Plate",
);
check(
	"LOGO matches anywhere and ignores case",
	filterClassNames(classes, "LOGO").join(",") === "P02_Logo",
);
check(
	"an empty query keeps model order",
	filterClassNames(classes, "  ").join(",") === classes.join(","),
);
check(
	"duplicate names are listed once",
	filterClassNames(["bolt", "bolt", ""], "").join(",") === "bolt",
);

const menu = classChoices(classes, "", true, "Hinge_Plate").map(
	(row) => row.label,
);
check("no child offers None first", menu[0] === "None" && menu[1] === "Hinge_Plate", menu.join(","));

const saved = classChoices(["P02_Logo"], "old", false, "Old_Plate").map(
	(row) => row.name,
);
check(
	"a saved class stays in the list when the model no longer has it",
	saved.join(",") === "Old_Plate",
	saved.join(","),
);

if (failed > 0) throw new Error(`${failed} class name checks failed`);
console.log("class name tests passed");
