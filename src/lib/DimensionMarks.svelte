<svelte:options namespace="svg" />
<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import type { DimensionMark } from "$lib/metrology";

export let marks: DimensionMark[] = [];
// A dark line under a neutral pass stroke. Out-of-spec marks ignore this.
export let underlayPass = false;
</script>

{#each marks as mark (mark.key)}
	<g
		data-dimension={mark.kind}
		data-metrology-line={mark.mismatch ? "DIMENSION_MISMATCH" : "pass"}
		pointer-events="none"
	>
		{#if underlayPass && !mark.mismatch}
			<line
				x1={mark.x1}
				y1={mark.y1}
				x2={mark.x2}
				y2={mark.y2}
				stroke="#171717"
				stroke-width="3.5"
				stroke-linecap="butt"
				vector-effect="non-scaling-stroke"
			/>
		{/if}
		<line
			x1={mark.x1}
			y1={mark.y1}
			x2={mark.x2}
			y2={mark.y2}
			stroke={mark.stroke}
			stroke-width="1.25"
			stroke-linecap="butt"
			vector-effect="non-scaling-stroke"
		/>
		<polygon points={mark.head1} fill={mark.stroke} />
		<polygon points={mark.head2} fill={mark.stroke} />
		<rect
			x={mark.labelX}
			y={mark.labelY}
			width={mark.labelW}
			height={mark.labelH}
			rx={mark.radius}
			ry={mark.radius}
			fill={mark.labelFill}
		/>
		<text
			x={mark.labelX + mark.padX}
			y={mark.labelY + mark.labelH / 2}
			fill="#ffffff"
			font-size={mark.font}
			font-family="ui-monospace, monospace"
			font-weight="600"
			dominant-baseline="central"
		>
			{mark.text}
		</text>
	</g>
{/each}
