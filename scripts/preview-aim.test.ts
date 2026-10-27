// Preview crosshair readout and marquee gesture. Run with:
// node scripts/preview-aim.test.ts

import { formatPxMm } from "../src/lib/measure.ts";
import {
	marqueeGesture,
	type MarqueeMark,
	topMarkAt,
} from "../src/lib/preview-marquee.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const scale = { mmPerPx: 0.1, width: 1000, height: 750 };

check(
	"crosshair without a scale is pixels only",
	formatPxMm(120.4, 1000, null) === "120 px",
);

check(
	"crosshair uses the camera scale",
	formatPxMm(120, 1000, scale) === "120 px · 12.0 mm",
);

check(
	"a narrower image scales the millimetres",
	formatPxMm(50, 500, scale) === "50 px · 10.0 mm",
);

const under: MarqueeMark = {
	index: 0,
	embed: true,
	x: 0,
	y: 0,
	w: 40,
	h: 40,
};
const over: MarqueeMark = {
	index: 1,
	embed: true,
	x: 10,
	y: 10,
	w: 20,
	h: 20,
};

check(
	"the later box wins the hit test",
	topMarkAt([under, over], 15, 15)?.index === 1,
);
check("empty space hits nothing", topMarkAt([over], 0, 0) === null);

check(
	"a large drag is a crop",
	marqueeGesture(30, 12, over, 8).type === "marquee",
);
check(
	"a short press selects the box",
	marqueeGesture(2, 2, over, 8).type === "select" &&
		(marqueeGesture(2, 2, over, 8) as { index: number }).index === 1,
);
const plain: MarqueeMark = { ...over, embed: false, index: 4 };
check(
	"a short press on a box without an embedding is ignored",
	marqueeGesture(1, 1, plain, 8).type === "ignore",
);
check(
	"a short press on empty space is ignored",
	marqueeGesture(0, 0, null, 8).type === "ignore",
);

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
console.log("preview aim ok");
