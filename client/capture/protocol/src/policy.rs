use crate::{blocked_name, ipc::Handle};
use anyhow::{ensure, Result};
use windows_sys::Win32::{
    Foundation::*, System::Diagnostics::ToolHelp::*, UI::WindowsAndMessaging::*,
};

pub fn allow(pid: u32, window: usize) -> Result<()> {
    let hwnd = window as HWND;
    unsafe {
        let mut actual = 0;
        GetWindowThreadProcessId(hwnd, &mut actual);
        ensure!(
            actual == pid && pid != 0,
            "la finestra non appartiene al processo richiesto"
        );
        let mut affinity = 0;
        ensure!(
            GetWindowDisplayAffinity(hwnd, &mut affinity) != 0 && affinity == WDA_NONE,
            "finestra protetta o protezione non verificabile"
        );
        let snapshot = Handle::checked(CreateToolhelp32Snapshot(
            TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32,
            pid,
        ))?;
        let mut m = MODULEENTRY32W {
            dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
            ..std::mem::zeroed()
        };
        ensure!(
            Module32FirstW(snapshot.0, &mut m) != 0,
            "impossibile controllare i moduli del gioco"
        );
        loop {
            let end = m
                .szModule
                .iter()
                .position(|v| *v == 0)
                .unwrap_or(m.szModule.len());
            let name = String::from_utf16_lossy(&m.szModule[..end]);
            ensure!(
                !blocked_name(&name),
                "anti-cheat rilevato ({name}): uso solo WGC"
            );
            if Module32NextW(snapshot.0, &mut m) == 0 {
                ensure!(
                    GetLastError() == ERROR_NO_MORE_FILES,
                    "elenco moduli incompleto"
                );
                break;
            }
        }
    }
    Ok(())
}
