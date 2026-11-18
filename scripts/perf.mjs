import { spawn, execSync } from "node:child_process";
import dns from "node:dns";
import net from "node:net";
import http from "node:http";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const [exe, dir, mode = "startup", runs = "7", extra = ""] = process.argv.slice(2);
const port = 9400 + Math.floor(Math.random() * 400);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const now = () => performance.now();
const med = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)];
const fmt = (a) => (a.length ? `med ${med(a).toFixed(0)} min ${Math.min(...a).toFixed(0)} max ${Math.max(...a).toFixed(0)} n=${a.length}` : "n/a");

function launch(flags = extra) {
	const env = { ...process.env, RB_DATA_DIR: dir, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} ${flags}` };
	const t0 = now();
	const p = spawn(exe, [], { env, stdio: "ignore" });
	return { p, t0 };
}

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

async function targets() {
	try {
		return await (await fetch(`http://127.0.0.1:${port}/json`)).json();
	} catch {
		return [];
	}
}

function cdp(url) {
	const ws = new WebSocket(url);
	let id = 0;
	const pend = new Map();
	const ev = [];
	ws.onmessage = (m) => {
		const d = JSON.parse(m.data);
		if (d.id && pend.has(d.id)) pend.get(d.id)(d);
		else if (d.method) ev.forEach((f) => f(d));
	};
	const ready = new Promise((r) => (ws.onopen = r));
	return {
		ready,
		on: (f) => ev.push(f),
		send: (method, params = {}) =>
			new Promise((r) => {
				const i = ++id;
				pend.set(i, r);
				ws.send(JSON.stringify({ id: i, method, params }));
			}),
		close: () => ws.close(),
	};
}

async function paintMs(t0, url) {
	const c = cdp(url);
	await c.ready;
	await c.send("Performance.enable");
	for (;;) {
		const t = now();
		const r = await c.send("Performance.getMetrics");
		const m = Object.fromEntries((r.result?.metrics ?? []).map((x) => [x.name, x.value]));
		const fmp = m.FirstMeaningfulPaint > 0 ? m.FirstMeaningfulPaint : m.DomContentLoaded;
		if (fmp > 0) {
			c.close();
			return t - (m.Timestamp - fmp) * 1000 - t0;
		}
		await sleep(10);
	}
}

async function startup() {
	const out = { chrome: [], tab: [] };
	for (let i = 0; i < +runs; i++) {
		const { p, t0 } = launch();
		const got = {};
		const seen = new Set();
		const jobs = [];
		while (Object.keys(got).length < 2 && now() - t0 < 20000) {
			for (const t of await targets()) {
				if (t.type !== "page") continue;
				const k = t.url.includes("/chrome") ? "chrome" : t.url.includes("/newtab") ? "tab" : null;
				if (k && !seen.has(t.id)) {
					seen.add(t.id);
					got[k] ??= 0;
					jobs.push(paintMs(t0, t.webSocketDebuggerUrl).then((v) => (got[k] = got[k] ? Math.min(got[k], v) : v)));
				}
			}
			await sleep(10);
		}
		await Promise.race([Promise.all(jobs), sleep(8000)]);
		for (const k of ["chrome", "tab"]) if (got[k] > 0) out[k].push(got[k]);
		stop(p);
		await sleep(1500);
	}
	console.log("startup->first meaningful paint ms");
	console.log(" chrome UI:", fmt(out.chrome));
	console.log(" first tab (DCL):", fmt(out.tab));
}

const SITES = ["https://en.wikipedia.org/wiki/Rust_(programming_language)", "https://github.com/rust-lang/rust", "https://www.youtube.com/", "http://127.0.0.1:8099/index.html"];

