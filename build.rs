// Embedded Python: tell the linker where libpython / Python3.framework lives at run time,
// and record the interpreter's site-packages (an embedded Python ignores a venv on its own).
// Uses the same interpreter PyO3 builds against (PYO3_PYTHON, else python3).
fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    let python = std::env::var("PYO3_PYTHON").unwrap_or_else(|_| "python3".into());
    let ask = |code: &str| {
        let out = std::process::Command::new(&python).args(["-c", code]).output().unwrap_or_else(|e| {
            panic!("could not run Python at '{python}' ({e}). Set PYO3_PYTHON to a real interpreter, e.g. .venv/bin/python3")
        });
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let lib = ask("import sysconfig as s; print(s.get_config_var('PYTHONFRAMEWORKPREFIX') or s.get_config_var('LIBDIR'))");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{lib}");
    let site = ask("import sysconfig; print(sysconfig.get_paths()['purelib'])");
    println!("cargo:rustc-env=BRIDGE_SITE_PACKAGES={site}");
}
