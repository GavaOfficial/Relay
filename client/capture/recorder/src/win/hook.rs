use super::{
    device::Gpu,
    timing::{self, Stage},
    window::Handle,
};
use anyhow::{ensure, Context, Result};
use relay_hook_protocol::{
    ipc::{Channel, Handle as Process},
    policy,
};
use std::{
    cell::RefCell,
    os::windows::process::CommandExt,
    process::{Child, Command, Stdio},
    time::Instant,
};
use windows::{
    core::Interface,
    Win32::{
        Foundation::HANDLE,
        Graphics::{
            Direct3D11::*,
            Dxgi::{Common::*, IDXGIDevice, IDXGIKeyedMutex},
        },
    },
};
use windows_sys::Win32::Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS};
use windows_sys::Win32::System::Threading::*;

#[derive(Default)]
struct Latest {
    texture: Option<ID3D11Texture2D>,
    size: (u32, u32),
    frames: u64,
    last: Option<Instant>,
    shared: Vec<((u64, u64, u32), ID3D11Texture2D, IDXGIKeyedMutex)>,
}
pub struct Source {
    channel: Channel,
    child: RefCell<Option<Child>>,
    gpu: Gpu,
    latest: RefCell<Latest>,
    producer: Process,
}
impl Source {
    pub fn start_at(gpu: &Gpu, pid: u32, window: Handle, fps: u32) -> Result<Self> {
        let dir = std::env::current_exe()?
            .parent()
            .context("cartella recorder")?
            .to_path_buf();
        Self::start_in(gpu, pid, window, &dir, fps)
    }
    fn start_in(
        gpu: &Gpu,
        pid: u32,
        window: Handle,
        dir: &std::path::Path,
        fps: u32,
    ) -> Result<Self> {
        Self::start_mode(gpu, pid, window, dir, fps, relay_hook_protocol::GPU_ONLY)
    }
    fn start_mode(
        gpu: &Gpu,
        pid: u32,
        window: Handle,
        dir: &std::path::Path,
        fps: u32,
        mode: u32,
    ) -> Result<Self> {
        policy::allow(pid, window.0 as _)?;
        let process =
            Process::checked(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
        let mut wow = 0;
        ensure!(
            unsafe { IsWow64Process(process.0, &mut wow) } != 0,
            "architettura del gioco non verificabile"
        );
        let arch = if wow != 0 { "x86" } else { "x64" };
        let bundled = dir.join("hooks").join(arch).join("relay-inject.exe");
        let injector = if bundled.is_file() {
            bundled
        } else {
            ensure!(
                arch == if cfg!(target_pointer_width = "64") {
                    "x64"
                } else {
                    "x86"
                },
                "iniettore {arch} non installato"
            );
            dir.join("relay-inject.exe")
        };
        let vulkan = relay_hook_protocol::ipc::vulkan_loaded(pid);
        ensure!(
            vulkan || injector.is_file(),
            "iniettore {arch} non installato"
        );
        let channel = if mode == relay_hook_protocol::GPU_ONLY {
            Channel::create_gpu(pid, window.0 as u64)?
        } else {
            Channel::create_for_window(pid, window.0 as u64)?
        };
        let dxgi: IDXGIDevice = gpu.device.cast()?;
        let luid = unsafe { dxgi.GetAdapter()?.GetDesc()? }.AdapterLuid;
        channel.configure(
            mode,
            fps,
            ((luid.HighPart as u32 as u64) << 32) | luid.LowPart as u64,
        )?;
        let producer = Process::checked(unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) })?;
        let child = if vulkan {
            None
        } else {
            Some(
                Command::new(injector)
                    .args([pid.to_string(), (window.0 as usize).to_string()])
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .context("avvio iniettore")?,
            )
        };
        Ok(Self {
            channel,
            child: RefCell::new(child),
            gpu: gpu.clone(),
            latest: RefCell::new(Latest::default()),
            producer,
        })
    }
    fn poll(&self) -> Result<()> {
        let mut l = self.latest.borrow_mut();
        let Some((h, mut pixels)) = self.channel.read(l.frames) else {
            return Ok(());
        };
        if h.gpu_valid() {
            return self.poll_gpu(&mut l, &h);
        }
        if h.flipped != 0 {
            let row = h.stride as usize;
            for y in 0..h.height as usize / 2 {
                let (top, bottom) = pixels.split_at_mut((h.height as usize - y - 1) * row);
                top[y * row..(y + 1) * row].swap_with_slice(&mut bottom[..row]);
            }
        }
        let size = (h.width, h.height);
        if l.texture.is_none() || l.size != size {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: h.width,
                Height: h.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                ..Default::default()
            };
            let mut fresh = None;
            unsafe {
                self.gpu
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut fresh))
            }?;
            l.texture = fresh;
            l.size = size;
        }
        if let Some(t) = &l.texture {
            unsafe {
                self.gpu
                    .context
                    .UpdateSubresource(t, 0, None, pixels.as_ptr().cast(), h.stride, 0);
            }
            l.frames = h.frames;
            l.last = Some(Instant::now());
        }
        Ok(())
    }
    fn poll_gpu(&self, l: &mut Latest, h: &relay_hook_protocol::Header) -> Result<()> {
        let size = (h.width, h.height);
        let identity = (h.texture_epoch, h.texture_handle, h.api);
        if !l.shared.iter().any(|s| s.0 == identity) {
            let mut duplicate = std::ptr::null_mut();
            ensure!(
                unsafe {
                    DuplicateHandle(
                        self.producer.0,
                        h.texture_handle as _,
                        GetCurrentProcess(),
                        &mut duplicate,
                        0,
                        0,
                        DUPLICATE_SAME_ACCESS,
                    )
                } != 0,
                "apertura handle texture hook"
            );
            let duplicate = Process::checked(duplicate)?;
            let device: ID3D11Device1 = self.gpu.device.cast()?;
            let shared: ID3D11Texture2D =
                unsafe { device.OpenSharedResource1(HANDLE(duplicate.0)) }?;
            let mutex: IDXGIKeyedMutex = shared.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            unsafe {
                shared.GetDesc(&mut desc);
            }
            ensure!(
                (desc.Width, desc.Height) == size
                    && desc.Format
                        == match h.format {
                            relay_hook_protocol::RGBA8 => DXGI_FORMAT_R8G8B8A8_UNORM,
                            relay_hook_protocol::BGRX8 => DXGI_FORMAT_B8G8R8X8_UNORM,
                            _ => DXGI_FORMAT_B8G8R8A8_UNORM,
                        },
                "texture hook incompatibile"
            );
            desc.MiscFlags = 0;
            let mut owned = None;
            unsafe {
                self.gpu
                    .device
                    .CreateTexture2D(&desc, None, Some(&mut owned))
            }?;
            if l.shared.len() == 3 {
                l.shared.remove(0);
            }
            l.texture = owned;
            l.size = size;
            l.shared.push((identity, shared, mutex));
        }
        let (_, shared, mutex) = l.shared.iter().find(|s| s.0 == identity).unwrap();
        let measure = timing::span(Stage::Acquire);
        let status = unsafe { (mutex.vtable().AcquireSync)(mutex.as_raw(), 1, 0).0 };
        drop(measure);
        if status != 0 {
            return Ok(());
        }
        unsafe {
            let measure = timing::span(Stage::Copy);
            self.gpu
                .context
                .CopyResource(l.texture.as_ref().context("texture hook mancante")?, shared);
            drop(measure);
            let measure = timing::span(Stage::Flush);
            self.gpu.context.Flush();
            drop(measure);
            let _measure = timing::span(Stage::Release);
            mutex.ReleaseSync(0)?;
        }
        l.frames = h.frames;
        l.last = Some(Instant::now());
        Ok(())
    }
    pub fn closed(&self) -> bool {
        let mut child = self.child.borrow_mut();
        if let Some(c) = child.as_mut() {
            if let Ok(Some(exit)) = c.try_wait() {
                *child = None;
                if !exit.success() {
                    self.channel.stop();
                }
            }
        }
        !self.channel.alive()
    }
    pub fn frames(&self) -> u64 {
        if self.poll().is_err() {
            self.channel.stop();
        }
        self.latest.borrow().frames
    }
    pub fn last_frame(&self) -> Option<Instant> {
        self.latest.borrow().last
    }
    pub fn frame(&self) -> Option<(ID3D11Texture2D, (u32, u32))> {
        if self.poll().is_err() {
            self.channel.stop();
        }
        let l = self.latest.borrow();
        l.texture.as_ref().map(|t| (t.clone(), l.size))
    }
}

