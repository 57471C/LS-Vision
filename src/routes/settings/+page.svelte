<script lang="ts">
// biome-ignore-all lint/correctness/noUnusedVariables: Biome 2.4 does not treat Svelte template references as uses
import { onMount } from "svelte";

const DEV_PASSWORD = "admin";
const UNLOCK_KEY = "ls-vision-settings-open";

type BomSource = "json" | "mssql";
type AuthMode = "windows" | "sql";
type MssqlFields = { server: string; database: string; user: string };

let unlocked = false;
let password = "";
let gateMessage = "";
let source: BomSource = "json";
let auth: AuthMode = "windows";
let mssql: MssqlFields = { server: "", database: "", user: "" };
let mssqlPassword = "";
let notice = "";
let saving = false;

function errorMessage(err: unknown, fallback: string): string {
	return err instanceof Error && err.message ? err.message : fallback;
}

async function loadSource() {
	notice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/bom-source");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		source = data.source === "mssql" ? "mssql" : "json";
		auth = data.mssql?.auth === "sql" ? "sql" : "windows";
		mssql = {
			server: data.mssql?.server ?? "",
			database: data.mssql?.database ?? "",
			user: auth === "windows" ? "" : (data.mssql?.user ?? ""),
		};
		mssqlPassword = auth === "windows" ? "" : (data.mssql?.password ?? "");
	} catch (err: unknown) {
		notice = errorMessage(
			err,
			"Could not load the BoM source. Is the vision sidecar running?",
		);
	}
}

function unlock() {
	if (password !== DEV_PASSWORD) {
		gateMessage = "Wrong password";
		return;
	}
	password = "";
	gateMessage = "";
	unlocked = true;
	sessionStorage.setItem(UNLOCK_KEY, "1");
	loadSource();
}

function onAuthChange() {
	if (auth === "windows") {
		mssql.user = "";
		mssqlPassword = "";
	}
}

function lock() {
	unlocked = false;
	password = "";
	gateMessage = "";
	mssqlPassword = "";
	sessionStorage.removeItem(UNLOCK_KEY);
}

async function saveSource() {
	if (auth === "windows") {
		mssql.user = "";
		mssqlPassword = "";
	} else if (!mssql.user.trim() || !mssqlPassword.trim()) {
		notice =
			"Username and password are required for SQL Server Authentication.";
		return;
	}
	saving = true;
	notice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/bom-source", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({
				source,
				server: mssql.server,
				database: mssql.database,
				auth,
				user: auth === "windows" ? "" : mssql.user,
				password: auth === "windows" ? "" : mssqlPassword,
			}),
		});
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		if (data.status !== "pass") throw new Error(data.message || "Save failed");
		source = data.source === "mssql" ? "mssql" : source;
		auth = data.mssql?.auth === "sql" ? "sql" : "windows";
		mssql.user = data.mssql?.user ?? "";
		mssqlPassword = data.mssql?.password ?? "";
		notice = data.message || "Saved";
	} catch (err: unknown) {
		notice = errorMessage(err, "Could not save the BoM source");
	} finally {
		saving = false;
	}
}

onMount(() => {
	if (sessionStorage.getItem(UNLOCK_KEY) === "1") {
		unlocked = true;
		loadSource();
	}
});
</script>

