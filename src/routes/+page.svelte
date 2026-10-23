<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses

import { onDestroy, onMount } from "svelte";
// biome-ignore lint/correctness/noUnusedImports: the operator template uses this component
import CameraView from "$lib/operator/CameraView.svelte";
// biome-ignore lint/correctness/noUnusedImports: the operator template uses this component
import OperatorConsole from "$lib/operator/OperatorConsole.svelte";
// biome-ignore lint/correctness/noUnusedImports: the operator template uses this component
import OperatorToolbar from "$lib/operator/OperatorToolbar.svelte";
// biome-ignore lint/correctness/noUnusedImports: the operator template uses this component
import ResultsPanel from "$lib/operator/ResultsPanel.svelte";
import { createOperator } from "$lib/operator/session";
import { provideOperator } from "$lib/stores/operator";

const session = createOperator();
provideOperator(session);
const {
	operator,
	camerasMissing,
	cameraFeedOn,
	refs,
	start,
	stop,
	onImagePicked,
	onImageCancel,
	onErpKey,
	toggleModelMenu,
	closeModelMenu,
	chooseModel,
	closeErp,
	onIframeError,
	openErpInBrowser,
	toggleConsole,
	dismissPartRuleWarning,
} = session;

onMount(() => start());
onDestroy(() => stop());
</script>


<svelte:window on:keydown={onErpKey} />

