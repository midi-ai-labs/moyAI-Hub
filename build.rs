fn main() {
    embed_web_assets();
}

fn embed_web_assets() {
    use std::{env, fs, path::Path};
    fn collect(root: &Path, directory: &Path, entries: &mut Vec<(String, String)>) {
        let Ok(children) = fs::read_dir(directory) else {
            return;
        };
        for entry in children.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(root, &path, entries);
            } else if path.is_file() {
                let name = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                entries.push((
                    name,
                    fs::canonicalize(path)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                ));
            }
        }
    }
    println!("cargo:rerun-if-changed=ui/dist");
    let mut entries = Vec::new();
    collect(Path::new("ui/dist"), Path::new("ui/dist"), &mut entries);
    entries.sort();
    let mut source = String::from(
        "fn bundled_asset(path: &str) -> Option<(&'static str, &'static [u8])> { match path {\n",
    );
    for (name, path) in entries {
        let mime = match Path::new(&name)
            .extension()
            .and_then(|value| value.to_str())
        {
            Some("html") => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("ico") => "image/x-icon",
            Some("woff2") => "font/woff2",
            _ => continue,
        };
        source.push_str(&format!(
            "{name:?} => Some(({mime:?}, include_bytes!({path:?}))),\n"
        ));
    }
    source.push_str("_ => None, } }\n");
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("web_assets.rs"),
        source,
    )
    .unwrap();
}
