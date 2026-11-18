use crate::{settings, update};
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, HANDLE};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegCreateKeyExW, RegDeleteKeyW, RegQueryValueExW, RegSetValueExW};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::Shell::{IShellLinkW, IsUserAnAdmin, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW, SW_SHOWNORMAL};
use windows::core::{HRESULT, Interface, PCWSTR};

const EXE: &str = "ferrum.exe";
const REAL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Ferrum";
const TEST_KEY: &str = r"Software\Ferrum-Test\Uninstall\Ferrum";
const NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, PartialEq)]
pub enum Cmd {
    Run { no_install: bool },
    Install,
    Uninstall { purge: bool },
    Apply { src: PathBuf, target: Option<PathBuf>, wait: Option<u32> },
}

#[derive(Debug, PartialEq)]
pub enum Reg {
    Str(String),
    Dword(u32),
}

pub fn parse_args(a: &[String]) -> Cmd {
    let val = |f: &str| a.iter().position(|x| x == f).and_then(|i| a.get(i + 1));
    let has = |f: &str| a.iter().any(|x| x == f);
    if let Some(src) = val("--apply-update") {
        return Cmd::Apply { src: src.into(), target: val("--target").map(PathBuf::from), wait: val("--wait-pid").and_then(|p| p.parse().ok()) };
    }
    if has("--uninstall") {
        return Cmd::Uninstall { purge: has("--purge") };
    }
    if has("--install") {
        return Cmd::Install;
    }
    Cmd::Run { no_install: has("--no-install") }
}

pub fn url_arg() -> Option<String> {
    std::env::args().skip(1).find(|a| !a.starts_with("--"))
}

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\").to_lowercase();
    s.strip_prefix(r"\\?\").unwrap_or(&s).trim_end_matches('\\').to_owned()
}

pub fn is_under(exe: &Path, dir: &Path) -> bool {
    let (e, d) = (norm(exe), norm(dir));
    e.len() > d.len() && e.starts_with(&d) && e.as_bytes()[d.len()] == b'\\'
}

pub fn eligible(exe: &Path, dir: &Path, no_install: bool, env_off: bool) -> bool {
    !no_install && !env_off && !is_under(exe, dir)
}

pub fn nag_ok(declined: Option<&str>, version: &str) -> bool {
    declined.map(str::trim) != Some(version)
}

pub fn should_copy(installed: Option<&str>, own: &str, target_exists: bool) -> bool {
    !target_exists || installed.is_none_or(|v| update::newer(own, v))
}

pub fn lnk_path(menu: &Path) -> PathBuf {
    menu.join("Ferrum.lnk")
}

pub fn uninstall_key(test: bool) -> (bool, &'static str) {
    if test { (false, TEST_KEY) } else { (true, REAL_KEY) }
}

pub fn uninstall_values(exe: &Path, dir: &Path, version: &str, size_kb: u32) -> Vec<(&'static str, Reg)> {
    let s = |v: &str| Reg::Str(v.to_owned());
    vec![
        ("DisplayName", s("Ferrum")),
        ("DisplayVersion", s(version)),
        ("Publisher", s("Ferrum")),
        ("DisplayIcon", s(&exe.to_string_lossy())),
        ("InstallLocation", s(&dir.to_string_lossy())),
        ("UninstallString", s(&format!("\"{}\" --uninstall", exe.display()))),
        ("NoModify", Reg::Dword(1)),
        ("NoRepair", Reg::Dword(1)),
        ("EstimatedSize", Reg::Dword(size_kb)),
    ]
}

pub fn quote(a: &[&OsStr]) -> String {
    a.iter().map(|s| format!("\"{}\"", s.to_string_lossy().trim_end_matches('\\'))).collect::<Vec<_>>().join(" ")
}

fn test_mode() -> bool {
    std::env::var_os("FERRUM_INSTALL_DIR").is_some()
}

fn skip_elevate() -> bool {
    std::env::var_os("FERRUM_SKIP_ELEVATE").is_some()
}

fn env_dir(var: &str, fallback: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(fallback)
}

pub fn install_dir() -> PathBuf {
    env_dir("FERRUM_INSTALL_DIR", || env_dir("ProgramFiles", || r"C:\Program Files".into()).join("Ferrum"))
}

fn menu_dir() -> PathBuf {
    env_dir("FERRUM_STARTMENU_DIR", || env_dir("ProgramData", || r"C:\ProgramData".into()).join(r"Microsoft\Windows\Start Menu\Programs"))
}

fn target_exe() -> PathBuf {
    install_dir().join(EXE)
}

