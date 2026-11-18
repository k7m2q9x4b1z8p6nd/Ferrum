use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.0.0";
const SEG: u64 = 4 << 20;
const MAX_SEGS: u64 = 256;
const CONN: usize = 8;
const BIG_CONN: usize = 16;
const BIG: u64 = 256 << 20;
const BUF: usize = 256 * 1024;
const TRIES: u32 = 4;
const HOPS: usize = 10;
pub const EXT: &str = "ferrum";

#[derive(Default)]
pub struct Rt {
    pub paused: AtomicBool,
    pub cancel: AtomicBool,
}

impl Rt {
    fn stop(&self) -> bool {
        self.paused.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed)
    }
}

pub struct Req {
    pub url: String,
    pub cookie: String,
    pub referer: String,
}

pub enum Msg {
    Info { total: u64, ranged: bool },
    Progress(u64, u64, Vec<(u64, u64)>),
    Paused,
    Cancelled,
    Done,
    Failed,
}

pub fn own(url: &str) -> bool {
    url.starts_with("http://") || url.starts_with("https://")
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut n = dest.as_os_str().to_owned();
    n.push(".");
    n.push(EXT);
    PathBuf::from(n)
}

pub fn unique(dest: PathBuf) -> PathBuf {
    let taken = |p: &Path| p.exists() || part_path(p).exists();
    if !taken(&dest) {
        return dest;
    }
    let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = dest.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|i| dest.with_file_name(format!("{stem} ({i}){ext}")))
        .find(|p| !taken(p))
        .unwrap_or(dest)
}

pub fn segments(from: u64, total: u64) -> Vec<(u64, u64)> {
    let seg = SEG.max(total.div_ceil(MAX_SEGS));
    (0u64..).map(|i| from + i * seg).take_while(|s| *s < total).map(|s| (s, (s + seg).min(total))).collect()
}

fn authority(u: &str) -> &str {
    let r = u.split_once("://").map_or(u, |(_, r)| r);
    r.split(['/', '?', '#']).next().unwrap_or(r)
}

