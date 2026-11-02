// Preview Detect payload shape. Run with:
// node scripts/preview-detect.test.ts

import {
	detectFailure,
	detectionsFromBody,
	multiSlot,
} from "../src/lib/preview-detect.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const full = detectionsFromBody({
	detections: [
		{ ic: "bolt", confidence: 0.4, bbox: [10, 20, 30, 40], embed: true },
		{ ic: "skip", bbox: [1, 1, 1, 2] },
		{ ic: "nut", confidence: 0.2, bbox: [4, 4, 8, 9] },
	],
});

check("full frame keeps two boxes", full.length === 2, JSON.stringify(full));
check(
	"full frame index skips the rejected box",
	full[0]?.index === 0 && full[1]?.index === 2,
	JSON.stringify(full.map((box) => box.index)),
);
check("full frame has no slot", full.every((box) => box.slot === null));
check("one object is not multi-slot", !multiSlot(full));

const slotBody = [
	{
		slot: 1,
		detections: [{ ic: "bolt", confidence: 0.5, bbox: [0, 0, 10, 10] }],
	},
	{ slot: 2, detections: [] },
	{
		slot: 3,
		detections: [
			{ ic: "nut", confidence: 0.31, bbox: [8, 8, 4, 4], embed: true },
		],
	},
];
const slots = detectionsFromBody(slotBody);

check("slots flatten in payload order", slots.length === 2, JSON.stringify(slots));
check(
	"slot numbers stay on the boxes",
	slots[0]?.slot === 1 && slots[1]?.slot === 3,
);
check(
	"slot indices continue across panels",
	slots[0]?.index === 0 && slots[1]?.index === 1,
);
check(
	"reversed bbox is ordered",
	slots[1]?.bbox.join(",") === "4,4,8,8",
	JSON.stringify(slots[1]?.bbox),
);
check("two slots are multi-slot", multiSlot(slots));
check("one slot is not multi-slot", !multiSlot(slots.slice(0, 1)));

check("idle list is empty", detectionsFromBody([]).length === 0);
check(
	"failure message is read from an object",
	detectFailure({ message: "Model is missing.", detections: [] }) ===
		"Model is missing.",
);
check("a slot array has no failure message", detectFailure(slotBody) === "");
check("blank message is ignored", detectFailure({ message: "  " }) === "");

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
