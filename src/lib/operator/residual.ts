// Display category for one operator box. QTY SEEN and PASS/FAIL ignore it.
// The two-pass status stays matched_bom, wrong_part, or unrecognized.

export type Residual = "matched" | "surplus" | "near_miss" | "rogue";

export type ResidualTolerances = {
	lengthMm: number;
	diameterMm: number;
};

export const RESIDUAL_TOLERANCES: ResidualTolerances = {
	lengthMm: 1.5,
	diameterMm: 0.8,
};

type ResidualCandidate = {
	ic?: string;
	description?: string;
	partType?: string;
};

type ResidualMatch = {
	recognized?: boolean;
	label?: string;
	top?: ResidualCandidate | null;
};

type ResidualPass = {
	status?: string;
	resolvedIc?: string | null;
	description?: string | null;
	partType?: string | null;
	score?: number;
};

type ResidualMeasure = {
	status?: string;
	lengthMm?: number;
	diameterMm?: number;
	expectedLabel?: string;
	note?: string;
};

export type ResidualBox = {
	ic: string;
	consumed?: boolean;
	referenceMatch?: boolean;
	catalogMatch?: ResidualMatch;
	twoPass?: ResidualPass;
	metrology?: unknown;
};

function measureOf(value: unknown): ResidualMeasure | undefined {
	if (!value || typeof value !== "object") return undefined;
	return value as ResidualMeasure;
}

export type BomLine = {
	ic: string;
	des?: string;
	qty: number;
};

export type ResidualStyle = {
	label: string;
	color: string;
	dashed: boolean;
	labelInk: string;
};

const STYLE: Record<Residual, Omit<ResidualStyle, "label">> = {
	matched: { color: "#16a34a", dashed: false, labelInk: "#ffffff" },
	near_miss: { color: "#dc2626", dashed: false, labelInk: "#ffffff" },
	surplus: { color: "#f59e0b", dashed: true, labelInk: "#171717" },
	rogue: { color: "#7c3aed", dashed: true, labelInk: "#ffffff" },
};

function clean(value: string | null | undefined): string {
	return value?.trim() ?? "";
}

function familyType(text: string): string {
	const lower = text.toLowerCase().replace(/[_-]+/g, " ");
	if (/cable\s*tie/.test(lower)) return "cable_tie";
	if (/washer/.test(lower)) return "washer";
	if (/spring/.test(lower)) return "spring";
	if (/rivet/.test(lower)) return "rivet";
	if (/nut/.test(lower)) return "nut";
	if (/set\s*screw|\bgrub\b/.test(lower)) return "screw";
	if (/screw/.test(lower)) return "screw";
	if (/bolt/.test(lower)) return "bolt";
	return "";
}

function sizedPair(text: string): { diameter: number; length: number } | null {
	const match = text
		.toLowerCase()
		.match(/(\d+(?:\.\d+)?)\s*[x×]\s*(\d+(?:\.\d+)?)/);
	if (!match) return null;
	const diameter = Number(match[1]);
	const length = Number(match[2]);
	if (!Number.isFinite(diameter) || !Number.isFinite(length)) return null;
	return { diameter, length };
}

function nominalDiameter(text: string): number | null {
	const pair = sizedPair(text);
	if (pair) return pair.diameter;
	const metric = text.toLowerCase().match(/\bm\s*(\d+(?:\.\d+)?)\b/);
	if (!metric) return null;
	const diameter = Number(metric[1]);
	return Number.isFinite(diameter) ? diameter : null;
}

function boxText(item: ResidualBox): string {
	return [
		item.twoPass?.partType,
		item.twoPass?.description,
		measureOf(item.metrology)?.expectedLabel,
		item.catalogMatch?.top?.partType,
		item.catalogMatch?.top?.description,
		item.catalogMatch?.label,
	]
		.map((value) => clean(value))
		.filter(Boolean)
		.join(" ");
}

function lineText(line: BomLine): string {
	return `${clean(line.des)} ${clean(line.ic)}`.trim();
}