fn w(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn is_admin() -> bool {
    unsafe { IsUserAnAdmin().as_bool() }
}

fn dev_tree(exe: &Path) -> bool {
    exe.ancestors().skip(1).take(5).any(|d| d.join("Cargo.toml").exists())
}

fn declined_file() -> PathBuf {
    settings::dir().join("install_declined")
}

fn root(hklm: bool) -> HKEY {
    if hklm { HKEY_LOCAL_MACHINE } else { HKEY_CURRENT_USER }
}

fn reg_set(values: &[(&'static str, Reg)]) -> Result<(), String> {
    let (hklm, sub) = uninstall_key(test_mode());
    let mut k = HKEY::default();
    unsafe {
        RegCreateKeyExW(root(hklm), PCWSTR(w(sub).as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, None, &mut k, None).ok().map_err(|e| e.to_string())?;
        let mut res = Ok(());
        for (name, v) in values {
            let (t, bytes): (REG_VALUE_TYPE, Vec<u8>) = match v {
                Reg::Str(s) => (REG_SZ, w(s).iter().flat_map(|u| u.to_le_bytes()).collect()),
                Reg::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
            };
            if let Err(e) = RegSetValueExW(k, PCWSTR(w(name).as_ptr()), None, t, Some(&bytes)).ok() {
                res = Err(e.to_string());
                break;
            }
        }
        let _ = RegCloseKey(k);
        res
    }
}

fn installed_version() -> Option<String> {
    let (hklm, sub) = uninstall_key(test_mode());
    let mut k = HKEY::default();
    unsafe {
        RegCreateKeyExW(root(hklm), PCWSTR(w(sub).as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_READ, None, &mut k, None).ok().ok()?;
        let (mut buf, mut len, mut t) = ([0u16; 64], 128u32, REG_VALUE_TYPE::default());
        let r = RegQueryValueExW(k, PCWSTR(w("DisplayVersion").as_ptr()), None, Some(&mut t), Some(buf.as_mut_ptr().cast()), Some(&mut len)).ok();
        let _ = RegCloseKey(k);
        r.ok()?;
        let n = (len as usize / 2).min(buf.len());
        let s = String::from_utf16_lossy(&buf[..n]);
        let s = s.trim_end_matches('\0').trim();
        (!s.is_empty()).then(|| s.to_owned())
    }
}

fn reg_remove() {
    let (hklm, sub) = uninstall_key(test_mode());
    unsafe {
        let _ = RegDeleteKeyW(root(hklm), PCWSTR(w(sub).as_ptr()));
    }
}

fn make_lnk(lnk: &Path, exe: &Path) -> Result<(), String> {
    if let Some(p) = lnk.parent() {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = (|| -> windows::core::Result<()> {
            let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            l.SetPath(PCWSTR(w(exe).as_ptr()))?;
            l.SetWorkingDirectory(PCWSTR(w(exe.parent().unwrap_or(Path::new(""))).as_ptr()))?;
            l.SetDescription(PCWSTR(w("Ferrum").as_ptr()))?;
            l.SetIconLocation(PCWSTR(w(exe).as_ptr()), 0)?;
            l.cast::<IPersistFile>()?.Save(PCWSTR(w(lnk).as_ptr()), true)
        })();
        if init {
            CoUninitialize();
        }
        r.map_err(|e| e.to_string())
    }
}

fn register(target: &Path) -> Result<(), String> {
    make_lnk(&lnk_path(&menu_dir()), target)?;
    let kb = std::fs::metadata(target).map_or(0, |m| (m.len() / 1024) as u32);
    reg_set(&uninstall_values(target, target.parent().unwrap_or(Path::new("")), update::VERSION, kb))
}

fn copy_replace(src: &Path, target: &Path) -> Result<(), String> {
    if norm(src) == norm(target) {
        return Ok(());
    }
    std::fs::create_dir_all(target.parent().ok_or("Bad target")?).map_err(|e| e.to_string())?;
    let tmp = target.with_extension("exe.new");
    std::fs::copy(src, &tmp).map_err(|e| e.to_string())?;
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        match std::fs::rename(&tmp, target) {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() > end => {
                let _ = std::fs::remove_file(&tmp);
                return Err(e.to_string());
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }
    }
}

fn do_install(src: &Path) -> Result<PathBuf, String> {
    let target = target_exe();
    if should_copy(installed_version().as_deref(), update::VERSION, target.exists()) {
        copy_replace(src, &target)?;
        register(&target)?;
    }
    Ok(target)
}

enum Elev {
    Declined,
    Failed(String),
}

fn run_elevated(exe: &Path, args: &str, wait: bool) -> Result<u32, Elev> {
    let (file, params, verb) = (w(exe), w(args), w("runas"));
    let mut i = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut i) {
            return Err(if e.code() == HRESULT::from_win32(ERROR_CANCELLED.0) { Elev::Declined } else { Elev::Failed(e.to_string()) });
        }
        let h: HANDLE = i.hProcess;
        if !wait || h.is_invalid() {
            if !h.is_invalid() {
                let _ = CloseHandle(h);
            }
            return Ok(0);
        }
        WaitForSingleObject(h, INFINITE);
        let mut code = 1;
        let _ = GetExitCodeProcess(h, &mut code);
        let _ = CloseHandle(h);
        Ok(code)
    }
}

fn wait_pid(pid: u32, max: Duration) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, max.as_millis() as u32);
            let _ = CloseHandle(h);
        }
    }
}