async function nav() {
	const { p, t0 } = launch();
	let tab;
	while (!tab && now() - t0 < 20000) {
		tab = (await targets()).find((t) => t.type === "page" && t.url.includes("/newtab"));
		await sleep(50);
	}
	await sleep(3000);
	const c = cdp(tab.webSocketDebuggerUrl);
	await c.ready;
	await c.send("Page.enable");
	await c.send("Network.enable");
	await c.send("Performance.enable");
	let waiter;
	let tim;
	c.on((d) => {
		if (d.method === "Network.responseReceived" && d.params.type === "Document" && !tim) tim = d.params.response.timing;
		if (d.method === "Page.loadEventFired" && waiter) waiter();
	});
	const go = async (url) => {
		tim = undefined;
		const t = now();
		const w = new Promise((r) => (waiter = r));
		await c.send("Page.navigate", { url });
		await Promise.race([w, sleep(30000)]);
		const load = now() - t;
		const m = Object.fromEntries(((await c.send("Performance.getMetrics")).result?.metrics ?? []).map((x) => [x.name, x.value]));
		return { tim, load, dcl: (m.DomContentLoaded - m.NavigationStart) * 1000, fmp: m.FirstMeaningfulPaint > 0 ? (m.FirstMeaningfulPaint - m.NavigationStart) * 1000 : NaN };
	};
	for (const site of SITES) {
		const res = { cold: [], warm: [], dcl: [], fmp: [], dns: [], con: [], ttfb: [] };
		for (let i = 0; i < +runs; i++) {
			await go("about:blank");
			await c.send("Network.clearBrowserCache");
			const a = await go(site);
			res.cold.push(a.load);
			if (a.tim) {
				const T = a.tim;
				res.dns.push(Math.max(0, T.dnsEnd - T.dnsStart));
				res.con.push(Math.max(0, T.connectEnd - T.connectStart));
				res.ttfb.push(T.receiveHeadersStart);
			}
			res.dcl.push(a.dcl);
			if (a.fmp > 0) res.fmp.push(a.fmp);
			await go("about:blank");
			res.warm.push((await go(site)).load);
		}
		console.log(site);
		console.log("  cold load ms:", fmt(res.cold), "| dcl:", fmt(res.dcl), "| fmp:", fmt(res.fmp));
		console.log("  warm load ms:", fmt(res.warm));
		console.log("  cold dns:", fmt(res.dns), "| connect+tls:", fmt(res.con), "| ttfb:", fmt(res.ttfb));
	}
	stop(p);
}

async function first() {
	const url = extra.split(" ").find((x) => x.startsWith("http"));
	const rest = extra.replace(url, "");
	const res = { load: [], dns: [], con: [], ttfb: [] };
	for (let i = 0; i < +runs; i++) {
		const { p, t0 } = launch();
		let tab;
		while (!tab && now() - t0 < 20000) {
			tab = (await targets()).find((t) => t.type === "page" && t.url.includes("/newtab"));
			await sleep(50);
		}
		await sleep(2500);
		const c = cdp(tab.webSocketDebuggerUrl);
		await c.ready;
		await c.send("Page.enable");
		await c.send("Network.enable");
		let tim, waiter;
		c.on((d) => {
			if (d.method === "Network.responseReceived" && d.params.type === "Document" && !tim) tim = d.params.response.timing;
			if (d.method === "Page.loadEventFired" && waiter) waiter();
		});
		await c.send("Network.clearBrowserCache");
		if (rest.includes("pre")) {
			const h = new URL(url).hostname;
			await new Promise((r) => dns.lookup(h, () => r()));
			await new Promise((r) => {
				const k = net.connect(443, h, () => (k.destroy(), r()));
				k.on("error", r);
			});
		}
		const t = now();
		const w = new Promise((r) => (waiter = r));
		await c.send("Page.navigate", { url });
		await Promise.race([w, sleep(30000)]);
		res.load.push(now() - t);
		if (tim) {
			res.dns.push(Math.max(0, tim.dnsEnd - tim.dnsStart));
			res.con.push(Math.max(0, tim.connectEnd - tim.connectStart));
			res.ttfb.push(tim.receiveHeadersStart);
		}
		stop(p);
		await sleep(1200);
	}
	console.log(url, rest);
	console.log("  load:", fmt(res.load), "| dns:", fmt(res.dns), "| connect+tls:", fmt(res.con), "| ttfb:", fmt(res.ttfb));
}