function identityIc(item: ResidualBox): string {
	return (
		clean(item.twoPass?.resolvedIc) ||
		clean(item.catalogMatch?.top?.ic) ||
		clean(item.ic)
	);
}

// Same rule as QTY SEEN. A wrong part and an unrecognised box do not fill a line.
function countedIc(item: ResidualBox): string {
	if (item.consumed === true) return "";
	const pass = item.twoPass;
	if (pass) {
		if (pass.status === "matched_bom") return clean(pass.resolvedIc);
		return "";
	}
	if (item.referenceMatch === false) return "";
	return clean(item.ic);
}

function expectedQty(parts: readonly BomLine[]): Map<string, number> {
	const map = new Map<string, number>();
	for (const part of parts) {
		const ic = clean(part.ic);
		if (!ic) continue;
		const qty = Number.isFinite(part.qty) ? part.qty : 0;
		const current = map.get(ic);
		map.set(ic, current === undefined ? qty : Math.max(current, qty));
	}
	return map;
}

function openLines(
	parts: readonly BomLine[],
	expected: ReadonlyMap<string, number>,
	seen: ReadonlyMap<string, number>,
): BomLine[] {
	return parts.filter((part) => {
		const ic = clean(part.ic);
		if (!ic) return false;
		return (seen.get(ic) ?? 0) < (expected.get(ic) ?? 0);
	});
}

function sameFamily(item: ResidualBox, line: BomLine): boolean {
	const left = familyType(boxText(item));
	const right = familyType(lineText(line));
	if (!left || !right || left !== right) return false;
	const boxDiameter = nominalDiameter(boxText(item));
	const lineDiameter = nominalDiameter(lineText(line));
	if (
		boxDiameter !== null &&
		lineDiameter !== null &&
		Math.abs(boxDiameter - lineDiameter) > 0.05
	) {
		return false;
	}
	return true;
}

function matchesLine(item: ResidualBox, line: BomLine): boolean {
	const ic = identityIc(item);
	if (ic && ic === clean(line.ic)) return true;
	return sameFamily(item, line);
}

function positive(value: number | undefined): number | null {
	if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
		return null;
	}
	return value;
}

function within(
	measured: number | null,
	expected: number,
	tolerance: number,
): boolean {
	if (measured === null) return false;
	return Math.abs(measured - expected) <= tolerance;
}

function dimensionalMatch(
	item: ResidualBox,
	lines: readonly BomLine[],
	tolerances: ResidualTolerances,
): boolean {
	const measured = measureOf(item.metrology);
	const measuredLength = positive(measured?.lengthMm);
	const measuredDiameter = positive(measured?.diameterMm);
	if (measuredLength === null && measuredDiameter === null) return false;
	const kind = familyType(boxText(item));
	for (const line of lines) {
		const text = lineText(line);
		const lineKind = familyType(text);
		if (kind && lineKind && kind !== lineKind) continue;
		const pair = sizedPair(text);
		if (pair) {
			if (
				within(measuredDiameter, pair.diameter, tolerances.diameterMm) &&
				within(measuredLength, pair.length, tolerances.lengthMm)
			) {
				return true;
			}
			continue;
		}
		const diameter = nominalDiameter(text);
		if (
			diameter !== null &&
			within(measuredDiameter, diameter, tolerances.diameterMm)
		) {
			return true;
		}
	}
	return false;
}

function noEmbeddingHit(item: ResidualBox): boolean {
	if (item.twoPass) return item.twoPass.status === "unrecognized";
	if (item.referenceMatch === false) return true;
	if (
		item.catalogMatch &&
		item.catalogMatch.recognized === false &&
		item.referenceMatch !== true
	) {
		return true;
	}
	return false;
}

function isRogue(
	item: ResidualBox,
	lines: readonly BomLine[],
	tolerances: ResidualTolerances,
): boolean {
	const status = item.twoPass?.status;
	if (status === "unrecognized" || noEmbeddingHit(item)) return true;
	if (status !== "wrong_part") return false;
	return !dimensionalMatch(item, lines, tolerances);
}

