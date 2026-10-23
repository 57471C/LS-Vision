<script lang="ts">
	// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

	import { classChoices, type ClassChoice } from "$lib/class-names";

	export let value = "";
	export let classes: readonly string[] = [];
	export let label = "Class";
	export let disabled = false;
	export let allowEmpty = false;
	export let placeholder = "Choose a class";

	let open = false;
	let query = "";
	let active = 0;
	let inputEl: HTMLInputElement | undefined;
	let menuStyle = "";

	$: rows = classChoices(classes, open ? query : "", allowEmpty, value);
	$: if (active >= rows.length) active = 0;

	function place() {
		if (!inputEl) return;
		const rect = inputEl.getBoundingClientRect();
		const spaceBelow = window.innerHeight - rect.bottom;
		const above = spaceBelow < 180 && rect.top > spaceBelow;
		menuStyle = above
			? `top:${rect.top - 4}px;left:${rect.left}px;width:${rect.width}px;transform:translateY(-100%);`
			: `top:${rect.bottom + 4}px;left:${rect.left}px;width:${rect.width}px;`;
	}

	function showCommitted() {
		if (inputEl) inputEl.value = value;
	}

	function openList() {
		if (disabled) return;
		query = "";
		open = true;
		const current = rows.findIndex((row) => row.name === value);
		active = current >= 0 ? current : 0;
		if (inputEl) inputEl.value = "";
		place();
	}

	function commit(name: string) {
		value = name;
		query = "";
		open = false;
		if (inputEl) inputEl.value = name;
	}

	function closeList() {
		open = false;
		query = "";
		showCommitted();
	}

	function onType(event: Event) {
		if (disabled) return;
		query = (event.currentTarget as HTMLInputElement).value;
		open = true;
		active = 0;
		place();
	}

	function onKey(event: KeyboardEvent) {
		if (disabled) return;
		if (event.key === "Escape") {
			closeList();
			return;
		}
		if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
			event.preventDefault();
			openList();
			return;
		}
		if (!open) return;
		if (event.key === "ArrowDown") {
			event.preventDefault();
			active = Math.min(active + 1, Math.max(rows.length - 1, 0));
			return;
		}
		if (event.key === "ArrowUp") {
			event.preventDefault();
			active = Math.max(active - 1, 0);
			return;
		}
		if (event.key === "Enter") {
			event.preventDefault();
			const row = rows[active];
			if (row) commit(row.name);
		}
	}

	function onBlur() {
		closeList();
	}

	function followMenu(_node: HTMLElement) {
		const move = () => place();
		window.addEventListener("scroll", move, true);
		window.addEventListener("resize", move);
		return {
			destroy() {
				window.removeEventListener("scroll", move, true);
				window.removeEventListener("resize", move);
			},
		};
	}

	function choiceId(row: ClassChoice, index: number): string {
		const slug = `${label}-${index}-${row.name || "none"}`.replace(/\s+/g, "-");
		return slug;
	}

	function activeDescendant(): string | undefined {
		const row = rows[active];
		if (!open || !row) return undefined;
		return choiceId(row, active);
	}
</script>

<div class="relative min-w-32">
	<input
		bind:this={inputEl}
		class="w-full border border-neutral-600 bg-neutral-950 px-2 py-2 font-mono text-sm text-neutral-100 disabled:cursor-not-allowed disabled:text-neutral-500"
		role="combobox"
		aria-label={label}
		aria-expanded={open}
		aria-autocomplete="list"
		aria-activedescendant={activeDescendant()}
		autocomplete="off"
		spellcheck="false"
		{disabled}
		{placeholder}
		value={open ? query : value}
		on:focus={openList}
		on:input={onType}
		on:keydown={onKey}
		on:blur={onBlur}
	/>
	{#if open}
		<ul
			use:followMenu
			class="fixed z-30 max-h-48 overflow-y-auto border border-neutral-600 bg-neutral-950 py-1 shadow-lg"
			style={menuStyle}
			role="listbox"
			aria-label={label}
		>
			{#each rows as row, index (row.label + index)}
				<li>
					<button
						type="button"
						id={choiceId(row, index)}
						role="option"
						aria-selected={index === active}
						class="block w-full px-2 py-1.5 text-left font-mono text-sm {index === active
							? 'bg-neutral-800 text-neutral-100'
							: 'text-neutral-200'} {row.name ? '' : 'text-neutral-400'}"
						on:mousedown|preventDefault={() => commit(row.name)}
						on:mouseenter={() => (active = index)}
					>
						{row.label}
					</button>
				</li>
			{:else}
				<li class="px-2 py-2 text-sm text-neutral-500">No matching class</li>
			{/each}
		</ul>
	{/if}
</div>