<div class="relative flex h-dvh max-h-dvh flex-col overflow-hidden overscroll-none bg-neutral-950 text-neutral-100">
	<input
		bind:this={refs.imagePicker}
		type="file"
		accept="image/*"
		class="absolute h-px w-px opacity-0"
		tabindex="-1"
		on:change={onImagePicked}
		on:cancel={onImageCancel}
	/>
	<header class="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">LS-Vision</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Operator</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<p
				class="inline-flex min-h-9 items-center gap-2 border px-2.5 text-xs font-semibold uppercase tracking-wide {$operator.snapshot ||
				$camerasMissing
					? 'border-amber-700/80 bg-amber-950 text-amber-200'
					: $operator.feedOn && $cameraFeedOn
						? 'border-neutral-600 bg-neutral-800 text-neutral-100'
						: 'border-neutral-700 bg-neutral-900 text-neutral-400'}"
			>
				<span
					class="h-2 w-2 shrink-0 {$operator.snapshot || $camerasMissing
						? 'bg-amber-400'
						: $operator.feedOn && $cameraFeedOn
							? 'bg-green-400'
							: 'bg-neutral-500'}"
					aria-hidden="true"
				></span>
				{$operator.snapshot
					? "Stream stopped"
					: $camerasMissing
						? "No cameras found"
						: $operator.feedOn && $cameraFeedOn
							? `Streaming · ${$operator.streamLeft}s`
							: "Stream paused"}
			</p>
			{#if $operator.admin}
				<button
					type="button"
					class="inline-flex h-11 items-center border px-3 text-sm font-semibold {$operator.consoleOpen
						? 'border-neutral-400 bg-neutral-700 text-neutral-100'
						: 'border-neutral-600 bg-neutral-800 text-neutral-100'}"
					aria-expanded={$operator.consoleOpen}
					aria-controls="operator-console"
					on:click={toggleConsole}
				>
					Console
				</button>
			{/if}
			<a
				href="/results"
				class="inline-flex h-11 items-center border border-neutral-600 bg-neutral-800 px-3 text-sm font-semibold text-neutral-100"
			>
				Results
			</a>
			<a
				href="/settings"
				aria-label="Settings"
				class="inline-flex h-11 w-11 items-center justify-center border border-neutral-600 bg-neutral-800 text-neutral-100"
			>
				<svg
					xmlns="http://www.w3.org/2000/svg"
					viewBox="0 0 24 24"
					class="h-5 w-5"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					aria-hidden="true"
				>
					<circle cx="12" cy="12" r="3" />
					<path
						d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9c.3.6.9 1.1 1.5 1.1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1.1z"
					/>
				</svg>
			</a>
		</div>
	</header>

	<OperatorToolbar />

	<div class="grid min-h-0 flex-1 gap-3 overflow-hidden p-3 lg:grid-cols-[minmax(0,1.7fr)_minmax(22rem,28rem)]">
		<CameraView />

		<ResultsPanel />
	</div>

	{#if $operator.partRuleWarning}
		<div
			class="pointer-events-none absolute inset-x-0 bottom-14 z-30 flex justify-center px-4"
			role="status"
		>
			<div
				class="pointer-events-auto flex max-w-xl items-start gap-3 border border-amber-700/80 bg-neutral-900 px-4 py-3 text-sm text-amber-100 shadow-lg"
			>
				<p>Some part rules could not be applied (missing classes). Check Settings.</p>
				<button
					type="button"
					class="shrink-0 border border-amber-700/80 px-2 py-1 text-xs font-semibold uppercase tracking-wide text-amber-100"
					on:click={dismissPartRuleWarning}
				>
					Dismiss
				</button>
			</div>
		</div>
	{/if}

	{#if $operator.admin && $operator.consoleOpen}
		<OperatorConsole />
	{/if}

	<footer class="flex shrink-0 items-center border-t border-neutral-800 bg-neutral-900 px-4 py-1.5">
		<div class="text-[11px] font-semibold uppercase tracking-widest text-neutral-500">
			Model
			{#if $operator.admin}
				<span class="relative inline-block">
					<button
						type="button"
						class="ml-2 font-mono text-xs font-medium normal-case tracking-normal text-neutral-300 underline-offset-2 hover:underline disabled:text-neutral-500"
						aria-haspopup="menu"
						aria-expanded={$operator.modelMenuOpen}
						aria-label="Choose model"
						disabled={$operator.savingModel}
						on:click={toggleModelMenu}
					>
						{$operator.modelName}
					</button>
					{#if $operator.modelMenuOpen}
						<button
							type="button"
							class="fixed inset-0 z-40 cursor-default"
							aria-label="Close model menu"
							on:click={closeModelMenu}
						></button>
						<span
							role="menu"
							aria-label="Models"
							class="absolute bottom-full left-0 z-50 mb-2 max-h-60 min-w-44 overflow-auto border border-neutral-600 bg-neutral-900 py-1 shadow-lg"
						>
							{#each $operator.modelFiles as file (file)}
								<button
									type="button"
									role="menuitem"
									class="block w-full px-3 py-2 text-left font-mono text-xs font-medium normal-case tracking-normal hover:bg-neutral-800 disabled:text-neutral-500 {file.toLowerCase() ===
									$operator.modelName.toLowerCase()
										? 'bg-neutral-800 text-neutral-100'
										: 'text-neutral-300'}"
									disabled={$operator.savingModel}
									on:click={() => chooseModel(file)}
								>
									{file}
								</button>
							{:else}
								<span class="block px-3 py-2 text-sm font-medium normal-case tracking-normal text-neutral-400">
									No .pt or .onnx files in the app data models folder.
								</span>
							{/each}
						</span>
					{/if}
				</span>
			{:else}
				<span class="ml-2 font-mono text-xs font-medium normal-case tracking-normal text-neutral-300">
					{$operator.modelName}
				</span>
			{/if}
			{#if $operator.modelNotice}
				<span class="ml-3 font-medium normal-case tracking-normal text-red-300">
					{$operator.modelNotice}
				</span>
			{/if}
			{#if $operator.admin}
				<span class="ml-4">Barcode</span>
				<span class="ml-2 font-mono text-xs font-medium normal-case tracking-normal text-neutral-300">
					{$operator.barcodeDecoder || "—"}
				</span>
			{/if}
		</div>
	</footer>

	<button
		type="button"
		bind:this={refs.erpCloseButton}
		class="absolute inset-y-0 left-0 z-50 flex w-12 flex-col items-center justify-center gap-3 border-r border-neutral-700 bg-neutral-900 text-yellow-300 transition-transform duration-300 ease-out motion-reduce:transition-none {$operator.erpOpen
			? 'pointer-events-auto translate-x-0'
			: 'pointer-events-none -translate-x-full'}"
		aria-label="Back to vision"
		aria-hidden={$operator.erpOpen ? "false" : "true"}
		tabindex={$operator.erpOpen ? 0 : -1}
		on:click={closeErp}
	>
		<svg
			xmlns="http://www.w3.org/2000/svg"
			viewBox="0 0 24 24"
			class="h-6 w-6"
			fill="none"
			stroke="currentColor"
			stroke-width="2"
			aria-hidden="true"
		>
			<path d="M15 18l-6-6 6-6" />
		</svg>
		<span class="text-xs font-semibold uppercase tracking-widest [writing-mode:vertical-rl]">
			Back
		</span>
	</button>
	<div
		class="absolute inset-y-0 left-12 right-0 z-40 border-l border-neutral-800 bg-neutral-950 transition-transform duration-300 ease-out motion-reduce:transition-none {$operator.erpOpen
			? 'pointer-events-auto translate-x-0'
			: 'pointer-events-none translate-x-full'}"
		aria-hidden={$operator.erpOpen ? "false" : "true"}
	>
		{#if $operator.erpLoadedUrl}
			<iframe
				title={$operator.erpName || "ERP"}
				src={$operator.erpLoadedUrl}
				class="h-full w-full border-0 bg-white"
				on:error={onIframeError}
			></iframe>
		{/if}
		{#if $operator.iframeFailed}
			<div class="absolute inset-0 flex flex-col items-center justify-center gap-4 bg-neutral-950 px-8 text-center">
				<p class="text-lg font-medium text-neutral-100">Could not load this page in the panel.</p>
				<p class="max-w-xl break-all font-mono text-sm text-neutral-400">{$operator.erpUrl}</p>
				<button
					type="button"
					class="inline-flex min-h-11 items-center border border-yellow-300 bg-yellow-400 px-4 text-sm font-semibold text-neutral-950"
					on:click={openErpInBrowser}
				>
					Open in system browser
				</button>
			</div>
		{:else if $operator.erpChecking && !$operator.erpLoadedUrl}
			<div class="absolute inset-0 flex items-center justify-center bg-neutral-950">
				<p class="text-lg text-neutral-300">Opening {$operator.erpName || "ERP"}...</p>
			</div>
		{/if}
	</div>
</div>
