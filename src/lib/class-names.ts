// Filter the active model's class names for the part-rule dropdowns.

export type ClassChoice = {
	name: string;
	label: string;
};

export function filterClassNames(
	classes: readonly string[],
	query: string,
): string[] {
	const needle = query.trim().toLowerCase();
	const found: string[] = [];
	const seen = new Set<string>();
	for (const name of classes) {
		if (!name || seen.has(name)) continue;
		seen.add(name);
		if (!needle || name.toLowerCase().includes(needle)) found.push(name);
	}
	return found;
}

// A saved name stays selectable when the current model no longer lists it.
// None is offered for a blank child on a no-child rule.
export function classChoices(
	classes: readonly string[],
	query: string,
	allowEmpty: boolean,
	current: string,
): ClassChoice[] {
	const matched = filterClassNames(classes, query);
	const rows: ClassChoice[] = matched.map((name) => ({ name, label: name }));
	const needle = query.trim().toLowerCase();
	const saved = current.trim();
	if (
		saved &&
		!matched.includes(saved) &&
		(!needle || saved.toLowerCase().includes(needle))
	) {
		rows.unshift({ name: saved, label: saved });
	}
	if (allowEmpty && !needle) {
		rows.unshift({ name: "", label: "None" });
	}
	return rows;
}
