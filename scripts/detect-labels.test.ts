// Preview Detect label boxes. Run with:
// node scripts/detect-labels.test.ts

import { boxesFromDetect } from "../src/lib/detect-labels.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const full = boxesFromDetect({
	detections: [
		{ ic: "bolt", confidence: 0.2, bbox: [10, 20, 110, 220] },
		{ ic: "nut", confidence: 0.9, bbox: [0, 0, 4, 8] },
		{ ic: "bad", bbox: [5, 5, 5, 9] },
		{ ic: "nan", bbox: [1, Number.NaN, 2, 3] },
	],
});

check(
	"full frame keeps payload order",
	full.length === 2,
	JSON.stringify(full),
);
check(
	"full frame first box",
	full[0]?.join(",") === "10,20,110,220",
	JSON.stringify(full[0]),
);
check(
	"full frame second box",
	full[1]?.join(",") === "0,0,4,8",
	JSON.stringify(full[1]),
);

const slots = boxesFromDetect([
	{
		slot: 1,
		detections: [{ ic: "washer", bbox: [8, 8, 4, 4] }],
	},
	{ slot: 2, detections: [] },
	{
		slot: 3,
		detections: [{ ic: "bolt", bbox: [100, 40, 180, 90] }, { bbox: "nope" }],
	},
]);

check("slots flatten in order", slots.length === 2, JSON.stringify(slots));
check(
	"slot box corners are ordered",
	slots[0]?.join(",") === "4,4,8,8",
	JSON.stringify(slots[0]),
);
check(
	"later slot is included",
	slots[1]?.join(",") === "100,40,180,90",
	JSON.stringify(slots[1]),
);

check("empty detections", boxesFromDetect({ detections: [] }).length === 0);
check("idle slot list", boxesFromDetect([]).length === 0);
check(
	"failure object has no boxes",
	boxesFromDetect({ message: "Model is missing." }).length === 0,
);
check("non-object", boxesFromDetect(null).length === 0);

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
console.log("detect labels ok");
