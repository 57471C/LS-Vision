// Catalogue labels for a YOLO box.
// A two-pass result drives the badge, the seen count, and the Not in BoM list.
// Without one, the older catalogue label and the YOLO class stay in use.

export type CatalogCandidate = {
	ic: string;
	description: string;
	partType: string;
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
	return {
		ic: row.ic,
		description: typeof row.description === "string" ? row.description : "",
		partType: typeof row.partType === "string" ? row.partType : "",
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
	const match: TwoPassMatch = {
		boundingBox,
		status: row.status,
		resolvedIc: readText(row.resolvedIc),
		description: readText(row.description),
		partType: readText(row.partType),
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

// Operator badge. The percent is the cosine score, rounded.
export function badgeLabel(match: TwoPassMatch): string {
	const pct = matchPercent(match.score);
	if (match.status === "matched_bom") {
		return `${match.resolvedIc ?? "—"} - ${pct}%`;
	}
	if (match.status === "wrong_part") {
		return `WRONG PART: ${match.resolvedIc ?? "—"} (${pct}%)`;
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
				const des = pass.description?.trim() ?? "";
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