impl Drop for Source {
    fn drop(&mut self) {
        self.channel.stop();
        if let Some(mut child) = self.child.get_mut().take() {
            let _ = std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::win::{convert::Converter, encoder, engine, window};
    use std::{
        io::{BufRead, BufReader},
        path::PathBuf,
        time::Duration,
    };

    struct Zoo(Child, isize);
    impl Drop for Zoo {
        fn drop(&mut self) {
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
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "opens a real OpenGL window; requires release zoo, injector, DLL and ffmpeg"]
    fn hook_to_h264_keeps_counter_and_orientation() -> Result<()> {
        super::super::com_init();
        encoder::startup();
        window::dpi_aware();
        let runtime = std::env::var_os("RELAY_ZOO_RUNTIME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/release")
            });
        let mut command = Command::new(runtime.join("examples/relay-zoo-opengl.exe"));
        if std::env::var_os("RELAY_ZOO_EXCLUSIVE").is_some() {
            command.arg("--exclusive");
        }
        let mut zoo = Zoo(command.stdout(Stdio::piped()).spawn()?, 0);
        let mut line = String::new();
        BufReader::new(zoo.0.stdout.take().context("stdout zoo")?).read_line(&mut line)?;
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 2, "zoo non avviato");
        let pid = fields[0].parse()?;
        zoo.1 = fields[1].parse()?;
        let output = (384, 240);
        let choice = engine::choose_encoder(crate::EncoderPref::Auto, output, 30, 4000, &|m| {
            eprintln!("{m}")
        })?;
        let source = Source::start_in(&choice.gpu, pid, Handle(zoo.1), &runtime, 30)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while source.frames() == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        ensure!(source.frames() > 0, "nessun fotogramma hook");
        ensure!(
            source.channel.snapshot().is_some_and(|h| h.gpu_valid()),
            "il test richiede il percorso GPU senza copie CPU"
        );
        let mut converter = Converter::new(&choice.gpu, output, 30)?;
        let mut encoder =
            encoder::H264::open(&choice.candidate, &choice.gpu, output, 30, 4000, 30)?;
        let mut encoded = Vec::new();
        let mut units = crate::h264::AccessUnits::default();
        if let Some(header) = &encoder.sequence_header {
            units.remember(header);
        }
        let previous = source.frames();
        let fresh_deadline = Instant::now() + Duration::from_secs(2);
        while source.frames() == previous && Instant::now() < fresh_deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        ensure!(
            source.frames() > previous,
            "nessuna immagine nuova dopo l'avvio dell'encoder"
        );
        let start = Instant::now();
        for frame in 0..90u64 {
            let due = start + Duration::from_secs_f64(frame as f64 / 30.0);
            if due > Instant::now() {
                std::thread::sleep(due - Instant::now());
            }
            let input = source.frame().context("immagine hook sparita")?;
            let mut target = encoder.frame()?;
            for _ in 0..100 {
                if target.is_some() {
                    break;
                }
                encoder.poll(&mut encoded)?;
                std::thread::sleep(Duration::from_millis(1));
                target = encoder.frame()?;
            }
            let target = target.context("encoder occupato")?;
            converter.convert(Some((&input.0, input.1)), &target.texture, target.slice)?;
            let time = (frame * 10_000_000 / 30) as i64;
            let next = ((frame + 1) * 10_000_000 / 30) as i64;
            encoder.encode(target, time, next - time, frame % 30 == 0, &mut encoded)?;
        }
        encoder.drain(&mut encoded)?;
        drop(source);
        let path = runtime.join("zoo-hook.h264");
        let mut stream = Vec::new();
        for packet in encoded {
            stream.extend_from_slice(&units.prepare(&packet.data).0);
        }
        std::fs::write(&path, stream)?;
        let decoded = Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"])
            .output()?;
        ensure!(
            decoded.status.success(),
            "decodifica: {}",
            String::from_utf8_lossy(&decoded.stderr)
        );
        let bytes = output.0 as usize * output.1 as usize * 4;
        ensure!(
            decoded.stdout.len() == 90 * bytes,
            "numero fotogrammi codificati errato: {}",
            decoded.stdout.len() / bytes
        );
        let mut last = None;
        let mut changes = 0;
        for pixels in decoded.stdout.chunks_exact(bytes) {
            let mut number = 0u32;
            for bit in 0..24usize {
                let x = (2 * bit + 1) * output.0 as usize / 48;
                let p = (output.1 as usize / 2 * output.0 as usize + x) * 4;
                ensure!(
                    pixels[p] > 160 || pixels[p + 2] > 160,
                    "pixel del contatore nero"
                );
                if pixels[p + 2] > pixels[p] {
                    number |= 1 << bit;
                }
            }
            let x = output.0 as usize / 2;
            let green = (0..output.1 as usize / 2).any(|y| {
                let p = (y * output.0 as usize + x) * 4;
                pixels[p + 1] > 160 && pixels[p] < 80 && pixels[p + 2] < 80
            });
            ensure!(
                green,
                "marker superiore assente: immagine capovolta o ritagliata"
            );
            if let Some(old) = last {
                ensure!(
                    number >= old && number <= old + 1,
                    "contatore video non consecutivo: {old} -> {number}"
                );
                if number != old {
                    changes += 1;
                }
            }
            last = Some(number);
        }
        ensure!(
            changes >= 25,
            "video bloccato: {changes} cambiamenti in 3 secondi"
        );
        println!(
            "90 fotogrammi H.264, {changes} contatori consecutivi, encoder {}, file {}",
            choice.candidate.kind.name(),
            path.display()
        );
        Ok(())
    }

