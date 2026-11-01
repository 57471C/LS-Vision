<div class="flex flex-col items-center gap-3" role="status" aria-live="polite">
	<svg
		width="60"
		height="60"
		viewBox="0 0 50 50"
		aria-hidden="true"
		class="ls-detect-mark"
	>
		<g fill="none" stroke="#16A34A" stroke-width="2">
			<circle cx="25" cy="25" r="15">
				<animate
					attributeName="r"
					values="15;12;15"
					dur="1.5s"
					repeatCount="indefinite"
				/>
			</circle>
			<path d="M25 10v30M10 25h30">
				<animate
					attributeName="stroke-dasharray"
					values="0,40;40,0"
					dur="2s"
					repeatCount="indefinite"
				/>
			</path>
		</g>
	</svg>
	<p class="text-lg font-semibold tracking-wide text-white">Detecting</p>
</div>

<style>
	@media (prefers-reduced-motion: reduce) {
		.ls-detect-mark animate {
			display: none;
		}
	}
</style>
