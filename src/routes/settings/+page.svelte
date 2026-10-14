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
let erpName = "";
let erpUrl = "";
let erpNotice = "";
let savingErp = false;
let scanSeconds = 8;
let scanNotice = "";
let savingScan = false;

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
		await loadErp();
		await loadScan();
	} catch (err: unknown) {
		notice = errorMessage(
			err,
			"Could not load the BoM source. Is the vision sidecar running?",
		);
	}
}

async function loadScan() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/scan");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		const seconds = Number(data.seconds);
		if (Number.isInteger(seconds)) scanSeconds = seconds;
	} catch (err: unknown) {
		scanNotice = errorMessage(err, "Could not load the scan interval");
	}
}

async function loadErp() {
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp");
		if (!res.ok) throw new Error(await res.text());
		const data = await res.json();
		erpName = typeof data.name === "string" ? data.name : "";
		erpUrl = typeof data.url === "string" ? data.url : "";
	} catch (err: unknown) {
		erpNotice = errorMessage(err, "Could not load the ERP button");
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

async function saveScan() {
	savingScan = true;
	scanNotice = "";
	const seconds = Number(scanSeconds);
	if (!Number.isInteger(seconds) || seconds < 1 || seconds > 60) {
		scanNotice = "Enter a whole number of seconds from 1 to 60.";
		savingScan = false;
		return;
	}
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/scan", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ seconds }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || "Save failed");
		}
		if (Number.isInteger(Number(data.seconds)))
			scanSeconds = Number(data.seconds);
		scanNotice = data.message || "Saved";
	} catch (err: unknown) {
		scanNotice = errorMessage(err, "Could not save the scan interval");
	} finally {
		savingScan = false;
	}
}