async function speedo() {
	const { p, t0 } = launch();
	let tab;
	while (!tab && now() - t0 < 20000) {
		tab = (await targets()).find((t) => t.type === "page" && t.url.includes("/newtab"));
		await sleep(50);
	}
	await sleep(3000);
	const c = cdp(tab.webSocketDebuggerUrl);
	await c.ready;
	await c.send("Page.enable");
	await c.send("Page.navigate", { url: "https://browserbench.org/Speedometer3.1/?startAutomatically" });
	await sleep(+runs * 1000);
	const r = await c.send("Page.captureScreenshot", { format: "png" });
	(await import("node:fs")).writeFileSync(extra, Buffer.from(r.result.data, "base64"));
	stop(p);
}

const MEDIA = path.join(os.tmpdir(), "rb_media");
const mimes = { ".jpg": "image/jpeg", ".png": "image/png", ".gif": "image/gif", ".webm": "video/webm", ".mp4": "video/mp4", ".html": "text/html" };

function serve() {
	const srv = http.createServer((q, r) => {
		const f = path.join(MEDIA, path.basename(new URL(q.url, "http://x").pathname));
		if (!fs.existsSync(f)) return r.writeHead(404).end();
		const size = fs.statSync(f).size;
		const m = /bytes=(\d+)-(\d*)/.exec(q.headers.range ?? "");
		const h = { "Content-Type": mimes[path.extname(f)] ?? "application/octet-stream", "Accept-Ranges": "bytes", "Cache-Control": "no-store" };
		if (!m) return r.writeHead(200, { ...h, "Content-Length": size }), fs.createReadStream(f).pipe(r);
		const a = +m[1];
		const b = m[2] ? +m[2] : size - 1;
		r.writeHead(206, { ...h, "Content-Range": `bytes ${a}-${b}/${size}`, "Content-Length": b - a + 1 });
		fs.createReadStream(f, { start: a, end: b }).pipe(r);
	});
	return new Promise((r) => srv.listen(8097, "127.0.0.1", () => r(srv)));
}

async function boot(flags) {
	const { p, t0 } = launch(flags);
	let tab;
	while (!tab && now() - t0 < 20000) {
		tab = (await targets()).find((t) => t.type === "page" && t.url.includes("/newtab"));
		await sleep(50);
	}
	await sleep(2500);
	const c = cdp(tab.webSocketDebuggerUrl);
	await c.ready;
	const ver = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
	const b = cdp(ver.webSocketDebuggerUrl);
	await b.ready;
	return { p, c, b };
}

const cpu = async (b) => ((await b.send("SystemInfo.getProcessInfo")).result?.processInfo ?? []).reduce((a, x) => a + x.cpuTime, 0);
const ev = async (c, expr) => {
	const r = await c.send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
	if (r.result?.exceptionDetails) throw new Error(JSON.stringify(r.result.exceptionDetails));
	return r.result.result.value;
};

const IMAGES = `(async()=>{const r=Math.random();const t=performance.now();const box=document.body;box.innerHTML='';
const im=[...Array(24)].flatMap((_,i)=>[['jpg',i],['png',i]]).map(([e,i])=>{const x=new Image();x.style.cssText='width:300px;height:200px';x.src='http://127.0.0.1:8097/photo'+i+'.'+e+'?r='+r;box.append(x);return x});
await Promise.all(im.map(x=>x.decode()));const dec=performance.now()-t;
await new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)));return {dec,paint:performance.now()-t}})()`;

const GIFS = `(async()=>{const r=Math.random();const box=document.body;box.innerHTML='';const t=performance.now();
const im=[0,1,2,3].map(i=>{const x=new Image();x.src='http://127.0.0.1:8097/anim'+i+'.gif?r='+r;box.append(x);return x});
await Promise.all(im.map(x=>x.decode()));const dec=performance.now()-t;const d=[];let last=performance.now();
await new Promise(res=>{const f=n=>{d.push(n-last);last=n;n-t<4000?requestAnimationFrame(f):res()};requestAnimationFrame(f)});
d.shift();d.sort((a,b)=>a-b);return {dec,frames:d.length,jank:d.filter(x=>x>25).length,p95:d[Math.floor(d.length*.95)]}})()`;

