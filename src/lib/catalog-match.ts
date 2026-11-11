// Catalogue labels for a YOLO box.
// A two-pass result drives the badge, the seen count, and the Not in BoM list.
// Without one, the older catalogue label and the YOLO class stay in use.

export type CatalogCandidate = {
	ic: string;
	description: string;
	partType: string;
	size?: string;
	score: number;
};

export type CatalogMatch = {
	recognized: boolean;
	label: string;
	top: CatalogCandidate | null;
	runnersUp: CatalogCandidate[];
};

function readCandidate(value: unknown): CatalogCandidate | null {
	if (!value || typeof value !== "object") return null;
	const row = value as Record<string, unknown>;
	if (typeof row.ic !== "string" || !row.ic.trim()) return null;
	const score = typeof row.score === "number" ? row.score : Number.NaN;
	if (!Number.isFinite(score)) return null;
	const size = typeof row.size === "string" ? row.size.trim() : "";
	return {
		ic: row.ic,
		description: typeof row.description === "string" ? row.description : "",
		partType: typeof row.partType === "string" ? row.partType : "",
		...(size ? { size } : {}),
		score,
	};
}

// Undefined when this detect did not return a catalogue label.
export function readCatalogMatch(value: unknown): CatalogMatch | undefined {
	if (!value || typeof value !== "object") return undefined;
	const row = value as Record<string, unknown>;
	const label = typeof row.label === "string" ? row.label.trim() : "";
	if (!label) return undefined;
	const runnersUp: CatalogCandidate[] = [];
	if (Array.isArray(row.runnersUp)) {
		for (const item of row.runnersUp) {
			const candidate = readCandidate(item);
			if (candidate) runnersUp.push(candidate);
		}
	}
	return {
		recognized: row.recognized === true,
		label,
		top: readCandidate(row.top),
		runnersUp,
	};
}

export type TwoPassStatus = "matched_bom" | "wrong_part" | "unrecognized";

export type TwoPassMatch = {
	boundingBox: number[];
	status: TwoPassStatus;
	resolvedIc: string | null;
	description: string | null;
	partType: string | null;
	size?: string;
	score: number;
	inBom: boolean;
	candidates?: CatalogCandidate[];
};

function readText(value: unknown): string | null {
	if (typeof value !== "string") return null;
	const text = value.trim();
	return text ? text : null;
}

function isTwoPassStatus(value: unknown): value is TwoPassStatus {
	return (
		value === "matched_bom" ||
		value === "wrong_part" ||
		value === "unrecognized"
	);
}

// Undefined when this detect did not return a two-pass result.
// `candidates` stays absent unless the payload included that field.
export function readTwoPass(value: unknown): TwoPassMatch | undefined {
	if (!value || typeof value !== "object") return undefined;
	const row = value as Record<string, unknown>;
	if (!isTwoPassStatus(row.status)) return undefined;
	const score = typeof row.score === "number" ? row.score : Number.NaN;
	if (!Number.isFinite(score)) return undefined;
	const box = Array.isArray(row.boundingBox) ? row.boundingBox : [];
	const boundingBox =
		box.length >= 4 &&
		box
			.slice(0, 4)
			.every((item) => typeof item === "number" && Number.isFinite(item))
			? box.slice(0, 4)
			: [0, 0, 0, 0];
	const size = readText(row.size);
	const match: TwoPassMatch = {
		boundingBox,
		status: row.status,
		resolvedIc: readText(row.resolvedIc),
		description: readText(row.description),
		partType: readText(row.partType),
		...(size ? { size } : {}),
		score,
		inBom: row.inBom === true,
	};
	if (Array.isArray(row.candidates)) {
		const candidates: CatalogCandidate[] = [];
		for (const item of row.candidates) {
			const candidate = readCandidate(item);
			if (candidate) candidates.push(candidate);
		}
		match.candidates = candidates;
	}
	return match;
}

function matchPercent(score: number): number {
	const clamped = Math.min(1, Math.max(0, score));
	return Math.round(clamped * 100);
}

// Highest catalogue scores for one box, at most three. The same order Preview
// Detect shows. A rejected box lists these under the badge.
export function dinoGuessLine(
	match: CatalogMatch | undefined,
	candidates?: readonly CatalogCandidate[] | null,
): string {
	const listed = (candidates ?? []).filter((item) => item.ic.trim());
	const rows: CatalogCandidate[] = [];
	if (listed.length > 0) {
		for (const item of listed) {
			if (rows.length >= 3) break;
			rows.push(item);
		}
	} else if (match?.top?.ic.trim()) {
		rows.push(match.top);
		for (const item of match.runnersUp) {
			if (rows.length >= 3) break;
			if (!item.ic.trim()) continue;
			rows.push(item);
		}
	}
	return rows
		.map((row) => `${row.ic.trim()} ${matchPercent(row.score)}%`)
		.join(" · ");
}

function sizedCode(ic: string, size: string | undefined): string {
	const extra = size?.trim() ?? "";
	return extra ? `${ic} ${extra}` : ic;
}

// Operator badge. The percent is the cosine score, rounded.
// A catalogue size sits beside the item code, for example `2BB08C2HZ M8x20 - 91%`.
export function badgeLabel(match: TwoPassMatch): string {
	const pct = matchPercent(match.score);
	const code = sizedCode(match.resolvedIc ?? "—", match.size);
	if (match.status === "matched_bom") {
		return `${code} - ${pct}%`;
	}
	if (match.status === "wrong_part") {
		return `WRONG PART: ${code} (${pct}%)`;
	}
	const kind = match.partType?.trim();
	return kind ? `UNKNOWN ${kind} (${pct}%)` : `UNKNOWN (${pct}%)`;
}