    fn cpu_ticks(process: windows_sys::Win32::Foundation::HANDLE) -> Result<u64> {
        use windows_sys::Win32::Foundation::FILETIME;
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        ensure!(
            unsafe { GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user) }
                != 0,
            "lettura tempo CPU"
        );
        let ticks = |v: FILETIME| ((v.dwHighDateTime as u64) << 32) | v.dwLowDateTime as u64;
        Ok(ticks(kernel) + ticks(user))
    }

    #[test]
    #[ignore = "opens a real window; verifies GPU resource replacement and restart"]
    fn gpu_resize_and_restart() -> Result<()> {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        };
        super::super::com_init();
        encoder::startup();
        window::dpi_aware();
        let runtime = std::env::var_os("RELAY_ZOO_RUNTIME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/release")
            });
        let gpu = super::super::device::create(None)?;
        let mut zoo = Zoo(
            Command::new(runtime.join("examples/relay-zoo-opengl.exe"))
                .stdout(Stdio::piped())
                .spawn()?,
            0,
        );
        let mut line = String::new();
        BufReader::new(zoo.0.stdout.take().context("stdout zoo")?).read_line(&mut line)?;
        let fields: Vec<_> = line.split_whitespace().collect();
        ensure!(fields.len() == 2, "zoo non avviato");
        let pid = fields[0].parse()?;
        zoo.1 = fields[1].parse()?;
        for session in 0..2 {
            let source = Source::start_in(&gpu, pid, Handle(zoo.1), &runtime, 30)?;
            let until = Instant::now() + Duration::from_secs(3);
            while source.frames() == 0 && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            ensure!(source.frames() > 0, "sessione {session}: nessuna immagine");
            let before = source.frame().context("immagine GPU")?.1;
            ensure!(
                unsafe {
                    SetWindowPos(
                        zoo.1 as _,
                        std::ptr::null_mut(),
                        0,
                        0,
                        960 - session * 160,
                        600 - session * 100,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER,
                    )
                } != 0,
                "resize zoo"
            );
            let until = Instant::now() + Duration::from_secs(3);
            while source.frame().is_none_or(|(_, size)| size == before) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            let after = source.frame().context("immagine dopo resize")?.1;
            ensure!(after != before && !source.closed(), "resize GPU fallito");
            ensure!(
                source
                    .channel
                    .snapshot()
                    .is_some_and(|h| h.gpu_valid() && h.capacity == 0),
                "trasporto GPU non valido"
            );
            println!("sessione GPU {session}: {before:?} -> {after:?}");
            drop(source);
            std::thread::sleep(Duration::from_millis(400));
        }
        Ok(())
    }

    #[test]
    #[ignore = "visible synthetic performance benchmark: baseline, CPU, GPU; no encoder"]
    fn capture_performance_comparison() -> Result<()> {
        use std::io::Read;
        super::super::com_init();
        encoder::startup();
        window::dpi_aware();
        let gpu = super::super::device::create(None)?;
        let runtime = std::env::var_os("RELAY_ZOO_RUNTIME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/release")
            });
        let mut report = String::from("Synthetic OpenGL capture at 60 fps; excludes encoding. CPU is milliseconds of process CPU time; swap is wall time on the game's presenting thread. Not a real-game FPS benchmark.\n");
        for (label, mode) in [
            ("baseline", None),
            ("cpu", Some(relay_hook_protocol::CPU_ONLY)),
            ("gpu", Some(relay_hook_protocol::GPU_ONLY)),
        ] {
            let mut command = Command::new(runtime.join("examples/relay-zoo-opengl.exe"));
            command.arg("--benchmark");
            if std::env::var_os("RELAY_ZOO_EXCLUSIVE").is_some() {
                command.arg("--exclusive");
            }
            let mut zoo = Zoo(command.stdout(Stdio::piped()).spawn()?, 0);
            let mut reader = BufReader::new(zoo.0.stdout.take().context("stdout zoo")?);
            let mut line = String::new();
            reader.read_line(&mut line)?;
            let fields: Vec<_> = line.split_whitespace().collect();
            ensure!(fields.len() == 2, "zoo non avviato");
            let pid = fields[0].parse()?;
            zoo.1 = fields[1].parse()?;
            let process = Process::checked(unsafe {
                OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
            })?;
            let source = mode
                .map(|mode| Source::start_mode(&gpu, pid, Handle(zoo.1), &runtime, 60, mode))
                .transpose()?;
            let warmup = Instant::now();
            while warmup.elapsed() < Duration::from_secs(3) {
                if let Some(s) = &source {
                    s.frame();
                }
                std::thread::sleep(Duration::from_millis(16));
            }
            let our_start = cpu_ticks(unsafe { GetCurrentProcess() })?;
            let game_start = cpu_ticks(process.0)?;
            let first_frame = source.as_ref().map_or(0, |s| s.frames());
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(4) {
                if let Some(s) = &source {
                    s.frame();
                }
                std::thread::sleep(Duration::from_millis(16));
            }
            let elapsed = start.elapsed().as_secs_f64();
            let our_ms = (cpu_ticks(unsafe { GetCurrentProcess() })? - our_start) as f64 / 10_000.0;
            let game_ms = (cpu_ticks(process.0)? - game_start) as f64 / 10_000.0;
            let frames = source
                .as_ref()
                .map_or(0, |s| s.frames().saturating_sub(first_frame));
            let size = source
                .as_ref()
                .and_then(|s| s.channel.snapshot())
                .map(|h| (h.width, h.height));
            if let Some(s) = &source {
                ensure!(
                    frames >= 80 && !s.closed(),
                    "{label}: cattura non funzionante"
                );
                if mode == Some(relay_hook_protocol::GPU_ONLY) {
                    ensure!(
                        s.channel.snapshot().is_some_and(|h| h.gpu_valid()),
                        "ripiego CPU inatteso"
                    );
                }
            }
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    zoo.1 as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
            ensure!(zoo.0.wait()?.success(), "zoo fallito");
            let mut timings = String::new();
            reader.read_to_string(&mut timings)?;
            let row = format!("{label}: seconds={elapsed:.3} size={size:?} captured={frames} recorder_cpu_ms={our_ms:.1} game_cpu_ms={game_ms:.1} {}", timings.trim());
            println!("{row}");
            report.push_str(&row);
            report.push('\n');
            drop(source);
        }
        std::fs::write(runtime.join("capture-performance.txt"), report)?;
        Ok(())
    }
}