const videoExpr = (file) => `(async()=>{const box=document.body;box.innerHTML='';const v=document.createElement('video');v.muted=true;v.loop=true;v.style.cssText='width:1280px;height:720px';box.append(v);
const t=performance.now();const first=new Promise(r=>v.requestVideoFrameCallback(()=>r(performance.now()-t)));v.src='http://127.0.0.1:8097/${file}?r='+Math.random();v.play();const ff=await first;
await new Promise(r=>setTimeout(r,6000));const q=v.getVideoPlaybackQuality();return {first:ff,total:q.totalVideoFrames,dropped:q.droppedVideoFrames}})()`;

const REAL = `new Promise(res=>{let lcp=0;new PerformanceObserver(l=>{for(const e of l.getEntries())lcp=e.startTime}).observe({type:'largest-contentful-paint',buffered:true});
setTimeout(async()=>{const im=[...document.images].filter(i=>i.complete&&i.naturalWidth);const t=performance.now();await Promise.all(im.map(i=>i.decode().catch(()=>0)));res({lcp,imgs:im.length,decode:performance.now()-t,load:performance.timing.loadEventEnd-performance.timing.navigationStart})},1500)})`;

const REAL_PAGES = ["https://commons.wikimedia.org/wiki/Main_Page", "https://en.wikipedia.org/wiki/Animated_GIF"];

async function nav2(c, url) {
	await c.send("Page.enable");
	let w;
	const done = new Promise((r) => (w = r));
	c.on((d) => d.method === "Page.loadEventFired" && w());
	await c.send("Page.navigate", { url });
	await Promise.race([done, sleep(30000)]);
}

async function runOnce(flags, vids) {
	const { p, c, b } = await boot(flags);
	const o = {};
	try {
		await c.send("Network.enable");
		await c.send("Network.setCacheDisabled", { cacheDisabled: true });
		await nav2(c, "http://127.0.0.1:8097/blank.html");
		const info = (await b.send("SystemInfo.getInfo")).result?.gpu?.featureStatus ?? {};
		o.feat = ["gpu_compositing", "rasterization", "video_decode", "canvas_oop_rasterization", "webgl"].map((k) => `${k}=${info[k] ?? "?"}`).join(" ");
		let c0 = await cpu(b);
		o.img = await ev(c, IMAGES);
		o.imgCpu = (await cpu(b)) - c0;
		c0 = await cpu(b);
		o.gif = await ev(c, GIFS);
		o.gifCpu = (await cpu(b)) - c0;
		for (const f of vids) {
			c0 = await cpu(b);
			o[f] = await ev(c, videoExpr(f));
			o[f].cpu = (await cpu(b)) - c0;
			o[f].hw = await ev(c, `navigator.mediaCapabilities.decodingInfo({type:'file',video:{contentType:'${f.endsWith("mp4") ? 'video/mp4; codecs="avc1.640028"' : 'video/webm; codecs="vp09.00.10.08"'}',width:1280,height:720,bitrate:2000000,framerate:30}}).then(r=>r.powerEfficient)`);
		}
		if (process.env.REAL) {
			o.real = {};
			for (const u of REAL_PAGES) {
				await c.send("Network.clearBrowserCache");
				await nav2(c, u);
				o.real[u] = await ev(c, REAL);
			}
		}
	} finally {
		c.close();
		b.close();
		stop(p);
	}
	return o;
}