fn resolve(base: &str, loc: &str) -> String {
    if own(loc) {
        return loc.to_owned();
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("https", base));
    let auth = authority(base);
    if let Some(p) = loc.strip_prefix("//") {
        return format!("{scheme}://{p}");
    }
    if loc.starts_with('/') {
        return format!("{scheme}://{auth}{loc}");
    }
    let path = rest[auth.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rfind('/').map_or("/", |i| &path[..=i]);
    format!("{scheme}://{auth}{dir}{loc}")
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .max_redirects_will_error(false)
        .build()
        .into()
}

fn get(agent: &ureq::Agent, req: &Req, range: Option<String>) -> Result<ureq::http::Response<ureq::Body>, String> {
    let host = authority(&req.url).to_owned();
    let mut url = req.url.clone();
    for _ in 0..HOPS {
        let mut r = agent.get(&url).header("User-Agent", UA).header("Accept-Encoding", "identity");
        if authority(&url) == host {
            if !req.cookie.is_empty() {
                r = r.header("Cookie", &req.cookie);
            }
            if !req.referer.is_empty() {
                r = r.header("Referer", &req.referer);
            }
        }
        if let Some(x) = &range {
            r = r.header("Range", x);
        }
        let resp = r.call().map_err(|e| e.to_string())?;
        if resp.status().is_redirection() {
            let Some(loc) = resp.headers().get("location").and_then(|v| v.to_str().ok()) else { return Err("Bad redirect".into()) };
            url = resolve(&url, loc);
            continue;
        }
        return Ok(resp);
    }
    Err("Too many redirects".into())
}

fn header_u64(r: &ureq::http::Response<ureq::Body>, name: &str) -> u64 {
    r.headers().get(name).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

fn range_total(v: &str) -> u64 {
    v.rsplit('/').next().and_then(|t| t.trim().parse().ok()).unwrap_or(0)
}

type Rd = Box<dyn Read + Send>;

enum Probe {
    Ranged(u64, Rd),
    Stream(u64, Rd),
}

fn probe(agent: &ureq::Agent, req: &Req) -> Result<Probe, String> {
    let r = get(agent, req, Some(format!("bytes=0-{}", SEG - 1)))?;
    let total = if r.status().as_u16() == 206 { r.headers().get("content-range").and_then(|v| v.to_str().ok()).map_or(0, range_total) } else { 0 };
    let len = header_u64(&r, "content-length");
    let rd: Rd = Box::new(r.into_body().into_reader());
    Ok(if total > 0 { Probe::Ranged(total, rd) } else { Probe::Stream(len, rd) })
}

fn open(agent: &ureq::Agent, req: &Req, from: u64, to: u64) -> Result<Rd, String> {
    let r = get(agent, req, Some(format!("bytes={from}-{to}")))?;
    if r.status().as_u16() != 206 {
        return Err("Range not honoured".into());
    }
    Ok(Box::new(r.into_body().into_reader()))
}

struct Pool {
    queue: Mutex<VecDeque<(u64, u64)>>,
    slots: Vec<Mutex<(u64, u64)>>,
    recv: AtomicU64,
    first: Mutex<Option<Rd>>,
}

impl Pool {
    fn snap(&self) -> Vec<(u64, u64)> {
        let q = self.queue.lock().unwrap();
        let mut v: Vec<_> = self.slots.iter().map(|s| *s.lock().unwrap()).filter(|s| s.0 < s.1).collect();
        v.extend(q.iter().copied());
        v
    }

    fn take(&self, i: usize) -> Option<(u64, u64)> {
        let seg = self.queue.lock().unwrap().pop_front()?;
        *self.slots[i].lock().unwrap() = seg;
        Some(seg)
    }

    fn requeue(&self, i: usize) {
        let mut q = self.queue.lock().unwrap();
        let mut s = self.slots[i].lock().unwrap();
        if s.0 < s.1 {
            q.push_front(*s);
        }
        *s = (0, 0);
    }
}

fn es(e: std::io::Error) -> String {
    e.to_string()
}

fn copy(rd: &mut dyn Read, f: &mut std::fs::File, slot: &Mutex<(u64, u64)>, recv: &AtomicU64, rt: &Rt, buf: &mut [u8]) -> Result<(), String> {
    loop {
        if rt.stop() {
            return Ok(());
        }
        let (cur, end) = *slot.lock().unwrap();
        if cur >= end {
            return Ok(());
        }
        let want = ((end - cur) as usize).min(buf.len());
        let n = rd.read(&mut buf[..want]).map_err(es)?;
        if n == 0 {
            return Err("Connection closed early".into());
        }
        f.write_all(&buf[..n]).map_err(es)?;
        slot.lock().unwrap().0 += n as u64;
        recv.fetch_add(n as u64, Ordering::Relaxed);
    }
}

fn worker(i: usize, agent: &ureq::Agent, req: &Req, part: &Path, pool: &Pool, rt: &Rt) {
    let Ok(mut f) = OpenOptions::new().write(true).open(part) else { return };
    let mut buf = vec![0u8; BUF];
    while !rt.stop() {
        let Some((s, _)) = pool.take(i) else { return };
        let mut pre = if s == 0 { pool.first.lock().unwrap().take() } else { None };
        let mut tries = 0u32;
        loop {
            let (cur, end) = *pool.slots[i].lock().unwrap();
            if cur >= end || rt.stop() {
                break;
            }
            let r = (|| {
                let mut rd = match pre.take() {
                    Some(r) => r,
                    None => open(agent, req, cur, end - 1)?,
                };
                f.seek(SeekFrom::Start(cur)).map_err(es)?;
                copy(&mut *rd, &mut f, &pool.slots[i], &pool.recv, rt, &mut buf)
            })();
            if r.is_ok() {
                continue;
            }
            tries = if pool.slots[i].lock().unwrap().0 > cur { 1 } else { tries + 1 };
            if tries >= TRIES {
                pool.requeue(i);
                return;
            }
            std::thread::sleep(Duration::from_millis(500 * u64::from(tries)));
        }
    }
}

fn ranged(agent: &ureq::Agent, req: &Req, part: &Path, total: u64, segs: Vec<(u64, u64)>, first: Option<Rd>, rt: &Rt, on: &(impl Fn(Msg) + Sync)) -> bool {
    let fresh = first.is_some();
    let file = if fresh { OpenOptions::new().write(true).create(true).truncate(true).open(part).and_then(|f| f.set_len(total)) } else { OpenOptions::new().write(true).open(part).map(|_| ()) };
    if file.is_err() {
        return false;
    }
    let conns = (if total >= BIG { BIG_CONN } else { CONN }).min(segs.len()).max(1);
    let left: u64 = segs.iter().map(|s| s.1 - s.0).sum();
    let pool = Pool { queue: Mutex::new(segs.into()), slots: (0..conns).map(|_| Mutex::new((0, 0))).collect(), recv: AtomicU64::new(total - left), first: Mutex::new(first) };
    let live = AtomicUsize::new(conns);
    std::thread::scope(|s| {
        for i in 0..conns {
            let (pool, live) = (&pool, &live);
            s.spawn(move || {
                worker(i, agent, req, part, pool, rt);
                live.fetch_sub(1, Ordering::Relaxed);
            });
        }
        while live.load(Ordering::Relaxed) > 0 {
            std::thread::sleep(Duration::from_millis(250));
            on(Msg::Progress(pool.recv.load(Ordering::Relaxed), total, pool.snap()));
        }
    });
    let rest = pool.snap();
    on(Msg::Progress(pool.recv.load(Ordering::Relaxed), total, rest.clone()));
    rest.is_empty() || rt.stop()
}

fn stream(mut rd: Rd, total: u64, part: &Path, rt: &Rt, on: &impl Fn(Msg)) -> bool {
    let Ok(mut f) = std::fs::File::create(part) else { return false };
    let mut buf = vec![0u8; BUF];
    let (mut got, mut last) = (0u64, std::time::Instant::now());
    loop {
        if rt.cancel.load(Ordering::Relaxed) {
            return true;
        }
        match rd.read(&mut buf) {
            Ok(0) => return total == 0 || got >= total,
            Ok(n) => {
                if f.write_all(&buf[..n]).is_err() {
                    return false;
                }
                got += n as u64;
                if last.elapsed() >= Duration::from_millis(250) {
                    last = std::time::Instant::now();
                    on(Msg::Progress(got, total, vec![]));
                }
            }
            Err(_) => return false,
        }
    }
}

fn finish(ok: bool, pausable: bool, part: &Path, dest: &Path, rt: &Rt, on: &impl Fn(Msg)) {
    if rt.cancel.load(Ordering::Relaxed) || !ok {
        let _ = std::fs::remove_file(part);
        return on(if ok { Msg::Cancelled } else { Msg::Failed });
    }
    if rt.paused.load(Ordering::Relaxed) && pausable {
        return on(Msg::Paused);
    }
    on(if std::fs::rename(part, dest).is_ok() { Msg::Done } else { Msg::Failed });
}

pub fn run(req: &Req, dest: &Path, resume: Option<(u64, Vec<(u64, u64)>)>, rt: &Rt, on: &(impl Fn(Msg) + Sync)) {
    let agent = agent();
    let part = part_path(dest);
    let (total, segs, first) = match resume {
        Some((t, c)) => (t, c, None),
        None => match probe(&agent, req) {
            Ok(Probe::Ranged(t, rd)) => {
                let head = SEG.min(t);
                let mut segs = vec![(0, head)];
                segs.extend(segments(head, t));
                (t, segs, Some(rd))
            }
            Ok(Probe::Stream(len, rd)) => {
                on(Msg::Info { total: 0, ranged: false });
                let ok = stream(rd, len, &part, rt, on);
                return finish(ok, false, &part, dest, rt, on);
            }
            Err(_) => return on(Msg::Failed),
        },
    };
    on(Msg::Info { total, ranged: true });
    let ok = ranged(&agent, req, &part, total, segs, first, rt, on);
    finish(ok, true, &part, dest, rt, on);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_cover_total_without_gaps() {
        for total in [1, SEG - 1, SEG, 5 * SEG + 17, 100 * SEG, u32::MAX as u64 + 5, 50 << 30] {
            let c = segments(0, total);
            assert!(c.len() as u64 <= MAX_SEGS + 1);
            assert_eq!(c[0].0, 0);
            assert_eq!(c.last().unwrap().1, total);
            assert!(c.windows(2).all(|w| w[0].1 == w[1].0));
            assert!(c.iter().all(|c| c.0 < c.1));
        }
        assert_eq!(segments(0, SEG).len(), 1);
        assert_eq!(segments(SEG, 3 * SEG), vec![(SEG, 2 * SEG), (2 * SEG, 3 * SEG)]);
        assert!(segments(SEG, SEG).is_empty());
    }

    #[test]
    fn part_path_appends_extension() {
        assert_eq!(part_path(Path::new("C:/d/a.zip")), PathBuf::from("C:/d/a.zip.ferrum"));
    }

    #[test]
    fn unique_skips_existing_and_staged_files() {
        let d = std::env::temp_dir().join(format!("rb_fetch_unique_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.bin");
        assert_eq!(unique(p.clone()), p);
        std::fs::write(&p, b"x").unwrap();
        assert_eq!(unique(p.clone()), d.join("a (1).bin"));
        std::fs::write(part_path(&d.join("a (1).bin")), b"x").unwrap();
        assert_eq!(unique(p), d.join("a (2).bin"));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn redirects_resolve_and_scheme_is_checked() {
        assert_eq!(resolve("https://a.com/x/y?q=1", "/z"), "https://a.com/z");
        assert_eq!(resolve("https://a.com/x/y?q=1", "z"), "https://a.com/x/z");
        assert_eq!(resolve("https://a.com/x", "//b.com/p"), "https://b.com/p");
        assert_eq!(resolve("https://a.com/x", "http://c.org/q"), "http://c.org/q");
        assert_eq!(authority("https://a.com:8080/x?y"), "a.com:8080");
        assert!(own("https://a/b") && !own("blob:https://a/b") && !own("data:text/plain,x"));
        assert_eq!(range_total("bytes 0-0/12345"), 12345);
        assert_eq!(range_total("bytes 0-0/*"), 0);
    }

    const MB: usize = 1 << 20;

    fn serve(data: Vec<u8>, ranges: bool, limit: usize) -> String {
        let active = std::sync::Arc::new(AtomicUsize::new(0));
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/f.bin", l.local_addr().unwrap());
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let (data, active) = (data.clone(), active.clone());
                std::thread::spawn(move || {
                    let mut c = c;
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    if limit > 0 && now > limit {
                        let _ = c.write_all(b"HTTP/1.1 503 Busy\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        active.fetch_sub(1, Ordering::SeqCst);
                        return;
                    }
                    let mut buf = vec![0u8; 4096];
                    let n = c.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_lowercase();
                    let range = head.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| r.split_once('-')).and_then(|(a, b)| Some((a.trim().parse::<usize>().ok()?, b.trim().parse::<usize>().ok()?)));
                    let (code, body, extra) = match range.filter(|_| ranges) {
                        Some((a, b)) => ("206 Partial Content", &data[a..=b.min(data.len() - 1)], format!("Content-Range: bytes {a}-{}/{}
", b.min(data.len() - 1), data.len())),
                        None => ("200 OK", &data[..], String::new()),
                    };
                    let _ = c.write_all(format!("HTTP/1.1 {code}
Content-Length: {}
{extra}Connection: close

", body.len()).as_bytes());
                    let _ = c.write_all(body);
                    active.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        url
    }

    fn sample(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn run_to_end(url: String, dest: &Path) -> Vec<String> {
        let log = std::sync::Mutex::new(vec![]);
        let req = Req { url, cookie: String::new(), referer: String::new() };
        run(&req, dest, None, &Rt::default(), &|m| {
            log.lock().unwrap().push(match m {
                Msg::Info { total, ranged } => format!("info {total} {ranged}"),
                Msg::Done => "done".into(),
                Msg::Failed => "failed".into(),
                Msg::Cancelled => "cancelled".into(),
                Msg::Paused => "paused".into(),
                Msg::Progress(..) => return,
            })
        });
        log.into_inner().unwrap()
    }

    #[test]
    fn chunked_download_matches_source_and_removes_staging_file() {
        let data = sample(5 * MB + 123);
        let d = std::env::temp_dir().join(format!("rb_fetch_chunk_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("f.bin");
        let log = run_to_end(serve(data.clone(), true, 0), &dest);
        assert_eq!(log, vec![format!("info {} true", data.len()), "done".to_string()]);
        assert!(std::fs::read(&dest).unwrap() == data);
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn server_without_ranges_streams_into_staging_then_renames() {
        let data = sample(300_000);
        let d = std::env::temp_dir().join(format!("rb_fetch_stream_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("f.bin");
        let log = run_to_end(serve(data.clone(), false, 0), &dest);
        assert_eq!(log, vec!["info 0 false".to_string(), "done".to_string()]);
        assert!(std::fs::read(&dest).unwrap() == data);
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn pause_keeps_ferrum_file_and_resume_completes() {
        let data = sample(8 * MB);
        let d = std::env::temp_dir().join(format!("rb_fetch_pause_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("f.bin");
        let req = Req { url: serve(data.clone(), true, 0), cookie: String::new(), referer: String::new() };
        let rt = Rt::default();
        rt.paused.store(true, Ordering::Relaxed);
        let snap = std::sync::Mutex::new((0u64, vec![]));
        let last = std::sync::Mutex::new(String::new());
        run(&req, &dest, None, &rt, &|m| match m {
            Msg::Progress(_, t, c) => *snap.lock().unwrap() = (t, c),
            Msg::Paused => *last.lock().unwrap() = "paused".into(),
            _ => {}
        });
        assert_eq!(*last.lock().unwrap(), "paused");
        assert!(part_path(&dest).exists() && !dest.exists());
        let (total, chunks) = snap.into_inner().unwrap();
        assert_eq!(total, data.len() as u64);
        let rt = Rt::default();
        let done = std::sync::Mutex::new(false);
        run(&req, &dest, Some((total, chunks)), &rt, &|m| {
            if matches!(m, Msg::Done) {
                *done.lock().unwrap() = true;
            }
        });
        assert!(*done.lock().unwrap());
        assert!(std::fs::read(&dest).unwrap() == data);
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn small_file_needs_a_single_request() {
        let data = sample(3 * MB);
        let d = std::env::temp_dir().join(format!("rb_fetch_small_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("f.bin");
        let log = run_to_end(serve(data.clone(), true, 1), &dest);
        assert_eq!(log, vec![format!("info {} true", data.len()), "done".to_string()]);
        assert!(std::fs::read(&dest).unwrap() == data);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn connection_limited_server_degrades_instead_of_failing() {
        let data = sample(40 * MB);
        let d = std::env::temp_dir().join(format!("rb_fetch_limit_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dest = d.join("f.bin");
        let log = run_to_end(serve(data.clone(), true, 2), &dest);
        assert_eq!(log.last().map(String::as_str), Some("done"));
        assert!(std::fs::read(&dest).unwrap() == data);
        assert!(!part_path(&dest).exists());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
