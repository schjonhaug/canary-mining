use std::{env, fs, path::Path};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let logo_dir = Path::new(&manifest_dir).join("ui/pool-logos");
    println!("cargo:rerun-if-changed={}", logo_dir.display());

    let mut entries = fs::read_dir(&logo_dir)
        .expect("read ui/pool-logos")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "svg"))
        .collect::<Vec<_>>();
    entries.sort();

    let mut generated = String::from(
        "fn generated_pool_logo_asset(path: &str) -> Option<(&'static str, &'static [u8])> {\n    match path {\n",
    );
    for path in entries {
        let Some(filename) = path.file_name().and_then(|filename| filename.to_str()) else {
            continue;
        };
        let escaped_path = path.display().to_string().replace('\\', "\\\\");
        generated.push_str(&format!(
            "        {filename:?} => Some((\"image/svg+xml\", include_bytes!({escaped_path:?}))),\n"
        ));
    }
    generated.push_str("        _ => None,\n    }\n}\n");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    fs::write(Path::new(&out_dir).join("pool_logos.rs"), generated)
        .expect("write generated pool logo lookup");
}
