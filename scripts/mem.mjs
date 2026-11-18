import { spawn, execSync } from "node:child_process";
import http from "node:http";
import fs from "node:fs";

const [exe, dir, tabs = "1", runs = "5", label = "run", extra = "", out = "", idle = "15", sites = "local"] = process.argv.slice(2);
const port = 9800 + Math.floor(Math.random() * 150);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const med = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
const REAL = ["https://en.wikipedia.org/wiki/Rust_(programming_language)", "https://github.com/rust-lang/rust", "https://example.com/", "https://www.rust-lang.org/", "https://news.ycombinator.com/"];

const servers = [];
function serve(p) {
	const s = http.createServer((q, r) => {
		r.setHeader("content-type", "text/html");
		r.end(`<!doctype html><title>t${p}</title><body><h1>origin ${p}</h1>${"<p>lorem ipsum dolor sit amet</p>".repeat(200)}`);
	});
	s.listen(p, "127.0.0.1");
	servers.push(s);
}
for (let i = 0; i < 12; i++) serve(18100 + i);

const url = (i) => (sites === "real" ? REAL[i % REAL.length] : `http://s${i}.localhost:${18100 + i}/`);

function stop(p) {
	try {
		execSync(`taskkill /PID ${p.pid} /T /F`, { stdio: "ignore" });
	} catch {}
	try {
		const w = dir.replaceAll("/", "\\");
		const q = "Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'msedgewebview2.exe' -and $_.CommandLine -like '*" + w + "*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }";
		execSync(`powershell -NoProfile -Command "${q}"`, { stdio: "ignore" });
	} catch {}
}

const PS = `$p=Get-CimInstance Win32_Process | Where-Object { $_.Name -in 'msedgewebview2.exe','ferrum.exe' } | Select-Object ProcessId,ParentProcessId,Name,CommandLine; $f=@{}; Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $_.Name -like 'msedgewebview2*' -or $_.Name -like 'ferrum*' } | ForEach-Object { $f[[int]$_.IDProcess]=@($_.WorkingSetPrivate,$_.WorkingSet) }; $p | ForEach-Object { [pscustomobject]@{pid=$_.ProcessId;ppid=$_.ParentProcessId;name=$_.Name;cmd=$_.CommandLine;priv=$f[[int]$_.ProcessId][0];ws=$f[[int]$_.ProcessId][1]} } | ConvertTo-Json -Compress`;
function sample(root) {
	const all = JSON.parse(execSync(`powershell -NoProfile -Command "${PS.replaceAll('"', '\\"')}"`, { maxBuffer: 1 << 26 }).toString() || "[]");
	const by = new Map(all.map((x) => [x.pid, x]));
	const keep = new Set([root]);
	for (let ch = true; ch; ) {
		ch = false;
		for (const x of all) if (!keep.has(x.pid) && keep.has(x.ppid)) (keep.add(x.pid), (ch = true));
	}
	return [...keep].map((p) => by.get(p)).filter(Boolean).map((x) => {
		const c = x.cmd ?? "";
		const t = /--type=([\w-]+)/.exec(c)?.[1];
		const u = /--utility-sub-type=([\w.]+)/.exec(c)?.[1];
		const kind = x.name === "ferrum.exe" ? "app" : !t ? "browser" : t === "utility" ? `utility:${u ?? "?"}` : t === "renderer" && c.includes("--extension-process") ? "renderer:ext" : t;
		return { pid: x.pid, kind, priv: +x.priv || 0, ws: +x.ws || 0 };
	});
}

async function targets() {
	try {
		return await (await fetch(`http://127.0.0.1:${port}/json`)).json();
	} catch {
		return [];
	}
}

async function chrome() {
	for (let i = 0; i < 100; i++) {
		const t = (await targets()).find((t) => t.type === "page" && t.url.includes("/chrome"));
		if (t) {
			const ws = new WebSocket(t.webSocketDebuggerUrl);
			await new Promise((r) => (ws.onopen = r));
			let id = 0;
			const pend = new Map();
			ws.onmessage = (m) => {
				const d = JSON.parse(m.data);
				pend.get(d.id)?.(d);
			};
			return { close: () => ws.close(), ev: (expr) => new Promise((r) => (pend.set(++id, r), ws.send(JSON.stringify({ id, method: "Runtime.evaluate", params: { expression: expr } })))) };
		}
		await sleep(100);
	}
	throw new Error("no chrome target");
}

async function once() {
	const env = { ...process.env, RB_DATA_DIR: dir, FERRUM_NO_INSTALL: "1", WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} ${extra}` };
	const p = spawn(exe, [], { env, stdio: "ignore" });
	try {
		const c = await chrome();
		await sleep(8000);
		const born = new Map();
		const note = (tag) => {
			for (const x of sample(p.pid)) if (!born.has(x.pid)) born.set(x.pid, tag);
		};
		note("startup");
		const send = (o) => c.ev(`ipc.postMessage(${JSON.stringify(JSON.stringify(o))})`);
		for (let i = 0; i < +tabs; i++) {
			if (i) await send({ c: "newtab" });
			await sleep(400);
			await send({ c: "nav", q: url(i) });
			await sleep(sites === "real" ? 4000 : 1500);
			note(`tab${i + 1}`);
		}
		await sleep(+idle * 1000);
		const s = sample(p.pid).map((x) => ({ ...x, born: born.get(x.pid) ?? "late" }));
		const sus = (await c.ev("document.querySelectorAll('.tab.sus').length")).result.result.value;
		const tg = (await targets()).map((t) => `${t.type}:${t.url.slice(0, 48)}`);
		c.close();
		return { s, tg, sus };
	} finally {
		stop(p);
		await sleep(1500);
	}
}

const rows = [];
for (let i = 0; i < +runs; i++) {
	const r = await once();
	const count = r.s.length;
	const priv = r.s.reduce((a, x) => a + x.priv, 0) / 1048576;
	const ws = r.s.reduce((a, x) => a + x.ws, 0) / 1048576;
	rows.push({ label, tabs: +tabs, count, priv, ws, procs: r.s, targets: r.tg, sus: r.sus });
	console.log(`${label} tabs=${tabs} run=${i + 1} procs=${count} private=${priv.toFixed(0)}MB ws=${ws.toFixed(0)}MB sus=${r.sus}`);
	if (out) fs.appendFileSync(out, JSON.stringify(rows.at(-1)) + "\n");
}
console.log(`${label} tabs=${tabs} MEDIAN procs=${med(rows.map((r) => r.count))} private=${med(rows.map((r) => r.priv)).toFixed(0)}MB ws=${med(rows.map((r) => r.ws)).toFixed(0)}MB`);
const m = rows.sort((a, b) => a.priv - b.priv)[Math.floor(rows.length / 2)];
for (const x of m.procs.sort((a, b) => b.priv - a.priv)) console.log(`  ${String(x.pid).padStart(6)} ${x.kind.padEnd(18)} ${x.born.padEnd(8)} priv ${(x.priv / 1048576).toFixed(1).padStart(6)}MB ws ${(x.ws / 1048576).toFixed(1).padStart(6)}MB`);
console.log("  targets:", m.targets.join(" | "));
servers.forEach((s) => s.close());
process.exit(0);