export type BadgeStyle = {
	label: string;
	color: string;
	dashed: boolean;
	labelInk: string;
};

const BADGE_COLOR: Record<TwoPassStatus, string> = {
	matched_bom: "#16a34a",
	wrong_part: "#ea580c",
	unrecognized: "#eab308",
};

export function badgeStyle(match: TwoPassMatch): BadgeStyle {
	return {
		label: badgeLabel(match),
		color: BADGE_COLOR[match.status],
		dashed: match.status === "unrecognized",
		labelInk: match.status === "unrecognized" ? "#171717" : "#ffffff",
	};
}

// A missing match keeps the class IC. "No Match" is the low-score label.
export function overlayLabel(
	match: CatalogMatch | undefined,
	fallbackIc: string,
): string {
	if (!match) return fallbackIc;
	return match.label;
}

// Item code for the Not in BoM row. The canvas draws this same code, plus its percent.
// A low score stays "No Match", which is what the box already shows.
export function resolvedItemIc(
	match: CatalogMatch | undefined,
	fallbackIc: string,
): string {
	if (match?.recognized) {
		const ic = match.top?.ic.trim() ?? "";
		if (ic) return ic;
	}
	if (match?.label) return match.label;
	return fallbackIc;
}

export type OutsideCount = {
	ic: string;
	count: number;
	des: string;
};

type OutsideBox = {
	ic: string;
	consumed?: boolean;
	referenceMatch?: boolean;
	catalogMatch?: CatalogMatch;
	twoPass?: TwoPassMatch;
};

// Catalogue size for one BoM line. The first detection of that item code wins.
export function catalogueSize(
	items: readonly { twoPass?: { resolvedIc?: string | null; size?: string } }[],
	ic: string,
): string {
	const wanted = ic.trim();
	if (!wanted) return "";
	for (const item of items) {
		const pass = item.twoPass;
		if (!pass || (pass.resolvedIc ?? "").trim() !== wanted) continue;
		const size = pass.size?.trim() ?? "";
		if (size) return size;
	}
	return "";
}

// Job-card text, then the generated size when the catalogue has one.
export function descriptionWithSize(des: string, size: string | undefined): string {
	const text = des.trim();
	const extra = size?.trim() ?? "";
	if (!extra) return text;
	if (!text) return extra;
	if (text === extra || text.endsWith(` ${extra}`)) return text;
	return `${text} ${extra}`;
}

function catalogueDescription(item: OutsideBox): string {
	const match = item.catalogMatch;
	if (!match?.recognized) return "";
	return match.top?.description.trim() ?? "";
}

function unknownGroup(partType: string | null): string {
	const kind = partType?.trim() ?? "";
	return kind ? `Unknown ${kind}` : "Unknown";
}

// Not in BoM rows. A two-pass box groups a wrong part by its catalogue IC and
// description, and an unrecognised box as "Unknown <part type>". A matched BoM
// box is left out. Without a two-pass result, the row still uses the catalogue
// IC drawn on the box. `descriptions` fills a code the catalogue left blank.
export function partsOutsideBom(
	items: readonly OutsideBox[],
	parts: readonly { ic: string }[] | null,
	descriptions: Readonly<Record<string, string>>,
): OutsideCount[] {
	const groups = new Map<string, OutsideCount>();
	const known = new Set((parts ?? []).map((part) => part.ic));
	function add(key: string, ic: string, des: string) {
		const current = groups.get(key);
		if (current) {
			current.count += 1;
			if (!current.des && des) current.des = des;
			return;
		}
		groups.set(key, { ic, count: 1, des });
	}
	for (const item of items) {
		if (item.consumed === true) continue;
		const pass = item.twoPass;
		if (pass) {
			if (pass.status === "matched_bom") continue;
			if (pass.status === "wrong_part") {
				const ic = pass.resolvedIc?.trim() ?? "";
				if (!ic) continue;
				const des = descriptionWithSize(
					pass.description?.trim() ?? "",
					pass.size,
				);
				add(`${ic}\0${des}`, ic, des);
				continue;
			}
			const label = unknownGroup(pass.partType);
			add(label, label, "");
			continue;
		}
		if (item.referenceMatch === false) continue;
		const ic = resolvedItemIc(item.catalogMatch, item.ic).trim();
		if (!ic || known.has(ic)) continue;
		add(ic, ic, catalogueDescription(item));
	}
	return [...groups.values()]
		.map((row) => ({
			...row,
			des: row.des || descriptions[row.ic]?.trim() || "",
		}))
		.sort(
			(left, right) =>
				left.ic.localeCompare(right.ic) || left.des.localeCompare(right.des),
		);
}

function skipsDescriptionLookup(ic: string): boolean {
	return ic === "No Match" || ic === "Unknown" || ic.startsWith("Unknown ");
}

// Codes that still need the description endpoint. A catalogue description,
// a "No Match" label, and an Unknown part-type row are not posted.
export function inventoryCodesOutsideBom(
	rows: readonly { ic: string; des?: string }[],
): string[] {
	return rows
		.filter((row) => !skipsDescriptionLookup(row.ic) && !(row.des ?? "").trim())
		.map((row) => row.ic);
}
