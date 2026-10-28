// Optional class rewrite after detection. Rules come from config.
// An empty list leaves every class as the model named it.
// Rules run once, top to bottom, against those original class names.
// The first match renames that parent box. A result IC is not matched again.

import type { CatalogMatch, TwoPassMatch } from "$lib/catalog-match";

export const PART_RULE_LIMIT = 40;
export const PART_LABEL_LIMIT = 80;

export type PartCondition = "contains" | "no-child";

export type PartRule = {
	parent: string;
	child: string;
	condition: PartCondition;
	result: string;
};

export type PartBox = {
	ic: string;
	confidence: number;
	bbox: number[];
	// True when this box was the child of a matching rule and was not rewritten.
	consumed?: boolean;
	// Kept from detection. A part rule rename does not clear it.
	referenceMatch?: boolean;
	// Kept from detection. The overlay reads it when twoPass is absent.
	catalogMatch?: CatalogMatch;
	// Kept from detection. A part rule rename does not clear it.
	twoPass?: TwoPassMatch;
};

export type PartComposition = {
	detections: PartBox[];
	log: string[];
	skipped: number;
};

type Rect = {
	x1: number;
	y1: number;
	x2: number;
	y2: number;
};

export function conditionLabel(condition: PartCondition): string {
	return condition === "no-child" ? "no child" : "contains";
}

function cleanLabel(value: unknown): string {
	if (typeof value !== "string") return "";
	const text = value.trim();
	if (!text || text.length > PART_LABEL_LIMIT) return "";
	return text;
}

function readCondition(value: unknown): PartCondition | null {
	if (value === "contains") return "contains";
	if (value === "no-child" || value === "no child") return "no-child";
	return null;
}

// Drop rows that are missing a class, a condition, or a result IC.
export function parsePartRules(value: unknown): PartRule[] {
	if (!Array.isArray(value)) return [];
	const rules: PartRule[] = [];
	for (const item of value) {
		if (!item || typeof item !== "object") continue;
		const row = item as Record<string, unknown>;
		const parent = cleanLabel(row.parent);
		const child = cleanLabel(row.child);
		const result = cleanLabel(row.result);
		const condition = readCondition(row.condition);
		if (!parent || !result || !condition) continue;
		// A blank child is only meaningful for "no child": any class inside the parent.
		if (condition === "contains" && !child) continue;
		rules.push({ parent, child, condition, result });
		if (rules.length >= PART_RULE_LIMIT) break;
	}
	return rules;
}

function rectOf(bbox: number[]): Rect | null {
	if (bbox.length < 4) return null;
	const [rawX1, rawY1, rawX2, rawY2] = bbox;
	if (
		![rawX1, rawY1, rawX2, rawY2].every(
			(value) => typeof value === "number" && Number.isFinite(value),
		)
	) {
		return null;
	}
	const x1 = Math.min(rawX1, rawX2);
	const x2 = Math.max(rawX1, rawX2);
	const y1 = Math.min(rawY1, rawY2);
	const y2 = Math.max(rawY1, rawY2);
	if (x2 <= x1 || y2 <= y1) return null;
	return { x1, y1, x2, y2 };
}

// The child counts as inside when its center lies in the parent, edges included.
function centerInside(child: Rect, parent: Rect): boolean {
	const x = (child.x1 + child.x2) / 2;
	const y = (child.y1 + child.y2) / 2;
	return x >= parent.x1 && x <= parent.x2 && y >= parent.y1 && y <= parent.y2;
}

// Every box of the child class whose center sits inside this parent.
// A blank child class means any other box.
function childIndexes(
	detections: readonly PartBox[],
	boxes: readonly (Rect | null)[],
	parentIndex: number,
	parentBox: Rect,
	childClass: string,
): number[] {
	const found: number[] = [];
	for (let otherIndex = 0; otherIndex < detections.length; otherIndex += 1) {
		if (otherIndex === parentIndex) continue;
		if (childClass && detections[otherIndex]?.ic !== childClass) continue;
		const childBox = boxes[otherIndex];
		if (!childBox || !centerInside(childBox, parentBox)) continue;
		found.push(otherIndex);
	}
	return found;
}

function formatBox(box: Rect): string {
	const n = (value: number) =>
		Number.isInteger(value) ? String(value) : value.toFixed(1);
	return `${n(box.x1)},${n(box.y1)},${n(box.x2)},${n(box.y2)}`;
}

function missingClassNote(
	rule: PartRule,
	missingParent: boolean,
	missingChild: boolean,
): string {
	if (missingParent && missingChild) {
		return `parent ${rule.parent} and child ${rule.child} are not in the model class list`;
	}
	if (missingParent)
		return `parent ${rule.parent} is not in the model class list`;
	return `child ${rule.child} is not in the model class list`;
}

// modelClasses null means this detect did not return a class list, so every
// saved rule still runs. An empty list means the model has no class names.
export function applyPartRules(
	detections: readonly PartBox[],
	rules: readonly PartRule[],
	modelClasses: readonly string[] | null = null,
): PartComposition {
	if (rules.length === 0) {
		return { detections: detections.slice(), log: [], skipped: 0 };
	}
	const known = modelClasses === null ? null : new Set(modelClasses);
	const usable: { rule: PartRule; number: number }[] = [];
	const log: string[] = [];
	let skipped = 0;
	for (let ruleIndex = 0; ruleIndex < rules.length; ruleIndex += 1) {
		const rule = rules[ruleIndex];
		if (known) {
			const missingParent = !known.has(rule.parent);
			const missingChild = rule.child.length > 0 && !known.has(rule.child);
			if (missingParent || missingChild) {
				skipped += 1;
				log.push(
					`Part rule ${ruleIndex + 1} skipped: ${missingClassNote(rule, missingParent, missingChild)}`,
				);
				continue;
			}
		}
		usable.push({ rule, number: ruleIndex + 1 });
	}
	const boxes = detections.map((item) => rectOf(item.bbox));
	const next = detections.slice();
	const rewritten = new Set<number>();
	const consumed = new Set<number>();
	let matched = false;
	for (let index = 0; index < detections.length; index += 1) {
		const item = detections[index];
		const parentBox = boxes[index];
		if (!item || !parentBox) continue;
		for (const entry of usable) {
			const rule = entry.rule;
			if (item.ic !== rule.parent) continue;
			if (rule.condition === "contains" && !rule.child) continue;
			const hits = childIndexes(
				detections,
				boxes,
				index,
				parentBox,
				rule.child,
			);
			const matchedRule =
				rule.condition === "contains" ? hits.length > 0 : hits.length === 0;
			if (!matchedRule) continue;
			next[index] = { ...item, ic: rule.result };
			rewritten.add(index);
			// Those child boxes made the rule match. They are not BoM parts.
			if (rule.condition === "contains") {
				for (const hit of hits) consumed.add(hit);
			}
			const childText = rule.child || "any class";
			log.push(
				`Part rule ${entry.number} matched: ${rule.parent} ${conditionLabel(rule.condition)} ${childText} -> ${rule.result} at ${formatBox(parentBox)}`,
			);
			matched = true;
			break;
		}
	}
	for (const index of consumed) {
		if (rewritten.has(index)) continue;
		const item = next[index];
		if (!item) continue;
		next[index] = { ...item, consumed: true };
	}
	if (!matched && usable.length > 0) log.push("Part rules: none matched");
	return { detections: next, log, skipped };
}
