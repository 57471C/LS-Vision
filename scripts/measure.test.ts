// Scale factor and line length. Run with:
// node scripts/measure.test.ts

import {
	assignActiveCamera,
	readCameraScale,
	retainCameraSettings,
	settingsForCamera,
} from "../src/lib/camera-settings.ts";
import {
	effectiveMmPerPx,
	finishLine,
	formatReadout,
	formatScale,
	type ImageLine,
	imagePoint,
	lengthMm,
	lineLength,
	measureFrameBox,
	measureReadout,
	parseMillimetres,
	scaleFromLength,
} from "../src/lib/measure.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const line: ImageLine = { x1: 10, y1: 20, x2: 110, y2: 20 };
check("horizontal line is 100 px", lineLength(line) === 100);

const diagonal = finishLine({ x: 0, y: 0 }, { x: 3, y: 4 });
check("a 5 px line is too short", diagonal === null);
const kept = finishLine({ x: 0, y: 0 }, { x: 8, y: 0 });
check("an 8 px line is kept", kept !== null && lineLength(kept) === 8);

const point = imagePoint(
	50,
	25,
	{ left: 0, top: 0, width: 100, height: 50 },
	200,
	100,
);
check(
	"pointer maps into image pixels",
	point !== null && point.x === 100 && point.y === 50,
);
const outside = imagePoint(
	-10,
	80,
	{ left: 0, top: 0, width: 100, height: 50 },
	200,
	100,
);
check(
	"a pointer outside the image clamps",
	outside !== null && outside.x === 0 && outside.y === 100,
);

check("blank length is rejected", parseMillimetres("  ") === null);
check("zero length is rejected", parseMillimetres("0") === null);
check("a comma decimal is accepted", parseMillimetres("12,5") === 12.5);
check(
	"ten metres is the longest accepted length",
	parseMillimetres("10000") === 10000,
);
check("a longer value is rejected", parseMillimetres("10001") === null);

const scale = scaleFromLength(50, 100, 1280, 720);
check(
	"50 mm over 100 px is 0.5 mm/px",
	scale !== null && scale.mmPerPx === 0.5,
);
check(
	"the same image measures 25 mm for 50 px",
	scale !== null && lengthMm(50, 1280, scale) === 25,
);
check(
	"a 3840-wide frame of the same view is one third",
	scale !== null && Math.abs((lengthMm(150, 3840, scale) ?? 0) - 25) < 1e-9,
);
check(
	"effective scale on the calibration image is the stored factor",
	scale !== null && effectiveMmPerPx(scale, 1280) === 0.5,
);
check(
	"the preview label uses mm per pixel",
	scale !== null && formatScale(scale, 1280) === "Scale 0.500 mm/px",
);

const missing = measureReadout(80, 1280, null);
check(
	"no scale names the pixels and does not invent millimetres",
	missing.length === null &&
		missing.message === "No scale set for this camera." &&
		formatReadout(missing) === "80 px. No scale set for this camera.",
);
const present = measureReadout(100, 1280, scale);
check(
	"a stored scale shows pixels and millimetres",
	present.length === "50.0 mm" && formatReadout(present) === "100 px · 50.0 mm",
);

check(
	"a bad scale object is ignored",
	readCameraScale({ mmPerPx: 0, width: 10, height: 10 }) === null,
);
check(
	"a scale round-trips",
	scale !== null && readCameraScale(scale)?.mmPerPx === 0.5,
);

const fitted = measureFrameBox(null, 200, 100, 100, 100);
check(
	"contain centers a square image in a wide pane",
	fitted !== null &&
		fitted.left === 50 &&
		fitted.top === 0 &&
		fitted.width === 100,
);
const zoomed = measureFrameBox(
	{ left: 4, top: 6, width: 80, height: 40 },
	200,
	100,
	100,
	100,
);
check(
	"a zoom box is the image box",
	zoomed !== null && zoomed.left === 4 && zoomed.width === 80,
);

const document: Record<string, unknown> = {
	source: "json",
	camera: {
		index: 0,
		resolution: "1920x1080@30",
		displayFps: 15,
		focus: 40,
		brightness: 55,
		contrast: 45,
	},
};
const first = settingsForCamera(document.camera, 0);
check("a camera with no scale starts unset", first.scale === null);
if (scale) first.scale = scale;
assignActiveCamera(document, first);

const reloaded = JSON.parse(JSON.stringify(document)) as Record<
	string,
	unknown
>;
const again = settingsForCamera(reloaded.camera, 0);
check(
	"the scale survives a JSON reload",
	again.scale?.mmPerPx === 0.5 &&
		again.scale.width === 1280 &&
		again.focus === 40,
);
check(
	"another camera does not inherit the scale",
	settingsForCamera(reloaded.camera, 1).scale === null,
);

again.resolution = "1280x720@30";
assignActiveCamera(reloaded, again);
const keptScale = settingsForCamera(reloaded.camera, 0);
check(
	"saving another camera field keeps the scale",
	keptScale.resolution === "1280x720@30" && keptScale.scale?.mmPerPx === 0.5,
);

const other = settingsForCamera(reloaded.camera, 2);
other.scale = scaleFromLength(10, 40, 640, 480);
retainCameraSettings(reloaded, other);
const second = settingsForCamera(reloaded.camera, 2).scale;
check(
	"a second camera keeps its own scale",
	second?.mmPerPx === 0.25 &&
		second.width === 640 &&
		settingsForCamera(reloaded.camera, 0).scale?.width === 1280,
);
check(
	"storing another camera does not change the active resolution",
	settingsForCamera(reloaded.camera, 0).resolution === "1280x720@30",
);

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
console.log("measure tests passed");