fn launch(exe: &Path, args: &[String]) {
    let _ = if is_admin() && !skip_elevate() { Command::new("explorer.exe").arg(exe).spawn() } else { Command::new(exe).args(args).spawn() };
}

fn writable(dir: &Path) -> bool {
    let p = dir.join(format!(".ferrum-w{}", std::process::id()));
    std::fs::create_dir_all(dir).is_ok() && std::fs::write(&p, b"").and_then(|_| std::fs::remove_file(&p)).is_ok()
}

pub fn spawn_apply(new: &Path) -> Result<(), String> {
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let (pid, flags) = (std::process::id().to_string(), ["--apply-update", "--target", "--wait-pid"]);
    let args = quote(&[flags[0].as_ref(), new.as_os_str(), flags[1].as_ref(), target.as_os_str(), flags[2].as_ref(), pid.as_ref()]);
    if skip_elevate() || writable(target.parent().unwrap_or(Path::new(""))) {
        Command::new(new).raw_arg(&args).spawn().map(|_| ()).map_err(|e| e.to_string())
    } else {
        run_elevated(new, &args, false).map(|_| ()).map_err(|e| match e {
            Elev::Declined => "Administrator permission was declined".to_owned(),
            Elev::Failed(m) => m,
        })
    }
}

fn apply(src: &Path, target: Option<PathBuf>, wait: Option<u32>) -> Result<PathBuf, String> {
    let target = target.unwrap_or_else(target_exe);
    if let Some(p) = wait {
        wait_pid(p, Duration::from_secs(60));
    }
    copy_replace(src, &target)?;
    if installed_version().is_some() && is_under(&target, &install_dir()) {
        let _ = register(&target);
    }
    Ok(target)
}

fn uninstall(purge: bool) -> Result<(), String> {
    if !skip_elevate() && !is_admin() {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        return match run_elevated(&exe, &args, true) {
            Ok(0) => Ok(()),
            Ok(_) => Err("Uninstall failed".into()),
            Err(Elev::Declined) => Err("Administrator permission was declined".into()),
            Err(Elev::Failed(m)) => Err(m),
        };
    }
    let (dir, exe) = (install_dir(), target_exe());
    let _ = std::fs::remove_file(lnk_path(&menu_dir()));
    reg_remove();
    if std::fs::remove_file(&exe).is_err() && exe.exists() {
        let script = format!("/C ping -n 3 127.0.0.1 >nul & del /F /Q \"{}\" & rmdir \"{}\"", exe.display(), dir.display());
        let _ = Command::new("cmd.exe").raw_arg(script).creation_flags(NO_WINDOW).spawn();
    } else {
        let _ = std::fs::remove_dir(&dir);
    }
    if purge {
        let _ = std::fs::remove_dir_all(settings::dir());
    }
    Ok(())
}

fn fail(msg: &str) {
    if skip_elevate() {
        eprintln!("{msg}");
    } else {
        unsafe {
            MessageBoxW(None, PCWSTR(w(msg).as_ptr()), PCWSTR(w("Ferrum").as_ptr()), MB_OK | MB_ICONERROR);
        }
    }
}

