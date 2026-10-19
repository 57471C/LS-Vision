<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { afterUpdate } from "svelte";
import { useOperator } from "$lib/stores/operator";

const { clearConsole, operator, toggleConsole } = useOperator();

let scroller: HTMLDivElement | null = null;
let stick = true;

function onScroll() {
	if (!scroller) return;
	const gap =
		scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
	stick = gap < 24;
}

function lineClass(text: string): string {
	return /failed|error/i.test(text) ? "text-red-300" : "text-neutral-200";
}

afterUpdate(() => {
	if (stick && scroller) scroller.scrollTop = scroller.scrollHeight;
});
</script>

<section
	id="operator-console"
	class="flex h-56 max-h-[40dvh] shrink-0 flex-col border-t border-neutral-800 bg-neutral-950"
>
	<div class="flex shrink-0 items-center justify-between gap-2 border-b border-neutral-800 bg-neutral-900 px-3 py-1.5">
		<h2 class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
			Console
		</h2>
		<div class="flex items-center gap-2">
			<button
				type="button"
				class="inline-flex min-h-8 items-center border border-neutral-600 bg-neutral-800 px-2.5 text-xs font-semibold text-neutral-100 disabled:cursor-not-allowed disabled:opacity-50"
				on:click={clearConsole}
				disabled={$operator.consoleLines.length === 0}
			>
				Clear
			</button>
			<button
				type="button"
				class="inline-flex min-h-8 items-center border border-neutral-600 bg-neutral-800 px-2.5 text-xs font-semibold text-neutral-100"
				on:click={toggleConsole}
			>
				Close
			</button>
		</div>
	</div>
	<div
		class="console-scroll min-h-0 flex-1 overflow-y-auto px-3 py-2 font-mono text-xs leading-5"
		bind:this={scroller}
		on:scroll={onScroll}
		role="log"
		aria-live="polite"
		aria-relevant="additions"
		aria-label="Operator console"
	>
		{#each $operator.consoleLines as line (line.id)}
			<p class="whitespace-pre-wrap break-words">
				<span class="text-neutral-500">{line.time}</span>
				<span class={lineClass(line.text)}> {line.text}</span>
			</p>
		{:else}
			<p class="text-neutral-500">No lines yet.</p>
		{/each}
	</div>
</section>

<style>
	.console-scroll {
		color-scheme: dark;
		scrollbar-color: #a3a3a3 #171717;
		scrollbar-width: auto;
	}

	.console-scroll::-webkit-scrollbar {
		width: 12px;
		height: 12px;
	}

	.console-scroll::-webkit-scrollbar-track {
		background: #171717;
	}

	.console-scroll::-webkit-scrollbar-thumb {
		background: #a3a3a3;
		border: 2px solid #171717;
	}

	.console-scroll::-webkit-scrollbar-thumb:hover {
		background: #d4d4d4;
	}

	.console-scroll::-webkit-scrollbar-corner {
		background: #171717;
	}
</style>