function percent(score: number | undefined): number {
	const value = typeof score === "number" && Number.isFinite(score) ? score : 0;
	return Math.round(Math.min(1, Math.max(0, value)) * 100);
}

function oneDecimal(value: number): string {
	return value.toFixed(1);
}

function measuredText(measure: ResidualMeasure | undefined): string {
	if (!measure) return "";
	const parts: string[] = [];
	const length = positive(measure.lengthMm);
	const diameter = positive(measure.diameterMm);
	if (length !== null) parts.push(`${oneDecimal(length)} mm`);
	if (diameter !== null) parts.push(`Ø ${oneDecimal(diameter)} mm`);
	return parts.join(", ");
}

function nearMissLabel(item: ResidualBox): string {
	const measure = measureOf(item.metrology);
	const note = clean(measure?.note);
	if (note) return note;
	const expected = clean(measure?.expectedLabel);
	const measured = measuredText(measure);
	if (expected && measured) {
		return `WRONG LENGTH Expected ${expected}, measured ${measured}`;
	}
	if (measured) return `WRONG LENGTH ${measured}`;
	return "WRONG LENGTH";
}

function matchedLabel(item: ResidualBox): string {
	const ic = clean(item.twoPass?.resolvedIc) || clean(item.ic);
	if (item.twoPass && ic) return `${ic} - ${percent(item.twoPass.score)}%`;
	return ic || "—";
}

function surplusIc(item: ResidualBox): string {
	return countedIc(item) || identityIc(item) || "—";
}

function rogueLabel(item: ResidualBox): string {
	if (item.twoPass?.status === "wrong_part") {
		const ic = clean(item.twoPass.resolvedIc);
		if (ic) return ic;
	}
	return "UNKNOWN";
}

export function residualStyle(
	item: ResidualBox & { residual?: Residual },
): ResidualStyle | null {
	if (!item.residual) return null;
	const face = STYLE[item.residual];
	let label = "";
	if (item.residual === "matched") label = matchedLabel(item);
	else if (item.residual === "near_miss") label = nearMissLabel(item);
	else if (item.residual === "surplus") label = `EXTRA ${surplusIc(item)}`;
	else label = rogueLabel(item);
	return { label, ...face };
}

function classify(
	item: ResidualBox,
	parts: readonly BomLine[],
	expected: ReadonlyMap<string, number>,
	seen: ReadonlyMap<string, number>,
	tolerances: ResidualTolerances,
): Residual {
	const ic = countedIc(item);
	if (
		ic &&
		expected.has(ic) &&
		(seen.get(ic) ?? 0) >= (expected.get(ic) ?? 0)
	) {
		return "surplus";
	}
	const open = openLines(parts, expected, seen);
	const mismatch = measureOf(item.metrology)?.status === "DIMENSION_MISMATCH";
	if (mismatch && open.some((line) => matchesLine(item, line))) {
		return "near_miss";
	}
	if (isRogue(item, open, tolerances)) return "rogue";
	if (ic && expected.has(ic)) return "matched";
	if (
		item.twoPass?.status === "wrong_part" &&
		dimensionalMatch(item, open, tolerances)
	) {
		return "matched";
	}
	return "rogue";
}

// One list, in detection order. Each slot is classified on its own so one
// slot does not use up another slot's quantity. Consumed children are skipped.
export function assignResiduals<T extends ResidualBox>(
	items: readonly T[],
	parts: readonly BomLine[],
	tolerances: ResidualTolerances = RESIDUAL_TOLERANCES,
): (T & { residual?: Residual })[] {
	const expected = expectedQty(parts);
	const seen = new Map<string, number>();
	return items.map((item) => {
		if (item.consumed === true) return { ...item };
		const residual = classify(item, parts, expected, seen, tolerances);
		const ic = countedIc(item);
		if (ic && expected.has(ic)) seen.set(ic, (seen.get(ic) ?? 0) + 1);
		return { ...item, residual };
	});
}