async function saveErp() {
	savingErp = true;
	erpNotice = "";
	try {
		const res = await fetch("http://127.0.0.1:8008/settings/erp", {
			method: "POST",
			headers: { "Content-Type": "application/json" },
			body: JSON.stringify({ name: erpName, url: erpUrl }),
		});
		const data = await res.json().catch(() => ({}));
		if (!res.ok || data.status === "fail") {
			throw new Error(data.message || (await res.text()) || "Save failed");
		}
		erpName = typeof data.name === "string" ? data.name : erpName;
		erpUrl = typeof data.url === "string" ? data.url : erpUrl;
		erpNotice = data.message || "Saved";
	} catch (err: unknown) {
		erpNotice = errorMessage(err, "Could not save the ERP button");
	} finally {
		savingErp = false;
	}
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

<div class="flex h-dvh max-h-dvh flex-col overflow-hidden overscroll-none bg-neutral-950 text-neutral-100">
	<header class="flex shrink-0 items-center justify-between gap-3 border-b border-neutral-800 bg-neutral-900 px-4 py-2">
		<div class="flex items-baseline gap-3">
			<h1 class="text-lg font-semibold tracking-tight">Settings</h1>
			<p class="text-xs font-medium uppercase tracking-widest text-neutral-500">Admin</p>
		</div>
		<a
			href="/"
			class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
		>
			Operator
		</a>
	</header>

	<main class="min-h-0 flex-1 overflow-y-auto">
		<div class="mx-auto flex max-w-2xl flex-col gap-4 p-4">
			{#if !unlocked}
				<section class="flex flex-col gap-3 border border-neutral-700 bg-neutral-900 p-4">
					<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
						Password
						<input
							type="password"
							autocomplete="off"
							class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
							bind:value={password}
							on:keydown={(event) => event.key === "Enter" && unlock()}
						/>
					</label>
					<button
						type="button"
						class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white"
						on:click={unlock}
					>
						Unlock
					</button>
					{#if gateMessage}
						<p class="text-sm font-semibold text-red-400">{gateMessage}</p>
					{/if}
				</section>
			{:else}
				<form
					class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
					on:submit|preventDefault={saveSource}
				>
					<fieldset class="flex flex-col gap-3">
						<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							BoM source
						</legend>
						<label class="flex items-center gap-3 text-base font-medium">
							<input
								class="h-4 w-4 accent-blue-500"
								type="radio"
								name="bom-source"
								value="json"
								bind:group={source}
							/>
							JSON file
							<span class="text-sm font-normal text-neutral-400">vision/sample-bom.json</span>
						</label>
						<label class="flex items-center gap-3 text-base font-medium">
							<input
								class="h-4 w-4 accent-blue-500"
								type="radio"
								name="bom-source"
								value="mssql"
								bind:group={source}
							/>
							MSSQL
							<span class="text-sm font-normal text-neutral-400">vision/bom_queries.sql</span>
						</label>
					</fieldset>

					{#if source === "mssql"}
						<div class="flex flex-col gap-3 border-t border-neutral-800 pt-4">
							<fieldset class="flex flex-col gap-2">
								<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
									Authentication
								</legend>
								<label class="flex items-center gap-3 text-base font-medium">
									<input
										class="h-4 w-4 accent-blue-500"
										type="radio"
										name="mssql-auth"
										value="windows"
										bind:group={auth}
										on:change={onAuthChange}
									/>
									Windows Authentication
								</label>
								<label class="flex items-center gap-3 text-base font-medium">
									<input
										class="h-4 w-4 accent-blue-500"
										type="radio"
										name="mssql-auth"
										value="sql"
										bind:group={auth}
										on:change={onAuthChange}
									/>
									SQL Server Authentication
								</label>
							</fieldset>
							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								Server
								<input
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
									bind:value={mssql.server}
									autocomplete="off"
								/>
							</label>
							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								Database
								<input
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
									bind:value={mssql.database}
									autocomplete="off"
								/>
							</label>
							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								<span>
									Username
									{#if auth === "sql"}
										<span class="font-normal normal-case tracking-normal text-red-400">Required</span>
									{/if}
								</span>
								<input
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100 disabled:bg-neutral-800 disabled:text-neutral-500"
									bind:value={mssql.user}
									autocomplete="off"
									disabled={auth === "windows"}
									required={auth === "sql"}
								/>
							</label>
							<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
								<span>
									Password
									{#if auth === "sql"}
										<span class="font-normal normal-case tracking-normal text-red-400">Required</span>
									{/if}
								</span>
								<input
									type="password"
									class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100 disabled:bg-neutral-800 disabled:text-neutral-500"
									bind:value={mssqlPassword}
									autocomplete="new-password"
									disabled={auth === "windows"}
									required={auth === "sql"}
								/>
							</label>
							<p class="text-sm text-neutral-400">
								Inspect runs the queries in vision/bom_queries.sql.
							</p>
						</div>
					{/if}

					<div class="flex flex-wrap items-center gap-2">
						<button
							type="submit"
							class="inline-flex min-h-11 items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
							disabled={saving}
						>
							{saving ? "Saving..." : "Save"}
						</button>
						<button
							type="button"
							class="inline-flex min-h-11 items-center border border-neutral-600 bg-neutral-800 px-4 text-sm font-semibold text-neutral-100"
							on:click={lock}
						>
							Lock
						</button>
					</div>
					{#if notice}
						<p class="text-sm text-neutral-200">{notice}</p>
					{/if}
				</form>

				<form
					class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
					method="dialog"
					novalidate
					on:submit|preventDefault={saveScan}
				>
					<fieldset class="flex flex-col gap-3">
						<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							Barcode scan
						</legend>
						<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							Interval (seconds)
							<input
								type="number"
								class="w-28 border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
								bind:value={scanSeconds}
								min="1"
								max="60"
								step="1"
								required
							/>
						</label>
						<p class="text-sm text-neutral-400">
							How often the live feed checks for a barcode. The default is 8 seconds.
						</p>
					</fieldset>
					<button
						type="submit"
						class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
						disabled={savingScan}
					>
						{savingScan ? "Saving..." : "Save"}
					</button>
					{#if scanNotice}
						<p class="text-sm text-neutral-200">{scanNotice}</p>
					{/if}
				</form>

				<form
					class="flex flex-col gap-4 border border-neutral-700 bg-neutral-900 p-4"
					on:submit|preventDefault={saveErp}
				>
					<fieldset class="flex flex-col gap-3">
						<legend class="text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							ERP
						</legend>
						<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							Button name
							<input
								class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
								bind:value={erpName}
								maxlength="40"
								autocomplete="off"
								placeholder="ERP"
							/>
						</label>
						<label class="flex flex-col gap-1 text-[11px] font-semibold uppercase tracking-widest text-neutral-400">
							URL
							<input
								type="url"
								class="border border-neutral-600 bg-neutral-950 px-3 py-2 text-base font-medium normal-case tracking-normal text-neutral-100"
								bind:value={erpUrl}
								autocomplete="off"
								placeholder="https://"
							/>
						</label>
						<p class="text-sm text-neutral-400">
							The operator page shows a button with this name. It slides the URL over the vision view. Leave the URL empty to hide the button.
						</p>
					</fieldset>
					<button
						type="submit"
						class="inline-flex min-h-11 w-fit items-center border border-blue-500 bg-blue-600 px-4 text-sm font-semibold text-white disabled:border-neutral-700 disabled:bg-neutral-800 disabled:text-neutral-500"
						disabled={savingErp}
					>
						{savingErp ? "Saving..." : "Save"}
					</button>
					{#if erpNotice}
						<p class="text-sm text-neutral-200">{erpNotice}</p>
					{/if}
				</form>
			{/if}
		</div>
	</main>
</div>
