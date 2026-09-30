#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    win::run()
}
#[cfg(not(windows))]
fn main() {}
#[cfg(windows)]
mod win {
    use anyhow::{ensure, Context};
    use relay_hook_protocol::ipc::Channel;
    use std::{
        io::{BufRead, BufReader},
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Zoo(Child, usize);
    impl Drop for Zoo {
        fn drop(&mut self) {
            if self.1 != 0 {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                        self.1 as _,
                        windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                        0,
                        0,
                    );
                }
                let until = Instant::now() + Duration::from_secs(2);
                while Instant::now() < until {
                    if self.0.try_wait().ok().flatten().is_some() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    pub fn run() -> anyhow::Result<()> {
        let root = std::env::current_exe()?
            .parent()
            .context("examples")?
            .to_path_buf();
        let mut command = Command::new(root.join("relay-zoo-opengl.exe"));
        if std::env::args().any(|a| a == "--exclusive") {
            command.arg("--exclusive");
        }
        let mut zoo = Zoo(command.stdout(Stdio::piped()).spawn()?, 0);
        let mut line = String::new();
        BufReader::new(zoo.0.stdout.take().context("stdout zoo")?).read_line(&mut line)?;
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(
            fields.len() == 2,
            "lo zoo non ha comunicato PID e HWND: {line}"
        );
        let pid: u32 = fields[0].parse()?;
        zoo.1 = fields[1].parse()?;
        for session in 0..2 {
            let channel = Channel::create_for_window(pid, zoo.1 as u64)?;
            channel.configure(relay_hook_protocol::CPU_ONLY, 60, 0)?;
            let mut inject_command =
                Command::new(root.parent().context("debug")?.join("relay-inject.exe"));
            inject_command.args(&fields);
            if std::env::args().any(|a| a == "--remote") {
                inject_command.arg("--remote");
            }
            let mut injector = inject_command.spawn()?;
            let start = Instant::now();
            let mut sequence = 0;
            let mut last_number = None;
            let mut changes = 0;
            let mut skipped = 0;
            while start.elapsed() < Duration::from_secs(4) {
                if let Some((header, pixels)) = channel.read(sequence) {
                    sequence = header.frames;
                    ensure!(
                        header.api == relay_hook_protocol::OPENGL && header.flipped == 1,
                        "metadati OpenGL errati"
                    );
                    let mut number = 0u32;
                    let y = header.height as usize / 2;
                    for bit in 0..24 {
                        let x = (2 * bit + 1) * header.width as usize / 48;
                        let p = &pixels[(y * header.width as usize + x) * 4..][..4];
                        ensure!(
                            (p[0] > 240 && p[2] < 15) || (p[2] > 240 && p[0] < 15),
                            "pixel non valido {p:?}"
                        );
                        if p[2] > 240 {
                            number |= 1 << bit;
                        }
                    }
                    let top = ((header.height - 2) * header.stride) as usize;
                    ensure!(
                        pixels[top + 1] > 240 && pixels[1] < 15,
                        "orientamento o marker errati"
                    );
                    if let Some(last) = last_number {
                        ensure!(
                            number >= last,
                            "contatore tornato indietro: {last} -> {number}"
                        );
                        if number > last {
                            changes += 1;
                            skipped += number - last - 1;
                        }
                    }
                    last_number = Some(number);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            channel.stop();
            ensure!(injector.wait()?.success(), "iniettore fallito");
            ensure!(
                changes >= 20,
                "fotogrammi bloccati o mancanti: {changes} cambiamenti"
            );
            ensure!(skipped == 0, "contatori saltati: {skipped}");
            println!("sessione {session}: {sequence} fotogrammi, {changes} contatori consecutivi, orientamento corretto");
            drop(channel);
            std::thread::sleep(Duration::from_millis(400));
        }
        Ok(())
    }
}