<div class="min-h-screen bg-neutral-100 text-neutral-950">
	<div class="mx-auto flex max-w-xl flex-col gap-6 p-6">
		<div class="flex items-center justify-between gap-4">
			<h1 class="text-2xl font-black tracking-tight">Settings</h1>
			<a href="/" class="border-2 border-neutral-950 bg-white px-4 py-3 text-base font-bold">Operator</a>
		</div>

		{#if !unlocked}
			<section class="flex flex-col gap-3 border-4 border-neutral-950 bg-white p-5">
				<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
					Password
					<input
						type="password"
						autocomplete="off"
						class="border-2 border-neutral-950 bg-white px-3 py-3 text-lg font-semibold normal-case tracking-normal"
						bind:value={password}
						on:keydown={(event) => event.key === "Enter" && unlock()}
					/>
				</label>
				<button
					type="button"
					class="w-fit border-2 border-neutral-950 bg-neutral-950 px-5 py-3 text-lg font-bold text-white"
					on:click={unlock}
				>
					Unlock
				</button>
				{#if gateMessage}
					<p class="text-lg font-bold text-red-700">{gateMessage}</p>
				{/if}
			</section>
		{:else}
			<form
				class="flex flex-col gap-4 border-4 border-neutral-950 bg-white p-5"
				on:submit|preventDefault={saveSource}
			>
				<fieldset class="flex flex-col gap-3">
					<legend class="text-sm font-bold uppercase tracking-wide">BoM source</legend>
					<label class="flex items-center gap-3 text-lg font-semibold">
						<input type="radio" name="bom-source" value="json" bind:group={source} />
						JSON file
						<span class="text-base font-normal text-neutral-600">vision/sample-bom.json</span>
					</label>
					<label class="flex items-center gap-3 text-lg font-semibold">
						<input type="radio" name="bom-source" value="mssql" bind:group={source} />
						MSSQL
						<span class="text-base font-normal text-neutral-600">not connected yet</span>
					</label>
				</fieldset>

				{#if source === "mssql"}
					<div class="flex flex-col gap-3">
						<fieldset class="flex flex-col gap-2">
							<legend class="text-sm font-bold uppercase tracking-wide">Authentication</legend>
							<label class="flex items-center gap-3 text-lg font-semibold">
								<input
									type="radio"
									name="mssql-auth"
									value="windows"
									bind:group={auth}
									on:change={onAuthChange}
								/>
								Windows Authentication
							</label>
							<label class="flex items-center gap-3 text-lg font-semibold">
								<input
									type="radio"
									name="mssql-auth"
									value="sql"
									bind:group={auth}
									on:change={onAuthChange}
								/>
								SQL Server Authentication
							</label>
						</fieldset>
						<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
							Server
							<input
								class="border-2 border-neutral-950 px-3 py-3 text-lg font-semibold normal-case tracking-normal"
								bind:value={mssql.server}
								autocomplete="off"
							/>
						</label>
						<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
							Database
							<input
								class="border-2 border-neutral-950 px-3 py-3 text-lg font-semibold normal-case tracking-normal"
								bind:value={mssql.database}
								autocomplete="off"
							/>
						</label>
						<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
							<span>
								Username
								{#if auth === "sql"}
									<span class="font-normal normal-case text-red-700">Required</span>
								{/if}
							</span>
							<input
								class="border-2 border-neutral-950 px-3 py-3 text-lg font-semibold normal-case tracking-normal disabled:bg-neutral-200"
								bind:value={mssql.user}
								autocomplete="off"
								disabled={auth === "windows"}
								required={auth === "sql"}
							/>
						</label>
						<label class="flex flex-col gap-1 text-sm font-bold uppercase tracking-wide">
							<span>
								Password
								{#if auth === "sql"}
									<span class="font-normal normal-case text-red-700">Required</span>
								{/if}
							</span>
							<input
								type="password"
								class="border-2 border-neutral-950 px-3 py-3 text-lg font-semibold normal-case tracking-normal disabled:bg-neutral-200"
								bind:value={mssqlPassword}
								autocomplete="new-password"
								disabled={auth === "windows"}
								required={auth === "sql"}
							/>
						</label>
						<p class="text-sm text-neutral-600">MSSQL lookup is not implemented yet.</p>
					</div>
				{/if}

				<div class="flex flex-wrap items-center gap-3">
					<button
						type="submit"
						class="border-2 border-neutral-950 bg-neutral-950 px-5 py-3 text-lg font-bold text-white disabled:bg-neutral-400"
						disabled={saving}
					>
						{saving ? "Saving..." : "Save"}
					</button>
					<button
						type="button"
						class="border-2 border-neutral-950 bg-white px-4 py-3 text-base font-bold"
						on:click={lock}
					>
						Lock
					</button>
				</div>
				{#if notice}
					<p class="text-lg">{notice}</p>
				{/if}
			</form>
		{/if}
	</div>
</div>
