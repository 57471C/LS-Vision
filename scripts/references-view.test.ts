// Catalogue reference dialog helpers. Run with:
// node scripts/references-view.test.ts

import {
	addedLabel,
	duplicateHashes,
	filterParts,
	primaryOrder,
	readCatalogueList,
	referencesMessage,
	refsLabel,
	reorderIndexes,
} from "../src/lib/settings/references-view.ts";

let failed = 0;

function check(name: string, condition: boolean, detail = ""): void {
	if (condition) {
		console.log(`ok ${name}`);
		return;
	}
	failed += 1;
	console.error(`FAIL ${name}${detail ? `: ${detail}` : ""}`);
}

const parts = [
	{ ic: "2BBMNM08", description: "M8 nut", crops: [{ hash: "aa" }, { hash: "bb" }] },
	{ ic: "FAST-00124", description: "Washer", crops: [{ hash: "aa" }, { hash: "" }] },
];

const filtered = filterParts(parts, "m8");
check("filter matches a description", filtered.length === 1 && filtered[0]?.ic === "2BBMNM08");
check("filter is case-insensitive on the IC", filterParts(parts, "fast-00124").length === 1);
check("filter miss is empty", filterParts(parts, "nomatch").length === 0);
check(
	"filter miss says No matches",
	referencesMessage(parts.length, 0, "nomatch") === "No matches",
);
check("empty catalogue has no filter message", referencesMessage(0, 0, "m8") === "");
check("blank query keeps every part", filterParts(parts, "  ").length === 2);

const now = Date.UTC(2026, 10, 4, 12, 0, 0);
check("added today", addedLabel(Math.floor(now / 1000), now) === "Added today");
check(
	"added one day ago",
	addedLabel(Math.floor((now - 86_400_000) / 1000), now) === "Added 1 day ago",
);
check(
	"added three days ago",
	addedLabel(Math.floor((now - 3 * 86_400_000) / 1000), now) === "Added 3 days ago",
);
check("missing date is blank", addedLabel(null, now) === "");

check("one ref", refsLabel(1) === "1 ref");
check("twelve refs", refsLabel(12) === "12 refs");

const dupes = duplicateHashes(parts);
check("shared hash is a duplicate", dupes.has("aa") && dupes.size === 1);
check("empty hash is ignored", !dupes.has(""));

const order = reorderIndexes([0, 1, 2], 2, 0);
check("reorder moves the last image first", order?.join(",") === "2,0,1");
check("same position is not a reorder", reorderIndexes([0, 1], 0, 0) === null);
check("primary moves a later image first", primaryOrder([0, 1, 2], 1)?.join(",") === "1,0,2");
check("first image is already primary", primaryOrder([0, 1], 0) === null);

const list = readCatalogueList({
	path: "vision/catalogue.json",
	parts: [
		{
			ic: "2BBMNM08",
			description: "M8 nut",
			type: "nut",
			dimensions: "acrossFlats 13, h 6.5",
			addedUnix: 1_700_000_000,
			thumbs: ["data:image/jpeg;base64,old"],
			crops: [
				{ index: 0, src: "data:image/jpeg;base64,aaa", hash: "abc" },
				{ index: 1, src: "", hash: "" },
			],
			sizes: [{ key: "h", label: "h", value: "6.5", toleranceMm: 0.25 }],
		},
	],
});
check("list keeps the path", list?.path === "vision/catalogue.json");
check("list reads one part", list?.parts.length === 1 && list.parts[0]?.ic === "2BBMNM08");
check("crop hash is kept", list?.parts[0]?.crops[0]?.hash === "abc");
check("tolerance is kept", list?.parts[0]?.sizes[0]?.toleranceMm === 0.25);
check("added unix is kept", list?.parts[0]?.addedUnix === 1_700_000_000);

const fallback = readCatalogueList({
	parts: [{ ic: "ONLY-THUMBS", thumbs: ["data:image/jpeg;base64,thumb"] }],
});
check(
	"thumbs become crops when crops are missing",
	fallback?.parts[0]?.crops.length === 1 && fallback.parts[0]?.crops[0]?.hash === "",
);

if (failed > 0) {
	console.error(`${failed} failed`);
	process.exit(1);
}
console.log("all passed");
