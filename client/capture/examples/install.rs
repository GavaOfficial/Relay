use relay_capture::install::{self, Canceller};

fn main() -> anyhow::Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("cartella di destinazione"));
    let start = std::time::Instant::now();
    let mut last_line = String::new();
    install::install(&dir, &Canceller::new(), &mut |p| {
        let groups: Vec<String> = p
            .groups
            .iter()
            .map(|g| format!("{} {:.0}%", g.label, g.percent))
            .collect();
        let line = format!("{:5.1}%  {}", p.percent, groups.join(" | "));
        if line != last_line {
            println!("{line}");
            last_line = line;
        }
    })?;
    println!(
        "fatto in {:.1} s, pronto: {}",
        start.elapsed().as_secs_f32(),
        install::is_ready(&dir)
    );
    Ok(())
}