async function ab() {
	const srv = await serve();
	const vids = ["v.mp4", "v.webm"].filter((f) => fs.existsSync(path.join(MEDIA, f)));
	fs.writeFileSync(path.join(MEDIA, "blank.html"), "<!doctype html><body style='margin:0'>");
	const sets = { base: [], cand: [] };
	for (let i = 0; i < +runs; i++) {
		for (const k of ["base", "cand"]) {
			sets[k].push(await runOnce(k === "base" ? "" : extra, vids));
			await sleep(1200);
		}
	}
	const col = (k, f) => sets[k].map(f).filter((x) => x != null && !Number.isNaN(x));
	const row = (name, f, d = 1) => {
		const s = (k) => {
			const a = col(k, f);
			return a.length ? `${med(a).toFixed(d)} [${Math.min(...a).toFixed(d)}-${Math.max(...a).toFixed(d)}]` : "n/a";
		};
		console.log(`  ${name.padEnd(22)} base ${s("base").padEnd(24)} cand ${s("cand")}`);
	};
	console.log("flags:", extra);
	console.log(" base:", sets.base[0].feat);
	console.log(" cand:", sets.cand[0].feat);
	row("img decode ms", (o) => o.img.dec, 0);
	row("img paint ms", (o) => o.img.paint, 0);
	row("img cpu s", (o) => o.imgCpu, 2);
	row("gif decode ms", (o) => o.gif.dec, 0);
	row("gif jank frames", (o) => o.gif.jank, 0);
	row("gif p95 frame ms", (o) => o.gif.p95, 1);
	row("gif cpu s/4s", (o) => o.gifCpu, 2);
	for (const f of vids) {
		console.log(`  ${f} powerEfficient(hw): base ${sets.base[0][f].hw} cand ${sets.cand[0][f].hw}`);
		row(`${f} first frame ms`, (o) => o[f].first, 0);
		row(`${f} dropped/total`, (o) => o[f].dropped / o[f].total, 3);
		row(`${f} cpu s/6s`, (o) => o[f].cpu, 2);
	}
	if (process.env.REAL)
		for (const u of REAL_PAGES) {
			console.log(" ", u);
			row("  lcp ms", (o) => o.real[u].lcp, 0);
			row("  decode ms", (o) => o.real[u].decode, 0);
			row("  load ms", (o) => o.real[u].load, 0);
		}
	srv.close();
}

async function genvideo() {
	const { p, c } = await boot("");
	await nav2(c, "about:blank");
	const code = (mime) => `(async()=>{const cv=document.createElement('canvas');cv.width=1280;cv.height=720;document.body.append(cv);const g=cv.getContext('2d');
let n=0;const draw=()=>{n++;g.fillStyle='#223';g.fillRect(0,0,1280,720);for(let i=0;i<120;i++){g.fillStyle='hsl('+((i*13+n*3)%360)+',70%,50%)';g.fillRect((i*97+n*7)%1280,(i*53+n*3)%720,90,60)}requestAnimationFrame(draw)};draw();
if(!MediaRecorder.isTypeSupported('${mime}'))return null;const rec=new MediaRecorder(cv.captureStream(30),{mimeType:'${mime}',videoBitsPerSecond:4000000});const ch=[];rec.ondataavailable=e=>ch.push(e.data);rec.start();await new Promise(r=>setTimeout(r,8000));
const done=new Promise(r=>rec.onstop=r);rec.stop();await done;const buf=await new Blob(ch).arrayBuffer();let s='';const u=new Uint8Array(buf);for(let i=0;i<u.length;i+=32768)s+=String.fromCharCode(...u.subarray(i,i+32768));return btoa(s)})()`;
	for (const [file, mime] of [["v.webm", "video/webm;codecs=vp9"], ["v.mp4", "video/mp4;codecs=avc1.640028"]]) {
		const b64 = await ev(c, code(mime));
		if (b64) {
			fs.writeFileSync(path.join(MEDIA, file), Buffer.from(b64, "base64"));
			console.log(file, fs.statSync(path.join(MEDIA, file)).size);
		} else console.log(file, "unsupported");
	}
	c.close();
	stop(p);
}

await (mode === "ab" ? ab() : mode === "genvideo" ? genvideo() : mode === "speedo" ? speedo() : mode === "startup" ? startup() : mode === "first" ? first() : nav());
process.exit(0);
