use std::path::Path;

pub fn strip_version(name: &str) -> String {
    let original = name.trim();
    let mut s = original.to_string();
    loop {
        let before = s.clone();
        let t = s.trim_end();
        if let Some(open) = t.ends_with([']', ')']).then(|| t.rfind(['[', '('])).flatten() {
            let inside = &t[open + 1..t.len() - 1];
            if inside.chars().any(|c| c.is_ascii_digit())
                && inside.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | 'v' | 'V' | ' '))
            {
                s = t[..open].to_string();
            }
        }
        let t = s.trim_end().to_string();
        if let Some(pos) = t.rfind([' ', '_', '-']) {
            let word = &t[pos + 1..];
            let digits = word.strip_prefix(['v', 'V']).unwrap_or(word);
            if !digits.is_empty()
                && digits.chars().next().is_some_and(|c| c.is_ascii_digit())
                && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                s = t[..pos].to_string();
            }
        }
        s = s.trim_end_matches([' ', '_', '-', '.']).to_string();
        if s == before {
            break;
        }
    }
    if s.trim().is_empty() {
        original.to_string()
    } else {
        s.trim().to_string()
    }
}

pub fn folder_name(folder: &str) -> String {
    let path = Path::new(folder.trim_end_matches(['\\', '/']));
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    match name(path) {
        Some(n) if n.eq_ignore_ascii_case("bin") => path.parent().and_then(name).unwrap_or(n),
        Some(n) => n,
        None => folder.to_string(),
    }
}

fn generic_exe(stem: &str) -> bool {
    let s: String = stem.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    s.contains("engine")
        || ["fridaynightfunkin", "funkin", "fnf", "game", "nightmarevision", "flixelcrashhandler"].contains(&s.as_str())
}

pub fn game_name(folder: &Path) -> String {
    let stems: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            let l = n.to_lowercase();
            (l.ends_with(".exe") && !l.starts_with("unins") && !l.contains("crash"))
                .then(|| n[..n.len() - 4].to_string())
        })
        .collect();
    match stems.as_slice() {
        [one] if !generic_exe(one) => strip_version(one),
        _ => strip_version(&folder_name(&folder.to_string_lossy())),
    }
}

pub fn aliases(name: &str, old: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for o in old {
        let o = o.trim();
        if !o.is_empty() && !o.eq_ignore_ascii_case(name) && !out.iter().any(|x| x.eq_ignore_ascii_case(o)) {
            out.push(o.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_removed_from_the_end() {
        assert_eq!(strip_version("Vs Nonsense V1.5"), "Vs Nonsense");
        assert_eq!(strip_version("Project Crafted [v1.0.2]"), "Project Crafted");
        assert_eq!(strip_version("Marios Madness (2.0.1)"), "Marios Madness");
        assert_eq!(strip_version("crafter_s_v15"), "crafter_s");
        assert_eq!(strip_version("PoldHub Demo v2"), "PoldHub Demo");
        assert_eq!(strip_version("d-sides_redux_10"), "d-sides_redux");
        assert_eq!(strip_version("Vs Cord v1.04"), "Vs Cord");
        assert_eq!(strip_version("FNF: Haniel Mix"), "FNF: Haniel Mix");
        assert_eq!(strip_version("Funkin' on the Heights!"), "Funkin' on the Heights!");
        assert_eq!(strip_version("2hot"), "2hot");
        assert_eq!(strip_version("v2"), "v2", "non resta un nome vuoto");
    }

    #[test]
    fn a_game_is_named_after_its_own_exe() {
        let dir = tempfile::tempdir().unwrap();
        let g = dir.path().join("VSROSS2.1HOTFIX");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("VsRoss.exe"), "").unwrap();
        assert_eq!(game_name(&g), "VsRoss");

        let g = dir.path().join("Vee Funkin V5 Demo");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("PsychEngine.exe"), "").unwrap();
        assert_eq!(game_name(&g), "Vee Funkin V5 Demo", "exe generico: conta la cartella");

        let g = dir.path().join("CatNap V2").join("bin");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(g.join("Corrupt Engine.exe"), "").unwrap();
        assert_eq!(game_name(&g), "CatNap");
    }

    #[test]
    fn old_names_are_listed_once() {
        assert_eq!(aliases("VsRoss", &["VSROSS2.1HOTFIX", "vsross", "", "VSROSS2.1HOTFIX"]), ["VSROSS2.1HOTFIX"]);
    }
}
