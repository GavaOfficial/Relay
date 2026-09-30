use anyhow::{ensure, Context, Result};
use relay_hook_protocol::{
    ipc::{wide, Channel, Handle},
    policy,
};
use std::{
    ffi::c_void,
    path::Path,
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        Diagnostics::{Debug::WriteProcessMemory, ToolHelp::*},
        LibraryLoader::*,
        Memory::*,
        Threading::*,
    },
    UI::WindowsAndMessaging::*,
};

struct Library(HMODULE);
impl Drop for Library {
    fn drop(&mut self) {
        unsafe {
            FreeLibrary(self.0);
        }
    }
}
struct Hook(HHOOK);
impl Drop for Hook {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                UnhookWindowsHookEx(self.0);
            }
        }
    }
}

pub fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let force_remote = args.len() == 4 && args[3] == "--remote";
    ensure!(
        args.len() == 3 || force_remote,
        "uso: relay-inject <pid> <hwnd> [--remote]"
    );
    let pid = args[1].parse::<u32>()?;
    let hwnd = args[2].parse::<usize>()? as HWND;
    policy::allow(pid, hwnd as usize)?;
    let channel = Channel::open(pid)?;
    ensure!(channel.alive(), "registratore non attivo");
    ensure!(
        channel.accepts_window(hwnd as u64),
        "finestra non autorizzata dal registratore"
    );
    let dll = std::env::current_exe()?
        .parent()
        .context("cartella iniettore")?
        .join("relay_hook.dll")
        .canonicalize()?;
    unsafe {
        let process = Handle::checked(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid))?;
        let mut target_wow = 0;
        let mut our_wow = 0;
        ensure!(
            IsWow64Process(process.0, &mut target_wow) != 0
                && IsWow64Process(GetCurrentProcess(), &mut our_wow) != 0,
            "architettura non verificabile"
        );
        ensure!(
            target_wow == our_wow,
            "iniettore e gioco devono avere la stessa architettura"
        );
        let path: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
        let lib = Library(LoadLibraryExW(
            path.as_ptr(),
            ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32,
        ));
        ensure!(!lib.0.is_null(), "caricamento DLL hook");
        let proc = GetProcAddress(lib.0, c"RelayHookProc".as_ptr().cast())
            .context("RelayHookProc mancante")?;
        let mut actual = 0;
        let tid = GetWindowThreadProcessId(hwnd, &mut actual);
        ensure!(
            actual == pid && tid != 0,
            "finestra cambiata durante l'aggancio"
        );
        let hook = Hook(if force_remote {
            ptr::null_mut()
        } else {
            SetWindowsHookExW(
                WH_GETMESSAGE,
                Some(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(i32, WPARAM, LPARAM) -> LRESULT,
                >(proc)),
                lib.0,
                tid,
            )
        });
        if !hook.0.is_null() {
            PostThreadMessageW(tid, WM_NULL, 0, 0);
            let until = Instant::now() + Duration::from_millis(1200);
            while channel.alive() && Instant::now() < until {
                if channel.read(0).is_some() {
                    return Ok(());
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        drop(hook);
        ensure!(channel.alive(), "registrazione terminata");
        policy::allow(pid, hwnd as usize)?;
        remote(pid, &dll, lib.0)?;
    }
    Ok(())
}
use std::os::windows::ffi::OsStrExt;

unsafe fn module_base(pid: u32, filename: &str) -> Result<usize> {
    let snap = Handle::checked(CreateToolhelp32Snapshot(
        TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32,
        pid,
    ))?;
    let mut m: MODULEENTRY32W = std::mem::zeroed();
    m.dwSize = std::mem::size_of_val(&m) as u32;
    ensure!(
        Module32FirstW(snap.0, &mut m) != 0,
        "elenco moduli non disponibile"
    );
    loop {
        let end = m
            .szModule
            .iter()
            .position(|v| *v == 0)
            .unwrap_or(m.szModule.len());
        if String::from_utf16_lossy(&m.szModule[..end]).eq_ignore_ascii_case(filename) {
            return Ok(m.modBaseAddr as usize);
        }
        if Module32NextW(snap.0, &mut m) == 0 {
            anyhow::bail!("modulo {filename} non trovato");
        }
    }
}

unsafe fn call(process: HANDLE, address: usize, argument: *const c_void) -> Result<()> {
    let thread = Handle::checked(CreateRemoteThread(
        process,
        ptr::null(),
        0,
        Some(std::mem::transmute::<
            usize,
            unsafe extern "system" fn(*mut c_void) -> u32,
        >(address)),
        argument,
        0,
        ptr::null_mut(),
    ))?;
    ensure!(
        WaitForSingleObject(thread.0, 5000) == WAIT_OBJECT_0,
        "avvio hook scaduto"
    );
    Ok(())
}

unsafe fn remote(pid: u32, dll: &Path, local: HMODULE) -> Result<()> {
    let process = Handle::checked(OpenProcess(
        PROCESS_CREATE_THREAD
            | PROCESS_QUERY_INFORMATION
            | PROCESS_VM_OPERATION
            | PROCESS_VM_WRITE
            | PROCESS_VM_READ,
        0,
        pid,
    ))?;
    let path: Vec<u16> = dll.as_os_str().encode_wide().chain(Some(0)).collect();
    let bytes = path.len() * 2;
    let allocation = VirtualAllocEx(
        process.0,
        ptr::null(),
        bytes,
        MEM_COMMIT | MEM_RESERVE,
        PAGE_READWRITE,
    );
    ensure!(!allocation.is_null(), "allocazione percorso DLL");
    let mut written = 0;
    if WriteProcessMemory(
        process.0,
        allocation,
        path.as_ptr().cast(),
        bytes,
        &mut written,
    ) == 0
        || written != bytes
    {
        VirtualFreeEx(process.0, allocation, 0, MEM_RELEASE);
        anyhow::bail!("scrittura percorso DLL");
    }
    let kernel = GetModuleHandleW(wide("kernel32.dll").as_ptr());
    let load =
        GetProcAddress(kernel, c"LoadLibraryW".as_ptr().cast()).context("LoadLibraryW mancante")?;
    let mut owner = ptr::null_mut();
    ensure!(
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            load as *const u16,
            &mut owner
        ) != 0,
        "modulo LoadLibraryW"
    );
    let mut owner_path = [0u16; 32768];
    let n = GetModuleFileNameW(owner, owner_path.as_mut_ptr(), owner_path.len() as u32);
    let owner_name = String::from_utf16_lossy(&owner_path[..n as usize]);
    let base = module_base(pid, owner_name.rsplit('\\').next().context("nome modulo")?)?;
    call(process.0, base + load as usize - owner as usize, allocation)?;
    VirtualFreeEx(process.0, allocation, 0, MEM_RELEASE);
    let base = module_base(pid, "relay_hook.dll")?;
    let start =
        GetProcAddress(local, c"RelayStart".as_ptr().cast()).context("RelayStart mancante")?;
    call(
        process.0,
        base + start as usize - local as usize,
        ptr::null(),
    )
}