fn portable(exe: &Path, no_install: bool) -> bool {
    let dir = install_dir();
    if !eligible(exe, &dir, no_install, std::env::var_os("FERRUM_NO_INSTALL").is_some()) || dev_tree(exe) {
        return true;
    }
    let declined = std::fs::read_to_string(declined_file()).ok();
    if !nag_ok(declined.as_deref(), update::VERSION) {
        return true;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let done = if skip_elevate() || is_admin() {
        do_install(exe).is_ok()
    } else {
        let flag = quote(&[OsStr::new("--install")]);
        matches!(run_elevated(exe, &flag, true), Ok(0))
    };
    let target = target_exe();
    if done && target.exists() {
        launch(&target, &args);
        return false;
    }
    let _ = std::fs::write(declined_file(), update::VERSION);
    true
}

pub fn startup() -> bool {
    let Ok(exe) = std::env::current_exe() else { return true };
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Cmd::Run { no_install } => portable(&exe, no_install),
        Cmd::Install => match do_install(&exe) {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1)
            }
        },
        Cmd::Uninstall { purge } => match uninstall(purge) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                fail(&e);
                std::process::exit(1)
            }
        },
        Cmd::Apply { src, target, wait } => {
            let t = target.clone().unwrap_or_else(target_exe);
            match apply(&src, target, wait) {
                Ok(t) => {
                    launch(&t, &[]);
                    std::process::exit(0)
                }
                Err(e) => {
                    fail(&format!("Ferrum could not update: {e}"));
                    launch(&t, &[]);
                    std::process::exit(1)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn args() {
        assert_eq!(parse_args(&a(&[])), Cmd::Run { no_install: false });
        assert_eq!(parse_args(&a(&["https://x.y", "--no-install"])), Cmd::Run { no_install: true });
        assert_eq!(parse_args(&a(&["--install"])), Cmd::Install);
        assert_eq!(parse_args(&a(&["--uninstall"])), Cmd::Uninstall { purge: false });
        assert_eq!(parse_args(&a(&["--uninstall", "--purge"])), Cmd::Uninstall { purge: true });
        assert_eq!(parse_args(&a(&["--apply-update", "C:\\t\\n.exe", "--target", "C:\\p\\f.exe", "--wait-pid", "42"])), Cmd::Apply { src: "C:\\t\\n.exe".into(), target: Some("C:\\p\\f.exe".into()), wait: Some(42) });
        assert_eq!(parse_args(&a(&["--apply-update", "n.exe", "--wait-pid", "x"])), Cmd::Apply { src: "n.exe".into(), target: None, wait: None });
    }

    #[test]
    fn location() {
        let pf = Path::new(r"C:\Program Files\Ferrum");
        assert!(is_under(Path::new(r"C:\Program Files\Ferrum\ferrum.exe"), pf));
        assert!(is_under(Path::new(r"c:/program files/FERRUM/ferrum.exe"), pf));
        assert!(is_under(Path::new(r"\\?\C:\Program Files\Ferrum\ferrum.exe"), Path::new(r"C:\Program Files\Ferrum\")));
        assert!(!is_under(Path::new(r"C:\Program Files\Ferrum2\ferrum.exe"), pf));
        assert!(!is_under(Path::new(r"C:\Program Files\Ferrum"), pf));
        assert!(!is_under(Path::new(r"C:\Users\x\Downloads\ferrum.exe"), pf));
    }

    #[test]
    fn eligibility() {
        let pf = Path::new(r"C:\Program Files\Ferrum");
        let dl = Path::new(r"C:\Users\x\Downloads\ferrum.exe");
        assert!(eligible(dl, pf, false, false));
        assert!(!eligible(dl, pf, true, false));
        assert!(!eligible(dl, pf, false, true));
        assert!(!eligible(&pf.join("ferrum.exe"), pf, false, false));
        assert!(nag_ok(None, "1.0.0"));
        assert!(nag_ok(Some("0.9.0"), "1.0.0"));
        assert!(!nag_ok(Some("1.0.0\n"), "1.0.0"));
    }

    #[test]
    fn copy_decision() {
        assert!(should_copy(None, "1.0.0", false));
        assert!(should_copy(None, "1.0.0", true));
        assert!(should_copy(Some("1.0.0"), "1.0.1", true));
        assert!(!should_copy(Some("1.0.1"), "1.0.0", true));
        assert!(!should_copy(Some("1.0.0"), "1.0.0", true));
        assert!(should_copy(Some("2.0.0"), "1.0.0", false));
    }

    #[test]
    fn shortcut_and_registry() {
        assert_eq!(lnk_path(Path::new(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs")), Path::new(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Ferrum.lnk"));
        assert_eq!(uninstall_key(false), (true, REAL_KEY));
        assert_eq!(uninstall_key(true), (false, TEST_KEY));
        let v = uninstall_values(Path::new(r"C:\Program Files\Ferrum\ferrum.exe"), Path::new(r"C:\Program Files\Ferrum"), "1.2.3", 9000);
        let get = |n: &str| &v.iter().find(|(k, _)| *k == n).unwrap().1;
        assert_eq!(get("DisplayVersion"), &Reg::Str("1.2.3".into()));
        assert_eq!(get("UninstallString"), &Reg::Str(r#""C:\Program Files\Ferrum\ferrum.exe" --uninstall"#.into()));
        assert_eq!(get("NoModify"), &Reg::Dword(1));
        assert_eq!(get("EstimatedSize"), &Reg::Dword(9000));
        assert_eq!(get("InstallLocation"), &Reg::Str(r"C:\Program Files\Ferrum".into()));
    }

    #[test]
    fn quoting() {
        assert_eq!(quote(&[OsStr::new("--apply-update"), OsStr::new(r"C:\a b\"), OsStr::new("7")]), r#""--apply-update" "C:\a b" "7""#);
    }
}
